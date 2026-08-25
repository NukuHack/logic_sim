//! Chip-customization state machine (`Overlay::CustomizeChip`): owns the
//! cloned [`ChipDescription`] being customized plus whatever grab/resize
//! interaction is in flight, applies the interactions between frames
//! against the last-built [`PreviewLayout`], and commits the draft back
//! onto the library entry on Confirm -- mirroring
//! `ChipSaveMenu`/`ChipCustomizationMenu`'s "edit a preview copy, write
//! through on confirm" shape.
//!
//! Rendering lives across the module boundary in
//! [`crate::render::customize_ui`] (plain data in, frames out); this
//! module holds everything stateful.

use crate::description::{ChipDescription, DisplayDescription, NameLocation};
use crate::render::customize_ui::{default_display_scale, display_entries, CustomizeCtx, CustomizeFrameOut, CustomizeInteraction};
use crate::render::layout::{self, GRID_SIZE};
use crate::render::scene::lookup::SimulatorPinState;
use crate::render::theme;
use crate::structs::Vec2;
use crate::viewer::state::{close_top_overlay, open_overlay, Overlay, ViewerState};

/// Draft customization session for the open chip. `saved_save_text`
/// snapshots the save popup's name field: the shared text buffer is
/// borrowed by the hex colour field while this workspace is open and
/// restored on close (see `close_top_overlay`'s CustomizeChip arm).
pub(crate) struct CustomizeState {
	pub(crate) draft: ChipDescription,
	pub(crate) saved_save_text: String,
	pub(crate) interaction: CustomizeInteraction,
	/// DISPLAYS list scroll offset in px, clamped against `list_scroll_max`.
	pub(crate) list_scroll: f32,
	pub(crate) list_scroll_max: f32,
	/// Preview zoom multiplier around auto-fit (1.0 = exactly fits).
	pub(crate) zoom_factor: f32,
	/// Screen-space cache written back by every built frame -- event
	/// handlers map cursors through it between frames.
	pub(crate) layout: crate::render::customize_ui::PreviewLayout,
}

// ---- open / close ------------------------------------------------------

/// Save popup's CUSTOMIZE button: clones the open chip into a draft and
/// stacks [`Overlay::CustomizeChip`] on top of the popup. The shared text
/// buffer switches to holding the hex colour field's contents until the
/// workspace closes.
pub(crate) fn open_customize(v: &mut ViewerState) {
	let mut draft = v.library.get(&v.root_chip_name).clone();
	if draft.size == Vec2::ZERO {
		draft.size = min_size_for(&draft);
	} else {
		enforce_min_size(&mut draft);
	}

	let colour_seed = if draft.colour[3] > 0.0 { draft.colour } else { theme::CHIP_BODY_COL };

	v.customize = Some(CustomizeState {
		saved_save_text: std::mem::take(&mut v.overlay_text_input),
		draft,
		interaction: CustomizeInteraction::None,
		list_scroll: 0.0,
		list_scroll_max: 0.0,
		zoom_factor: 1.0,
		layout: Default::default(),
	});
	open_overlay(v, Overlay::CustomizeChip);
	v.overlay_text_input = hex_of(colour_seed);
}

/// CONFIRM (or Enter): writes the draft's customized fields onto the
/// library's entry for the open chip, refreshes the simulation, and pops
/// back to the save popup (whose name field this restores on the way out,
/// via `close_top_overlay`). Saving *to disk* stays the save popup's job.
pub(crate) fn confirm_customize(v: &mut ViewerState, status: &mut Option<String>) {
	let Some(customize) = v.customize.take() else { return };
	let root_chip_name = v.root_chip_name.clone();
	{
		let chip = v.library.get_mut(&root_chip_name);
		chip.colour = finalize_colour(customize.draft.colour);
		chip.name_location = customize.draft.name_location;
		chip.size = customize.draft.size;
		chip.displays = customize.draft.displays.clone();
	}
	v.rebuild_sim();
	v.overlay_text_input = customize.saved_save_text;
	close_top_overlay(v);
	*status = Some(format!("Customized '{}'", root_chip_name));
}

/// CANCEL (or Escape over the whole workspace): discards the draft --
/// `close_top_overlay` restores both the text buffer and drops
/// `v.customize`.
pub(crate) fn cancel_customize(v: &mut ViewerState) {
	close_top_overlay(v);
}

// ---- option column -----------------------------------------------------

/// Centre -> Top -> Hidden -> Centre; Hidden frees the width the label
/// reserved, so the minimum-size floor is re-applied afterwards.
pub(crate) fn cycle_name_location(v: &mut ViewerState) {
	let Some(customize) = v.customize.as_mut() else { return };
	customize.draft.name_location = match customize.draft.name_location {
		NameLocation::Centre => NameLocation::Top,
		NameLocation::Top => NameLocation::Hidden,
		NameLocation::Hidden => NameLocation::Centre,
	};
	enforce_min_size(&mut customize.draft);
}

/// Palette swatch picked: set the body colour and regenerate the hex
/// field's contents so the two stay in lockstep.
pub(crate) fn pick_colour(v: &mut ViewerState, palette_index: usize) {
	let colour = theme::COLORS[palette_index % theme::COLORS.len()];
	let Some(customize) = v.customize.as_mut() else { return };
	customize.draft.colour = [colour[0], colour[1], colour[2], 1.0];
	v.overlay_text_input = hex_of(customize.draft.colour);
}

/// Re-parses whatever's typed in the hex field (typed chars were filtered
/// by the key handler already) into the draft colour. Invalid prefixes
/// simply leave the previous colour until the text becomes valid again.
pub(crate) fn apply_hex_input(v: &mut ViewerState) {
	let parsed = parse_hex_colour(&v.overlay_text_input);
	if let (Some(customize), Some(colour)) = (v.customize.as_mut(), parsed) {
		customize.draft.colour = colour;
	}
}

// ---- interactions ------------------------------------------------------

/// Corner-bracket press: begins resizing from that corner.
pub(crate) fn start_resize(v: &mut ViewerState, corner: usize) {
	if let Some(customize) = v.customize.as_mut() {
		if !customize.interaction.is_active() {
			customize.interaction = CustomizeInteraction::Resizing { corner: corner % 4 };
		}
	}
}

/// Press on a placed display's body: pick it up under the cursor.
pub(crate) fn start_move_display(v: &mut ViewerState, index: usize) {
	let Some(customize) = v.customize.as_mut() else { return };
	if customize.interaction.is_active() || customize.draft.displays.get(index).is_none() {
		return;
	}
	let cursor_world = customize.layout.screen_to_world(v.last_cursor);
	let centre = customize.draft.displays[index].position;
	customize.interaction =
		CustomizeInteraction::MovingDisplay { index, grab_offset: cursor_world - centre, original_pos: customize.draft.displays[index].position };
}

/// Press near a placed display's scale corner: drag distance sets scale.
pub(crate) fn start_scale_display(v: &mut ViewerState, index: usize) {
	let Some(customize) = v.customize.as_mut() else { return };
	if customize.interaction.is_active() || customize.draft.displays.get(index).is_none() {
		return;
	}
	let cursor_world = customize.layout.screen_to_world(v.last_cursor);
	let centre = customize.draft.displays[index].position;
	customize.interaction = CustomizeInteraction::ScalingDisplay {
		index,
		centre,
		start_dist: (cursor_world - centre).magnitude().max(0.05),
		start_scale: customize.draft.displays[index].scale,
	};
}

/// DISPLAYS-list row press: an unplaced entry picks its display up as a
/// ghost (only while idle; rows are disabled otherwise anyway); a
/// *placed* entry's click removes that display again -- the row's second
/// job, mirrored by its "(placed)" label in the builder.
pub(crate) fn place_list_entry(v: &mut ViewerState, entry_index: usize) {
	let Some(customize) = v.customize.as_mut() else { return };
	if customize.interaction.is_active() {
		return;
	}
	let library = &v.library;
	let entries = display_entries(&customize.draft, library);
	let Some(entry) = entries.get(entry_index) else { return };

	if entry.placed {
		customize.draft.displays.retain(|d| d.sub_chip_id != entry.sub_chip_id);
		return;
	}
	customize.interaction = CustomizeInteraction::PlacingDisplay { sub_chip_id: entry.sub_chip_id };
}

/// A preview click with no hotspot under it: commits whatever's being
/// carried (drops a placement at the cursor, finishes moves/resizes).
pub(crate) fn handle_preview_click(v: &mut ViewerState) {
	let Some(customize) = v.customize.as_mut() else { return };
	if !customize.layout.preview.contains(v.last_cursor) {
		return;
	}
	match customize.interaction {
		CustomizeInteraction::PlacingDisplay { sub_chip_id } => {
			let world = customize.layout.screen_to_world(v.last_cursor);
			let chip_type =
				customize.draft.sub_chips.iter().find(|s| s.id == sub_chip_id).and_then(|s| v.library.try_get(&s.name)).map(|d| d.chip_type);
			let scale = chip_type.map(default_display_scale).unwrap_or(1.0);
			customize.draft.displays.push(DisplayDescription::new(sub_chip_id, snap(world), scale));
			customize.interaction = CustomizeInteraction::None;
		}
		CustomizeInteraction::MovingDisplay { .. } | CustomizeInteraction::ScalingDisplay { .. } | CustomizeInteraction::Resizing { .. } => {
			customize.interaction = CustomizeInteraction::None;
		}
		CustomizeInteraction::None => {}
	}
}

/// Escape/right-click mid-interaction: put things back. Moves restore
/// their pre-grab position, scales their pre-grab size; ghosts vanish.
pub(crate) fn cancel_interaction(v: &mut ViewerState) {
	let Some(customize) = v.customize.as_mut() else { return };
	match customize.interaction {
		CustomizeInteraction::MovingDisplay { index, original_pos, .. } => {
			if let Some(display) = customize.draft.displays.get_mut(index) {
				display.position = original_pos;
			}
		}
		CustomizeInteraction::ScalingDisplay { index, start_scale, .. } => {
			if let Some(display) = customize.draft.displays.get_mut(index) {
				display.scale = start_scale;
			}
		}
		_ => {}
	}
	customize.interaction = CustomizeInteraction::None;
}

/// Delete while carrying: remove the carried display outright (or just
/// drop a fresh ghost's placement).
pub(crate) fn delete_held_display(v: &mut ViewerState) {
	let Some(customize) = v.customize.as_mut() else { return };
	match customize.interaction {
		CustomizeInteraction::MovingDisplay { index, .. } | CustomizeInteraction::ScalingDisplay { index, .. } => {
			customize.draft.displays.remove(index);
			customize.interaction = CustomizeInteraction::None;
		}
		CustomizeInteraction::PlacingDisplay { .. } | CustomizeInteraction::Resizing { .. } | CustomizeInteraction::None => {
			customize.interaction = CustomizeInteraction::None;
		}
	}
}

// ---- wheel -------------------------------------------------------------

/// Wheel over the DISPLAYS viewport: vertical scroll, clamped.
pub(crate) fn scroll_list(v: &mut ViewerState, amount_px: f32) {
	let Some(customize) = v.customize.as_mut() else { return };
	customize.list_scroll = (customize.list_scroll - amount_px).clamp(0.0, customize.list_scroll_max.max(0.0));
}

/// Wheel over the preview: zoom around auto-fit.
pub(crate) fn zoom_preview(v: &mut ViewerState, factor: f32) {
	let Some(customize) = v.customize.as_mut() else { return };
	customize.zoom_factor = (customize.zoom_factor * factor).clamp(0.35, 4.0);
}

// ---- per-frame ---------------------------------------------------------

/// Runs once per redraw before the UI stack is rebuilt: applies the
/// in-flight interaction against the current cursor so the freshly built
/// frame shows its live effect (the same immediate-mode beat the canvas's
/// camera pan follows).
pub(crate) fn update_live_interaction(v: &mut ViewerState) {
	let Some(customize) = v.customize.as_mut() else { return };
	if !customize.layout.valid || matches!(customize.interaction, CustomizeInteraction::None | CustomizeInteraction::PlacingDisplay { .. }) {
		return;
	}
	let cursor_world = customize.layout.screen_to_world(v.last_cursor);

	match customize.interaction {
		CustomizeInteraction::Resizing { .. } => {
			// The dragged corner tracks the cursor symmetrically: whichever
			// corner is grabbed, the body grows/shrinks about its centre,
			// snapped to the grid so pins stay aligned with grid lines.
			let desired = Vec2::new(snap_scalar(cursor_world.x.abs()) * 2.0, snap_scalar(cursor_world.y.abs()) * 2.0);
			customize.draft.size = component_max(desired, min_size_for(&customize.draft));
		}
		CustomizeInteraction::MovingDisplay { index, grab_offset, .. } => {
			if let Some(display) = customize.draft.displays.get_mut(index) {
				display.position = snap(cursor_world - grab_offset);
			}
		}
		CustomizeInteraction::ScalingDisplay { index, centre, start_dist, start_scale } => {
			if let Some(display) = customize.draft.displays.get_mut(index) {
				let dist = (cursor_world - centre).magnitude();
				display.scale = (start_scale * dist / start_dist).clamp(0.05, 100.0);
			}
		}
		CustomizeInteraction::None | CustomizeInteraction::PlacingDisplay { .. } => {}
	}
}

/// Builds the customize layer from live state (frame.rs calls this for
/// `Overlay::CustomizeChip`) and caches the produced layout back onto the
/// state for the next frame's event mapping.
pub(crate) fn build_layer(v: &ViewerState, vw: f32, vh: f32, mouse: Vec2) -> CustomizeFrameOut {
	let Some(customize) = v.customize.as_ref() else {
		return CustomizeFrameOut { frame: Default::default(), layout: Default::default(), list_scroll_max: 0.0 };
	};
	let entries = display_entries(&customize.draft, &v.library);
	let sim_guard = v.sim.lock();
	let pin_state = SimulatorPinState { sim: &sim_guard, scope: sim_guard.root() };
	let ctx = CustomizeCtx {
		draft: &customize.draft,
		library: &v.library,
		entries: &entries,
		interaction: customize.interaction,
		hex_text: &v.overlay_text_input,
		list_scroll: customize.list_scroll,
		zoom_factor: customize.zoom_factor,
		pin_state: &pin_state,
	};

	let mut out = crate::render::customize_ui::build_chip_customizer(&ctx, vw, vh, mouse);
	out.list_scroll_max = out.list_scroll_max.max(0.0);
	out
}

/// Writes the freshly-built frame's screen facts back onto the customize
/// state (called by frame.rs right after `build_layer`).
pub(crate) fn cache_layout(v: &mut ViewerState, layout: crate::render::customize_ui::PreviewLayout, list_scroll_max: f32) {
	if let Some(customize) = v.customize.as_mut() {
		customize.layout = layout;
		customize.list_scroll_max = list_scroll_max;
	}
}

// ---- helpers -----------------------------------------------------------

/// Component-wise minimum body footprint for `draft`: pins plus the name
/// label's estimated width (which `NameLocation::Hidden` waives).
fn min_size_for(draft: &ChipDescription) -> Vec2 {
	let inputs: Vec<_> = draft.input_pins.iter().map(|p| p.bit_count).collect();
	let outputs: Vec<_> = draft.output_pins.iter().map(|p| p.bit_count).collect();
	layout::calculate_min_chip_size(&inputs, &outputs, draft, theme::FONT_SIZE_CHIP_NAME)
}

/// Grows `draft.size` to at least its own minimum (never shrinks).
fn enforce_min_size(draft: &mut ChipDescription) {
	draft.size = component_max(draft.size, min_size_for(draft));
}

fn component_max(a: Vec2, b: Vec2) -> Vec2 {
	Vec2::new(a.x.max(b.x), a.y.max(b.y))
}

fn snap(v: Vec2) -> Vec2 {
	Vec2::new(snap_scalar(v.x), snap_scalar(v.y))
}

fn snap_scalar(value: f32) -> f32 {
	(value / GRID_SIZE).round() * GRID_SIZE
}

/// Alpha-0 ("unset") colours commit as fully-opaque defaults rather than
/// staying invisible-on-save.
fn finalize_colour(colour: [f32; 4]) -> [f32; 4] {
	if colour[3] > 0.0 {
		colour
	} else {
		let body = theme::CHIP_BODY_COL;
		[body[0], body[1], body[2], 1.0]
	}
}

fn hex_of(colour: [f32; 4]) -> String {
	let byte = |c: f32| ((c.clamp(0.0, 1.0)) * 255.0).round() as u8;
	format!("#{:02X}{:02X}{:02X}", byte(colour[0]), byte(colour[1]), byte(colour[2]))
}

/// Accepts `#RRGGBB` / `RRGGBB`; anything else is `None` (field keeps
/// its last valid colour meanwhile).
fn parse_hex_colour(text: &str) -> Option<[f32; 4]> {
	let trimmed = text.trim();
	let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
	if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
		return None;
	}
	let value = u32::from_str_radix(digits, 16).ok()?;
	Some([((value >> 16) & 0xFF) as f32 / 255.0, ((value >> 8) & 0xFF) as f32 / 255.0, (value & 0xFF) as f32 / 255.0, 1.0])
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{ChipType, PinBitCount, PinDescription};
	use crate::pin_state::PinState;
	use crate::render::customize_ui::{display_entries, CustomizeInteraction};

	fn sized_draft() -> ChipDescription {
		let mut d = ChipDescription::new("D", ChipType::Custom);
		d.input_pins.push(PinDescription::new("A", 1, PinBitCount::Bit1));
		d.output_pins.push(PinDescription::new("Q", 2, PinBitCount::Bit1));
		d.size = Vec2::new(2.0, 2.0);
		d
	}

	#[test]
	fn hex_round_trip_and_rejects() {
		assert_eq!(parse_hex_colour("#FF8000"), Some([1.0, 128.0 / 255.0, 0.0, 1.0]));
		assert_eq!(parse_hex_colour("00ff00"), Some([0.0, 1.0, 0.0, 1.0]));
		assert_eq!(parse_hex_colour("#F80"), None);
		assert_eq!(parse_hex_colour("#GGGGGG"), None);
		assert_eq!(hex_of([1.0, 0.5, 0.0, 1.0]), "#FF8000");
	}

	#[test]
	fn snapping_lands_on_grid_lines() {
		assert!((snap_scalar(0.13) - GRID_SIZE).abs() < 1e-6);
		assert_eq!(snap(Vec2::new(0.06, -0.06)), Vec2::ZERO, "inside the half-grid deadzone snaps back to the line");
	}

	#[test]
	fn component_max_never_shrinks_a_dimension() {
		assert_eq!(component_max(Vec2::new(3.0, 0.5), Vec2::new(1.0, 2.0)), Vec2::new(3.0, 2.0));
	}

	#[test]
	fn min_size_respects_pins_and_hidden_names() {
		let mut draft = sized_draft();
		draft.name = "A very long chip name indeed".to_string();
		let visible = min_size_for(&draft);
		draft.name_location = NameLocation::Hidden;
		let hidden = min_size_for(&draft);
		assert!(visible.x > hidden.x, "hiding the name must free width");
		assert!((visible.y - hidden.y).abs() < 1e-6, "height comes from pins either way");
	}

	#[test]
	fn interaction_payload_shapes_are_copyable_data() {
		// The interaction enum crosses the render boundary by value; make
		// sure it stays a small Copy payload.
		let i = CustomizeInteraction::MovingDisplay { index: 1, grab_offset: Vec2::ZERO, original_pos: Vec2::ZERO };
		let j = i;
		assert_eq!(i, j);
	}

	#[test]
	fn display_entries_feed_place_list_flow() {
		// Sanity-check the shared helper this module's place_list_entry
		// consumes (full behaviour covered in customize_ui's own tests).
		let mut lib = crate::ChipLibrary::new();
		lib.add(sized_draft());
		let host = ChipDescription::new("H", ChipType::Custom);
		assert!(display_entries(&host, &lib).is_empty());
	}

	/// Full-pipeline check: with a driven input wired into an embedded
	/// 7-segment, the customize layer's geometry must contain the lit
	/// segment colour (not just the off palette) -- i.e. the preview reads
	/// live simulator state, not a blank.
	#[test]
	fn customize_preview_lights_embedded_displays_from_live_sim_state() {
		use crate::viewer::frame::build_viewer_stack;
		use crate::viewer::state::{editor_action, Overlay, ViewerAction};
		use crate::{register_all_builtins, render::ui_stack::LayerId};

		let mut library = crate::ChipLibrary::new();
		register_all_builtins(&mut library);

		let mut panel = ChipDescription::new("Panel", crate::ChipType::Custom);
		panel.input_pins.push(PinDescription::new("IN", 1, PinBitCount::Bit1));
		panel.size = Vec2::new(3.0, 2.0);
		panel.sub_chips.push(crate::SubChipDescription {
			name: "7-SEGMENT".into(),
			id: 4,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		panel.wires.push(crate::WireDescription::new(crate::PinAddress::new(1, 0), crate::PinAddress::new(4, 0)));
		panel.displays.push(DisplayDescription::new(4, Vec2::new(0.5, 0.25), 1.0));
		library.add(panel.clone());

		let mut v = ViewerState::new("", library, "Panel".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		v.last_cursor = Vec2::new(640.0, 400.0);
		v.camera_fitted = true;

		// Drive the input high (what toggling a switch does), then open the
		// customizer and build one frame of the real UI stack. Pausing and
		// requesting a single step makes the background sim thread advance
		// by exactly one deterministic tick (wall-clock pacing would make
		// "one frame" an arbitrary number of ticks).
		v.sim.lock().set_driven_input(1, PinState::HIGH);
		v.prefs.prefs_sim_paused = true;
		open_customize(&mut v);
		assert!(v.overlays.contains(&Overlay::CustomizeChip));

		// A warm-up frame pushes the pause pref into the sim handle --
		// what the running app's earlier frames have always done by the
		// time the player pauses and single-steps.
		let _ = build_viewer_stack(&mut v, None, 1280.0, 800.0, Vec2::new(900.0, 400.0));

		v.sim.request_single_step();
		let stepped = {
			let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
			while std::time::Instant::now() < deadline && v.sim.lock().simulation_frame == 0 {
				std::thread::sleep(std::time::Duration::from_micros(200));
			}
			v.sim.lock().simulation_frame >= 1
		};
		assert!(stepped, "the paused single step never ran");

		let stack = build_viewer_stack(&mut v, None, 1280.0, 800.0, Vec2::new(900.0, 400.0));
		let layer = stack.layers().iter().find(|l| l.id == LayerId::CustomizePanel).expect("customize layer built");
		let _ = editor_action as fn(crate::render::editor_ui::EditorAction) -> ViewerAction;

		let lit: std::collections::HashSet<_> = layer.geometry.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect();
		let seg_on_a = theme::SEVEN_SEG_COLS[1].map(f32::to_bits);
		assert!(lit.contains(&seg_on_a), "lit 7-segment colour must appear; got {} distinct colours", lit.len());
	}
}
