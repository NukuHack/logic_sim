//! Embedded chip-display rendering ("customize" feature): the live
//! display surfaces a chip can carry inside its own body. Ported from
//! `DevSceneDrawer.DrawSubchipDisplays`/`DrawDisplayWithBackground`/
//! `DrawDisplay` -- each of a chip's `displays` entries shows one of its
//! own display-type subchips (7-segment / RGB / dot / LED) at an offset
//! and scale inside the chip body.
//!
//! Content is clipped to the owning chip's body rect (the original masks
//! via a shader scope; here every primitive is intersected with the clip
//! rect instead), and -- when asked -- any display whose content doesn't
//! fit entirely inside the body gets a translucent red quad drawn over
//! its full extent, mirroring the original's out-of-bounds indicator.
//! A custom chip's own embedded displays recurse into their children,
//! descending one simulation scope per level so live state keeps
//! resolving (`PinStateLookup::enter_scope`); scopes that can't be
//! descended draw blank, matching the original's `sim == null` branches.

use crate::description::{ChipDescription, ChipLibrary, ChipType, Color, DisplayDescription, SubChipDescription};
use crate::pin_state::LogicState;
use crate::render::foundation::SceneGeometry;
use crate::render::scene::lookup::{AllLow, PinStateLookup};
use crate::render::theme::{self, Rgba};
use crate::structs::Vec2;

/// Component-wise minimum/maximum -- deliberately local helpers rather
/// than `Vec2::max`, whose second component reads `self.x` (kept as-is
/// elsewhere for compatibility; new code shouldn't inherit that).
fn vec_min(a: Vec2, b: Vec2) -> Vec2 {
	Vec2::new(a.x.min(b.x), a.y.min(b.y))
}

fn vec_max(a: Vec2, b: Vec2) -> Vec2 {
	Vec2::new(a.x.max(b.x), a.y.max(b.y))
}

/// Translucent red of the original's out-of-bounds overlay (`new(1, 0, 0, 0.24)`).
const OUT_OF_BOUNDS_COL: Rgba = [1.0, 0.0, 0.0, 0.24];

/// Axis-aligned region display content is clipped to while drawing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipRect {
	min: Vec2,
	max: Vec2,
}

impl ClipRect {
	fn from_centre_size(centre: Vec2, size: Vec2) -> Self {
		let half = size * 0.5;
		Self { min: centre - half, max: centre + half }
	}

	/// An unclipped region covering everything (for callers reusing this
	/// module's content painters without a chip body to clip against).
	pub(crate) const OPEN: Self = Self { min: Vec2 { x: f32::MIN, y: f32::MIN }, max: Vec2 { x: f32::MAX, y: f32::MAX } };

	fn add_rect(&self, geo: &mut SceneGeometry, centre: Vec2, size: Vec2, colour: Rgba) {
		let half = size * 0.5;
		let (lo, hi) = (centre - half, centre + half);
		let min = vec_max(lo, self.min);
		let max = vec_min(hi, self.max);
		if max.x > min.x && max.y > min.y {
			geo.add_rect((min + max) * 0.5, max - min, colour);
		}
	}

	fn contains_rect(&self, centre: Vec2, size: Vec2) -> bool {
		let half = size * 0.5;
		centre.x - half.x >= self.min.x && centre.x + half.x <= self.max.x && centre.y - half.y >= self.min.y && centre.y + half.y <= self.max.y
	}
}

/// Natural world-space footprint of one display at scale 1 -- chosen so
/// **scale 1 reproduces the placed component's visible content exactly**
/// (the brief for embedded displays): the originals' builtin display
/// chips carry a self-display whose `Scale` equals their content width --
/// 7-segment `1.0` (body `GridSize*10` minus insets), dot `1.5`
/// (pin-stack height `1.75` minus `GridSize*2`), RGB `2.375`
/// (`GridSize*21` body), LED `0.1875` (`0.25` body minus
/// `GridSize*0.5`). Heights follow each painter's aspect (7-segment is
/// 1 x 1.75; the rest square). `None` for chip types that can't be shown
/// as an embedded display.
pub fn display_base_size(chip_type: ChipType) -> Option<Vec2> {
	match chip_type {
		ChipType::SevenSegmentDisplay => Some(Vec2::new(1.0, 1.75)),
		ChipType::DisplayRgb => Some(Vec2::splat(2.375)),
		ChipType::DisplayDot => Some(Vec2::splat(1.5)),
		ChipType::DisplayLed => Some(Vec2::splat(0.1875)),
		_ => None,
	}
}

/// Whether `chip_type` can be placed as an embedded display on another
/// chip (the customize menu lists exactly these subchips).
pub fn is_display_type(chip_type: ChipType) -> bool {
	display_base_size(chip_type).is_some()
}

/// Whether a whole chip description can be shown as an embedded display:
/// one of the four builtin display types, or a custom chip that carries
/// displays of its own -- placing that cascades its entire display tree
/// into the host (mirrors the original's `Description.HasDisplay()`
/// filter for the customization DISPLAYS list, and `SubChipHelper.
/// CreateDisplayInstances`'s recursion).
pub fn can_be_embedded_display(desc: &ChipDescription) -> bool {
	is_display_type(desc.chip_type) || (desc.chip_type == ChipType::Custom && !desc.displays.is_empty())
}

/// Depth cap for walking display cascades. Well-placed chips can never
/// contain themselves (`would_create_cycle` blocks it in the editor), but
/// a hand-edited save file could -- this keeps a cycle from hanging the
/// renderer instead of trusting the input the way the original does.
const MAX_CASCADE_DEPTH: u32 = 32;

/// Resolves a display entry's subchip id against its owning chip's own
/// sub-chip list (ids are unique per owner, not globally -- each cascade
/// level resolves through *its* node's list, not the host's).
fn resolve_sub_chip<'a>(owner: &ChipDescription, sub_chip_id: i32, library: &'a ChipLibrary) -> Option<&'a ChipDescription> {
	owner.sub_chips.iter().find(|s| s.id == sub_chip_id).and_then(|s| library.try_get(&s.name))
}

/// World-space `(centre offset from the node's own position, size)` that
/// one custom-chip display node's cascaded content occupies at `scale`:
/// the union of everything its tree paints, with child positions/scales
/// composing multiplicatively down the levels. `None` when the tree has
/// no drawable content at all. Mirrors the bounds the original's
/// `DrawDisplayWithBackground` accumulates to size its backing quad.
fn cascade_bounds(desc: &ChipDescription, scale: f32, library: &ChipLibrary, depth: u32) -> Option<(Vec2, Vec2)> {
	if depth >= MAX_CASCADE_DEPTH {
		return None;
	}
	let mut min = Vec2::splat(f32::MAX);
	let mut max = Vec2::splat(f32::MIN);
	for child in &desc.displays {
		let Some(child_desc) = resolve_sub_chip(desc, child.sub_chip_id, library) else { continue };
		let Some((offset, size)) = (match child_desc.chip_type {
			ChipType::Custom => cascade_bounds(child_desc, child.scale * scale, library, depth + 1),
			t => display_base_size(t).map(|base| (Vec2::ZERO, base * child.scale * scale)),
		}) else {
			continue;
		};
		let centre = child.position * scale + offset;
		min = vec_min(min, centre - size * 0.5);
		max = vec_max(max, centre + size * 0.5);
	}
	(min.x <= max.x).then(|| ((min + max) * 0.5, max - min))
}

/// World-space `(centre offset from the host anchor, size)` one display
/// entry's painted content occupies: builtin types fill their known
/// base-size rect about the anchor; a custom entry takes the union of its
/// whole cascade (see [`cascade_bounds`]). `None` when there's nothing to
/// show.
pub fn display_entry_bounds(display: &DisplayDescription, desc: &ChipDescription, library: &ChipLibrary) -> Option<(Vec2, Vec2)> {
	match desc.chip_type {
		ChipType::Custom => cascade_bounds(desc, display.scale, library, 0),
		t => display_base_size(t).map(|base| (Vec2::ZERO, base * display.scale)),
	}
}

/// Border colour drawn around an embedded display, derived from the host
/// chip's body colour (`GetChipDisplayBorderCol`): darkened for light
/// bodies, brightened for dark ones.
fn display_border_col(chip_colour: Rgba) -> Rgba {
	let body = if chip_colour[3] > 0.0 { chip_colour } else { theme::CHIP_BODY_COL };
	let darken = theme::text_colour_for_background(body)[0] == 0.0;
	let shift = |c: f32| if darken { c - 0.13 } else { c + 0.13 };
	[shift(body[0]).clamp(0.0, 1.0), shift(body[1]).clamp(0.0, 1.0), shift(body[2]).clamp(0.0, 1.0), 1.0]
}

/// Canvas-path entry: draws one *placed* subchip's embedded displays
/// (`mark_out_of_bounds` is deliberately off here -- the red flag is a
/// customize-preview-only affordance, matching the original's
/// `outOfBoundsDisplay` parameter). Called once per placed subchip by
/// `build_scene_with_spans`, bracketed into that component's own vertex
/// span -- see `render::scene::components::draw_component`'s doc comment.
pub(crate) fn draw_placed_displays_for(
	geo: &mut SceneGeometry,
	sub: &crate::render::scene::placed::PlacedSubChip,
	library: &ChipLibrary,
	pin_state: &dyn PinStateLookup,
) {
	if sub.desc.displays.is_empty() {
		return;
	}
	let desc = sub.desc;
	// The displays' `(subchip id, pin id)` addresses live *inside this
	// placed chip's own scope*, not the one this draw call was handed
	// -- descend one level before resolving, or every pin reads
	// unresolvable and nothing ever lights. Un-enterable scopes (e.g.
	// static previews with no simulator) draw blank, mirroring the
	// original's `sim == null` branch.
	let scoped: Box<dyn PinStateLookup> = pin_state.enter_scope(sub.id).unwrap_or_else(|| Box::new(AllLow));
	draw_subchip_displays(geo, sub.centre, sub.size, &desc.sub_chips, &desc.displays, library, scoped.as_ref(), desc.colour, false);
}

/// Draws every embedded display of a chip, clipped to the body rect at
/// (`chip_centre`, `chip_size`). Display entries resolve through
/// `owner_sub_chips` -- the owning chip's own sub-chip list whose ids the
/// entries reference; entries resolving to nothing display-carrying are
/// skipped, same as the original's "display has been deleted by player"
/// tolerance. `chip_colour` tints the border around each display (alpha 0
/// falls back to the theme default). With `mark_out_of_bounds`, displays
/// sticking out of the body are flagged with a translucent red quad over
/// their full extent (customize preview).
///
/// A custom-chip entry cascades: its target chip's own displays merge in
/// wholesale, positions/scales composing down the levels and backed by
/// one quad over the union -- mirroring `DrawDisplay`'s `ChildDisplays`
/// recursion plus the bounds-accumulating backing of
/// `DrawDisplayWithBackground`.
#[allow(clippy::too_many_arguments)] // one painter entry covering clip/colour/flag knobs
pub fn draw_subchip_displays(
	geo: &mut SceneGeometry,
	chip_centre: Vec2,
	chip_size: Vec2,
	owner_sub_chips: &[SubChipDescription],
	displays: &[DisplayDescription],
	library: &ChipLibrary,
	pin_state: &dyn PinStateLookup,
	chip_colour: Rgba,
	mark_out_of_bounds: bool,
) {
	if displays.is_empty() {
		return;
	}
	let clip = ClipRect::from_centre_size(chip_centre, chip_size);

	for display in displays {
		let Some(desc) = resolve_sub_chip_id(owner_sub_chips, display.sub_chip_id, library) else { continue };
		let origin = chip_centre + display.position;

		// The backing/border/out-of-bounds rects follow the *painted*
		// extent: builtin types fill their base-size rect about the entry
		// position; a custom entry's content unions wherever its cascade
		// actually lands (which may be off-centre from the entry itself).
		let (backing_centre, bounds_size, paint_scale) = match desc.chip_type {
			ChipType::Custom => {
				let Some((offset, size)) = cascade_bounds(desc, display.scale, library, 0) else { continue };
				(origin + offset, size, display.scale)
			}
			t => {
				let Some(base) = display_base_size(t) else { continue };
				(origin, base * display.scale, base.x * display.scale)
			}
		};

		// Backing + border first, so the clipped content drawn next lands
		// on top (mirrors the original's reserved-quad ordering).
		clip.add_rect(geo, backing_centre, bounds_size + Vec2::splat(0.03), display_border_col(chip_colour));
		clip.add_rect(geo, backing_centre, bounds_size, theme::STATE_DISCONNECTED_COL);

		draw_display_node(geo, clip, desc, display.sub_chip_id, origin, paint_scale, library, pin_state, 0);

		if mark_out_of_bounds && !clip.contains_rect(backing_centre, bounds_size) {
			geo.add_rect(backing_centre, bounds_size, OUT_OF_BOUNDS_COL);
		}
	}
}

/// Resolves a top-level display entry against an explicit sub-chip list
/// (callers like the customize ghost pass a list without a whole host
/// description at hand).
fn resolve_sub_chip_id<'a>(owner_sub_chips: &[SubChipDescription], sub_chip_id: i32, library: &'a ChipLibrary) -> Option<&'a ChipDescription> {
	owner_sub_chips.iter().find(|s| s.id == sub_chip_id).and_then(|s| library.try_get(&s.name))
}

/// Draws one display node's content -- recursing through custom chips'
/// own embedded displays (the cascade), descending one sim scope per
/// level. `scale` is the node's composed world multiplier: leaf painters
/// turn it into their final footprint via each child type's base size,
/// while custom children keep composing it.
#[allow(clippy::too_many_arguments)]
fn draw_display_node(
	geo: &mut SceneGeometry,
	clip: ClipRect,
	desc: &ChipDescription,
	owner_id: i32,
	centre: Vec2,
	scale: f32,
	library: &ChipLibrary,
	pin_state: &dyn PinStateLookup,
	depth: u32,
) {
	match desc.chip_type {
		ChipType::Custom => {
			if depth >= MAX_CASCADE_DEPTH {
				return;
			}
			// The child displays' subchip ids live inside this node's own
			// scope; positions/scales are relative to it.
			let child_pin_state: Box<dyn PinStateLookup> = pin_state.enter_scope(owner_id).unwrap_or_else(|| Box::new(AllLow));
			for child in &desc.displays {
				let Some(child_desc) = resolve_sub_chip(desc, child.sub_chip_id, library) else { continue };
				let child_scale = match child_desc.chip_type {
					ChipType::Custom => child.scale * scale,
					t => display_base_size(t).map_or(child.scale * scale, |base| base.x * child.scale * scale),
				};
				draw_display_node(
					geo,
					clip,
					child_desc,
					child.sub_chip_id,
					centre + child.position * scale,
					child_scale,
					library,
					&*child_pin_state,
					depth + 1,
				);
			}
		}
		ChipType::SevenSegmentDisplay => draw_seven_segment(geo, clip, centre, scale, owner_id, pin_state),
		ChipType::DisplayRgb => draw_pixel_grid(geo, clip, centre, scale, owner_id, pin_state, true),
		ChipType::DisplayDot => draw_pixel_grid(geo, clip, centre, scale, owner_id, pin_state, false),
		ChipType::DisplayLed => draw_led(geo, clip, centre, scale, owner_id, pin_state),
		_ => {}
	}
}

/// One 7-segment digit: black backing plus seven segments, sized `scale`
/// wide and 1.75x that tall. Segment colours come precomputed from the
/// caller's pin-state lookup so the plain-preview and live-sim paths stay
/// identical.
pub(crate) fn draw_seven_segment(geo: &mut SceneGeometry, clip: ClipRect, centre: Vec2, scale: f32, owner_id: i32, pin_state: &dyn PinStateLookup) {
	const TARGET_HEIGHT_ASPECT: f32 = 1.75;
	const SEGMENT_THICKNESS_FRAC: f32 = 0.165;
	const SEGMENT_VERTICAL_SPACING_FRAC: f32 = 0.07;
	const DISPLAY_INSET_FRAC: f32 = 0.2;

	let col_offset = if pin_state.is_high(owner_id, 7) == Some(true) { 3 } else { 0 };
	let seg_col = |pin_id: i32| {
		let on = pin_state.is_high(owner_id, pin_id) == Some(true);
		theme::SEVEN_SEG_COLS[(if on { 1 } else { 0 }) + col_offset]
	};

	let bounds_width = scale;
	let bounds_height = bounds_width * TARGET_HEIGHT_ASPECT;
	let segment_thickness = scale * SEGMENT_THICKNESS_FRAC;
	let segment_width = bounds_width - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_region_height = bounds_height - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_height = segment_region_height / 2.0 - scale * SEGMENT_VERTICAL_SPACING_FRAC;

	clip.add_rect(geo, centre, Vec2::new(bounds_width, bounds_height), theme::STATE_DISCONNECTED_COL);

	let offset_x = Vec2::new(segment_width / 2.0, 0.0);
	let offset_y = Vec2::new(0.0, segment_region_height / 4.0);
	let vertical_size = Vec2::new(segment_thickness, segment_height);
	let horizontal_size = Vec2::new(segment_width, segment_thickness);

	// Middle G, top A, bottom D, then F/E (left) and B/C (right).
	clip.add_rect(geo, centre, horizontal_size, seg_col(6));
	clip.add_rect(geo, centre + Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, seg_col(0));
	clip.add_rect(geo, centre - Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, seg_col(3));
	clip.add_rect(geo, centre - offset_x + offset_y, vertical_size, seg_col(5));
	clip.add_rect(geo, centre - offset_x - offset_y, vertical_size, seg_col(4));
	clip.add_rect(geo, centre + offset_x + offset_y, vertical_size, seg_col(1));
	clip.add_rect(geo, centre + offset_x - offset_y, vertical_size, seg_col(2));
}

/// Draws a `DisplayRgb`/`DisplayDot` pixel buffer (16x16, address
/// `y * 16 + x`; packed R|G<<4|B<<8 nibbles for RGB, plain on/off for
/// dot) at `scale` square. Blank dim pixels when not simulated.
pub(crate) fn draw_pixel_grid(
	geo: &mut SceneGeometry,
	clip: ClipRect,
	centre: Vec2,
	scale: f32,
	owner_id: i32,
	pin_state: &dyn PinStateLookup,
	is_rgb: bool,
) {
	const PIXELS_PER_ROW: usize = 16;
	const BORDER_FRAC: f32 = 0.95;
	const PIXEL_SIZE_FRAC: f32 = 0.925;
	const OFF_PIXEL_COL: Rgba = [0.1, 0.1, 0.1, 1.0];

	clip.add_rect(geo, centre, Vec2::splat(scale), theme::STATE_DISCONNECTED_COL);

	let size = scale * BORDER_FRAC;
	let pixel_size = size / PIXELS_PER_ROW as f32;
	let pixel_draw_size = Vec2::splat(pixel_size) * PIXEL_SIZE_FRAC;
	let bottom_left = centre - Vec2::splat(size) * 0.5;

	let internal_state = pin_state.internal_state(owner_id);

	fn unpack_4bit_channel(raw: u32) -> f32 {
		(raw & 0b1111) as f32 / 15.0
	}

	for y in 0..PIXELS_PER_ROW {
		for x in 0..PIXELS_PER_ROW {
			let col = match internal_state.and_then(|s| s.get(y * PIXELS_PER_ROW + x)) {
				Some(&pixel_state) => {
					if is_rgb {
						[unpack_4bit_channel(pixel_state), unpack_4bit_channel(pixel_state >> 4), unpack_4bit_channel(pixel_state >> 8), 1.0]
					} else {
						let v = (pixel_state != 0) as u32 as f32;
						[v, v, v, 1.0]
					}
				}
				None => OFF_PIXEL_COL,
			};

			let pos = bottom_left + Vec2::splat(pixel_size) * 0.5 + Vec2::new(pixel_size * x as f32, pixel_size * y as f32);
			clip.add_rect(geo, pos, pixel_draw_size, col);
		}
	}
}

/// Draws one LED tile: black backing plus a tinted inner square showing
/// the same three wire states -- lit/dim by the LED subchip's first input
/// pin's *tristate* level (so an unconnected LED renders flat black, not
/// dim), coloured by its saved palette index in `internal_data[0]` (white
/// when unconfigured).
pub(crate) fn draw_led(geo: &mut SceneGeometry, clip: ClipRect, centre: Vec2, scale: f32, owner_id: i32, pin_state: &dyn PinStateLookup) {
	clip.add_rect(geo, centre, Vec2::splat(scale), theme::STATE_DISCONNECTED_COL);

	let logic = pin_state.logic_state(owner_id, 0).unwrap_or(LogicState::Low);
	let colour_index = pin_state.internal_state(owner_id).and_then(|s| s.first().copied()).unwrap_or(Color::White.to_int() as u32);
	let palette = Color::from_int(colour_index as i32);
	let fill = theme::state_colour(logic, palette);

	clip.add_rect(geo, centre, Vec2::splat(scale * 0.975), fill);
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{PinBitCount, PinDescription};
	use std::collections::HashMap;

	/// Fixed-state lookup double: per-(owner, pin) logic states plus an
	/// optional internal buffer, mirroring the pattern used by
	/// `components.rs`' tests.
	struct FixedState {
		pins: HashMap<(i32, i32), bool>,
		internal: Option<Vec<u32>>,
	}

	impl PinStateLookup for FixedState {
		fn is_high(&self, owner_id: i32, pin_id: i32) -> Option<bool> {
			Some(*self.pins.get(&(owner_id, pin_id)).unwrap_or(&false))
		}

		fn internal_state(&self, _owner_id: i32) -> Option<&[u32]> {
			self.internal.as_deref()
		}
	}

	fn seg_desc() -> ChipDescription {
		let mut d = ChipDescription::new("7Seg", ChipType::SevenSegmentDisplay);
		for (i, name) in ["A", "B", "C", "D", "E", "F", "G", "COL"].iter().enumerate() {
			d.input_pins.push(PinDescription::new(*name, i as i32, PinBitCount::Bit1));
		}
		d
	}

	fn rgb_desc() -> ChipDescription {
		ChipDescription::new("RGB", ChipType::DisplayRgb)
	}

	fn rects_drawn(geo: &SceneGeometry) -> usize {
		assert_eq!(geo.triangles.len() % 6, 0);
		geo.triangles.len() / 6
	}

	#[test]
	fn display_base_size_covers_only_display_types() {
		// Scale-1 parity: these are the originals' builtin self-display
		// content widths, so an embedded display at scale 1 matches its
		// placed-component counterpart exactly.
		assert_eq!(display_base_size(ChipType::SevenSegmentDisplay), Some(Vec2::new(1.0, 1.75)));
		assert_eq!(display_base_size(ChipType::DisplayRgb), Some(Vec2::splat(2.375)));
		assert_eq!(display_base_size(ChipType::DisplayDot), Some(Vec2::splat(1.5)));
		assert_eq!(display_base_size(ChipType::DisplayLed), Some(Vec2::splat(0.1875)));
		assert_eq!(display_base_size(ChipType::Nand), None);
		assert!(is_display_type(ChipType::DisplayDot));
		assert!(!is_display_type(ChipType::Clock));
	}

	/// Library fixture keyed by name plus a host whose sub-chip list
	/// references them -- the shape every draw call resolves through.
	struct Fixture {
		library: ChipLibrary,
		host: ChipDescription,
	}

	fn fixture(chips: &[ChipDescription], host_subs: &[(&str, i32)]) -> Fixture {
		let mut library = ChipLibrary::new();
		for desc in chips {
			library.add(desc.clone());
		}
		let mut host = ChipDescription::new("HOST", ChipType::Custom);
		for (name, id) in host_subs {
			host.sub_chips.push(SubChipDescription {
				name: (*name).into(),
				id: *id,
				internal_data: None,
				position: Vec2::ZERO,
				label: None,
				pin_colour_info: vec![],
			});
		}
		Fixture { library, host }
	}

	fn draws_into(fixture: &Fixture, chip_size: Vec2, displays: &[DisplayDescription], out_of_bounds: bool) -> SceneGeometry {
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(
			&mut geo,
			Vec2::ZERO,
			chip_size,
			&fixture.host.sub_chips,
			displays,
			&fixture.library,
			&FixedState { pins: HashMap::new(), internal: None },
			theme::CHIP_BODY_COL,
			out_of_bounds,
		);
		geo
	}

	fn colours_present(geo: &SceneGeometry) -> std::collections::HashSet<[u32; 4]> {
		geo.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect()
	}

	#[test]
	fn out_of_bounds_display_gets_red_flag_inside_body_does_not() {
		let seg = seg_desc();
		let f = fixture(&[seg], &[("7Seg", 5)]);

		// Display fully outside the body.
		let geo = draws_into(&f, Vec2::splat(2.0), &[DisplayDescription::new(5, Vec2::new(50.0, 50.0), 1.0)], true);
		assert!(colours_present(&geo).contains(&OUT_OF_BOUNDS_COL.map(f32::to_bits)), "sticking-out display must be flagged red");

		// Same display centred well inside the body.
		let geo = draws_into(&f, Vec2::splat(4.0), &[DisplayDescription::new(5, Vec2::ZERO, 0.5)], true);
		assert!(!colours_present(&geo).contains(&OUT_OF_BOUNDS_COL.map(f32::to_bits)), "fitting display must not be flagged red");
	}

	#[test]
	fn content_is_clipped_to_the_host_body() {
		// A 1x1 LED pushed halfway past the body edge: every vertex of the
		// drawn content must sit within the body rect.
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let f = fixture(&[led], &[("LED", 3)]);
		let host_size = Vec2::splat(1.0);
		let displays = [DisplayDescription::new(3, Vec2::new(1.25, 0.0), 1.0)];

		// Out-of-bounds flag off: its red quad deliberately covers the
		// display's *full* extent (it must stay visible past the body edge),
		// so it isn't subject to the clipping under test here.
		let geo = draws_into(&f, host_size, &displays, false);

		let clip = ClipRect::from_centre_size(Vec2::ZERO, host_size);
		for v in &geo.triangles {
			assert!(v.pos.x >= clip.min.x - 1e-4 && v.pos.x <= clip.max.x + 1e-4, "vertex x {} escapes clip", v.pos.x);
			assert!(v.pos.y >= clip.min.y - 1e-4 && v.pos.y <= clip.max.y + 1e-4, "vertex y {} escapes clip", v.pos.y);
		}
	}

	#[test]
	fn non_display_and_unresolvable_ids_are_skipped() {
		let nand = ChipDescription::new("NAND", ChipType::Nand);
		let f = fixture(&[nand], &[("NAND", 1), ("GHOST", 99)]);
		let displays = [DisplayDescription::new(1, Vec2::ZERO, 1.0), DisplayDescription::new(99, Vec2::ZERO, 1.0)];

		let geo = draws_into(&f, Vec2::splat(4.0), &displays, true);
		assert_eq!(rects_drawn(&geo), 0, "neither entry may produce geometry");
	}

	#[test]
	fn empty_displays_draw_nothing_at_all() {
		let f = fixture(&[], &[]);
		let geo = draws_into(&f, Vec2::splat(1.0), &[], true);
		assert!(geo.triangles.is_empty());
	}

	#[test]
	fn rgb_pixel_content_reads_internal_buffer_with_packed_nibbles() {
		let rgb = rgb_desc();
		let mut internal = vec![0u32; 256];
		internal[0] = 0xF; // full red at pixel (0, 0)

		let mut library = ChipLibrary::new();
		library.add(rgb.clone());
		let mut host = ChipDescription::new("HOST", ChipType::Custom);
		host.sub_chips.push(SubChipDescription {
			name: "RGB".into(),
			id: 2,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});

		let displays = [DisplayDescription::new(2, Vec2::ZERO, 2.0)];
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(
			&mut geo,
			Vec2::ZERO,
			Vec2::splat(8.0),
			&host.sub_chips,
			&displays,
			&library,
			&FixedState { pins: HashMap::new(), internal: Some(internal) },
			theme::CHIP_BODY_COL,
			false,
		);

		assert!(colours_present(&geo).contains(&[1.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "the written pixel must decode to full red");
	}

	/// The painted LED content must occupy exactly its layout rect
	/// (`base * scale` -- the rect the customize preview hit-tests and the
	/// backing/border quads follow), not the raw `scale` value. A scale of
	/// 2 paints a 0.375-unit tile; before this was fixed it painted a full
	/// 1-unit tile that spilled ~5x past its own backing.
	#[test]
	fn embedded_led_paints_exactly_its_layout_rect() {
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let f = fixture(&[led], &[("LED", 3)]);

		let geo = draws_into(&f, Vec2::splat(10.0), &[DisplayDescription::new(3, Vec2::ZERO, 2.0)], false);

		let max_extent = geo.triangles.iter().fold(0.0f32, |m, v| m.max(v.pos.x.abs()).max(v.pos.y.abs()));
		// Widest quad is the border (content + 0.03 total), centred.
		let expected_half = display_base_size(ChipType::DisplayLed).unwrap().x * 2.0 / 2.0 + 0.015;
		assert!((max_extent - expected_half).abs() < 1e-4, "content half-extent must be {expected_half}, got {max_extent}");
	}

	/// The cascade: placing a custom chip that carries its own embedded
	/// LED merges that LED into the host -- its painted tile lands at
	/// `host entry position + child position` with sizes composing, and
	/// the whole union gets one backing (mirroring `DrawDisplay`'s
	/// `ChildDisplays` recursion).
	#[test]
	fn custom_display_entry_cascades_its_own_displays() {
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let mut panel = ChipDescription::new("PANEL", ChipType::Custom);
		panel.sub_chips.push(SubChipDescription {
			name: "LED".into(),
			id: 7,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		panel.displays.push(DisplayDescription::new(7, Vec2::new(0.25, -0.25), 2.0));

		// HOST holds PANEL as a subchip and places it as a display.
		let mut f = fixture(&[led, panel], &[("PANEL", 9)]);
		f.host.displays.push(DisplayDescription::new(9, Vec2::new(1.0, 0.5), 1.0));

		let geo = draws_into(&f, Vec2::splat(6.0), &f.host.displays, true);

		// The cascaded LED tile is base(0.1875) * panel-entry scale 1 *
		// inner scale 2 = 0.375 wide, centred at (1.0, 0.5) + (0.25,-0.25).
		let centre = Vec2::new(1.25, 0.25);
		let near = |p: Vec2| (p - centre).magnitude() < 0.5;
		let lit_vertices = geo.triangles.iter().filter(|v| near(v.pos)).count();
		assert!(lit_vertices > 0, "the nested LED's tiles must land at the composed position");

		// And the cascade's bounds drive the red out-of-bounds flag: push
		// the entry mostly out of a small body -- flagged.
		let small = [DisplayDescription::new(9, Vec2::new(2.9, 2.9), 1.0)];
		let geo = draws_into(&f, Vec2::splat(1.0), &small, true);
		assert!(colours_present(&geo).contains(&OUT_OF_BOUNDS_COL.map(f32::to_bits)), "a sticking-out cascade gets the red flag");
	}

	/// A custom display entry whose target carries nothing drawable draws
	/// no geometry at all (no backing for an empty union).
	#[test]
	fn empty_custom_display_entry_draws_nothing() {
		let mut panel = ChipDescription::new("PANEL", ChipType::Custom);
		panel.sub_chips.push(SubChipDescription {
			name: "X".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		let f = fixture(&[panel], &[("PANEL", 2)]);

		let geo = draws_into(&f, Vec2::splat(4.0), &[DisplayDescription::new(2, Vec2::ZERO, 1.0)], true);
		assert_eq!(rects_drawn(&geo), 0, "an empty cascade has nothing to back or draw");
	}

	/// The bounds contract the customize preview's hitboxes lean on:
	/// leaves anchor at the entry position with their base*scale extent;
	/// a cascade's union can sit off-centre from it.
	#[test]
	fn entry_bounds_match_painted_extents_for_leaves_and_cascades() {
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let seg = seg_desc();

		assert_eq!(
			display_entry_bounds(&DisplayDescription::new(0, Vec2::ZERO, 2.0), &led, &ChipLibrary::new()),
			Some((Vec2::ZERO, Vec2::splat(0.375)))
		);
		assert_eq!(
			display_entry_bounds(&DisplayDescription::new(0, Vec2::ZERO, 1.0), &seg, &ChipLibrary::new()),
			Some((Vec2::ZERO, Vec2::new(1.0, 1.75)))
		);

		// Panel with an LED at its origin and a 7-seg at (2, 0): union
		// spans x[-0.09375, 2.5], y[-0.875, 0.875].
		let mut panel = ChipDescription::new("PANEL", ChipType::Custom);
		panel.sub_chips.push(SubChipDescription {
			name: "LED".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		panel.sub_chips.push(SubChipDescription {
			name: "7Seg".into(),
			id: 2,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		panel.displays.push(DisplayDescription::new(1, Vec2::ZERO, 1.0));
		panel.displays.push(DisplayDescription::new(2, Vec2::new(2.0, 0.0), 1.0));

		let mut library = ChipLibrary::new();
		library.add(led);
		library.add(seg);
		library.add(panel.clone());

		let (offset, size) = display_entry_bounds(&DisplayDescription::new(0, Vec2::ZERO, 1.0), &panel, &library).unwrap();
		assert!((offset.x - 1.203125).abs() < 1e-4 && offset.y.abs() < 1e-4, "union centre {offset:?}");
		assert!((size.x - 2.59375).abs() < 1e-4 && (size.y - 1.75).abs() < 1e-4, "union size {size:?}");

		// Nothing drawable anywhere in the tree -> no bounds at all.
		let mut bare = ChipDescription::new("BARE", ChipType::Custom);
		bare.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 3,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		assert_eq!(display_entry_bounds(&DisplayDescription::new(3, Vec2::ZERO, 1.0), &bare, &library), None);
		library.add(bare);
	}

	/// LED fills follow the wire palette's three states: lit, dim, and --
	/// the state it used to lack -- flat black when the pin is
	/// disconnected rather than reading as merely "off".
	#[test]
	fn led_fill_shows_all_three_wire_states() {
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let mut library = ChipLibrary::new();
		library.add(led.clone());
		let mut host = ChipDescription::new("HOST", ChipType::Custom);
		host.sub_chips.push(SubChipDescription { name: "LED".into(), id: 3, internal_data: None, position: Vec2::ZERO, label: None, pin_colour_info: vec![] });
		let displays = [DisplayDescription::new(3, Vec2::ZERO, 4.0)];

		struct Level(LogicState);
		impl PinStateLookup for Level {
			fn is_high(&self, _o: i32, _p: i32) -> Option<bool> {
				Some(self.0 == LogicState::High)
			}
			fn logic_state(&self, _o: i32, _p: i32) -> Option<LogicState> {
				Some(self.0)
			}
		}

		let lit = theme::state_colour(LogicState::High, Color::White).map(f32::to_bits);
		let dim = theme::state_colour(LogicState::Low, Color::White).map(f32::to_bits);
		let black = theme::STATE_DISCONNECTED_COL.map(f32::to_bits);

		let black = theme::STATE_DISCONNECTED_COL.map(f32::to_bits); // also the backing quad
		let cases: [(LogicState, std::vec::Vec<[u32; 4]>, std::vec::Vec<[u32; 4]>); 3] = [
			(LogicState::High, vec![lit], vec![dim]),
			(LogicState::Low, vec![dim], vec![lit]),
			// The backing quad is black anyway; what matters is that the
			// *fill* no longer paints a tinted colour over it.
			(LogicState::Disconnected, vec![black], vec![lit, dim]),
		];
		for (level, present, absent) in cases {
			let mut geo = SceneGeometry::default();
			draw_subchip_displays(&mut geo, Vec2::ZERO, Vec2::splat(8.0), &host.sub_chips, &displays, &library, &Level(level), theme::CHIP_BODY_COL, false);
			let colours = colours_present(&geo);
			for expected in present {
				assert!(colours.contains(&expected), "{level:?}: expected {expected:?} among {}", colours.len());
			}
			for unexpected in absent {
				assert!(!colours.contains(&unexpected), "{level:?}: must not paint {unexpected:?}");
			}
		}
	}

	/// A hand-edited save could make a chip's display point back at
	/// itself; the depth guard must keep drawing/bounds from recursing
	/// forever. A real LED sits beside the cycle so there's genuine
	/// content to draw -- this test passing at all is the assertion (an
	/// unguarded walk would blow the stack).
	#[test]
	fn cyclic_cascade_is_cut_off_not_hung() {
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let mut loopy = ChipDescription::new("LOOPY", ChipType::Custom);
		loopy.sub_chips.push(SubChipDescription {
			name: "LOOPY".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		loopy.sub_chips.push(SubChipDescription {
			name: "LED".into(),
			id: 2,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		loopy.displays.push(DisplayDescription::new(2, Vec2::new(0.125, 0.0), 1.0));
		loopy.displays.push(DisplayDescription::new(1, Vec2::new(0.25, 0.0), 1.0));
		let mut f = fixture(&[led, loopy], &[("LOOPY", 1)]);
		f.host.displays.push(DisplayDescription::new(1, Vec2::ZERO, 1.0));

		let geo = draws_into(&f, Vec2::splat(20.0), &f.host.displays, false);
		assert!(rects_drawn(&geo) > 0, "the real content beside the cycle still backs and draws");
	}
}
