//! The actual, integrated Digital Logic Sim app: `cargo run` opens a project-picker startup screen
//! (same on-disk layout and location as the original Unity build), lets you open an existing project
//! or create a new one, then switches the same window over to the chip viewer for whichever project
//! you picked. It drives `MainMenu` from real mouse/keyboard events via `render::menu_ui`, and reuses
//! the same load/build/render sequence `viewer.rs` uses once a project is opened. Needs a real GPU.

use logic_sim::json::{ChipCollection, ProjectDescription};
use logic_sim::render::camera::Camera;
use logic_sim::render::context_menu::{self, ContextMenuAction, ContextMenuButton, ContextMenuItem, ContextMenuState};
use logic_sim::render::editor_ui::{self, EditorAction, EditorButton, LibrarySelection};
use logic_sim::render::gpu::Renderer;
use logic_sim::render::layout;
use logic_sim::render::menu_ui::{self, UiAction};
use logic_sim::render::scene::{
	apply_alpha, bounding_box, build_grid, build_scene, closest_wire_hit, delete_wire, hit_test_any_pin, hit_test_dev_pin, hit_test_sub_chip,
	hit_test_wire, place_sub_chips, AllLow, SceneGeometry, SimulatorPinState,
};
use logic_sim::render::theme;
use logic_sim::sim::key_mods_bits;
use logic_sim::sim::Simulator;
use logic_sim::structs::Vec2;
use logic_sim::ui_menu::{MainMenu, MenuOutcome, PopupKind};
use logic_sim::{
	default_chip_collections, default_starred_list, load_project, register_all_builtins, ChipDescription, ChipLibrary, ChipType, PinAddress,
	PinBitCount, SavePaths, Saver, SubChipDescription, WireDescription,
};
use std::path::PathBuf;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Which (if any) of the editor overlays from `render::editor_ui` is
/// currently open on top of the viewer. Only one at a time -- matches how
/// the original's popups/menus stack (library sidebar aside, which can
/// stay open alongside browsing, everything else here is modal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
	None,
	Library,
	Search,
	Preferences,
	Naming,
	KeySelect,
	RomEditor,
	SaveChip,
}

/// What `Overlay::Naming`'s Confirm/Enter should actually *do* with the
/// typed text once confirmed -- the popup itself (a title + one text
/// field) is generic, reused for the project-rename prompt as well as
/// every "Label"/"Configure" popup opened from a right-click context menu
/// (see `apply_context_menu_action`). Defaults to `RenameProject` so the
/// existing 'n' shortcut keeps working unchanged; every other variant is
/// set right before opening the overlay and consumed (reset back to
/// `RenameProject`) by `confirm_naming_popup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NamingPurpose {
	#[default]
	RenameProject,
	/// Set a placed subchip's display label (`SubChipDescription::label`).
	/// `i32` is that subchip's id within the current root chip.
	LabelComponent(i32),
	/// Set the name of one of the *current root chip's own* boundary
	/// dev-pins (`ChipDescription::input_pins`/`output_pins`) -- never a
	/// subchip's pin, per the brief.
	LabelDevPin { is_input: bool, id: i32 },
	/// Pulse length, in simulation ticks (`SubChipDescription::internal_data[0]`,
	/// mirrored into `Simulator`'s per-chip `internal_state[DURATION]` --
	/// see `sim::process_builtin_chip`'s `Pulse` arm).
	ConfigurePulseDuration(i32),
}

/// What `Overlay::KeySelect`'s Confirm/Enter should do with the chosen
/// key -- same idea as `NamingPurpose`, for the one overlay that isn't a
/// plain text field. Defaults to `Rebind` (today's placeholder "not yet
/// wired to an action" behaviour); `ConfigureKeyChar` is used when a
/// `Key` component's own "Configure" popup reuses this same overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum KeySelectPurpose {
	#[default]
	Rebind,
	/// `SubChipDescription::internal_data[0]`, the ASCII code this `Key`
	/// instance listens for -- `i32` is that subchip's id.
	ConfigureKeyChar(i32),
}

/// Working state for the ROM contents grid editor (`Overlay::RomEditor` /
/// `editor_ui::build_rom_editor_popup`) -- a draft copy of all 256 words
/// plus which one's currently selected, kept separate from
/// `SubChipDescription::internal_data` until "Apply" is clicked (same
/// "edit a draft, commit on confirm" shape every other overlay here
/// uses).
struct RomEditorState {
	/// Id of the subchip (within the current root chip) being configured.
	component_id: i32,
	data: Vec<u32>,
	selected: usize,
}

/// Convert winit's modifier state into the `Simulator::key_modifiers`
/// bitmask (see `key_mods_bits`), using winit's own boolean accessors
/// rather than its raw `bits()` value -- see the doc comment on
/// `key_mods_bits` for why.
fn encode_modifiers(mods: ModifiersState) -> u32 {
	let mut bits = 0u32;
	if mods.shift_key() {
		bits |= key_mods_bits::SHIFT;
	}
	if mods.control_key() {
		bits |= key_mods_bits::CONTROL;
	}
	if mods.alt_key() {
		bits |= key_mods_bits::ALT;
	}
	if mods.super_key() {
		bits |= key_mods_bits::SUPER;
	}
	bits
}

/// One endpoint of an in-progress wire placement (`ViewerState::pending_wire`),
/// fixed at the moment the wire is started -- either a real pin (a
/// subchip's own, or one of the current chip's own boundary dev-pins) or
/// a tap point along an existing wire's line.
#[derive(Debug, Clone, Copy)]
enum PendingWireEnd {
	Pin {
		owner_id: i32,
		pin_id: i32,
		is_source: bool,
		position: Vec2,
	},
	/// Tapping onto a wire always plays the *source* role for the new
	/// branch wire (see `try_start_pending_wire`'s doc comment) -- the
	/// tapped wire's own real source pin, needed to build the eventual
	/// `WireDescription::new_tapped_source`, travels along here rather
	/// than being re-looked-up at completion time.
	WireTap {
		wire_index: usize,
		segment_index: i32,
		point: Vec2,
		source_pin_address: PinAddress,
	},
}

impl PendingWireEnd {
	fn is_source(&self) -> bool {
		match self {
			PendingWireEnd::Pin { is_source, .. } => *is_source,
			PendingWireEnd::WireTap { .. } => true,
		}
	}

	fn position(&self) -> Vec2 {
		match self {
			PendingWireEnd::Pin { position, .. } => *position,
			PendingWireEnd::WireTap { point, .. } => *point,
		}
	}
}

/// State for an in-progress wire placement: the endpoint it started
/// from, plus any bend ("turn") points the player has since clicked on
/// empty canvas space, in click order -- becomes the finished
/// `WireDescription::points` once the wire is completed at a second,
/// opposite-role endpoint (reversed first if that second endpoint turns
/// out to be the wire's real *source*, since `points` always runs
/// source-to-target). `None` on `ViewerState` whenever no wire is being
/// placed.
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PendingWire {
	start: PendingWireEnd,
	bend_points: Vec<Vec2>,
	bit_count: PinBitCount,
}

/// State specific to viewing/simulating one open project's chip, split out
/// so `App` can hold either this or the menu depending on `Screen`.
struct ViewerState {
	project_name: String,
	library: ChipLibrary,
	root_chip_name: String,
	sim: Simulator,
	camera: Camera,
	dragging: bool,
	last_cursor: Vec2,
	camera_fitted: bool,
	show_grid: bool,

	/// The project's saved prefs/collections, edited live by the
	/// preferences/library overlays and written back to disk on Apply.
	prefs: ProjectDescription,
	overlay: Overlay,
	/// Shared text buffer for whichever overlay currently has a text
	/// field open (search query, or the naming popup's text).
	overlay_text_input: String,
	/// Pending key choice for the key-select popup.
	overlay_key_choice: Option<char>,
	/// What `Overlay::Naming`'s confirm should do -- see `NamingPurpose`'s
	/// docs.
	naming_purpose: NamingPurpose,
	/// What `Overlay::KeySelect`'s confirm should do -- see
	/// `KeySelectPurpose`'s docs.
	key_select_purpose: KeySelectPurpose,
	/// Draft state for `Overlay::RomEditor`, when open.
	rom_editor: Option<RomEditorState>,
	/// Hit-boxes from the overlay's *last drawn* frame -- same
	/// immediate-mode pattern as `App::last_menu_buttons`.
	last_overlay_buttons: Vec<EditorButton>,

	/// Which row of the real `Overlay::Library` panel is currently
	/// selected -- see `editor_ui::LibrarySelection`'s docs.
	library_selection: LibrarySelection,
	/// Whether the library's inline "new collection" name field is open.
	library_creating_collection: bool,
	/// Whether the library's inline "rename collection" name field is
	/// open (for whichever collection `library_selection` points at).
	library_renaming_collection: bool,
	/// Whether the library's inline chip-delete confirmation is open.
	library_confirming_chip_delete: bool,
	/// Whether the library's inline collection-delete confirmation is
	/// open.
	library_confirming_collection_delete: bool,
	/// Precomputed message shown by whichever of the above two
	/// confirmations is open -- see `chip_delete_confirm_message`.
	library_delete_message: String,
	/// Name of the starred collection whose flyout is currently open in
	/// the bottom bar (`editor_ui::build_starred_collection_popup`), if
	/// any.
	bottom_bar_open_collection: Option<String>,
	/// Hit-boxes from the bottom bar's *last drawn* frame -- same
	/// immediate-mode pattern as `last_overlay_buttons`.
	last_bottom_bar_buttons: Vec<EditorButton>,
	/// Hit-boxes from the bottom bar's starred-collection flyout's *last
	/// drawn* frame, if one is open.
	last_bottom_bar_popup_buttons: Vec<EditorButton>,

	/// The right-click popup currently open (over a canvas component or
	/// a library row), if any -- see `render::context_menu`. Always
	/// drawn/hit-tested as the top-most layer of the frame, above even a
	/// modal `overlay`, so it's never hidden behind (or accidentally
	/// swallowed by) whatever else is open.
	context_menu: Option<ContextMenuState>,
	/// Hit-boxes from the context menu's *last drawn* frame -- same
	/// immediate-mode pattern as `last_overlay_buttons`.
	last_context_menu_buttons: Vec<ContextMenuButton>,

	/// The wire currently being placed by clicking one endpoint then
	/// another, if any -- see `PendingWire`'s docs. Cleared (`None`)
	/// whenever the root chip changes, since a pending endpoint's
	/// `wire_index`/subchip id would otherwise silently refer to
	/// whatever now happens to sit at that index in the new chip.
	pending_wire: Option<PendingWire>,

	/// The name of a chip currently picked up for placement (the
	/// library's "USE" button, `EditorAction::PlaceChip`), if any --
	/// drawn as a translucent preview following the cursor
	/// (`build_pending_place_scene`) and dropped as a real
	/// `SubChipDescription` on the next canvas click that lands on free
	/// space (`try_place_pending_chip`). Mutually exclusive with
	/// `pending_wire` in practice (starting one clears the other), and
	/// cleared on the same triggers `pending_wire` is: Escape, a
	/// right-click, or the root chip changing.
	pending_place: Option<String>,
}

impl ViewerState {
	/// Rebuilds `self.sim` from `self.library`'s current copy of
	/// `self.root_chip_name` -- called after any edit that changes the
	/// simulated structure (deleting a component/wire, re-configuring a
	/// Pulse/Key/ROM, etc). Deliberately leaves the camera exactly where
	/// it is: an in-place edit to the chip you're already looking at
	/// shouldn't yank the view back to a fresh auto-fit every time (that
	/// was `camera_fitted`'s old, wrong role here). Actually switching to
	/// a *different* chip is a separate, explicit action --
	/// `open_chip_by_name` resets `camera_fitted` itself, only when the
	/// root chip is actually changing.
	fn rebuild_sim(&mut self) {
		let root_desc = self.library.get(&self.root_chip_name).clone();
		let held_keys = std::mem::take(&mut self.sim.held_keys);
		let key_modifiers = self.sim.key_modifiers;
		self.sim = Simulator::build(&root_desc, &self.library);
		self.sim.held_keys = held_keys;
		self.sim.key_modifiers = key_modifiers;
	}
}

/// Finds whichever bit of one of `root_desc`'s own boundary *input*
/// dev-pins (if any) `world_pos` landed on -- the same per-bit grid
/// `render::scene::draw_input_dev_pin_body` draws for each input pin
/// (one clickable circle for a 1-bit input, a 2x2/2x4 grid of cells for
/// 4/8-bit) -- returning that pin's own id and the clicked bit's index.
/// Output pins are never hit -- only inputs are meant to be toggled by a
/// click.
fn hit_test_root_input_pin_click(root_desc: &logic_sim::description::ChipDescription, world_pos: Vec2) -> Option<(i32, u32)> {
	for pin in &root_desc.input_pins {
		if let Some(bit_index) = logic_sim::render::scene::hit_test_input_dev_pin_bit(world_pos, pin.position, pin.bit_count) {
			return Some((pin.id, bit_index));
		}
	}
	None
}

/// Flips one bit (`bit_index`) of input dev-pin `pin_id`'s own
/// `PinDescription::driven_state`, directly on its entry in `library` --
/// see that field's docs for why it lives there rather than in a
/// separate lookup on `ViewerState`. The tristate flags half of the
/// packed state is left untouched (stays "driven", i.e. `0`) -- a
/// clicked input is always actively driven, never floating.
fn toggle_driven_input_bit(library: &mut ChipLibrary, root_chip_name: &str, pin_id: i32, bit_index: u32) {
	let chip = library.get_mut(root_chip_name);
	if let Some(pin) = chip.input_pins.iter_mut().find(|p| p.id == pin_id) {
		let last_state = pin.driven_state;
		let mut bits = logic_sim::pin_state::bit_states(last_state);
		bits ^= 1 << bit_index;
		logic_sim::pin_state::set(&mut pin.driven_state, bits, logic_sim::pin_state::tristate_flags(last_state));
	}
}

/// Fixed screen-pixel tolerance for landing a click on a wire, converted
/// to world units -- same value/reasoning as the one `handle_right_mouse_button`
/// already uses for wire deletion, so a tap-to-place click feels exactly
/// as forgiving as a click-to-delete one at any zoom level.
fn wire_click_tolerance(camera: &Camera) -> f32 {
	6.0 / camera.zoom.max(0.0001)
}

/// Attempts to start a new wire placement from whatever's under
/// `world_pos`: a subchip's own pin, one of the current chip's own
/// boundary *output* dev-pins, or a tap point along an existing wire's
/// line. Returns whether a placement was actually started (i.e. whether
/// the click should be treated as consumed).
fn try_start_pending_wire(v: &mut ViewerState, world_pos: Vec2) -> bool {
	let root_desc = v.library.get(&v.root_chip_name);
	let placed = place_sub_chips(root_desc, &v.library);

	if let Some(hit) = hit_test_any_pin(root_desc, &placed, world_pos) {
		v.pending_wire = Some(PendingWire {
			start: PendingWireEnd::Pin { owner_id: hit.owner_id, pin_id: hit.pin_id, is_source: hit.is_wire_source(), position: hit.position },
			bend_points: Vec::new(),
			bit_count: hit.bit_count,
		});
		return true;
	}

	let max_dist = wire_click_tolerance(&v.camera);
	if let Some(tap) = closest_wire_hit(root_desc, &v.library, world_pos, max_dist) {
		let source_pin_address = root_desc.wires[tap.wire_index].source_pin_address;
		v.pending_wire = Some(PendingWire {
			start: PendingWireEnd::WireTap { wire_index: tap.wire_index, segment_index: tap.segment_index, point: tap.point, source_pin_address },
			bend_points: Vec::new(),
			bit_count: tap.bit_count,
		});
		return true;
	}

	false
}

/// Advances an in-progress wire placement (`v.pending_wire`, assumed
/// `Some`) with a click at `world_pos`:
///  - landing on a pin of the *opposite* role (see `PinHit::is_wire_source`/
///    `PendingWireEnd::is_source`) completes the wire, connecting through
///    any bend points collected so far;
///  - landing on a pin of the *same* role (e.g. input-to-input,
///    output-to-output) is rejected with a status message, leaving the
///    placement active so the player can just try a different pin;
///  - landing on an existing wire or a component body is ignored outright
///    (deliberately *not* a "turn" -- see this method's caller's doc
///    comment on the empty-space branch below);
///  - anywhere else (empty canvas) adds a bend ("turn") point there and
///    leaves the placement active.
fn try_continue_pending_wire(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	let root_chip_name = v.root_chip_name.clone();
	let root_desc = v.library.get(&root_chip_name);
	let placed = place_sub_chips(root_desc, &v.library);

	if let Some(hit) = hit_test_any_pin(root_desc, &placed, world_pos) {
		let pending = v.pending_wire.as_ref().expect("caller only calls this with a pending wire");
		/*		// optional : if you want to only connect same bitcount wires
			   if pending.bit_count != hit.bit_count {
				   *status = Some("Can't connect different bitcounts".to_string());
				   return;
			   }
		*/
		if pending.start.is_source() == hit.is_wire_source() {
			*status = Some(if hit.is_wire_source() {
				"Can't connect an output to an output".to_string()
			} else {
				"Can't connect an input to an input".to_string()
			});
			return;
		}

		let pending = v.pending_wire.take().expect("checked above");
		let end_pin_address = PinAddress::new(hit.owner_id, hit.pin_id);

		let mut wire = if pending.start.is_source() {
			match pending.start {
				PendingWireEnd::Pin { owner_id, pin_id, .. } => WireDescription::new(PinAddress::new(owner_id, pin_id), end_pin_address),
				PendingWireEnd::WireTap { wire_index, segment_index, point, source_pin_address } => {
					WireDescription::new_tapped_source(source_pin_address, end_pin_address, wire_index as i32, segment_index, point)
				}
			}
		} else {
			// The clicked pin is the real source; the wire always started from a plain pin in this
			// branch (a wire tap is always treated as the source -- see `PendingWireEnd::is_source`).
			let PendingWireEnd::Pin { owner_id, pin_id, .. } = pending.start else {
				unreachable!("a wire tap is always the source end, so this branch never sees one")
			};
			WireDescription::new(end_pin_address, PinAddress::new(owner_id, pin_id))
		};

		wire.points = pending.bend_points;
		if !pending.start.is_source() {
			wire.points.reverse();
		}

		v.library.get_mut(&root_chip_name).wires.push(wire);
		v.rebuild_sim();
		*status = None;
		return;
	}

	let max_dist = wire_click_tolerance(&v.camera);
	let on_wire = hit_test_wire(root_desc, &v.library, world_pos, max_dist).is_some();
	let on_component = hit_test_sub_chip(&placed, world_pos).is_some();
	if on_wire || on_component {
		// Neither a pin nor empty space -- ignored outright (not a "turn"), so the placement just
		// stays exactly as it was and the player can click somewhere more useful instead.
		return;
	}

	let pending = v.pending_wire.as_mut().expect("caller only calls this with a pending wire");
	pending.bend_points.push(world_pos);
}

/// Alpha applied to a chip's translucent placement preview (see
/// `build_pending_place_scene`) -- 75%, so the ghost reads clearly as
/// "not yet placed" without being hard to make out against the canvas.
const PENDING_PLACEMENT_ALPHA: f32 = 0.75;

/// Next free subchip id for `chip`'s own `sub_chips` list: one past
/// whatever the highest existing id is, or `1` if it has none yet
/// (`SubChipDescription::id` docs say IDs are `> 0`).
fn next_sub_chip_id(chip: &ChipDescription) -> i32 {
	chip.sub_chips.iter().map(|s| s.id).max().unwrap_or(0) + 1
}

/// Attempts to drop `v.pending_place`'s chip (assumed `Some`) at
/// `world_pos`. Only actually places it -- and clears `v.pending_place`
/// -- when the click lands on genuinely free canvas space: not a
/// subchip's pin, one of the current chip's own boundary dev-pins, an
/// existing placed component's body, or a wire. Landing on any of those
/// just leaves the pending placement active untouched, so the player can
/// simply try again elsewhere (mirrors `try_continue_pending_wire`'s
/// "component/wire clicks are ignored outright" behaviour). The new
/// instance gets a fresh id (`next_sub_chip_id`), no label, no saved
/// internal data (chip types that need some, e.g. ROM/Key, start at
/// their type's default and are configured afterwards via their own
/// right-click popups, same as today), and no output-pin colour
/// overrides.
///
/// Also defensively re-checks `would_create_cycle` -- unlike a free-space
/// miss, this can never be resolved by clicking somewhere else, so
/// (unlike the free-space case) it cancels the pending placement outright
/// and reports why via `status`, rather than leaving it dangling for a
/// retry that could never succeed.
fn try_place_pending_chip(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	let root_chip_name = v.root_chip_name.clone();
	let root_desc = v.library.get(&root_chip_name);
	let placed = place_sub_chips(root_desc, &v.library);

	let max_dist = wire_click_tolerance(&v.camera);
	let blocked = hit_test_any_pin(root_desc, &placed, world_pos).is_some()
		|| hit_test_dev_pin(root_desc, world_pos).is_some()
		|| hit_test_sub_chip(&placed, world_pos).is_some()
		|| hit_test_wire(root_desc, &v.library, world_pos, max_dist).is_some();
	if blocked {
		return;
	}

	// Defensive re-check: the "USE"/bottom-bar buttons that set `pending_place` in the first
	// place are already greyed out for a chip that would cycle (see `would_create_cycle`'s
	// docs), but a click always gets the final say rather than trusting that alone.
	let name = v.pending_place.take().expect("caller only calls this with a pending placement");
	if would_create_cycle(&v.library, &root_chip_name, &name) {
		*status = Some(format!("Can't place '{name}' inside '{root_chip_name}' -- it would contain itself"));
		return;
	}

	let chip = v.library.get_mut(&root_chip_name);
	let id = next_sub_chip_id(chip);
	chip.sub_chips.push(SubChipDescription { name, id, internal_data: None, position: world_pos, label: None, pin_colour_info: Vec::new() });
	v.rebuild_sim();
}

/// Builds the translucent "ghost" preview of the chip currently pending
/// placement, floating at the cursor's live world position. Reuses the
/// exact same `build_scene` pipeline a real placed component draws
/// through -- body, pins, name label, and any type-specific rendering
/// (a Key's bound letter, an LED's tint, a display's live pixels, ...)
/// -- by wrapping the chip in a throwaway single-subchip `ChipDescription`,
/// so the preview can never drift out of sync with what actually gets
/// placed. Faded to `PENDING_PLACEMENT_ALPHA` via `scene::apply_alpha`.
/// Returns `None` if `chip_name` no longer resolves in `library` (e.g. it
/// was deleted while pending -- shouldn't normally happen, but avoids a
/// panic in `place_sub_chips` if it somehow does).
fn build_pending_place_scene(library: &ChipLibrary, chip_name: &str, cursor_world_pos: Vec2) -> Option<SceneGeometry> {
	library.try_get(chip_name)?;

	let mut ghost = ChipDescription::new("__pending_placement_ghost__", ChipType::Custom);
	ghost.sub_chips.push(SubChipDescription {
		name: chip_name.to_string(),
		id: 0,
		internal_data: None,
		position: cursor_world_pos,
		label: None,
		pin_colour_info: Vec::new(),
	});

	let mut geo = build_scene(&ghost, library, &AllLow, None);
	apply_alpha(&mut geo, PENDING_PLACEMENT_ALPHA);
	Some(geo)
}

/// Draws the in-progress wire preview: a line from its start endpoint,
/// through any turn points placed so far, to the cursor's current world
/// position -- so the player can see what they're about to connect
/// before actually clicking the second endpoint. Purely cosmetic (never
/// touches `chip.wires`), drawn in `theme::PIN_HIGHLIGHT_COL` so it
/// reads clearly against both wires and pins.
fn draw_pending_wire_preview(geo: &mut SceneGeometry, pending: &PendingWire, cursor_world_pos: Vec2) {
	let mut path = Vec::with_capacity(pending.bend_points.len() + 2);
	path.push(pending.start.position());
	path.extend_from_slice(&pending.bend_points);
	path.push(cursor_world_pos);
	geo.add_polyline(&path, layout::WIRE_THICKNESS, theme::PIN_HIGHLIGHT_COL);

	// Small markers at each already-placed turn point, so a bend the player just
	// clicked in stays visible even where the preview line passes straight through it.
	for &turn in &pending.bend_points {
		geo.add_circle(turn, layout::WIRE_THICKNESS * 1.5, theme::PIN_HIGHLIGHT_COL, 12);
	}
}

/// `editor_ui`'s builders lay out screen-pixel coordinates as if drawn
/// through a fixed camera positioned at `(vw/2, vh/2)` with `zoom = 1.0`
/// (see `menu_ui::to_world`, the same convention the main menu uses) --
/// appropriate for `Screen::Menu`, where that's exactly the camera used.
/// The viewer, though, draws its scene through `v.camera`, which pans and
/// zooms freely. Re-mapping each overlay vertex/label from "the pixel it
/// was drawn at under the fixed camera" to "the world point that lands on
/// that same pixel under `v.camera`" keeps overlays pinned to the screen
/// (constant position and size in pixels) no matter how far the chip
/// canvas underneath has been panned/zoomed, using one real render pass
/// instead of needing a second camera/pipeline in `render::gpu`.
fn pin_overlay_to_screen(mut geometry: SceneGeometry, camera: &Camera, _vw: f32, vh: f32) -> SceneGeometry {
	let to_screen_px = |world: Vec2| Vec2::new(world.x, vh - world.y); // inverse of `menu_ui::to_world`, which is its own inverse
	for v in &mut geometry.triangles {
		v.pos = camera.screen_to_world(to_screen_px(v.pos));
	}
	for l in &mut geometry.labels {
		l.pos = camera.screen_to_world(to_screen_px(l.pos));
		l.font_size /= camera.zoom;
		l.width /= camera.zoom;
	}
	geometry
}

/// Advances the wheel field at `row_index` (matching the row order
/// `editor_ui::build_preferences_panel` draws in) to its next option,
/// wrapping around.
fn cycle_pref(prefs: &mut ProjectDescription, row_index: usize) {
	match row_index {
		0 => prefs.prefs_main_pin_names_display_mode = (prefs.prefs_main_pin_names_display_mode + 1) % 3,
		1 => prefs.prefs_chip_pin_names_display_mode = (prefs.prefs_chip_pin_names_display_mode + 1) % 3,
		2 => prefs.prefs_grid_display_mode = (prefs.prefs_grid_display_mode + 1) % 2,
		3 => prefs.prefs_snapping = (prefs.prefs_snapping + 1) % 3,
		4 => prefs.prefs_straight_wires = (prefs.prefs_straight_wires + 1) % 3,
		5 => prefs.prefs_sim_paused = !prefs.prefs_sim_paused,
		_ => {}
	}
}

/// Zeroes `driven_state` on every input dev-pin of every chip in
/// `library` -- called whenever the viewer switches which chip is the
/// current root, so a switch clicked while viewing chip A doesn't stay
/// "remembered" the next time the player navigates back to A (each visit
/// starts from a fresh, all-off simulation, rather than the pin's state
/// being some kind of persistent save data).
fn reset_all_driven_inputs(library: &mut ChipLibrary) {
	for chip in library.iter_mut() {
		for pin in &mut chip.input_pins {
			pin.driven_state = 0;
		}
	}
}

/// Whether `name` is a chip the player actually authored (as opposed to
/// a built-in primitive like `AND`/`NAND`/`Pulse`) -- i.e. whether
/// "Open" makes any sense for it. Builtins have no `ChipDescription` of
/// their own worth navigating into (no subchips/wires to show), so every
/// "Open" context-menu row is disabled for them (see
/// `context_menu_items_for_chip_type`) and `open_chip_by_name` refuses to
/// act on one even if somehow invoked anyway.
fn is_custom_chip(library: &ChipLibrary, name: &str) -> bool {
	library.try_get(name).map(|d| d.chip_type == ChipType::Custom).unwrap_or(false)
}

/// True if placing `chip_to_place` as a new subchip inside `root_chip_name` would create a
/// recursive cycle -- either because it *is* `root_chip_name` itself, or because its own
/// definition, directly or transitively through its own subchips, already contains
/// `root_chip_name` somewhere inside it. In the latter case placing it back into
/// `root_chip_name` would close the loop (`root_chip_name` -> `chip_to_place` -> ... ->
/// `root_chip_name`), which `sim::build_recursive` has no cycle guard for and would recurse
/// forever trying to flatten. Gates the "USE"/bottom-bar placement buttons
/// (`editor_ui::ChipLibraryState::selected_chip_would_cycle`,
/// `build_starred_bottom_bar`/`build_starred_collection_popup`'s `cycle_blocked`) and is checked
/// again defensively in `try_place_pending_chip` itself, so a click can never place a cycle even
/// if the UI's greyed-out state somehow gets out of sync with what's actually open.
fn would_create_cycle(library: &ChipLibrary, root_chip_name: &str, chip_to_place: &str) -> bool {
	if chip_to_place.eq_ignore_ascii_case(root_chip_name) {
		return true;
	}
	let mut visited = std::collections::HashSet::new();
	chip_contains(library, chip_to_place, root_chip_name, &mut visited)
}

/// True if `chip_name`'s own definition includes `target` anywhere inside it, directly or via any
/// of its subchips recursively. `visited` (chip names already expanded, lower-cased) guards
/// against looping forever if `library` somehow already describes a cycle (e.g. a hand-edited
/// save) -- same defensive purpose as `scene::draw_pending_wire_preview`'s own recursion guard.
fn chip_contains(library: &ChipLibrary, chip_name: &str, target: &str, visited: &mut std::collections::HashSet<String>) -> bool {
	if !visited.insert(chip_name.to_ascii_lowercase()) {
		return false;
	}
	let Some(desc) = library.try_get(chip_name) else { return false };
	desc.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(target) || chip_contains(library, &s.name, target, visited))
}

/// Mandatory catch-all collection every project's library falls back to
/// -- mirrors `ChipLibraryMenu`'s `defaultOtherChipsCollectionName`.
const DEFAULT_LIBRARY_COLLECTION_NAME: &str = "OTHER";

/// Ensures `prefs.chip_collections` has an `OTHER` collection and that
/// every chip in `library` belongs to *some* collection, adding any
/// stragglers to `OTHER` -- mirrors the collection-syncing half of
/// `ChipLibraryMenu.OnMenuOpened`. Called whenever the library overlay
/// is opened, so newly-created/loaded chips that were never explicitly
/// filed always still show up somewhere in the panel.
fn sync_library_collections(prefs: &mut ProjectDescription, library: &ChipLibrary) {
	if !prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
		prefs.chip_collections.push(ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
	}
	let already_collected: std::collections::HashSet<String> =
		prefs.chip_collections.iter().flat_map(|c| c.chips.iter().map(|n| n.to_ascii_lowercase())).collect();
	let default_index =
		prefs.chip_collections.iter().position(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)).expect("just ensured above");

	let mut stray_names: Vec<String> =
		library.iter().map(|d| d.name.clone()).filter(|n| !already_collected.contains(&n.to_ascii_lowercase())).collect();
	stray_names.sort();
	prefs.chip_collections[default_index].chips.extend(stray_names);
}

/// Resets whichever inline popup (new/rename collection, delete
/// confirmation) is open in the library panel, without leaving the
/// library itself -- mirrors `ChipLibraryMenu.ResetPopupState`.
fn reset_library_popup_state(v: &mut ViewerState) {
	v.library_creating_collection = false;
	v.library_renaming_collection = false;
	v.library_confirming_chip_delete = false;
	v.library_confirming_collection_delete = false;
	v.library_delete_message.clear();
	v.overlay_text_input.clear();
}

/// Names of every custom chip in `library` that directly contains
/// `chip_name` as one of its own sub-chips -- a name-only simplification
/// of `ChipLibrary.GetDirectParentChips`, enough to build a delete
/// warning without needing the full chip-dependency graph this port
/// doesn't otherwise build.
fn direct_parent_chip_names(library: &ChipLibrary, chip_name: &str) -> Vec<String> {
	library.iter().filter(|d| d.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(chip_name))).map(|d| d.name.clone()).collect()
}

/// Builds the chip-library DELETE confirmation message -- mirrors
/// `ChipLibraryMenu.CreateDeleteConfirmationMessage`, simplified to a
/// single wrapped paragraph (no coloured-by-severity variant, since
/// `editor_ui`'s confirmation panel doesn't distinguish one).
fn chip_delete_confirm_message(v: &ViewerState, chip_name: &str) -> String {
	let mut parents = direct_parent_chip_names(&v.library, chip_name);
	let used_in_current = v.library.get(&v.root_chip_name).sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(chip_name));
	if used_in_current {
		parents.retain(|p| !p.eq_ignore_ascii_case(&v.root_chip_name));
	}

	let mut message = if used_in_current {
		"Are you sure you want to delete the chip you are CURRENTLY EDITING? ".to_string()
	} else {
		"Are you sure you want to delete this chip? ".to_string()
	};

	let mut uses: Vec<String> = Vec::new();
	if used_in_current {
		uses.push("the current chip".to_string());
	}
	uses.extend(parents.iter().map(|p| format!("\"{p}\"")));

	match uses.len() {
		0 => message.push_str("It is not used anywhere."),
		1 => message.push_str(&format!("It is used by {}.", uses[0])),
		2 => message.push_str(&format!("It is used by {} and {}.", uses[0], uses[1])),
		n => message.push_str(&format!("It is used by {} and {} others.", uses[0], n - 1)),
	}

	message
}

/// Actually deletes chip `name` -- from disk (via `Saver::delete_chip`,
/// backed up into the project's `Deleted Chips/` folder), from every
/// collection that lists it, and from the starred list -- then drops it
/// from `v.library` and clears the library selection. Mirrors the
/// `isConfirmingChipDeletion` branch of `ChipLibraryMenu`'s DELETE
/// button.
fn delete_chip_from_library(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	if let Err(e) = Saver::delete_chip(paths, &v.project_name, name, true) {
		*status = Some(format!("Failed to delete chip '{name}': {e}"));
		return;
	}
	for collection in &mut v.prefs.chip_collections {
		collection.chips.retain(|c| !c.eq_ignore_ascii_case(name));
	}
	v.prefs.set_starred(name, false, false);
	v.prefs.all_custom_chip_names.retain(|c| !c.eq_ignore_ascii_case(name));
	v.library.remove(name);
	v.library_selection = LibrarySelection::None;
}

/// Deletes the collection at `index` -- moves its chips into `OTHER`
/// (creating it first if somehow missing), drops its starred entry (if
/// any), then removes it from `prefs.chip_collections`. Mirrors
/// `ChipLibraryMenu.DeleteSelectedCollection`.
fn delete_collection(prefs: &mut ProjectDescription, index: usize) {
	if !prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
		prefs.chip_collections.push(ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
	}
	let Some(collection) = prefs.chip_collections.get(index) else { return };
	let name = collection.name.clone();
	let chips = collection.chips.clone();

	if let Some(default_collection) = prefs
		.chip_collections
		.iter_mut()
		.find(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME) && !c.name.eq_ignore_ascii_case(&name))
	{
		default_collection.chips.extend(chips);
	}

	prefs.set_starred(&name, false, true);
	prefs.chip_collections.remove(index);
}

/// Moves whatever's selected in the library panel one step within its
/// own list (`force_jump = false`, mirrors the original's combined
/// UP/DOWN buttons -- steps if it can, otherwise falls back to a jump),
/// or straight into the previous/next collection outright
/// (`force_jump = true`, mirrors the separate JUMP UP/DOWN buttons).
/// Only chip rows support jumping; collections and starred rows just
/// reorder within their own list either way.
fn move_selected_library_row(v: &mut ViewerState, down: bool, force_jump: bool) {
	match v.library_selection {
		LibrarySelection::Chip(ci, chi) => {
			let len = v.prefs.chip_collections.get(ci).map(|c| c.chips.len()).unwrap_or(0);
			let can_step = if down { chi + 1 < len } else { chi > 0 };
			if can_step && !force_jump {
				let new_idx = if down { chi + 1 } else { chi - 1 };
				if let Some(c) = v.prefs.chip_collections.get_mut(ci) {
					c.chips.swap(chi, new_idx);
				}
				v.library_selection = LibrarySelection::Chip(ci, new_idx);
				return;
			}
			let target_ci = if down { Some(ci + 1) } else { ci.checked_sub(1) };
			let Some(target_ci) = target_ci else { return };
			if target_ci >= v.prefs.chip_collections.len() {
				return;
			}
			let Some(name) = v.prefs.chip_collections.get_mut(ci).map(|c| c.chips.remove(chi)) else { return };
			let target = &mut v.prefs.chip_collections[target_ci];
			target.is_toggled_open = true;
			let new_idx = if down { 0 } else { target.chips.len() };
			target.chips.insert(new_idx, name);
			v.library_selection = LibrarySelection::Chip(target_ci, new_idx);
		}
		LibrarySelection::Collection(ci) => {
			let len = v.prefs.chip_collections.len();
			let can_step = if down { ci + 1 < len } else { ci > 0 };
			if can_step {
				let new_idx = if down { ci + 1 } else { ci - 1 };
				v.prefs.chip_collections.swap(ci, new_idx);
				v.library_selection = LibrarySelection::Collection(new_idx);
			}
		}
		LibrarySelection::Starred(i) => {
			let len = v.prefs.starred_list.len();
			let can_step = if down { i + 1 < len } else { i > 0 };
			if can_step {
				let new_idx = if down { i + 1 } else { i - 1 };
				v.prefs.starred_list.swap(i, new_idx);
				v.library_selection = LibrarySelection::Starred(new_idx);
			}
		}
		LibrarySelection::None => {}
	}
}

/// Determines which buttons `Overlay::SaveChip` should show for the
/// currently-typed name, by comparing it against `v.root_chip_name` (the
/// chip's current identity) and the rest of `v.library` -- see
/// `editor_ui::SaveChipMode`'s docs for what each variant means and
/// `build_save_chip_popup`'s docs for why this is re-derived identically
/// on both the render side and the click-handling side. Case-insensitive,
/// matching `ChipLibrary`'s own lookup rules.
fn save_chip_mode(v: &ViewerState, typed: &str) -> editor_ui::SaveChipMode {
	let typed = typed.trim();
	if typed.eq_ignore_ascii_case(&v.root_chip_name) {
		editor_ui::SaveChipMode::Save
	} else if v.library.try_get(typed).is_some() {
		editor_ui::SaveChipMode::Replace
	} else {
		editor_ui::SaveChipMode::SaveAsOrRename
	}
}

/// Adds `add_name` to the project's `all_custom_chip_names`/`chip_collections`
/// bookkeeping if it isn't already there (and removes `remove_name` from
/// both, if given), then persists the updated `ProjectDescription`.
/// Mirrors what the sidebar/search actually list -- without this, a
/// freshly Saved-As/Renamed chip would only be reachable if you already
/// remembered its exact name to type into search.
fn register_chip_name_in_project(v: &mut ViewerState, paths: &SavePaths, remove_name: Option<&str>, add_name: &str) {
	if let Some(old) = remove_name {
		v.prefs.all_custom_chip_names.retain(|n| n != old);
		for c in v.prefs.chip_collections.iter_mut() {
			c.chips.retain(|n| n != old);
		}
	}
	if !v.prefs.all_custom_chip_names.iter().any(|n| n == add_name) {
		v.prefs.all_custom_chip_names.push(add_name.to_string());
	}
	if !v.prefs.chip_collections.iter().any(|c| c.chips.iter().any(|n| n == add_name)) {
		if let Some(first) = v.prefs.chip_collections.first_mut() {
			first.chips.push(add_name.to_string());
		}
	}

	let mut desc = v.prefs.clone();
	match Saver::save_project_description(paths, &mut desc) {
		Ok(()) => v.prefs = desc,
		Err(e) => eprintln!("warning: failed to update project description: {e}"),
	}
}

/// Plain overwrite/create (`SaveChipMode::Save`): writes the current
/// in-memory chip back to its own file under its own (unchanged) name.
/// No other chip or file is touched.
fn save_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let name = v.root_chip_name.clone();
	let desc = v.library.get(&name).clone();
	match Saver::save_chip(paths, &v.project_name, &desc) {
		Ok(()) => *status = Some(format!("Saved '{name}'")),
		Err(e) => *status = Some(format!("Failed to save '{name}': {e}")),
	}
}

/// Saves a *copy* of the currently-open chip under `new_name`
/// (`SaveChipMode::SaveAsOrRename`, "Save As" button), leaving its
/// existing on-disk file (under its current name, if it has one)
/// completely untouched. Since that current identity's `v.library` entry
/// has been edited in place all session, once we fork away from it its
/// in-memory copy no longer matches what's actually on disk under that
/// name -- so it's reloaded fresh from its own file right after (see
/// `load_single_chip_from_disk`), discarding whatever of this session's
/// edits hadn't already been saved under *that* identity. The viewer
/// then switches over to the new name.
fn save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();

	match Saver::save_chip(paths, &v.project_name, &new_desc) {
		Ok(()) => {
			v.library.add(new_desc);
			register_chip_name_in_project(v, paths, None, new_name);

			if !old_name.eq_ignore_ascii_case(new_name) {
				match load_single_chip_from_disk(paths, &v.project_name, &old_name) {
					Ok(pristine) => {
						v.library.add(pristine);
					}
					Err(_) => {
						// No on-disk file for the old identity (it was never actually saved under that
						// name to begin with) -- nothing to revert to, so leave the in-memory draft as is.
					}
				}
			}

			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Saved as '{new_name}'"));
			v.rebuild_sim();
		}
		Err(e) => *status = Some(format!("Failed to save '{new_name}': {e}")),
	}
}

/// Backs up (moves to the project's "Deleted Chips" folder -- see
/// `Saver::delete_chip`'s `backup_in_deleted_folder`) whatever chip is
/// currently saved under `new_name`, then does exactly what
/// `save_chip_as` does. The chip's own existing file, if any under its
/// *current* name, is left untouched either way -- only the chip being
/// overwritten at the destination name is backed up.
fn replace_chip_with_current(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	if let Err(e) = Saver::delete_chip(paths, &v.project_name, new_name, true) {
		*status = Some(format!("Failed to back up existing '{new_name}': {e}"));
		return;
	}
	v.library.remove(new_name);
	save_chip_as(v, paths, status, new_name);
}

/// Actually renames the chip (`SaveChipMode::SaveAsOrRename`, "Rename"
/// button): moves its on-disk file to `new_name` -- no copy left under
/// the old name, the old file is deleted outright (no backup, since this
/// is a rename rather than a delete) -- and updates the project's
/// chip-name bookkeeping to match.
fn rename_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();

	match Saver::save_chip(paths, &v.project_name, &new_desc) {
		Ok(()) => {
			if let Err(e) = Saver::delete_chip(paths, &v.project_name, &old_name, false) {
				eprintln!("warning: renamed '{old_name}' to '{new_name}' but failed to remove the old file: {e}");
			}
			v.library.remove(&old_name);
			v.library.add(new_desc);
			register_chip_name_in_project(v, paths, Some(&old_name), new_name);
			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Renamed '{old_name}' to '{new_name}'"));
			v.rebuild_sim();
		}
		Err(e) => *status = Some(format!("Failed to rename to '{new_name}': {e}")),
	}
}

/// Applies the `Overlay::SaveChip` popup's Confirm action -- shared by
/// its "Save"/"Replace" button (`EditorAction::SaveChipConfirm`) and
/// pressing Enter directly for those same two (unambiguous) modes; see
/// the key-handler's own guard for why `SaveAsOrRename` never reaches
/// here via Enter (that mode's own two buttons call
/// `confirm_save_chip_as`/`confirm_save_chip_rename` directly instead,
/// since which of "keep both" or "actually rename" is meant can't be
/// inferred, only chosen).
fn confirm_save_chip_popup(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	match save_chip_mode(v, &typed) {
		editor_ui::SaveChipMode::Save => save_current_chip(v, paths, status),
		editor_ui::SaveChipMode::Replace => replace_chip_with_current(v, paths, status, &typed),
		editor_ui::SaveChipMode::SaveAsOrRename => return,
	}
	v.overlay = Overlay::None;
	v.overlay_text_input.clear();
}

fn confirm_save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	save_chip_as(v, paths, status, &typed);
	v.overlay = Overlay::None;
	v.overlay_text_input.clear();
}

fn confirm_save_chip_rename(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	rename_current_chip(v, paths, status, &typed);
	v.overlay = Overlay::None;
	v.overlay_text_input.clear();
}

/// Re-reads a single chip's own save file from disk, without touching
/// anything else in `v.library` -- used to revert one specific chip's
/// in-memory entry back to "whatever's actually saved" (e.g. the chip
/// left behind, untouched, by a Save-As/Replace under a new name; see
/// `save_chip_as`), as opposed to blindly reloading the whole project.
fn load_single_chip_from_disk(paths: &SavePaths, project_name: &str, chip_name: &str) -> std::io::Result<ChipDescription> {
	let path = paths.chips_path(project_name).join(format!("{chip_name}.json"));
	let json = std::fs::read_to_string(path)?;
	logic_sim::json::parse_chip_description(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Discards whatever unsaved edits this session made to whichever chip
/// is currently open (`v.root_chip_name`), by reloading its pristine
/// on-disk copy back over its `v.library` entry (same "reload from disk"
/// move `save_chip_as` already does for the identity it forks away
/// from -- see `load_single_chip_from_disk`). Called by `open_chip_by_name`
/// right before it actually switches away to a different chip. If the
/// chip has no file on disk yet (a brand new, never-saved chip), there's
/// nothing to revert to, so its in-memory draft is left exactly as it
/// was -- it simply isn't reachable again once you navigate away, same
/// as it already wasn't reachable after an app restart.
fn discard_unsaved_changes(v: &mut ViewerState, paths: &SavePaths) {
	let leaving = v.root_chip_name.clone();
	if !is_custom_chip(&v.library, &leaving) {
		return;
	}
	if let Ok(pristine) = load_single_chip_from_disk(paths, &v.project_name, &leaving) {
		v.library.add(pristine);
	}
}

/// Picks a fresh, not-yet-used (case-insensitively) name for a
/// brand-new chip, starting from "New Chip" and falling back to
/// "New Chip 2", "New Chip 3", ... the first suffix that isn't already
/// taken in `library` -- so hitting Ctrl+N repeatedly never collides
/// with an earlier still-unsaved draft (or a saved chip that happens to
/// already be named "New Chip").
fn unique_new_chip_name(library: &ChipLibrary) -> String {
	if library.try_get("New Chip").is_none() {
		return "New Chip".to_string();
	}
	let mut n = 2;
	loop {
		let candidate = format!("New Chip {n}");
		if library.try_get(&candidate).is_none() {
			return candidate;
		}
		n += 1;
	}
}

/// Ctrl+N: starts a brand-new, blank custom chip (no pins, no subchips,
/// no wires -- see `ChipDescription::new`) and switches the viewer over
/// to it, exactly as if it were an existing chip being opened. First
/// discards any unsaved edits on whichever chip is currently open, the
/// same as any other switch (see `discard_unsaved_changes`), so Ctrl+N
/// can't be used to accidentally lose track of that.
///
/// The new chip lives only in `v.library` until it's actually saved --
/// it isn't added to the project's `all_custom_chip_names`/library
/// sidebar (that's `register_chip_name_in_project`'s job, run from the
/// save flow) until then, so an uncommitted "New Chip" draft won't
/// clutter the sidebar or survive a switch away from it without being
/// saved first.
fn start_new_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	discard_unsaved_changes(v, paths);

	let name = unique_new_chip_name(&v.library);
	v.library.add(ChipDescription::new(&name, ChipType::Custom));

	v.root_chip_name = name.clone();
	reset_all_driven_inputs(&mut v.library);
	v.rebuild_sim();
	v.camera_fitted = false;
	v.pending_wire = None;
	v.pending_place = None;
	*status = Some(format!("New chip '{name}'"));
}

/// Actually switches the viewer over to `name`'s own definition -- i.e.
/// "open this chip" -- if it's a custom chip in `v.library`. This used to
/// be exactly what left-clicking a chip in the library sidebar did (via
/// `EditorAction::SelectChip`); it's now reached only through that row's
/// right-click "Open" popup, the search popup's `UseChip`, and this
/// module's own `EditorAction::UseChip`, so a left click alone no longer
/// jumps the viewer away from whatever chip is currently open. Builtins
/// are refused (see `is_custom_chip`) -- their "Open" row is greyed out
/// in the popup, so reaching this arm for one at all would mean the
/// disabled-row guard in `context_menu::build_context_menu` was bypassed
/// somehow.
///
/// On an actual switch (`name` differs from the chip currently open),
/// first discards any unsaved edits to the chip being left via
/// `discard_unsaved_changes` -- so `v.library`'s in-memory copy of it
/// reverts to whatever's actually on disk, and navigating back to it
/// later shows that saved state rather than the draft you were mid-edit
/// on. Persisting those edits instead is `Ctrl+S`'s job (see
/// `confirm_save_chip_popup`) and must happen *before* switching away.
/// Also only re-fits the camera on an actual switch, never on an
/// in-place edit of the chip already on screen (that's `rebuild_sim`'s
/// job to *not* do -- see its own doc comment).
fn open_chip_by_name(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	let switching = name != v.root_chip_name;

	if is_custom_chip(&v.library, name) {
		if switching {
			discard_unsaved_changes(v, paths);
		}
		v.root_chip_name = name.to_string();
		reset_all_driven_inputs(&mut v.library);
		v.rebuild_sim();
		if switching {
			v.camera_fitted = false;
			v.pending_wire = None;
			v.pending_place = None;
		}
	} else if v.library.try_get(name).is_some() {
		*status = Some(format!("Chip '{}' is a builtin component", name));
	} else {
		*status = Some(format!("Chip '{}' not found in library", name));
	}
}

/// One right-clickable "thing" a context menu can be attached to, parsed
/// back out of `ContextMenuState::target` (kept as a plain string by that
/// module so it stays generic -- see its docs). `id`s below are always
/// scoped to the *current root chip* (`v.root_chip_name`): a subchip's
/// own `SubChipDescription::id`, or a boundary dev-pin's `PinDescription::id`.
enum ContextTarget {
	/// A placed subchip instance on the canvas.
	Component(i32),
	/// One of the *current root chip's own* boundary dev-pins -- never a
	/// subchip's pin (the brief is explicit about that distinction).
	DevPin { is_input: bool, id: i32 },
	/// A row in the chip library sidebar, by chip name.
	LibChip(String),
	/// A plain chip's own button directly in the starred bottom bar (not
	/// one listed inside a collection's flyout) -- by chip name. Distinct
	/// from `FlyoutChip` only in which right-click rows it's offered
	/// (this one also gets "Un-star"; see `handle_right_mouse_button`).
	BarChip(String),
	/// A chip row inside an *open collection's* flyout
	/// (`build_starred_collection_popup`), by chip name.
	FlyoutChip(String),
}

impl ContextTarget {
	/// Inverse of however `handle_right_mouse_button` built the
	/// `target` string in the first place -- kept next to that so the
	/// two stay in sync.
	fn parse(target: &str) -> Option<Self> {
		if let Some(rest) = target.strip_prefix("component:") {
			rest.parse().ok().map(ContextTarget::Component)
		} else if let Some(rest) = target.strip_prefix("devpin:in:") {
			rest.parse().ok().map(|id| ContextTarget::DevPin { is_input: true, id })
		} else if let Some(rest) = target.strip_prefix("devpin:out:") {
			rest.parse().ok().map(|id| ContextTarget::DevPin { is_input: false, id })
		} else if let Some(rest) = target.strip_prefix("libchip:") {
			Some(ContextTarget::LibChip(rest.to_string()))
		} else if let Some(rest) = target.strip_prefix("barchip:") {
			Some(ContextTarget::BarChip(rest.to_string()))
		} else if let Some(rest) = target.strip_prefix("flyoutchip:") {
			Some(ContextTarget::FlyoutChip(rest.to_string()))
		} else {
			None
		}
	}
}

/// Builds the row list for a right-click popup opened on a placed
/// subchip of type `chip_type` -- shared by the canvas-component and (for
/// "Open"'s enabled state) library-row cases so the two stay consistent.
/// Every component gets "Label"; "Configure" is only offered for the
/// handful of chip types that actually have configurable
/// `internal_data` (see `NamingPurpose`/`KeySelectPurpose`'s docs for
/// what each one edits); "Open"/"Delete" are canvas-only (a library row
/// has no wires to cascade-delete and *is* the definition, not an
/// instance of it, so there's nothing to "open" beyond switching to it).
fn context_menu_items_for_component(library: &ChipLibrary, chip_name: &str) -> Vec<ContextMenuItem> {
	let mut items = vec![ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(library, chip_name))];
	items.push(ContextMenuItem::new("Label", ContextMenuAction::Label));
	let chip_type = library.try_get(chip_name).map(|d| d.chip_type);
	if matches!(chip_type, Some(ChipType::Pulse) | Some(ChipType::Key) | Some(ChipType::Rom256x16)) {
		items.push(ContextMenuItem::new("Configure", ContextMenuAction::Configure));
	}
	if chip_type.unwrap_or_default().is_bus_type() {
		items.push(ContextMenuItem::new("Flip", ContextMenuAction::Flip));
	}
	items.push(ContextMenuItem::new("Delete", ContextMenuAction::Delete));
	items
}

/// Un-stars `name` (a plain chip, never a collection -- see
/// `ContextTarget::BarChip`'s docs) from the right-click popup on its own
/// bottom-bar button, and immediately persists the change. Unlike
/// `EditorAction::ToggleStarred` (which only mutates `v.prefs` in memory,
/// relying on the library overlay's own exit/Tab handling to save when
/// the player eventually leaves it), this has no such exit event to
/// piggyback on -- the bottom bar is usable with the library closed --
/// so it saves right away, the same way `EditorAction::PlaceChip` does
/// for the same reason.
fn unstar_bottom_bar_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	v.prefs.set_starred(name, false, false);
	let mut desc = v.prefs.clone();
	if let Err(e) = Saver::save_project_description(paths, &mut desc) {
		*status = Some(format!("Failed to save chip library: {e}"));
	} else {
		v.prefs = desc;
	}
}

/// Applies a click on the currently-open right-click popup (see
/// `render::context_menu`) -- `target` is whatever `state.target` was set
/// to when the popup was opened (parsed back via `ContextTarget::parse`),
/// `action_id` is the clicked row's `ContextMenuItem::id`.
fn apply_context_menu_action(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, target: &str, action_id: ContextMenuAction) {
	let Some(parsed) = ContextTarget::parse(target) else { return };
	let root_chip_name = v.root_chip_name.clone();

	match (action_id, parsed) {
		(ContextMenuAction::Open, ContextTarget::Component(id)) => {
			let name = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).map(|s| s.name.clone());
			if let Some(name) = name {
				open_chip_by_name(v, paths, status, &name);
			}
		}
		(ContextMenuAction::Open, ContextTarget::LibChip(name)) => {
			open_chip_by_name(v, paths, status, &name);
			v.overlay = Overlay::None;
			reset_library_popup_state(v);
			v.library_selection = LibrarySelection::None;
		}
		(ContextMenuAction::Open, ContextTarget::BarChip(name)) | (ContextMenuAction::Open, ContextTarget::FlyoutChip(name)) => {
			open_chip_by_name(v, paths, status, &name);
		}
		(ContextMenuAction::Unstar, ContextTarget::BarChip(name)) => unstar_bottom_bar_chip(v, paths, status, &name),
		(ContextMenuAction::Delete, ContextTarget::LibChip(name)) => {
			v.library_delete_message = chip_delete_confirm_message(v, &name);
			v.library_confirming_chip_delete = true;
			// Right-click delete has no row selected yet (only a name), so
			// stash it as a `Chip` selection the confirmation can read back
			// from -- find where it actually lives in the collections list.
			for (ci, c) in v.prefs.chip_collections.iter().enumerate() {
				if let Some(chi) = c.chips.iter().position(|n| n.eq_ignore_ascii_case(&name)) {
					v.library_selection = LibrarySelection::Chip(ci, chi);
					break;
				}
			}
		}

		(ContextMenuAction::Label, ContextTarget::Component(id)) => {
			let current = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).and_then(|s| s.label.clone()).unwrap_or_default();
			v.overlay = Overlay::Naming;
			v.overlay_text_input = current;
			v.naming_purpose = NamingPurpose::LabelComponent(id);
		}
		(ContextMenuAction::Flip, ContextTarget::Component(id)) => {
			if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
				let mut data = sub.internal_data.clone().unwrap_or_default();
				if data.len() < 2 {
					data.resize(2, 0);
				}
				data[1] ^= 1;
				sub.internal_data = Some(data);
			}
			v.rebuild_sim();
		}
		(ContextMenuAction::Label, ContextTarget::DevPin { is_input, id }) => {
			let chip = v.library.get(&root_chip_name);
			let pins = if is_input { &chip.input_pins } else { &chip.output_pins };
			let current = pins.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
			v.overlay = Overlay::Naming;
			v.overlay_text_input = current;
			v.naming_purpose = NamingPurpose::LabelDevPin { is_input, id };
		}

		(ContextMenuAction::Configure, ContextTarget::Component(id)) => {
			let sub_chip_name = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).map(|s| s.name.clone());
			let chip_type = sub_chip_name.as_deref().and_then(|n| v.library.try_get(n)).map(|d| d.chip_type);
			let internal_data =
				v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).and_then(|s| s.internal_data.clone()).unwrap_or_default();
			match chip_type {
				Some(ChipType::Pulse) => {
					v.overlay = Overlay::Naming;
					v.overlay_text_input = internal_data.first().copied().unwrap_or(0).to_string();
					v.naming_purpose = NamingPurpose::ConfigurePulseDuration(id);
				}
				Some(ChipType::Key) => {
					v.overlay = Overlay::KeySelect;
					v.overlay_key_choice = internal_data.first().map(|&code| code as u8 as char);
					v.key_select_purpose = KeySelectPurpose::ConfigureKeyChar(id);
				}
				Some(ChipType::Rom256x16) => {
					let mut data = internal_data;
					data.resize(editor_ui::ROM_WORD_COUNT, 0);
					v.overlay = Overlay::RomEditor;
					v.overlay_text_input = data[0].to_string();
					v.rom_editor = Some(RomEditorState { component_id: id, data, selected: 0 });
				}
				_ => {}
			}
		}

		(ContextMenuAction::Delete, ContextTarget::Component(id)) => delete_component(v, id),

		_ => {}
	}
}

/// Applies whatever's typed into `Overlay::Naming`'s text field, per
/// `v.naming_purpose` -- shared by the popup's Confirm button
/// (`EditorAction::ConfirmName`) and pressing Enter directly, so the two
/// input paths can't drift apart. Always closes the popup and resets
/// `naming_purpose` back to its default afterwards, success or not.
fn confirm_naming_popup(v: &mut ViewerState, status: &mut Option<String>) {
	let trimmed = v.overlay_text_input.trim().to_string();
	let root_chip_name = v.root_chip_name.clone();

	match v.naming_purpose {
		NamingPurpose::RenameProject => {
			if !trimmed.is_empty() {
				v.project_name = trimmed;
			}
		}
		NamingPurpose::LabelComponent(id) => {
			if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
				sub.label = if trimmed.is_empty() { None } else { Some(trimmed) };
			}
		}
		NamingPurpose::LabelDevPin { is_input, id } => {
			let chip = v.library.get_mut(&root_chip_name);
			let pins = if is_input { &mut chip.input_pins } else { &mut chip.output_pins };
			if let Some(pin) = pins.iter_mut().find(|p| p.id == id) {
				if !trimmed.is_empty() {
					pin.name = trimmed;
				}
			}
		}
		NamingPurpose::ConfigurePulseDuration(id) => match trimmed.parse::<u32>() {
			Ok(ticks) => {
				if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
					// `Simulator::process_builtin_chip`'s `Pulse` arm indexes `internal_state` at three
					// fixed slots -- `[DURATION, TICKS_REMAINING, INPUT_OLD]`. Changing the configured
					// length also resets any in-flight pulse and forgets the last sampled input edge.
					sub.internal_data = Some(vec![ticks, 0, 0]);
				}
				v.rebuild_sim();
			}
			Err(_) => *status = Some("Pulse length must be a whole number of ticks".to_string()),
		},
	}

	v.overlay = Overlay::None;
	v.overlay_text_input.clear();
	v.naming_purpose = NamingPurpose::default();
}

/// Parses a single ROM cell value, same rule as the old comma-list
/// editor: a leading `0x`/`0X` means hex, otherwise decimal.
fn parse_rom_word(text: &str) -> Option<u32> {
	let text = text.trim();
	if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
		u32::from_str_radix(hex, 16).ok()
	} else {
		text.parse::<u32>().ok()
	}
}

/// Commits `v.overlay_text_input` into the currently-selected cell of
/// the open ROM editor (`EditorAction::RomConfirmCell`), then advances
/// selection to the next cell (wrapping) and loads *its* value into the
/// text field -- lets the player type several values in a row without
/// re-clicking between each one. A parse failure leaves the selection
/// and text field untouched (so the player can just fix their typo)
/// rather than silently discarding it.
fn confirm_rom_cell(v: &mut ViewerState, status: &mut Option<String>) {
	let Some(editor) = v.rom_editor.as_mut() else { return };
	match parse_rom_word(&v.overlay_text_input) {
		Some(value) => {
			if let Some(cell) = editor.data.get_mut(editor.selected) {
				*cell = value;
			}
			editor.selected = (editor.selected + 1) % editor_ui::ROM_WORD_COUNT;
			v.overlay_text_input = editor.data[editor.selected].to_string();
		}
		None => *status = Some("ROM cell value must be a number (decimal or 0x hex)".to_string()),
	}
}

/// Writes the ROM editor's whole draft buffer back onto the subchip
/// (`EditorAction::RomApply`) and closes the popup. Any value still only
/// sitting in the text field (typed but not yet committed via "Set"/Enter)
/// is committed first, so clicking straight from typing to "Apply" isn't
/// a silent no-op for that last cell.
fn apply_rom_editor(v: &mut ViewerState, status: &mut Option<String>) {
	confirm_rom_cell(v, status);
	if let Some(editor) = v.rom_editor.take() {
		let root_chip_name = v.root_chip_name.clone();
		if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == editor.component_id) {
			sub.internal_data = Some(editor.data);
		}
		v.rebuild_sim();
	}
	v.overlay = Overlay::None;
	v.overlay_text_input.clear();
}

/// Applies whatever's chosen in `Overlay::KeySelect`, per
/// `v.key_select_purpose` -- shared by the popup's Confirm button
/// (`EditorAction::ConfirmKey`) and pressing Enter directly, mirroring
/// `confirm_naming_popup`.
fn confirm_key_select_popup(v: &mut ViewerState, status: &mut Option<String>) {
	if let Some(c) = v.overlay_key_choice {
		match v.key_select_purpose {
			KeySelectPurpose::Rebind => {
				// No actual keybind system exists to rebind yet -- this
				// just reports the choice back so the popup is usable
				// and testable end-to-end ahead of that being wired up.
				*status = Some(format!("Key '{c}' chosen (not yet wired to an action)"));
			}
			KeySelectPurpose::ConfigureKeyChar(id) => {
				let root_chip_name = v.root_chip_name.clone();
				if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
					sub.internal_data = Some(vec![c as u32]);
				}
				v.rebuild_sim();
			}
		}
	}
	v.overlay = Overlay::None;
	v.key_select_purpose = KeySelectPurpose::default();
}

/// Deletes subchip `id` from the current root chip, plus every wire
/// directly attached to it -- but, per the brief, only the "shortest
/// possible section" of wiring: just the wire(s) whose source or target
/// pin actually belongs to this subchip (via `scene::delete_wire`, which
/// itself only cascades to wires *tapping onto* one of those, never
/// anything further away). A wire fanning out from one of this
/// component's *output* pins to some other, unrelated component is left
/// completely alone at the far end -- only the segment that touched the
/// deleted component goes.
fn delete_component(v: &mut ViewerState, id: i32) {
	let root_chip_name = v.root_chip_name.clone();
	let chip = v.library.get_mut(&root_chip_name);

	loop {
		let next = chip.wires.iter().position(|w| w.source_pin_address.pin_owner_id == id || w.target_pin_address.pin_owner_id == id);
		match next {
			Some(idx) => {
				logic_sim::render::scene::delete_wire(chip, idx);
			}
			None => break,
		}
	}

	chip.sub_chips.retain(|s| s.id != id);
	v.rebuild_sim();
}

/// Applies a click on one of the editor overlays. A free function (not an
/// `App` method) so it can be called from inside a `match &mut self.screen`
/// arm that's already holding `v`, while still touching the sibling
/// `self.paths` / `self.status` fields -- see `App::handle_mouse_button`.
fn apply_editor_action(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, action: EditorAction) {
	match action {
		EditorAction::ClosePopup => {
			v.overlay = Overlay::None;
			v.overlay_text_input.clear();
			v.rom_editor = None;
		}
		EditorAction::CyclePref(i) => cycle_pref(&mut v.prefs, i),
		EditorAction::ApplyPreferences => {
			v.show_grid = v.prefs.prefs_grid_display_mode == 1;
			let mut desc = v.prefs.clone();
			match Saver::save_project_description(paths, &mut desc) {
				Ok(()) => v.prefs = desc,
				Err(e) => *status = Some(format!("Failed to save preferences: {e}")),
			}
			v.overlay = Overlay::None;
		}
		EditorAction::SelectCollection(i) => {
			v.library_selection = LibrarySelection::Collection(i);
			if let Some(c) = v.prefs.chip_collections.get_mut(i) {
				c.is_toggled_open = !c.is_toggled_open;
			}
		}
		EditorAction::SelectChipRow { collection, chip } => {
			v.library_selection = LibrarySelection::Chip(collection, chip);
		}
		EditorAction::SelectStarredRow(i) => {
			v.library_selection = LibrarySelection::Starred(i);
		}
		EditorAction::ToggleStarred { name, is_collection } => {
			let now_starred = !v.prefs.is_starred(&name, is_collection);
			v.prefs.set_starred(&name, now_starred, is_collection);
		}
		EditorAction::MoveSelectedStep(down) => move_selected_library_row(v, down, false),
		EditorAction::MoveSelectedJump(down) => move_selected_library_row(v, down, true),
		EditorAction::OpenSelectedChip(name) => {
			open_chip_by_name(v, paths, status, &name);
			v.overlay = Overlay::None;
			reset_library_popup_state(v);
			v.library_selection = LibrarySelection::None;
			v.bottom_bar_open_collection = None;
		}
		EditorAction::RequestDeleteChip(name) => {
			v.library_delete_message = chip_delete_confirm_message(v, &name);
			v.library_confirming_chip_delete = true;
		}
		EditorAction::BeginNewCollection => {
			v.library_creating_collection = true;
			v.library_renaming_collection = false;
			v.overlay_text_input.clear();
		}
		EditorAction::BeginRenameCollection => {
			if let LibrarySelection::Collection(i) = v.library_selection {
				if let Some(c) = v.prefs.chip_collections.get(i) {
					v.overlay_text_input = c.name.clone();
					v.library_renaming_collection = true;
					v.library_creating_collection = false;
				}
			}
		}
		EditorAction::RequestDeleteCollection => {
			if let LibrarySelection::Collection(i) = v.library_selection {
				if v.prefs.chip_collections.get(i).is_some_and(|c| c.chips.is_empty()) {
					delete_collection(&mut v.prefs, i);
					v.library_selection = LibrarySelection::None;
				} else {
					v.library_delete_message = "Are you sure you want to delete this collection? Its chips will be moved to \"OTHER\".".to_string();
					v.library_confirming_collection_delete = true;
				}
			}
		}
		EditorAction::ConfirmCollectionName => {
			let new_name = v.overlay_text_input.trim().to_string();
			if !new_name.is_empty() {
				if v.library_creating_collection {
					v.prefs.chip_collections.push(ChipCollection::new(&new_name, Vec::<String>::new()));
					v.library_selection = LibrarySelection::Collection(v.prefs.chip_collections.len() - 1);
				} else if v.library_renaming_collection {
					if let LibrarySelection::Collection(i) = v.library_selection {
						if let Some(c) = v.prefs.chip_collections.get_mut(i) {
							let old_name = c.name.clone();
							c.name = new_name.clone();
							for item in &mut v.prefs.starred_list {
								if item.is_collection && item.name.eq_ignore_ascii_case(&old_name) {
									item.name = new_name.clone();
								}
							}
						}
					}
				}
			}
			reset_library_popup_state(v);
		}
		EditorAction::CancelLibraryPopup => reset_library_popup_state(v),
		EditorAction::ConfirmDelete => {
			if v.library_confirming_chip_delete {
				let name = match v.library_selection {
					LibrarySelection::Chip(ci, chi) => v.prefs.chip_collections.get(ci).and_then(|c| c.chips.get(chi)).cloned(),
					LibrarySelection::Starred(i) => v.prefs.starred_list.get(i).filter(|it| !it.is_collection).map(|it| it.name.clone()),
					_ => None,
				};
				if let Some(name) = name {
					delete_chip_from_library(v, paths, status, &name);
				}
			} else if v.library_confirming_collection_delete {
				if let LibrarySelection::Collection(i) = v.library_selection {
					delete_collection(&mut v.prefs, i);
				}
				v.library_selection = LibrarySelection::None;
			}
			reset_library_popup_state(v);
		}
		EditorAction::PlaceChip(name) => {
			let mut desc = v.prefs.clone();
			if let Err(e) = Saver::save_project_description(paths, &mut desc) {
				*status = Some(format!("Failed to save chip library: {e}"));
			} else {
				v.prefs = desc;
			}
			v.overlay = Overlay::None;
			reset_library_popup_state(v);
			v.library_selection = LibrarySelection::None;
			v.pending_wire = None;
			v.pending_place = Some(name);
		}
		EditorAction::ExitLibrary => {
			let mut desc = v.prefs.clone();
			if let Err(e) = Saver::save_project_description(paths, &mut desc) {
				*status = Some(format!("Failed to save chip library: {e}"));
			} else {
				v.prefs = desc;
			}
			v.overlay = Overlay::None;
			reset_library_popup_state(v);
			v.library_selection = LibrarySelection::None;
		}
		EditorAction::ToggleStarredCollectionPopup(name) => {
			v.bottom_bar_open_collection = if v.bottom_bar_open_collection.as_deref() == Some(name.as_str()) { None } else { Some(name) };
		}
		EditorAction::CloseStarredCollectionPopup => v.bottom_bar_open_collection = None,
		EditorAction::UseChip(name) => {
			open_chip_by_name(v, paths, status, &name);
			v.overlay = Overlay::None;
			v.overlay_text_input.clear();
		}
		EditorAction::ConfirmName => confirm_naming_popup(v, status),
		EditorAction::ChooseKey(c) => v.overlay_key_choice = Some(c),
		EditorAction::ConfirmKey => confirm_key_select_popup(v, status),
		EditorAction::RomSelectCell(idx) => {
			if let Some(editor) = v.rom_editor.as_mut() {
				editor.selected = idx.min(editor_ui::ROM_WORD_COUNT - 1);
				v.overlay_text_input = editor.data[editor.selected].to_string();
			}
		}
		EditorAction::RomConfirmCell => confirm_rom_cell(v, status),
		EditorAction::RomApply => apply_rom_editor(v, status),
		EditorAction::SaveChipConfirm => confirm_save_chip_popup(v, paths, status),
		EditorAction::SaveChipSaveAs => confirm_save_chip_as(v, paths, status),
		EditorAction::SaveChipRename => confirm_save_chip_rename(v, paths, status),
	}
}

enum Screen {
	Menu,
	Viewer(ViewerState),
}

struct RenderState {
	window: Arc<Window>,
	renderer: Renderer,
}

struct App {
	paths: SavePaths,
	menu: MainMenu,
	screen: Screen,
	text_input: String,
	status: Option<String>,

	// Rendering / windowing (shared by both screens -- the menu and the
	// viewer are drawn into the same window/surface, just with different
	// scene-building code and a different logical camera).
	state: Option<RenderState>,
	viewport: Vec2,
	mouse_pos: Vec2,

	/// Current keyboard modifier state (updated from `WindowEvent::ModifiersChanged`,
	/// which winit reports independently of individual key press/release events).
	modifiers: ModifiersState,

	// Hit-boxes from the menu screen's *last drawn* frame, used by the
	// next mouse click (immediate-mode UI: layout is recomputed every
	// frame, so "what did I just draw" is also "what can be clicked").
	last_menu_buttons: Vec<menu_ui::UiButton>,
	// Hit-boxes for the menu's popup (rename/new-project/delete-confirm dialog), if one is open, from
	// the same last-drawn frame. Kept separate from `last_menu_buttons` -- and always checked first --
	// so a click can't be mis-attributed to a screen button underneath the popup.
	last_popup_buttons: Vec<menu_ui::UiButton>,
}

impl App {
	fn new(paths: SavePaths) -> Self {
		let mut menu = MainMenu::new(paths.clone());
		menu.on_menu_opened();
		App {
			paths,
			menu,
			screen: Screen::Menu,
			text_input: String::new(),
			status: None,
			state: None,
			viewport: Vec2::new(1280.0, 800.0),
			mouse_pos: Vec2::ZERO,
			modifiers: ModifiersState::empty(),
			last_menu_buttons: Vec::new(),
			last_popup_buttons: Vec::new(),
		}
	}

	fn window_title(&self) -> String {
		match &self.screen {
			Screen::Menu => "Digital Logic Sim".to_string(),
			Screen::Viewer(v) => format!("Digital Logic Sim -- {} / {}", v.project_name, v.root_chip_name),
		}
	}

	fn set_window_title(&self) {
		if let Some(state) = &self.state {
			state.window.set_title(&self.window_title());
		}
	}

	// ---- Screen transitions ----

	fn open_project(&mut self, name: &str) {
		let project_dir = self.paths.project_path(name);
		match load_project(&project_dir) {
			Ok((project_desc, mut library, errors)) => {
				for e in &errors {
					eprintln!("warning: {e}");
				}
				register_all_builtins(&mut library);

				// Every project opens onto a blank, unsaved chip rather than jumping back into whichever
				// custom chip happens to be "last" (or biggest) -- mirrors Ctrl+N rather than remembering a chip to reopen.
				let root_chip_name = unique_new_chip_name(&library);
				library.add(ChipDescription::new(&root_chip_name, ChipType::Custom));

				let root_desc = library.get(&root_chip_name).clone();
				let mut sim = Simulator::build(&root_desc, &library);
				// In case modifier keys are already held down (e.g. Alt from the menu action that
				// opened this project) by the time the viewer appears, rather than only picking them up on the next change.
				sim.key_modifiers = encode_modifiers(self.modifiers);
				let show_grid = project_desc.prefs_grid_display_mode == 1;

				let mut prefs = project_desc;
				if prefs.chip_collections.is_empty() {
					prefs.chip_collections = default_chip_collections();
				}
				if prefs.starred_list.is_empty() {
					prefs.starred_list = default_starred_list();
				}

				self.screen = Screen::Viewer(ViewerState {
					project_name: name.to_string(),
					library,
					root_chip_name,
					sim,
					camera: Camera::new(self.viewport),
					dragging: false,
					last_cursor: Vec2::ZERO,
					camera_fitted: false,
					show_grid,
					prefs,
					overlay: Overlay::None,
					overlay_text_input: String::new(),
					overlay_key_choice: None,
					naming_purpose: NamingPurpose::default(),
					key_select_purpose: KeySelectPurpose::default(),
					rom_editor: None,
					last_overlay_buttons: Vec::new(),
					library_selection: LibrarySelection::None,
					library_creating_collection: false,
					library_renaming_collection: false,
					library_confirming_chip_delete: false,
					library_confirming_collection_delete: false,
					library_delete_message: String::new(),
					bottom_bar_open_collection: None,
					last_bottom_bar_buttons: Vec::new(),
					last_bottom_bar_popup_buttons: Vec::new(),
					context_menu: None,
					last_context_menu_buttons: Vec::new(),
					pending_wire: None,
					pending_place: None,
				});
				self.status = None;
				self.set_window_title();
			}
			Err(e) => {
				self.status = Some(format!("Failed to open project '{name}': {e}"));
			}
		}
	}

	fn return_to_menu(&mut self) {
		self.screen = Screen::Menu;
		self.menu.on_menu_opened();
		self.set_window_title();
	}

	// ---- Menu action handling ----

	fn open_name_popup_with(&mut self, prefill: &str) {
		self.text_input = prefill.to_string();
	}

	fn handle_menu_action(&mut self, action: UiAction, event_loop: &ActiveEventLoop) {
		match action {
			UiAction::NewProject => {
				self.menu.choose_new_project();
				self.open_name_popup_with("");
			}
			UiAction::OpenProjectScreen => self.menu.choose_open_project(),
			UiAction::SettingsScreen => self.menu.choose_settings(),
			UiAction::AboutScreen => self.menu.choose_about(),
			UiAction::Quit => event_loop.exit(),
			UiAction::BackToMain => self.menu.back_to_main(),

			UiAction::SelectProject(i) => self.menu.select_project(i),
			UiAction::OpenSelected => {
				if let Some(MenuOutcome::OpenProject { name }) = self.menu.open_selected() {
					self.open_project(&name);
				}
			}
			UiAction::RenameSelected => {
				let current = self.menu.selected_project().map(|p| p.project_name.clone()).unwrap_or_default();
				self.menu.request_rename_selected();
				if self.menu.popup() == PopupKind::RenameProject {
					self.open_name_popup_with(&current);
				}
			}
			UiAction::DuplicateSelected => {
				self.menu.request_duplicate_selected();
				if self.menu.popup() == PopupKind::DuplicateProject {
					self.open_name_popup_with("");
				}
			}
			UiAction::DeleteSelected => self.menu.request_delete_selected(),
			UiAction::RefreshProjects => self.menu.refresh_projects(),

			UiAction::PopupConfirm => self.confirm_popup(),
			UiAction::PopupCancel => {
				self.menu.cancel_popup();
				self.text_input.clear();
			}

			UiAction::ToggleVsync => {
				let mut s = self.menu.edited_settings();
				s.vsync_enabled = !s.vsync_enabled;
				self.menu.set_edited_settings(s);
			}
			UiAction::CycleFullscreenMode => {
				use logic_sim::FullScreenMode::*;
				let mut s = self.menu.edited_settings();
				s.fullscreen_mode = match s.fullscreen_mode {
					Windowed => FullScreenWindow,
					FullScreenWindow => MaximizedWindow,
					MaximizedWindow => ExclusiveFullScreen,
					ExclusiveFullScreen => Windowed,
				};
				self.menu.set_edited_settings(s);
			}
			UiAction::ApplySettings => {
				if let Err(e) = self.menu.apply_settings() {
					self.status = Some(format!("Failed to save settings: {e}"));
				}
			}
		}
	}

	fn confirm_popup(&mut self) {
		match self.menu.popup() {
			PopupKind::DeleteConfirmation => {
				if let Err(e) = self.menu.confirm_delete() {
					self.status = Some(format!("Failed to delete project: {e}"));
				}
			}
			PopupKind::NewProject | PopupKind::RenameProject | PopupKind::DuplicateProject => {
				match self.menu.confirm_name_popup(&self.text_input.clone()) {
					Ok(Some(MenuOutcome::OpenProject { name })) => {
						self.text_input.clear();
						self.open_project(&name);
					}
					Ok(_) => self.text_input.clear(),
					Err(e) => self.status = Some(format!("Failed: {e}")),
				}
			}
			PopupKind::None => {}
		}
	}

	// ---- Text input for name popups ----

	fn is_text_popup_open(&self) -> bool {
		matches!(self.menu.popup(), PopupKind::NewProject | PopupKind::RenameProject | PopupKind::DuplicateProject)
	}
}

impl ApplicationHandler for App {
	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		if self.state.is_some() {
			return;
		}

		let window_attrs = Window::default_attributes().with_title(self.window_title()).with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
		let window = Arc::new(event_loop.create_window(window_attrs).expect("failed to create window"));

		let size = window.inner_size();
		self.viewport = Vec2::new(size.width as f32, size.height as f32);
		let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
		let surface = instance.create_surface(window.clone()).expect("failed to create surface");
		let renderer = pollster::block_on(Renderer::new(&instance, surface, size.width, size.height));

		self.state = Some(RenderState { window, renderer });
	}

	fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
		if self.state.is_none() {
			return;
		}

		match event {
			WindowEvent::CloseRequested => event_loop.exit(),

			WindowEvent::Resized(size) => {
				if let Some(state) = self.state.as_mut() {
					state.renderer.resize(size.width, size.height);
				}
				self.viewport = Vec2::new(size.width as f32, size.height as f32);
				if let Screen::Viewer(v) = &mut self.screen {
					v.camera.resize_viewport(size.width as f32, size.height as f32);
				}
			}

			WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(event, event_loop),

			WindowEvent::ModifiersChanged(mods) => {
				self.modifiers = mods.state();
				if let Screen::Viewer(v) = &mut self.screen {
					v.sim.key_modifiers = encode_modifiers(self.modifiers);
				}
			}

			// Physically-held keys don't generate a release event if focus
			// is lost while they're down (e.g. alt-tabbing away) -- without
			// this, a Key/KeyMods chip could get stuck "on" indefinitely.
			WindowEvent::Focused(false) => {
				self.modifiers = ModifiersState::empty();
				if let Screen::Viewer(v) = &mut self.screen {
					v.sim.held_keys.clear();
					v.sim.key_modifiers = 0;
				}
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Left, .. } => {
				self.handle_mouse_button(btn_state, event_loop);
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Middle, .. } => {
				self.handle_middle_mouse_button(btn_state);
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Right, .. } => {
				self.handle_right_mouse_button(btn_state);
			}

			WindowEvent::CursorMoved { position, .. } => {
				let cursor = Vec2::new(position.x as f32, position.y as f32);
				self.mouse_pos = cursor;
				if let Screen::Viewer(v) = &mut self.screen {
					if v.dragging {
						let before = v.camera.screen_to_world(v.last_cursor);
						let after = v.camera.screen_to_world(cursor);
						v.camera.pan(Vec2::new(before.x - after.x, before.y - after.y));
					}
					v.last_cursor = cursor;
				}
			}

			WindowEvent::MouseWheel { delta, .. } => {
				if let Screen::Viewer(v) = &mut self.screen {
					let scroll = match delta {
						MouseScrollDelta::LineDelta(_, y) => y,
						MouseScrollDelta::PixelDelta(p) => (p.y / 100.0) as f32,
					};
					let zoom_factor = 1.0 + scroll * 0.1;
					v.camera.zoom_at(v.last_cursor, zoom_factor);
				}
			}

			WindowEvent::RedrawRequested => self.redraw(event_loop),

			_ => {}
		}
	}
}

impl App {
	/// Left-click handling: overlay/UI button hits first (unchanged), then
	/// -- new -- toggling one bit of a root input dev-pin if the click
	/// landed on one of its clickable cells. Falls through to the toggle
	/// check whenever the click wasn't swallowed by a *modal* popup --
	/// same "overlay is None or the (non-modal) Library sidebar" gate the
	/// old dragging code used, so switching a chip from the Library
	/// sidebar doesn't leave clicks stuck unable to reach the canvas.
	/// Camera panning is *not* handled here any more -- see
	/// `handle_middle_mouse_button`.
	fn handle_mouse_button(&mut self, btn_state: ElementState, event_loop: &ActiveEventLoop) {
		match &mut self.screen {
			Screen::Menu => {
				if btn_state == ElementState::Pressed {
					// The popup (if open) is the top-most layer, so it gets first refusal at every click.
					// This must check `last_popup_buttons` in isolation and return either way, or a
					// click landing where a popup button overlaps a screen button could fall through.
					if self.menu.popup() != PopupKind::None {
						let hit = self.last_popup_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
						if let Some(action) = hit {
							self.handle_menu_action(action, event_loop);
						}
						return;
					}

					let hit = self.last_menu_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
					if let Some(action) = hit {
						self.handle_menu_action(action, event_loop);
					}
				}
			}
			Screen::Viewer(v) => {
				// The context menu is always the top-most layer, so it gets first refusal at every
				// click -- a left click either picks one of its rows or closes it, either way
				// swallowing the click rather than letting it reach the overlay/canvas underneath.
				if btn_state == ElementState::Pressed && v.context_menu.is_some() {
					let hit = v.last_context_menu_buttons.iter().find(|b| b.rect.contains(self.mouse_pos)).map(|b| b.id.clone());
					let target = v.context_menu.take().map(|s| s.target);
					if let (Some(id), Some(target)) = (hit, target) {
						apply_context_menu_action(v, &self.paths, &mut self.status, &target, id);
					}
					return;
				}

				if btn_state == ElementState::Pressed && v.overlay != Overlay::None {
					let hit = v.last_overlay_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
					if let Some(action) = hit {
						apply_editor_action(v, &self.paths, &mut self.status, action);
					}
					// Every overlay -- including the library now -- is a
					// full-screen modal, so a click that missed every
					// button still belongs to it, not the canvas below.
					return;
				}

				if btn_state == ElementState::Pressed {
					let world_pos = v.camera.screen_to_world(self.mouse_pos);
					// handle UI
					{
						// The bottom bar's starred-collection flyout, if one's
						// open, gets first refusal -- same "top layer first"
						// priority the context menu and overlays get above.
						if v.bottom_bar_open_collection.is_some() {
							let hit = v
								.last_bottom_bar_popup_buttons
								.iter()
								.find(|b| b.enabled && b.rect.contains(self.mouse_pos))
								.map(|b| b.action.clone());
							if let Some(action) = hit {
								apply_editor_action(v, &self.paths, &mut self.status, action);
								v.bottom_bar_open_collection = None;
								return;
							}
						}

						let bar_hit =
							v.last_bottom_bar_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
						if let Some(action) = bar_hit {
							apply_editor_action(v, &self.paths, &mut self.status, action);
							return;
						}
						// Missed the bar and (if it was open) its flyout: a
						// click anywhere else closes the flyout, same as the
						// original's "click outside" dismissal.
						v.bottom_bar_open_collection = None;
					}
					// Handle canvas
					{
						// A chip picked up for placement claims every click ahead of anything else below,
						// same "claims the click" priority a wire in progress gets just below -- see
						// `try_place_pending_chip`'s doc comment for what actually happens with the click.
						if v.pending_place.is_some() {
							try_place_pending_chip(v, world_pos, &mut self.status);
							return;
						}

						// A wire already being placed claims every click ahead of anything else below --
						// including the input-pin toggle, so clicking a switch's pin finishes/bends the
						// wire instead of flipping it (see `try_continue_pending_wire`'s doc comment).
						if v.pending_wire.is_some() {
							try_continue_pending_wire(v, world_pos, &mut self.status);
							return;
						}
						if try_start_pending_wire(v, world_pos) {
							return;
						}

						let root_desc = v.library.get(&v.root_chip_name);
						if let Some((pin_id, bit_index)) = hit_test_root_input_pin_click(root_desc, world_pos) {
							let root_chip_name = v.root_chip_name.clone();
							toggle_driven_input_bit(&mut v.library, &root_chip_name, pin_id, bit_index);
						}
					}
				}
			}
		}
	}

	/// Right-click handling: opens (or, if the click didn't land on
	/// anything right-clickable, just closes) the generic context-menu
	/// popup from `render::context_menu`. For now this is wired up for
	/// two targets, both offering a single "Open" action:
	///  - a placed component on the canvas (opens *its* chip definition,
	///    same as double-clicking it used to in the original), and
	///  - a row in the (open) library sidebar (opens that chip -- left
	///    click there only highlights the row now, see
	///    `EditorAction::SelectChip`'s docs).
	/// Easy to attach to more things later: building a `ContextMenuState`
	/// with a different `target`/`items` and assigning it to
	/// `v.context_menu` is the whole integration surface.
	/// Right-click handling:
	///  - on a wire, deletes it immediately (no popup -- see
	///    `scene::hit_test_wire`/`delete_wire`'s docs for the "shortest
	///    possible section" semantics), taking priority over the popup
	///    path entirely;
	///  - on a placed component, a dev-pin of the current root chip, a
	///    library row, or a starred bottom-bar chip button (bar itself or
	///    an open collection's flyout), opens the generic context-menu
	///    popup from `render::context_menu` with whichever rows apply
	///    (see `context_menu_items_for_component`);
	///  - anywhere else, just closes whatever popup was already open.
	/// Hit-tests run in the same order things are actually drawn on top
	/// of each other (library row > bottom bar > dev-pin > component >
	/// wire), so a click that could plausibly land on more than one
	/// resolves to whichever one the player can actually see.
	fn handle_right_mouse_button(&mut self, btn_state: ElementState) {
		if btn_state != ElementState::Pressed {
			return;
		}
		let Screen::Viewer(v) = &mut self.screen else { return };

		// Right-clicking always closes whatever popup was already open
		// (matches normal desktop-app behaviour: a fresh right-click
		// replaces the previous context menu rather than stacking).
		v.context_menu = None;
		// Also the standard "cancel" gesture for an in-progress wire
		// placement or a chip pending placement, same as Escape (see
		// the keyboard handler).
		v.pending_wire = None;
		v.pending_place = None;

		// A right click while a *modal* overlay is open (anything but
		// the library sidebar) has nothing sensible to attach to, so
		// just leave the popup closed.
		if v.overlay != Overlay::None && v.overlay != Overlay::Library {
			return;
		}

		// 1) A chip row in the open library panel -- pop up its
		// Open/Delete menu. The library is a full-screen modal (like
		// every other overlay), so a click that misses every row still
		// belongs to the library, not the canvas behind it.
		if v.overlay == Overlay::Library {
			let hit = v.last_overlay_buttons.iter().find(|b| b.rect.contains(self.mouse_pos)).and_then(|b| match &b.action {
				EditorAction::SelectChipRow { collection, chip } => {
					v.prefs.chip_collections.get(*collection).and_then(|c| c.chips.get(*chip)).cloned()
				}
				_ => None,
			});
			if let Some(name) = hit {
				let custom = is_custom_chip(&v.library, &name);
				let items = vec![
					ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, custom),
					ContextMenuItem::new_enabled("Delete", ContextMenuAction::Delete, custom),
				];
				v.context_menu = Some(ContextMenuState::new(format!("libchip:{name}"), self.mouse_pos, items));
			}
			return;
		}

		let root_chip_name = v.root_chip_name.clone();
		let world_pos = v.camera.screen_to_world(self.mouse_pos);

		// 2) A chip button in the starred bottom bar -- either directly in the bar itself, or
		// listed inside an open collection's flyout (checked first, same "top layer first"
		// priority the flyout gets over the bar in `handle_mouse_button`'s left-click handling).
		// Both are screen-space, not world-space, so they're hit-tested against last frame's
		// button rects rather than `world_pos` like everything below.
		{
			let flyout_hit = v.last_bottom_bar_popup_buttons.iter().find(|b| b.rect.contains(self.mouse_pos)).and_then(|b| match &b.action {
				EditorAction::PlaceChip(name) => Some(name.clone()),
				_ => None,
			});
			if let Some(name) = flyout_hit {
				let items = vec![ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(&v.library, &name))];
				v.context_menu = Some(ContextMenuState::new(format!("flyoutchip:{name}"), self.mouse_pos, items));
				return;
			}

			let bar_hit = v.last_bottom_bar_buttons.iter().find(|b| b.rect.contains(self.mouse_pos)).and_then(|b| match &b.action {
				EditorAction::PlaceChip(name) => Some(name.clone()),
				_ => None,
			});
			if let Some(name) = bar_hit {
				let items = vec![
					ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(&v.library, &name)),
					ContextMenuItem::new("Un-star", ContextMenuAction::Unstar),
				];
				v.context_menu = Some(ContextMenuState::new(format!("barchip:{name}"), self.mouse_pos, items));
				return;
			}
		}

		// 3) One of the current root chip's own boundary dev-pins.
		{
			let root_desc = v.library.get(&root_chip_name);
			if let Some((is_input, pin_id)) = hit_test_dev_pin(root_desc, world_pos) {
				let target = format!("devpin:{}:{}", if is_input { "in" } else { "out" }, pin_id);
				v.context_menu = Some(ContextMenuState::new(target, self.mouse_pos, vec![ContextMenuItem::new("Label", ContextMenuAction::Label)]));
				return;
			}
		}

		// 4) A placed component on the canvas.
		{
			let root_desc = v.library.get(&root_chip_name);
			let placed = place_sub_chips(root_desc, &v.library);
			if let Some(sub) = hit_test_sub_chip(&placed, world_pos) {
				let id = sub.id;
				let chip_name = sub.desc.name.clone();
				let items = context_menu_items_for_component(&v.library, &chip_name);
				v.context_menu = Some(ContextMenuState::new(format!("component:{id}"), self.mouse_pos, items));
				return;
			}
		}

		// 5) A wire -- deleted immediately, no popup (see this method's
		// doc comment).
		{
			let root_desc = v.library.get(&root_chip_name);
			// Fixed screen-pixel tolerance converted to world units, so the click target stays the
			// same apparent size regardless of current zoom.
			let max_dist = 6.0 / v.camera.zoom.max(0.0001);
			if let Some(wire_idx) = hit_test_wire(root_desc, &v.library, world_pos, max_dist) {
				let chip = v.library.get_mut(&root_chip_name);
				delete_wire(chip, wire_idx);
				v.rebuild_sim();
			}
		}
	}

	/// Middle-click handling: drags/pans the camera, exactly like left-click
	/// used to. Split out from `handle_mouse_button` so left-click is free
	/// to toggle input dev-pins instead, and right-click free for
	/// `handle_right_mouse_button`'s context-menu popup. Mirrors the same
	/// "swallow clicks while a modal popup is open" gate `handle_mouse_button`
	/// applies, so panning can't happen "through" an open popup either.
	fn handle_middle_mouse_button(&mut self, btn_state: ElementState) {
		if let Screen::Viewer(v) = &mut self.screen {
			if btn_state == ElementState::Pressed && v.overlay != Overlay::None {
				return;
			}
			v.dragging = btn_state == ElementState::Pressed;
		}
	}

	fn handle_key_event(&mut self, event: winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
		// Feed the Key chip's held-key set on both press and release (not just press, unlike the
		// shortcut handling below) since it needs to know when a key stops being held. The chip
		// stores/compares its target letter in capitals, so lowercase 'a' must register as 'A' here.
		if let Key::Character(s) = &event.logical_key {
			if let Screen::Viewer(v) = &mut self.screen {
				if let Some(c) = s.chars().next() {
					let c = c.to_ascii_uppercase();
					match event.state {
						ElementState::Pressed => {
							v.sim.held_keys.insert(c);
						}
						ElementState::Released => {
							v.sim.held_keys.remove(&c);
						}
					}
				}
			}
		}

		if event.state != ElementState::Pressed {
			return;
		}

		match &mut self.screen {
			Screen::Menu => {
				if self.is_text_popup_open() {
					match &event.logical_key {
						Key::Named(NamedKey::Backspace) => {
							self.text_input.pop();
						}
						Key::Named(NamedKey::Enter) => self.confirm_popup(),
						Key::Named(NamedKey::Escape) => {
							self.menu.cancel_popup();
							self.text_input.clear();
						}
						Key::Character(s) => {
							if self.text_input.chars().count() < logic_sim::ui_menu::MAX_PROJECT_NAME_LENGTH {
								self.text_input.push_str(s);
							}
						}
						_ => {}
					}
				} else if self.menu.popup() == PopupKind::DeleteConfirmation {
					match &event.logical_key {
						Key::Named(NamedKey::Enter) => self.confirm_popup(),
						Key::Named(NamedKey::Escape) => self.menu.cancel_popup(),
						_ => {}
					}
				} else if event.logical_key == Key::Named(NamedKey::Escape) {
					self.menu.back_to_main();
				}
			}
			Screen::Viewer(v) => match &event.logical_key {
				// ---- Text entry for the search / naming / ROM-cell overlays ----
				Key::Named(NamedKey::Backspace)
					if matches!(v.overlay, Overlay::Search | Overlay::Naming | Overlay::RomEditor | Overlay::SaveChip)
						|| (v.overlay == Overlay::Library && (v.library_creating_collection || v.library_renaming_collection)) =>
				{
					v.overlay_text_input.pop();
				}
				Key::Named(NamedKey::Enter) if v.overlay == Overlay::Naming => {
					confirm_naming_popup(v, &mut self.status);
				}
				Key::Named(NamedKey::Enter) if v.overlay == Overlay::Library && (v.library_creating_collection || v.library_renaming_collection) => {
					apply_editor_action(v, &self.paths, &mut self.status, EditorAction::ConfirmCollectionName);
				}
				Key::Named(NamedKey::Enter) if v.overlay == Overlay::RomEditor => {
					confirm_rom_cell(v, &mut self.status);
				}
				Key::Named(NamedKey::Enter) if v.overlay == Overlay::KeySelect && v.overlay_key_choice.is_some() => {
					confirm_key_select_popup(v, &mut self.status);
				}
				// Enter only auto-confirms the unambiguous save-chip modes (a single "Save"/"Replace"
				// action) -- when both "Save As" and "Rename" are on offer, that choice needs a click.
				Key::Named(NamedKey::Enter)
					if v.overlay == Overlay::SaveChip && save_chip_mode(v, &v.overlay_text_input) != editor_ui::SaveChipMode::SaveAsOrRename =>
				{
					confirm_save_chip_popup(v, &self.paths, &mut self.status);
				}
				Key::Character(s)
					if matches!(v.overlay, Overlay::Search | Overlay::Naming | Overlay::SaveChip)
						|| (v.overlay == Overlay::Library && (v.library_creating_collection || v.library_renaming_collection)) =>
				{
					if v.overlay_text_input.chars().count() < 64 {
						v.overlay_text_input.push_str(s);
					}
				}
				// ROM cell values are short numbers -- a lower cap keeps a
				// stray paste from overflowing the little text field.
				Key::Character(s) if v.overlay == Overlay::RomEditor => {
					if v.overlay_text_input.chars().count() < 10 {
						v.overlay_text_input.push_str(s);
					}
				}
				// ---- Key-select overlay: capture the next alphanumeric key ----
				Key::Character(s) if v.overlay == Overlay::KeySelect => {
					if let Some(c) = s.chars().next() {
						let upper = c.to_ascii_uppercase();
						if editor_ui::KEY_SELECT_ALLOWED_CHARS.contains(upper) {
							v.overlay_key_choice = Some(upper);
						}
					}
				}
				// ---- Normal viewer shortcuts (only while nothing's open) ----
				Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("r") => v.rebuild_sim(),
				Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("f") => v.camera_fitted = !v.camera_fitted,
				Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("g") => v.show_grid = !v.show_grid,
				Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("p") => v.overlay = Overlay::Preferences,
				Key::Character(s)
					if (v.overlay == Overlay::None || v.overlay == Overlay::Library)
						&& self.modifiers.control_key()
						&& s.eq_ignore_ascii_case("f") =>
				{
					v.overlay = Overlay::Search;
					v.overlay_text_input.clear();
				}
				Key::Character(s)
					if (v.overlay == Overlay::None || v.overlay == Overlay::Library)
						&& self.modifiers.control_key()
						&& s.eq_ignore_ascii_case("s") =>
				{
					v.overlay = Overlay::SaveChip;
					v.overlay_text_input = v.root_chip_name.clone();
				}
				Key::Character(s)
					if (v.overlay == Overlay::None || v.overlay == Overlay::Library)
						&& self.modifiers.control_key()
						&& s.eq_ignore_ascii_case("n") =>
				{
					start_new_chip(v, &self.paths, &mut self.status);
				}
				Key::Named(NamedKey::Tab) => {
					if v.overlay == Overlay::Library {
						let mut desc = v.prefs.clone();
						if Saver::save_project_description(&self.paths, &mut desc).is_ok() {
							v.prefs = desc;
						}
						v.overlay = Overlay::None;
						reset_library_popup_state(v);
						v.library_selection = LibrarySelection::None;
					} else if v.overlay == Overlay::None {
						sync_library_collections(&mut v.prefs, &v.library);
						v.overlay = Overlay::Library;
					}
				}
				Key::Named(NamedKey::Escape) => {
					if v.overlay == Overlay::Library
						&& (v.library_creating_collection
							|| v.library_renaming_collection
							|| v.library_confirming_chip_delete
							|| v.library_confirming_collection_delete)
					{
						reset_library_popup_state(v);
					} else if v.overlay != Overlay::None {
						v.overlay = Overlay::None;
						v.overlay_text_input.clear();
						v.rom_editor = None;
						reset_library_popup_state(v);
						v.library_selection = LibrarySelection::None;
					} else if v.pending_wire.is_some() || v.pending_place.is_some() {
						v.pending_wire = None;
						v.pending_place = None;
					} else if v.bottom_bar_open_collection.is_some() {
						v.bottom_bar_open_collection = None;
					} else {
						self.return_to_menu();
					}
				}
				_ => {}
			},
		}

		let _ = event_loop;
	}

	fn redraw(&mut self, event_loop: &ActiveEventLoop) {
		let (vw, vh) = self.viewport.to_tuple();

		// Layers are drawn back-to-front, each as its own fully-submitted pass, so a later layer's
		// triangles paint over an earlier layer's text: 0 world (grid + chip scene), 1 ui_overlay
		// (library/search/preferences/naming/key-select), 2 context_menu (right-click popup, top-most).
		let world_layer;
		let mut ui_overlay_layer = SceneGeometry::default();
		let mut context_menu_layer = SceneGeometry::default();

		match &mut self.screen {
			Screen::Menu => {
				let mut frame = menu_ui::build_screen(&self.menu, vw, vh, self.mouse_pos);
				if let Some(msg) = &self.status {
					frame.geometry.labels.push(menu_ui::status_label(vw, vh, msg));
				}
				self.last_menu_buttons = frame.buttons.clone();
				world_layer = frame.geometry;

				// Popup (rename/new-project/delete-confirm), if open, is its own layer: guarantees its
				// background and text both composite on top of the screen underneath, rather than
				// sharing one pass with it, and lets clicks be tested against it in isolation.
				if self.menu.popup() != PopupKind::None {
					let popup_frame = menu_ui::build_popup_frame(&self.menu, vw, vh, &self.text_input, self.mouse_pos);
					self.last_popup_buttons = popup_frame.buttons;
					ui_overlay_layer = popup_frame.geometry;
				} else {
					self.last_popup_buttons.clear();
				}
			}
			Screen::Viewer(v) => {
				let root_desc = v.library.get(&v.root_chip_name);
				let external_inputs: Vec<logic_sim::sim::ExternalInput> = root_desc
					.input_pins
					.iter()
					.map(|pin| logic_sim::sim::ExternalInput { address: logic_sim::description::PinAddress::new(pin.id, 0), state: pin.driven_state })
					.collect();
				v.sim.run_simulation_step(&external_inputs);

				let root_desc = v.library.get(&v.root_chip_name);
				let lookup = SimulatorPinState { sim: &v.sim, scope: v.sim.root() };
				let hover_world_pos = Some(v.camera.screen_to_world(self.mouse_pos));
				let chip_scene = build_scene(root_desc, &v.library, &lookup, hover_world_pos);

				if !v.camera_fitted {
					let bounds = bounding_box(&chip_scene).or_else(|| bounding_box(&build_scene(root_desc, &v.library, &AllLow, None)));
					if let Some((min, max)) = bounds {
						v.camera.fit_to_bounds(min, max, 0.15);
					}
					v.camera_fitted = true;
				}

				let mut scene = if v.show_grid { build_grid(&v.camera, theme::GRID_COL) } else { SceneGeometry::default() };
				scene.triangles.extend(chip_scene.triangles);
				scene.labels.extend(chip_scene.labels);
				if let Some(pending) = &v.pending_wire {
					draw_pending_wire_preview(&mut scene, pending, hover_world_pos.expect("just set above"));
				}
				if let Some(chip_name) = &v.pending_place {
					if let Some(ghost) = build_pending_place_scene(&v.library, chip_name, hover_world_pos.expect("just set above")) {
						scene.triangles.extend(ghost.triangles);
						scene.labels.extend(ghost.labels);
					}
				}
				world_layer = scene;

				// Bottom bar of starred chips/collections is always drawn (mirrors `BottomBarUI`
				// always being visible), its buttons just disabled while a modal overlay is open --
				// see `EditorAction::ToggleStarredCollectionPopup`'s docs for what its "MENU" button
				// equivalent deliberately doesn't do here.
				let bar_enabled = v.overlay == Overlay::None;
				let bar_cycle_blocked: std::collections::HashSet<String> = v
					.prefs
					.starred_list
					.iter()
					.filter(|it| !it.is_collection && would_create_cycle(&v.library, &v.root_chip_name, &it.name))
					.map(|it| it.name.to_ascii_lowercase())
					.collect();
				let bar_frame = editor_ui::build_starred_bottom_bar(
					&v.prefs.starred_list,
					v.bottom_bar_open_collection.as_deref(),
					bar_enabled,
					&bar_cycle_blocked,
					vw,
					vh,
					self.mouse_pos,
				);
				v.last_bottom_bar_buttons = bar_frame.buttons;
				let mut bar_geometry = bar_frame.geometry;

				if bar_enabled {
					if let Some(open_name) = v.bottom_bar_open_collection.clone() {
						if let Some(collection) = v.prefs.chip_collections.iter().find(|c| c.name.eq_ignore_ascii_case(&open_name)) {
							let anchor_x = v
								.last_bottom_bar_buttons
								.iter()
								.find(|b| matches!(&b.action, EditorAction::ToggleStarredCollectionPopup(n) if n.eq_ignore_ascii_case(&open_name)))
								.map(|b| b.rect.x)
								.unwrap_or(8.0);
							let flyout_cycle_blocked: std::collections::HashSet<String> = collection
								.chips
								.iter()
								.filter(|n| would_create_cycle(&v.library, &v.root_chip_name, n))
								.map(|n| n.to_ascii_lowercase())
								.collect();
							let popup_frame =
								editor_ui::build_starred_collection_popup(collection, anchor_x, true, &flyout_cycle_blocked, vw, vh, self.mouse_pos);
							v.last_bottom_bar_popup_buttons = popup_frame.buttons;
							bar_geometry.triangles.extend(popup_frame.geometry.triangles);
							bar_geometry.labels.extend(popup_frame.geometry.labels);
						} else {
							v.last_bottom_bar_popup_buttons.clear();
						}
					} else {
						v.last_bottom_bar_popup_buttons.clear();
					}
				} else {
					v.last_bottom_bar_popup_buttons.clear();
				}

				ui_overlay_layer = pin_overlay_to_screen(bar_geometry, &v.camera, vw, vh);

				// Overlays are laid out in screen-pixel space by `editor_ui` -- remap that into
				// `v.camera`'s current world space so they stay pinned to the screen regardless of
				// pan/zoom. Appended onto the bottom bar's own layer (rather than replacing it) so
				// a modal overlay still composites on top of the bar drawn beneath it.
				if v.overlay != Overlay::None {
					let overlay_frame = match v.overlay {
						Overlay::Library => {
							let selected_chip_name = match v.library_selection {
								LibrarySelection::Chip(ci, chi) => v.prefs.chip_collections.get(ci).and_then(|c| c.chips.get(chi)).cloned(),
								LibrarySelection::Starred(i) => v.prefs.starred_list.get(i).filter(|it| !it.is_collection).map(|it| it.name.clone()),
								_ => None,
							};
							let selected_chip_is_custom = selected_chip_name.as_deref().is_some_and(|n| is_custom_chip(&v.library, n));
							let selected_chip_would_cycle =
								selected_chip_name.as_deref().is_some_and(|n| would_create_cycle(&v.library, &v.root_chip_name, n));
							let state = editor_ui::ChipLibraryState {
								collections: &v.prefs.chip_collections,
								starred_list: &v.prefs.starred_list,
								selection: v.library_selection,
								selected_chip_is_custom,
								selected_chip_would_cycle,
								creating_collection: v.library_creating_collection,
								renaming_collection: v.library_renaming_collection,
								name_field_text: &v.overlay_text_input,
								confirming_chip_delete: v.library_confirming_chip_delete,
								confirming_collection_delete: v.library_confirming_collection_delete,
								delete_confirm_message: &v.library_delete_message,
							};
							editor_ui::build_chip_library_panel(&state, vw, vh, self.mouse_pos)
						}
						Overlay::Search => {
							let mut names: Vec<String> = v.library.iter().map(|d| d.name.clone()).collect();
							names.sort();
							editor_ui::build_search_popup(&names, &v.overlay_text_input, vw, vh, self.mouse_pos)
						}
						Overlay::Preferences => editor_ui::build_preferences_panel(&v.prefs, vw, vh, self.mouse_pos),
						Overlay::Naming => {
							let confirm_enabled = !v.overlay_text_input.trim().is_empty();
							let title = match v.naming_purpose {
								NamingPurpose::RenameProject => "Rename project",
								NamingPurpose::LabelComponent(_) => "Label component",
								NamingPurpose::LabelDevPin { .. } => "Label pin",
								NamingPurpose::ConfigurePulseDuration(_) => "Pulse length (ticks)",
							};
							editor_ui::build_simple_naming_popup(title, &v.overlay_text_input, confirm_enabled, vw, vh, self.mouse_pos)
						}
						Overlay::KeySelect => editor_ui::build_key_select_popup(v.overlay_key_choice, vw, vh, self.mouse_pos),
						Overlay::RomEditor => {
							let (data, selected) = v
								.rom_editor
								.as_ref()
								.map(|e| (e.data.clone(), e.selected))
								.unwrap_or_else(|| (vec![0; editor_ui::ROM_WORD_COUNT], 0));
							editor_ui::build_rom_editor_popup(&data, selected, &v.overlay_text_input, vw, vh, self.mouse_pos)
						}
						Overlay::SaveChip => {
							let mode = save_chip_mode(v, &v.overlay_text_input);
							editor_ui::build_save_chip_popup(&v.root_chip_name, &v.overlay_text_input, mode, vw, vh, self.mouse_pos)
						}
						Overlay::None => unreachable!(),
					};
					v.last_overlay_buttons = overlay_frame.buttons;
					let pinned_overlay = pin_overlay_to_screen(overlay_frame.geometry, &v.camera, vw, vh);
					ui_overlay_layer.triangles.extend(pinned_overlay.triangles);
					ui_overlay_layer.labels.extend(pinned_overlay.labels);
				} else {
					v.last_overlay_buttons.clear();
				}

				// Right-click popup: the top-most layer of all, drawn (and composited) on top of the
				// world and the ui overlay layers above.
				if let Some(state) = &v.context_menu {
					let menu_frame = context_menu::build_context_menu(state, vw, vh, self.mouse_pos);
					v.last_context_menu_buttons = menu_frame.buttons;
					context_menu_layer = pin_overlay_to_screen(menu_frame.geometry, &v.camera, vw, vh);
				} else {
					v.last_context_menu_buttons.clear();
				}
			}
		};

		let camera = match &self.screen {
			Screen::Menu => Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport: Vec2::new(vw, vh) },
			Screen::Viewer(v) => v.camera,
		};

		if let Some(state) = self.state.as_mut() {
			let layers = [world_layer, ui_overlay_layer, context_menu_layer];
			match state.renderer.render(&layers, &camera, theme::BACKGROUND_COL) {
				Ok(()) => {}
				Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
					let size = state.window.inner_size();
					state.renderer.resize(size.width, size.height);
				}
				Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
				Err(e) => eprintln!("render error: {e:?}"),
			}
			state.window.request_redraw();
		}
	}
}

fn main() {
	env_logger::init();

	let data_dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(SavePaths::unity_persistent_data_dir);
	eprintln!("using save data directory: {}", data_dir.display());
	SavePaths::ensure_directory_exists(&data_dir).ok();

	let mut app = App::new(SavePaths::new(data_dir));
	app.menu.refresh_projects();

	let event_loop = EventLoop::new().expect("failed to create event loop");
	event_loop.run_app(&mut app).expect("event loop error");
}
