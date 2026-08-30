//! Chip customization workspace UI (`ChipSaveMenu`'s CUSTOMIZE +
//! `ChipCustomizationMenu`/`CustomizationSceneDrawer`): an option column
//! (name position, body colour, embedded-display list) beside a live
//! preview of the chip being customized -- body, edge pins, name label
//! and any displays already placed on it, drawn at true relative scale so
//! resizing reads correctly.
//!
//! Interactions are click-driven, matching this port's canvas idioms:
//! press a corner bracket to resize (click again to finish), pick a
//! display up from the list or off the body (click to drop, Delete
//! removes, Escape reverts), wheel-scroll the list, wheel-zoom the
//! preview. Same philosophy as [`crate::render::editor_ui`]: plain data
//! in, frame + hit-boxes out, no GPU types -- fully unit-testable.

use crate::description::{ChipDescription, ChipType, DisplayDescription, NameLocation, PinBitCount};
use crate::render::editor_ui::{EditorAction, EditorButton, EditorFrame};
use crate::render::foundation::{apply_alpha, SceneGeometry, TextLabel};
use crate::render::layout;
use crate::render::scene::displays;
use crate::render::scene::lookup::PinStateLookup;
use crate::render::theme::{self, Rgba};
use crate::render::ui_kit::{self, UiCtx, UiRect};
use crate::structs::Vec2;
use crate::ChipLibrary;

/// What the player is currently grabbing in the customize preview.
/// Payloads carry whatever the interaction needs to apply/cancel itself
/// (see `viewer::customize`'s update/commit handlers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CustomizeInteraction {
	None,
	/// Body resize in progress from corner `usize`
	/// (0=top-left, 1=top-right, 2=bottom-left, 3=bottom-right).
	Resizing {
		corner: usize,
	},
	/// Display picked up off the body: `grab_offset` keeps the cursor's
	/// offset from the display centre so it doesn't jump; `original_pos`
	/// restores on Escape.
	MovingDisplay {
		index: usize,
		grab_offset: Vec2,
		original_pos: Vec2,
	},
	/// Display being scaled around its fixed `centre`; scale tracks the
	/// cursor's distance ratio against `start_dist`.
	ScalingDisplay {
		index: usize,
		centre: Vec2,
		start_dist: f32,
		start_scale: f32,
	},
	/// Fresh display picked from the list, following the cursor until a
	/// preview click drops it (Escape/Delete cancels).
	PlacingDisplay {
		sub_chip_id: i32,
	},
}

impl CustomizeInteraction {
	/// Whether some grab/placement is in flight (preview clicks then
	/// commit rather than starting something new).
	pub(crate) fn is_active(self) -> bool {
		self != Self::None
	}
	/// Whether a body resize drag is in progress.
	pub(crate) fn is_resizing(self) -> bool {
		matches!(self, Self::Resizing { .. })
	}
}

/// One row of the customize workspace's DISPLAYS list: a subchip of the
/// chip being customized that can be shown as an embedded display -- one
/// of the four builtin display types, or a custom chip carrying displays
/// of its own (whose whole cascade then merges into this chip; see
/// `scene::displays::can_be_embedded_display`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayListEntry {
	pub(crate) sub_chip_id: i32,
	/// The instance's label, falling back to its chip's name -- mirrors
	/// `ChipCustomizationMenu.DrawDisplayScroll`'s choice.
	pub(crate) label: String,
	pub(crate) chip_type: ChipType,
	/// Already placed on the body (each subchip can drive one display).
	pub(crate) placed: bool,
}

/// Enumerates the DISPLAYS-list rows for `draft`: every display-carrying
/// subchip, in reading order (x then y, mirroring the original's
/// `OrderBy(Position.x).ThenBy(Position.y)`).
pub(crate) fn display_entries(draft: &ChipDescription, library: &ChipLibrary) -> Vec<DisplayListEntry> {
	let mut subs: Vec<_> = draft
		.sub_chips
		.iter()
		.filter_map(|sub| {
			let desc = library.try_get(&sub.name)?;
			displays::can_be_embedded_display(desc).then_some((sub, desc.chip_type))
		})
		.collect();
	subs.sort_by(|(a, _), (b, _)| a.position.x.total_cmp(&b.position.x).then(a.position.y.total_cmp(&b.position.y)));

	subs.into_iter()
		.map(|(sub, chip_type)| DisplayListEntry {
			sub_chip_id: sub.id,
			label: sub
				.label
				.clone()
				.filter(|l| !l.trim().is_empty())
				.unwrap_or_else(|| library.try_get(&sub.name).map(|d| d.name.clone()).unwrap_or_default()),
			chip_type,
			placed: draft.displays.iter().any(|d| d.sub_chip_id == sub.id),
		})
		.collect()
}

/// Starting world-size multiplier for a freshly-placed display: always
/// 1, because [`displays::display_base_size`] already encodes each type's
/// placed-component content footprint -- dropping a display lands it at
/// exactly the size that chip renders at on the canvas, per the
/// scale-1-parity rule.
pub(crate) fn default_display_scale(_chip_type: ChipType) -> f32 {
	1.0
}

/// Screen-pixel facts about the last-built customize frame, cached on
/// [`crate::viewer::customize::CustomizeState`] so event handlers can map
/// the cursor into preview-world coordinates between frames.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct PreviewLayout {
	/// False until the first frame has been built (events arriving before
	/// that have nothing sane to map onto).
	pub valid: bool,
	/// The whole preview area (clicks landing here interact with the chip).
	pub preview: UiRect,
	/// The DISPLAYS list' scroll viewport (wheel routes here first).
	pub list: UiRect,
	/// Pixel position the chip body's centre was drawn at.
	pub chip_centre_px: Vec2,
	/// Pixels per world unit the preview was drawn at.
	pub px_per_unit: f32,
}

impl PreviewLayout {
	/// Cursor pixels -> preview-world units (screen y flips into world y).
	pub(crate) fn screen_to_world(&self, p: Vec2) -> Vec2 {
		Vec2::new((p.x - self.chip_centre_px.x) / self.px_per_unit, -(p.y - self.chip_centre_px.y) / self.px_per_unit)
	}
}

/// Everything the builder reads from live state, bundled so the function
/// signature stays one line. See each field's source in `viewer::customize`.
pub(crate) struct CustomizeCtx<'a> {
	pub draft: &'a ChipDescription,
	pub library: &'a ChipLibrary,
	pub entries: &'a [DisplayListEntry],
	pub interaction: CustomizeInteraction,
	/// Contents of the hex colour field (the shared overlay buffer).
	pub hex_text: &'a str,
	pub list_scroll: f32,
	/// Zoom multiplier around the auto-fit scale (1.0 = exactly fits).
	pub zoom_factor: f32,
	pub pin_state: &'a dyn PinStateLookup,
}

pub(crate) struct CustomizeFrameOut {
	pub frame: EditorFrame,
	pub layout: PreviewLayout,
	/// How far the DISPLAYS list may scroll (content beyond the viewport).
	pub list_scroll_max: f32,
}

const MENU_W: f32 = 320.0;
const PAD: f32 = 16.0;
const ROW_H: f32 = 30.0;
const BTN_H: f32 = 34.0;
const GAP: f32 = 8.0;
const LIST_ROW_GAP: f32 = 3.0;

const NAME_LOCATION_LABELS: [&str; 3] = ["Name: Middle", "Name: Top", "Name: Hidden"];

fn list_scroll_max(entry_count: usize, viewport_h: f32) -> f32 {
	let content = entry_count as f32 * (ROW_H + LIST_ROW_GAP);
	(content - viewport_h).max(0.0)
}

fn effective_body_colour(draft: &ChipDescription) -> Rgba {
	if draft.colour[3] > 0.0 {
		draft.colour
	} else {
		theme::CHIP_BODY_COL
	}
}

/// Builds the whole customize workspace for one frame: dark backdrop,
/// option column on the left, live chip preview filling the rest.
pub(crate) fn build_chip_customizer(ctx: &CustomizeCtx, vw: f32, vh: f32, mouse: Vec2) -> CustomizeFrameOut {
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	ui_kit::fill_rect(&mut frame, ui, UiRect::new(0.0, 0.0, vw, vh), [0.0, 0.0, 0.0, 0.55]);

	let menu_rect = UiRect::new(PAD, PAD, MENU_W, vh - PAD * 2.0);
	let preview_rect = UiRect::new(MENU_W + PAD * 2.0, PAD, (vw - MENU_W - PAD * 4.0).max(80.0), vh - PAD * 2.0);

	build_preview(ctx, &mut frame, ui, preview_rect, mouse);
	let list_rect = build_menu(ctx, &mut frame, ui, menu_rect);

	let layout = PreviewLayout {
		valid: true,
		preview: preview_rect,
		list: list_rect,
		chip_centre_px: preview_rect.centre(),
		px_per_unit: fit_ppu(ctx, preview_rect),
	};

	CustomizeFrameOut { frame: ui_kit::finish(frame, mouse), layout, list_scroll_max: list_scroll_max(ctx.entries.len(), list_rect.h) }
}

/// Pixels-per-world-unit the preview draws at for the current zoom.
fn fit_ppu(ctx: &CustomizeCtx, rect: UiRect) -> f32 {
	let size = ctx.draft.size;
	let margin = 2.0_f32.max(layout::GRID_SIZE * 10.0);
	let fit = (rect.w / (size.x + margin)).min(rect.h / (size.y + margin));
	(fit * ctx.zoom_factor).clamp(4.0, 600.0)
}

/// The option column: title, contextual hint, confirm row, name-position
/// wheel, colour swatches + hex field, and the DISPLAYS list. Returns the
/// list's scroll viewport rect (part of [`PreviewLayout`]).
fn build_menu(ctx: &CustomizeCtx, frame: &mut EditorFrame, ui: UiCtx, rect: UiRect) -> UiRect {
	let inner_x = rect.x + 12.0;
	let inner_w = rect.w - 24.0;
	ui_kit::fill_rect(frame, ui, rect, [0.16, 0.16, 0.18, 0.98]);

	let mut y = rect.y + 14.0;
	ui_kit::add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, y + 12.0), inner_w, "Customize chip", [1.0; 4], 22.0);
	y += 34.0;

	use CustomizeInteraction as Ci;
	let hint = match ctx.interaction {
		Ci::None => ["Drag a corner bracket to resize.", "Pick a display below -- click a placed one to remove it."],
		Ci::Resizing { .. } => ["Release to finish resizing", "(size snaps to the grid)."],
		Ci::MovingDisplay { .. } => ["Click to drop · Delete removes", "Escape puts it back"],
		Ci::ScalingDisplay { .. } => ["Move toward/away from the centre", "to scale · click confirms"],
		Ci::PlacingDisplay { .. } => ["Click inside the preview to place", "Delete/Escape cancels"],
	};
	for line in hint {
		ui_kit::add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, y + 8.0), inner_w, line, [0.85, 0.65, 0.4, 1.0], 13.5);
		y += 20.0;
	}
	y += 6.0;

	// Cancel / Confirm
	let half_w = (inner_w - GAP) / 2.0;
	ui_kit::add_button(
		frame,
		ui,
		UiRect::new(inner_x, y, half_w, BTN_H),
		"Cancel",
		EditorAction::CustomizeCancel,
		true,
		Some(crate::render::theme::DANGEROUS_ACTION_COL),
	);
	ui_kit::add_button(frame, ui, UiRect::new(inner_x + half_w + GAP, y, half_w, BTN_H), "Confirm", EditorAction::CustomizeConfirm, true, None);
	y += BTN_H + GAP;

	// Name position wheel
	let name_label = NAME_LOCATION_LABELS[ctx.draft.name_location.to_int() as usize % NAME_LOCATION_LABELS.len()];
	ui_kit::add_button(frame, ui, UiRect::new(inner_x, y, inner_w, BTN_H), name_label, EditorAction::CustomizeCycleNameLocation, true, None);
	y += BTN_H + GAP;

	y = build_force_cache_row(ctx, frame, ui, inner_x, inner_w, y);
	y += 4.0;

	// Colour swatches (two rows of four) + hex field
	let swatch_w = (inner_w - GAP * 3.0) / 4.0;
	for (i, colour) in theme::COLORS.iter().enumerate() {
		let srect = UiRect::new(inner_x + (i % 4) as f32 * (swatch_w + GAP), y + (i / 4) as f32 * (26.0 + GAP), swatch_w, 26.0);
		ui_kit::fill_rect(frame, ui, srect, *colour);
		if same_rgb(effective_body_colour(ctx.draft), *colour) {
			frame.geometry.add_rect(ui_kit::to_world(srect.centre(), ui.vw, ui.vh), Vec2::new(srect.w + 4.0, srect.h + 4.0), [1.0, 1.0, 1.0, 0.35]);
		}
		frame.buttons.push(EditorButton { rect: srect, action: EditorAction::CustomizePickColour(i), enabled: true });
	}
	y += 26.0 * 2.0 + GAP * 2.0;

	ui_kit::text_field_row(frame, ui, UiRect::new(inner_x, y, inner_w, BTN_H - 4.0), ctx.hex_text, "#RRGGBB", 15.0, 12.0);
	y += BTN_H + GAP;

	// DISPLAYS header
	let header_bg = UiRect::new(inner_x, y, inner_w, ROW_H);
	ui_kit::fill_rect(frame, ui, header_bg, [0.11, 0.11, 0.12, 1.0]);
	ui_kit::add_label(frame, ui, header_bg.centre(), header_bg.w - 12.0, &format!("DISPLAYS ({})", ctx.entries.len()), [0.24, 0.82, 0.41, 1.0], 15.0);
	y += ROW_H + LIST_ROW_GAP;

	// Scrollable list viewport.
	let list_bottom = rect.y + rect.h - 46.0;
	let list_rect = UiRect::new(inner_x, y, inner_w - 6.0, (list_bottom - y).max(ROW_H));

	let max = list_scroll_max(ctx.entries.len(), list_rect.h);
	let scroll = ctx.list_scroll.clamp(0.0, max);
	if ctx.entries.is_empty() {
		ui_kit::add_label(
			frame,
			ui,
			Vec2::new(list_rect.centre().x, list_rect.y + 16.0),
			list_rect.w,
			"Place a display component first",
			[0.6, 0.6, 0.65, 1.0],
			13.5,
		);
	}
	for (index, entry) in ctx.entries.iter().enumerate() {
		let row_y = list_rect.y + index as f32 * (ROW_H + LIST_ROW_GAP) - scroll;
		if row_y + ROW_H <= list_rect.y || row_y >= list_rect.y + list_rect.h {
			continue;
		}
		let r = UiRect::new(list_rect.x, row_y, list_rect.w, ROW_H);
		// Rows toggle: an unplaced entry picks its display up for
		// placement; an already-placed one removes it again (the original
		// greyed duplicates out instead -- here the click gives the rows
		// a second job, so they stay live).
		let enabled = !ctx.interaction.is_active();
		let bg = if enabled && r.contains(ui.mouse) { [0.32, 0.32, 0.36, 1.0] } else { [0.22, 0.22, 0.25, 1.0] };
		ui_kit::fill_rect(frame, ui, r, bg);
		let label = if entry.placed { format!("{}  (placed)", entry.label) } else { entry.label.clone() };
		let label_colour = if entry.placed { [0.55, 0.85, 0.6, 1.0] } else { theme::text_colour_for_background(bg) };
		ui_kit::add_label(frame, ui, r.centre(), r.w - 12.0, &label, label_colour, 14.0);
		frame.buttons.push(EditorButton { rect: r, action: EditorAction::CustomizePlaceEntry(index), enabled });
	}

	if max > 0.0 {
		// Proportional scrollbar thumb on the viewport's right edge.
		let track = list_rect.h - 4.0;
		let thumb_h = (track * (list_rect.h / (list_rect.h + max))).clamp(18.0, track);
		let thumb_y = list_rect.y + 2.0 + (scroll / max) * (track - thumb_h);
		ui_kit::fill_rect(frame, ui, UiRect::new(list_rect.x + list_rect.w - 4.0, thumb_y, 3.0, thumb_h), [0.45, 0.45, 0.5, 1.0]);
	}

	list_rect
}

/// "Force chip caching" checkbox row -- a small tick box plus label,
/// mirroring `ChipCustomizationMenu`'s caching checkbox. Bound to
/// `ChipDescription::should_be_cached` (only meaningful once this chip's
/// combined input width climbs past the always-cached auto budget); a
/// hint line underneath spells out roughly what ticking it will cost, so
/// the choice isn't blind. Always shown, even under the auto-cache
/// budget, so a chip that later grows a wider input keeps its opt-in
/// intact from the start.
fn build_force_cache_row(ctx: &CustomizeCtx, frame: &mut EditorFrame, ui: UiCtx, inner_x: f32, inner_w: f32, y: f32) -> f32 {
	use crate::gate_op::{MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING};

	let box_size: f32 = 18.0;
	let row_h: f32 = box_size.max(18.0);
	let box_rect = UiRect::new(inner_x, y, box_size, box_size);
	let checked = ctx.draft.should_be_cached;

	ui_kit::fill_rect(frame, ui, box_rect, [0.09, 0.09, 0.1, 1.0]);
	frame.geometry.add_rect(ui_kit::to_world(box_rect.centre(), ui.vw, ui.vh), Vec2::new(box_size - 3.0, box_size - 3.0), [0.4, 0.4, 0.45, 1.0]);
	if checked {
		frame.geometry.add_rect(
			ui_kit::to_world(box_rect.centre(), ui.vw, ui.vh),
			Vec2::new(box_size - 8.0, box_size - 8.0),
			[0.24, 0.82, 0.41, 1.0],
		);
	}

	let label_x = inner_x + box_size + 8.0;
	ui_kit::add_label(
		frame,
		ui,
		Vec2::new(label_x + (inner_w - box_size - 8.0) / 2.0, box_rect.centre().y),
		inner_w - box_size - 8.0,
		"Force chip caching",
		[0.9, 0.9, 0.92, 1.0],
		14.0,
	);

	// The whole row toggles, not just the tick box itself.
	let hit_rect = UiRect::new(inner_x, y, inner_w, row_h);
	frame.buttons.push(EditorButton { rect: hit_rect, action: EditorAction::CustomizeToggleForceCache, enabled: true });

	let mut next_y = y + row_h + 4.0;

	let input_bits = total_input_bits(ctx.draft);
	if input_bits > MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING {
		let hint = if input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING {
			format!("{input_bits} input bits -- above the auto-cache limit ({MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING}); tick to cache anyway.")
		} else {
			format!("{input_bits} input bits -- too wide to cache even with this on ({MAX_NUM_INPUT_BITS_WHEN_USER_CACHING} max).")
		};
		ui_kit::add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, next_y + 8.0), inner_w, &hint, [0.6, 0.6, 0.65, 1.0], 12.0);
		next_y += 20.0;
	}

	next_y
}

/// Total input-pin width in bits for a not-yet-simulated draft --
/// mirrors `gate_op::caching::calculate_num_input_bits`, which needs a
/// live `Simulator`/`ChipIdx` this UI-only draft doesn't have.
fn total_input_bits(draft: &ChipDescription) -> u32 {
	draft.input_pins.iter().map(|p| p.bit_count.to_int() as u32).sum()
}

fn same_rgb(a: Rgba, b: Rgba) -> bool {
	(a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4
}

/// The live preview: dark surface, chip body + edge pins + name label +
/// embedded displays drawn in world units then transformed to screen
/// pixels, corner resize brackets, display grab/scale hotspots and the
/// placement ghost.
fn build_preview(ctx: &CustomizeCtx, frame: &mut EditorFrame, ui: UiCtx, rect: UiRect, mouse: Vec2) {
	ui_kit::fill_rect(frame, ui, rect, [0.09, 0.09, 0.105, 1.0]);

	let size = ctx.draft.size;
	let px_per_unit = fit_ppu(ctx, rect);
	let centre_px = rect.centre();
	let map = |p: Vec2| Vec2::new(centre_px.x + p.x * px_per_unit, centre_px.y - p.y * px_per_unit);

	let body_half_screen = Vec2::new(size.x * px_per_unit / 2.0, size.y * px_per_unit / 2.0);

	// World-space scene for the chip itself, transformed afterwards.
	let mut world = SceneGeometry::default();
	let body_colour = effective_body_colour(ctx.draft);
	world.add_rect(Vec2::ZERO, size, body_colour);

	// Outline (`DrawSubChip`'s border pass).
	let w = layout::CHIP_OUTLINE_WIDTH * 0.5;
	let (hw, hh) = (size.x / 2.0 + w, size.y / 2.0 + w);
	world.add_line(Vec2::new(-hw, hh), Vec2::new(hw, hh), w, theme::CHIP_OUTLINE_COL);
	world.add_line(Vec2::new(hw, hh), Vec2::new(hw, -hh), w, theme::CHIP_OUTLINE_COL);
	world.add_line(Vec2::new(hw, -hh), Vec2::new(-hw, -hh), w, theme::CHIP_OUTLINE_COL);
	world.add_line(Vec2::new(-hw, hh), Vec2::new(-hw, -hh), w, theme::CHIP_OUTLINE_COL);

	draw_edge_pins(&mut world, ctx.draft, size);

	// Name label + Top-mode background band.
	if ctx.draft.name_location != NameLocation::Hidden {
		if ctx.draft.name_location == NameLocation::Top {
			let band_h = theme::FONT_SIZE_CHIP_NAME * 1.8;
			world.add_rect(Vec2::new(0.0, size.y / 2.0 - band_h / 2.0), Vec2::new(size.x, band_h), darken(body_colour, 0.13));
		}
		let name_pos = match ctx.draft.name_location {
			NameLocation::Top => Vec2::new(0.0, size.y / 2.0 - theme::FONT_SIZE_CHIP_NAME / 2.0 - layout::GRID_SIZE / 2.0),
			_ => Vec2::ZERO,
		};
		world.labels.push(TextLabel {
			pos: name_pos,
			text: ctx.draft.name.clone(),
			colour: theme::text_colour_for_background(body_colour),
			font_size: theme::FONT_SIZE_CHIP_NAME,
			width: size.x,
		});
	}

	// Embedded displays (clipped to the body; sticking-out ones get the
	// red overlay flag from the shared painter).
	displays::draw_subchip_displays(
		&mut world,
		Vec2::ZERO,
		size,
		&ctx.draft.sub_chips,
		&ctx.draft.displays,
		ctx.library,
		ctx.pin_state,
		body_colour,
		true,
	);

	append_world_to_frame(&mut frame.geometry, &world, centre_px, px_per_unit, ui.vh);

	// Placement ghost, following the cursor at its would-be scale.
	if let CustomizeInteraction::PlacingDisplay { sub_chip_id } = ctx.interaction {
		if let Some(desc) = ctx.draft.sub_chips.iter().find(|s| s.id == sub_chip_id).and_then(|s| ctx.library.try_get(&s.name)) {
			let mut ghost = SceneGeometry::default();
			let cursor_world = Vec2::new((mouse.x - centre_px.x) / px_per_unit, -(mouse.y - centre_px.y) / px_per_unit);
			displays::draw_subchip_displays(
				&mut ghost,
				cursor_world,
				Vec2::splat(f32::MAX),
				&ctx.draft.sub_chips,
				&[DisplayDescription::new(sub_chip_id, Vec2::ZERO, default_display_scale(desc.chip_type))],
				ctx.library,
				ctx.pin_state,
				body_colour,
				false,
			);
			apply_alpha(&mut ghost, 0.65);
			append_world_to_frame(&mut frame.geometry, &ghost, centre_px, px_per_unit, ui.vh);
		}
	}

	// ---- Interaction affordances (screen-space) ----

	// Corner resize brackets: an L along each body corner extending past
	// the chip -- a partial border, deliberately not a filled square.
	let bracket_len = 18.0_f32.min(body_half_screen.x).min(body_half_screen.y);
	for (i, (sx, sy)) in CORNER_SIGNS.iter().enumerate() {
		// World-sign -> screen: +y is up in world space but down on screen.
		let corner_px = Vec2::new(centre_px.x + sx * body_half_screen.x, centre_px.y - sy * body_half_screen.y);
		let active = matches!(ctx.interaction, CustomizeInteraction::Resizing { corner } if corner == i);
		let hovered = !ctx.interaction.is_active()
			&& (ui.mouse.x - corner_px.x).abs() <= bracket_len * 1.25
			&& (ui.mouse.y - corner_px.y).abs() <= bracket_len * 1.25;
		let colour = if active || hovered { [1.0, 1.0, 1.0, 1.0] } else { [0.72, 0.72, 0.76, 1.0] };
		let thick = if active { 3.0 } else { 2.0 };

		// Legs run inward along the body edges (never diagonally), so the
		// pair reads as a partial border extending past the chip.
		frame.geometry.add_line(map(corner_px), map(corner_px + Vec2::new(-sx * bracket_len, 0.0)), thick, colour);
		frame.geometry.add_line(map(corner_px), map(corner_px + Vec2::new(0.0, sy * bracket_len)), thick, colour);

		if !ctx.interaction.is_active() {
			const HOT: f32 = 22.0;
			frame.buttons.push(EditorButton {
				rect: UiRect::new(corner_px.x - HOT / 2.0, corner_px.y - HOT / 2.0, HOT, HOT),
				action: EditorAction::CustomizeResizeStart(i),
				enabled: true,
			});
		}
	}

	// Live size read-out while resizing.
	if let CustomizeInteraction::Resizing { .. } = ctx.interaction {
		frame.geometry.labels.push(TextLabel {
			pos: ui_kit::to_world(Vec2::new(ui.mouse.x + 14.0, ui.mouse.y - 16.0), ui.vw, ui.vh),
			text: format!("{:.2} x {:.2}", size.x, size.y),
			colour: [1.0, 1.0, 1.0, 1.0],
			font_size: 14.0,
			width: 140.0,
		});
	}

	// Placed-display affordances: amber brackets when hovered, a small
	// scale-grab square at the bottom-right corner plus the body rect for
	// moving. Pushed back-to-front so the front-most display wins hits;
	// suppressed mid-interaction (the click then commits instead).
	let placed_rect = |display: &DisplayDescription, resolved: &ChipDescription| -> Option<(UiRect, UiRect)> {
		// Follows the painted extent (custom cascades included), so grab
		// and scale hotspots always sit exactly on what's drawn:
		// `display_entry_bounds` is relative to the entry's own anchor,
		// which for a body-placed display is its `position`.
		let (offset, dsize) = displays::display_entry_bounds(display, resolved, ctx.library)?;
		let centre = display.position + offset;
		let tl = map(centre + Vec2::new(-dsize.x / 2.0, dsize.y / 2.0));
		let br = map(centre + Vec2::new(dsize.x / 2.0, -dsize.y / 2.0));
		Some((rect_from_corners(tl, br), UiRect::new(br.x - 11.0, br.y - 11.0, 11.0, 11.0)))
	};

	if !ctx.interaction.is_active() {
		for (i, display) in ctx.draft.displays.iter().enumerate().rev() {
			let Some(resolved) = ctx.draft.sub_chips.iter().find(|s| s.id == display.sub_chip_id).and_then(|s| ctx.library.try_get(&s.name)) else {
				continue;
			};
			let Some((drect, scale_corner)) = placed_rect(display, resolved) else { continue };
			if drect.contains(mouse) {
				draw_display_brackets(&mut frame.geometry, ui, drect, [1.0, 0.8, 0.25, 1.0]);
			}
			frame.buttons.push(EditorButton { rect: scale_corner, action: EditorAction::CustomizeGrabDisplayScale(i), enabled: true });
			frame.buttons.push(EditorButton { rect: drect, action: EditorAction::CustomizeGrabDisplayMove(i), enabled: true });
		}
	} else {
		// Highlight whichever display is currently carried.
		let carried = match ctx.interaction {
			CustomizeInteraction::MovingDisplay { index, .. } | CustomizeInteraction::ScalingDisplay { index, .. } => Some(index),
			_ => None,
		};
		if let Some(i) = carried.and_then(|i| ctx.draft.displays.get(i)) {
			let resolved = ctx.draft.sub_chips.iter().find(|s| s.id == i.sub_chip_id).and_then(|s| ctx.library.try_get(&s.name));
			if let Some(resolved) = resolved {
				if let Some((drect, _)) = placed_rect(i, resolved) {
					draw_display_brackets(&mut frame.geometry, ui, drect, [1.0, 1.0, 1.0, 1.0]);
				}
			}
		}
	}
}

const CORNER_SIGNS: [(f32, f32); 4] = [(-1.0, 1.0), (1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];

fn darken(colour: Rgba, amount: f32) -> Rgba {
	[(colour[0] - amount).max(0.0), (colour[1] - amount).max(0.0), (colour[2] - amount).max(0.0), colour[3]]
}

fn rect_from_corners(a: Vec2, b: Vec2) -> UiRect {
	UiRect::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
}

/// Transforms world-unit scene geometry into the frame's pseudo-screen
/// space. The preview maps world y-up onto screen y-down (`centre_px` is
/// the on-screen chip centre, so a world point lands at pixel
/// `centre + (x, -y) * ppu`) -- but overlay layers get one y-compensation
/// applied later by the render pipeline (`ui_kit::pin_geometry_to_screen`
/// treats frame coordinates as screen pixels), so the geometry stored
/// here must be flipped *again* to survive it: net effect, world +y ends
/// up toward the top of the preview like everywhere else in the editor.
/// Button rects and cursor maths stay in plain screen pixels and don't
/// go through this.
fn append_world_to_frame(target: &mut SceneGeometry, world: &SceneGeometry, centre_px: Vec2, px_per_unit: f32, vh: f32) {
	let map = |p: Vec2| Vec2::new(centre_px.x + p.x * px_per_unit, vh - centre_px.y + p.y * px_per_unit);
	for v in &world.triangles {
		target.triangles.push(crate::render::foundation::SceneVertex { pos: map(v.pos), colour: v.colour });
	}
	for l in &world.labels {
		target.labels.push(TextLabel {
			pos: map(l.pos),
			text: l.text.clone(),
			colour: l.colour,
			font_size: l.font_size * px_per_unit,
			width: l.width * px_per_unit,
		});
	}
}

/// Input pins down the left edge, output pins down the right -- the
/// relative sizing anchor the customization preview exists to show.
fn draw_edge_pins(world: &mut SceneGeometry, draft: &ChipDescription, size: Vec2) {
	let input_bits: Vec<PinBitCount> = draft.input_pins.iter().map(|p| p.bit_count).collect();
	let output_bits: Vec<PinBitCount> = draft.output_pins.iter().map(|p| p.bit_count).collect();
	let (_, input_y) = layout::calculate_default_pin_layout(&input_bits);
	let (_, output_y) = layout::calculate_default_pin_layout(&output_bits);

	for (bits, ys, is_left) in [(&input_bits, &input_y, true), (&output_bits, &output_y, false)] {
		for (i, y) in ys.iter().enumerate() {
			let pos = layout::pin_world_position(Vec2::ZERO, size, *y, is_left);
			match bits[i] {
				PinBitCount::Bit1 => world.add_circle(pos, bits[i].pin_radius(), theme::PIN_COL, layout::PIN_SEGMENTS),
				wide => {
					let shape = wide.pin_visual_shape_size();
					world.add_rounded_rect(
						pos,
						shape,
						theme::PIN_COL,
						shape.y / 2.0,
						crate::render::foundation::RoundCorners::BOTH,
						layout::PIN_SEGMENTS / 4,
					);
				}
			}
		}
	}
}

/// Amber/white L-brackets around a placed display's screen rect.
fn draw_display_brackets(target: &mut SceneGeometry, ui: UiCtx, r: UiRect, colour: Rgba) {
	let len = 10.0_f32.min(r.w / 2.0).min(r.h / 2.0);
	for (cx, cy, dx, dy) in [(r.x, r.y, 1.0, 1.0), (r.x + r.w, r.y, -1.0, 1.0), (r.x, r.y + r.h, 1.0, -1.0), (r.x + r.w, r.y + r.h, -1.0, -1.0)] {
		target.add_line(ui_kit::to_world(Vec2::new(cx, cy), ui.vw, ui.vh), ui_kit::to_world(Vec2::new(cx + dx * len, cy), ui.vw, ui.vh), 2.0, colour);
		target.add_line(ui_kit::to_world(Vec2::new(cx, cy), ui.vw, ui.vh), ui_kit::to_world(Vec2::new(cx, cy + dy * len), ui.vw, ui.vh), 2.0, colour);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{PinBitCount, PinDescription};
	use crate::render::scene::lookup::AllLow;

	fn seg_desc() -> ChipDescription {
		let mut d = ChipDescription::new("7Seg", ChipType::SevenSegmentDisplay);
		for (i, name) in ["A", "B", "C", "D", "E", "F", "G", "COL"].iter().enumerate() {
			d.input_pins.push(PinDescription::new(*name, i as i32, PinBitCount::Bit1));
		}
		d
	}

	fn sample_draft() -> ChipDescription {
		let mut d = ChipDescription::new("Panel", ChipType::Custom);
		d.size = Vec2::new(4.0, 2.0);
		d.sub_chips.push(crate::SubChipDescription {
			name: "7Seg".into(),
			id: 4,
			internal_data: None,
			position: Vec2::new(0.0, 1.0),
			label: Some("out".into()),
			pin_colour_info: vec![],
		});
		d.sub_chips.push(crate::SubChipDescription {
			name: "NAND".into(),
			id: 5,
			internal_data: None,
			position: Vec2::new(1.0, 0.0),
			label: None,
			pin_colour_info: vec![],
		});
		d.displays.push(DisplayDescription::new(4, Vec2::ZERO, 0.75));
		d
	}

	fn lib_with_seg() -> ChipLibrary {
		let mut lib = ChipLibrary::new();
		lib.add(seg_desc());
		lib
	}

	#[test]
	fn display_entries_lists_only_display_type_subchips_with_labels_and_placed_flags() {
		let mut draft = sample_draft();
		draft.displays.clear(); // start unplaced (sample_draft ships one)
		let library = lib_with_seg();

		let entries = display_entries(&draft, &library);
		assert_eq!(entries.len(), 1, "the NAND must not appear");
		assert_eq!(entries[0].sub_chip_id, 4);
		assert_eq!(entries[0].label, "out", "instance label wins over chip name");
		assert!(!entries[0].placed);

		let mut placed = draft.clone();
		placed.displays.push(DisplayDescription::new(4, Vec2::ZERO, 1.0));
		assert!(display_entries(&placed, &library)[0].placed);
	}

	/// A placed *custom* chip that carries displays of its own joins the
	/// DISPLAYS list too (the cascade source); a custom chip without any
	/// doesn't.
	#[test]
	fn display_entries_lists_custom_chips_that_carry_displays() {
		let mut draft = sample_draft(); // subchips: NAND (id 1), 7Seg (id 4)
		draft.displays.clear();

		let mut carrying = ChipDescription::new("CARRYING", ChipType::Custom);
		carrying.displays.push(DisplayDescription::new(99, Vec2::ZERO, 1.0));
		let plain = ChipDescription::new("PLAIN", ChipType::Custom);

		let mut library = lib_with_seg();
		library.add(carrying);
		library.add(plain);
		draft.sub_chips.push(crate::SubChipDescription {
			name: "CARRYING".into(),
			id: 6,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		draft.sub_chips.push(crate::SubChipDescription {
			name: "PLAIN".into(),
			id: 7,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});

		let entries = display_entries(&draft, &library);
		let listed: Vec<_> = entries.iter().map(|e| e.sub_chip_id).collect();
		assert!(listed.contains(&4), "builtin display still listed");
		assert!(listed.contains(&6), "custom-with-displays listed as a cascade source");
		assert!(!listed.contains(&7), "display-less custom not listed");
		assert_eq!(entries.iter().find(|e| e.sub_chip_id == 6).unwrap().chip_type, ChipType::Custom);
	}

	#[test]
	fn preview_layout_maps_screen_and_world_coordinates_both_ways() {
		let lay = PreviewLayout {
			valid: true,
			preview: UiRect::default(),
			list: UiRect::default(),
			chip_centre_px: Vec2::new(400.0, 300.0),
			px_per_unit: 50.0,
		};

		let w = lay.screen_to_world(Vec2::new(450.0, 200.0));
		assert_eq!(w, Vec2::new(1.0, 2.0));
		// The inverse mapping (used by the corner-bracket layout) lands back on screen.
		assert_eq!(Vec2::new(400.0 + w.x * 50.0, 300.0 - w.y * 50.0), Vec2::new(450.0, 200.0));
	}

	#[test]
	fn default_display_scale_is_one_so_scale_matches_component_size() {
		for t in [ChipType::SevenSegmentDisplay, ChipType::DisplayRgb, ChipType::DisplayDot, ChipType::DisplayLed] {
			assert_eq!(default_display_scale(t), 1.0, "scale 1 must equal the placed component's content size");
		}
	}

	#[test]
	fn list_scroll_max_never_negative_and_zero_when_everything_fits() {
		assert_eq!(list_scroll_max(0, 100.0), 0.0);
		assert_eq!(list_scroll_max(3, 1000.0), 0.0);
		assert!(list_scroll_max(10, 50.0) > 0.0);
	}

	#[test]
	fn built_frame_reports_layout_and_hitboxes_for_the_current_state() {
		let draft = sample_draft();
		let library = lib_with_seg();
		let entries = display_entries(&draft, &library);
		let ctx = CustomizeCtx {
			draft: &draft,
			library: &library,
			entries: &entries,
			interaction: CustomizeInteraction::None,
			hex_text: "#FFFFFF",
			list_scroll: 0.0,
			zoom_factor: 1.0,
			pin_state: &AllLow,
		};

		let out = build_chip_customizer(&ctx, 1280.0, 800.0, Vec2::new(640.0, 400.0));

		assert!(out.layout.valid);
		assert!(out.layout.px_per_unit > 0.0);
		assert!(out.frame.text_field.is_some(), "hex field owns typing");
		assert!(out.frame.buttons.iter().any(|b| b.action == EditorAction::CustomizeConfirm));
		assert_eq!(
			out.frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::CustomizeResizeStart(_))).count(),
			4,
			"all four corners offer resizing"
		);

		// Every corner hotspot sits on its own body corner (world -> screen).
		let (cx, cy) = (out.layout.chip_centre_px.x, out.layout.chip_centre_px.y);
		for (i, (sx, sy)) in CORNER_SIGNS.iter().enumerate() {
			let expected = Vec2::new(cx + sx * draft.size.x * out.layout.px_per_unit / 2.0, cy - sy * draft.size.y * out.layout.px_per_unit / 2.0);
			let button = out
				.frame
				.buttons
				.iter()
				.find(|b| matches!(b.action, EditorAction::CustomizeResizeStart(c) if c == i))
				.expect("corner hotspot present");
			let c = button.rect.centre();
			assert!((c.x - expected.x).abs() <= 11.5 && (c.y - expected.y).abs() <= 11.5, "corner {i} at {c:?}, expected near {expected:?}");
		}

		assert!(out.frame.buttons.iter().any(|b| matches!(b.action, EditorAction::CustomizeGrabDisplayMove(_))), "placed display offers move");

		// Placed rows toggle: the row for the already-placed subchip stays
		// clickable (its click removes the display) and says so.
		let placed_row =
			out.frame.buttons.iter().find(|b| matches!(b.action, EditorAction::CustomizePlaceEntry(0))).expect("placed entry still listed");
		assert!(placed_row.enabled, "clicking a placed display's name must stay live so it can remove it");
	}

	/// Regression: every placed display's move/scale hotspots used to be
	/// anchored at the preview origin, so all interactions landed on the
	/// body centre no matter where the displays actually were. Each
	/// display's hitboxes must sit on *its own* painted rect -- builtin
	/// entries at `position`, cascade entries at `position + union
	/// offset` -- with the scale corner at that rect's bottom-right.
	#[test]
	fn placed_display_hotspots_sit_on_their_own_displays_not_the_body_centre() {
		let mut led = ChipDescription::new("LED", ChipType::DisplayLed);
		led.input_pins.push(crate::PinDescription::new("IN", 0, PinBitCount::Bit1));
		let mut panel = ChipDescription::new("PANEL", ChipType::Custom);
		panel.sub_chips.push(crate::SubChipDescription {
			name: "LED".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		panel.displays.push(DisplayDescription::new(1, Vec2::new(0.25, -0.25), 2.0)); // 0.375-unit tile

		let mut draft = ChipDescription::new("D", ChipType::Custom);
		draft.size = Vec2::new(6.0, 5.0);
		for (name, id) in [("7Seg", 4), ("LED", 5), ("PANEL", 6)] {
			draft.sub_chips.push(crate::SubChipDescription {
				name: name.into(),
				id,
				internal_data: None,
				position: Vec2::ZERO,
				label: None,
				pin_colour_info: vec![],
			});
		}
		// Leaf entries away from the centre; the cascade entry composes
		// its union offset on top of its own position.
		let placed = vec![
			DisplayDescription::new(4, Vec2::new(-2.0, 1.5), 1.0),   // seg: 1 x 1.75
			DisplayDescription::new(5, Vec2::new(2.25, -1.75), 1.0), // led: 0.1875 square
			DisplayDescription::new(6, Vec2::new(0.5, 0.25), 1.0),   // panel cascade -> tile at (0.75, 0.0)
		];
		draft.displays = placed.clone();

		let mut library = ChipLibrary::new();
		library.add(seg_desc());
		library.add(led);
		library.add(panel);

		let entries = display_entries(&draft, &library);
		let ctx = CustomizeCtx {
			draft: &draft,
			library: &library,
			entries: &entries,
			interaction: CustomizeInteraction::None,
			hex_text: "#FFFFFF",
			list_scroll: 0.0,
			zoom_factor: 1.0,
			pin_state: &AllLow,
		};
		let out = build_chip_customizer(&ctx, 1280.0, 800.0, Vec2::new(400.0, 400.0));
		let lay = out.layout;
		assert!(lay.valid);
		let map = |world: Vec2| Vec2::new(lay.chip_centre_px.x + world.x * lay.px_per_unit, lay.chip_centre_px.y - world.y * lay.px_per_unit);

		// (painted-content centre, painted size) per entry, hand-derived:
		// leaves anchor at their position with their base*scale extent;
		// the panel's tile sits at position + its inner child offset.
		let expected =
			[(placed[0].position, Vec2::new(1.0, 1.75)), (placed[1].position, Vec2::splat(0.1875)), (Vec2::new(0.75, 0.0), Vec2::splat(0.375))];
		for (i, (world_centre, dsize)) in expected.iter().enumerate() {
			let expected_move_centre = map(*world_centre);
			let move_btn = out.frame.buttons.iter().find(|b| b.action == EditorAction::CustomizeGrabDisplayMove(i)).expect("move hotspot");
			assert!(
				(move_btn.rect.centre() - expected_move_centre).magnitude() < 1e-3,
				"display {i} move hotspot at {:?}, expected {:?}",
				move_btn.rect.centre(),
				expected_move_centre
			);

			// The scale corner rides the rect's SCREEN bottom-right, which
			// (screen y grows downward) is world +x / -y.
			let br = map(*world_centre + Vec2::new(dsize.x * 0.5, -dsize.y * 0.5));
			let scale_btn = out.frame.buttons.iter().find(|b| b.action == EditorAction::CustomizeGrabDisplayScale(i)).expect("scale hotspot");
			let scale_centre = scale_btn.rect.centre();
			assert!(
				(scale_centre.x - (br.x - 5.5)).abs() < 1e-3 && (scale_centre.y - (br.y - 5.5)).abs() < 1e-3,
				"scale corner rides the rect's bottom-right: got {scale_centre:?}, expected ({:.3},{:.3})",
				br.x - 5.5,
				br.y - 5.5
			);
		}
	}

	#[test]
	fn mid_interaction_suppresses_grab_hotspots_but_keeps_confirm() {
		let draft = sample_draft();
		let library = lib_with_seg();
		let entries = display_entries(&draft, &library);
		let ctx = CustomizeCtx {
			draft: &draft,
			library: &library,
			entries: &entries,
			interaction: CustomizeInteraction::MovingDisplay { index: 0, grab_offset: Vec2::ZERO, original_pos: Vec2::ZERO },
			hex_text: "",
			list_scroll: 0.0,
			zoom_factor: 1.0,
			pin_state: &AllLow,
		};

		let out = build_chip_customizer(&ctx, 1280.0, 800.0, Vec2::new(900.0, 400.0));

		assert!(
			!out.frame.buttons.iter().any(|b| b.enabled
				&& matches!(
					b.action,
					EditorAction::CustomizeGrabDisplayMove(_) | EditorAction::CustomizeGrabDisplayScale(_) | EditorAction::CustomizeResizeStart(_)
				)),
			"no grab/resize hotspots while carrying"
		);
		assert!(
			out.frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::CustomizePlaceEntry(_))).all(|b| !b.enabled),
			"list rows disabled while carrying"
		);
		assert!(out.frame.buttons.iter().any(|b| b.enabled && b.action == EditorAction::CustomizeConfirm));
	}
}
