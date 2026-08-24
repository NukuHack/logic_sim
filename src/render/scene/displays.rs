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

use crate::description::{ChipDescription, ChipType, Color, DisplayDescription};
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

/// World-space rect (centre, size) a top-level display occupies inside
/// its host chip: `host_centre + offset`, sized `base * scale`.
pub fn display_world_rect(host_centre: Vec2, display: DisplayDescription, displayed_type: ChipType) -> Option<(Vec2, Vec2)> {
	Some((host_centre + display.position, display_base_size(displayed_type)? * display.scale))
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
	library: &crate::description::ChipLibrary,
	pin_state: &dyn PinStateLookup,
) {
	if sub.desc.displays.is_empty() {
		return;
	}
	let desc = sub.desc;
	let resolve = move |id: i32| desc.sub_chips.iter().find(|s| s.id == id).and_then(|s| library.try_get(&s.name));
	// The displays' `(subchip id, pin id)` addresses live *inside this
	// placed chip's own scope*, not the one this draw call was handed
	// -- descend one level before resolving, or every pin reads
	// unresolvable and nothing ever lights. Un-enterable scopes (e.g.
	// static previews with no simulator) draw blank, mirroring the
	// original's `sim == null` branch.
	let scoped: Box<dyn PinStateLookup> = pin_state.enter_scope(sub.id).unwrap_or_else(|| Box::new(AllLow));
	draw_subchip_displays(geo, sub.centre, sub.size, &desc.displays, resolve, scoped.as_ref(), desc.colour, false);
}

/// Draws every embedded display of a chip, clipped to the body rect at
/// (`chip_centre`, `chip_size`). `resolve` maps a subchip id to its
/// description via the caller's library; ids that don't resolve to a
/// display-type chip are skipped, same as the original's "display has
/// been deleted by player" tolerance. `chip_colour` tints the border
/// around each display (alpha 0 falls back to the theme default). With
/// `mark_out_of_bounds`, displays sticking out of the body are flagged
/// with a translucent red quad over their full extent (customize preview).
#[allow(clippy::too_many_arguments)] // one painter entry covering clip/colour/flag knobs
pub fn draw_subchip_displays<'a>(
	geo: &mut SceneGeometry,
	chip_centre: Vec2,
	chip_size: Vec2,
	displays: &[DisplayDescription],
	resolve: impl Fn(i32) -> Option<&'a ChipDescription>,
	pin_state: &dyn PinStateLookup,
	chip_colour: Rgba,
	mark_out_of_bounds: bool,
) {
	if displays.is_empty() {
		return;
	}
	let clip = ClipRect::from_centre_size(chip_centre, chip_size);

	for display in displays {
		let Some(desc) = resolve(display.sub_chip_id) else { continue };
		let Some(base) = display_base_size(desc.chip_type) else { continue };

		let centre = chip_centre + display.position;
		let bounds_size = base * display.scale;

		// Backing + border first, so the clipped content drawn next lands
		// on top (mirrors the original's reserved-quad ordering).
		clip.add_rect(geo, centre, bounds_size + Vec2::splat(0.03), display_border_col(chip_colour));
		clip.add_rect(geo, centre, bounds_size, theme::STATE_DISCONNECTED_COL);

		draw_display_node(geo, clip, desc, display.sub_chip_id, centre, display.scale, &resolve, pin_state);

		if mark_out_of_bounds && !clip.contains_rect(centre, bounds_size) {
			geo.add_rect(centre, bounds_size, OUT_OF_BOUNDS_COL);
		}
	}
}

/// Draws one display node's content -- recursing through custom chips'
/// own embedded displays, descending one sim scope per level.
#[allow(clippy::too_many_arguments)]
fn draw_display_node<'a>(
	geo: &mut SceneGeometry,
	clip: ClipRect,
	desc: &ChipDescription,
	owner_id: i32,
	centre: Vec2,
	scale: f32,
	resolve: &impl Fn(i32) -> Option<&'a ChipDescription>,
	pin_state: &dyn PinStateLookup,
) {
	match desc.chip_type {
		ChipType::Custom => {
			// The child displays' subchip ids live inside this node's own
			// scope; positions/scales are relative to it.
			let child_pin_state: Box<dyn PinStateLookup> = pin_state.enter_scope(owner_id).unwrap_or_else(|| Box::new(AllLow));
			for child in &desc.displays {
				let Some(child_desc) = resolve(child.sub_chip_id) else { continue };
				draw_display_node(
					geo,
					clip,
					child_desc,
					child.sub_chip_id,
					centre + child.position * scale,
					child.scale * scale,
					resolve,
					&*child_pin_state,
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

/// Draws one LED tile: black backing plus a tinted inner square, lit/dim
/// by the LED subchip's first input pin and coloured by its saved palette
/// index in `internal_data[0]` (white when unconfigured).
pub(crate) fn draw_led(geo: &mut SceneGeometry, clip: ClipRect, centre: Vec2, scale: f32, owner_id: i32, pin_state: &dyn PinStateLookup) {
	clip.add_rect(geo, centre, Vec2::splat(scale), theme::STATE_DISCONNECTED_COL);

	let lit = pin_state.is_high(owner_id, 0) == Some(true);
	let colour_index = pin_state.internal_state(owner_id).and_then(|s| s.first().copied()).unwrap_or(Color::White.to_int() as u32);
	let palette = Color::from_int(colour_index as i32);
	let fill = theme::state_colour(if lit { LogicState::High } else { LogicState::Low }, palette);

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

	/// Builds a resolver closure bound to `desc`'s real borrow region --
	/// a plain `move` closure returning references to its own captures
	/// trips the borrow checker's HRTB inference.
	fn resolves_to<'a>(desc: &'a ChipDescription, id: i32) -> impl Fn(i32) -> Option<&'a ChipDescription> {
		move |q: i32| (q == id).then_some(desc)
	}

	#[test]
	fn out_of_bounds_display_gets_red_flag_inside_body_does_not() {
		let seg = seg_desc();
		let resolve = resolves_to(&seg, 5);

		// Display fully outside the body.
		let outside = [DisplayDescription::new(5, Vec2::new(50.0, 50.0), 1.0)];
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(&mut geo, Vec2::ZERO, Vec2::splat(2.0), &outside, &resolve, &AllLow, theme::CHIP_BODY_COL, true);
		let colours: std::collections::HashSet<_> = geo.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect();
		assert!(colours.contains(&OUT_OF_BOUNDS_COL.map(f32::to_bits)), "sticking-out display must be flagged red");

		// Same display centred well inside the body.
		let inside = [DisplayDescription::new(5, Vec2::ZERO, 0.5)];
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(&mut geo, Vec2::ZERO, Vec2::splat(4.0), &inside, &resolve, &AllLow, theme::CHIP_BODY_COL, true);
		let colours: std::collections::HashSet<_> = geo.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect();
		assert!(!colours.contains(&OUT_OF_BOUNDS_COL.map(f32::to_bits)), "fitting display must not be flagged red");
	}

	#[test]
	fn content_is_clipped_to_the_host_body() {
		// A 1x1 LED pushed halfway past the body edge: every vertex of the
		// drawn content must sit within the body rect.
		let led = ChipDescription::new("LED", ChipType::DisplayLed);
		let host_size = Vec2::splat(1.0);
		let displays = [DisplayDescription::new(3, Vec2::new(1.25, 0.0), 1.0)];

		let mut geo = SceneGeometry::default();
		// Out-of-bounds flag off: its red quad deliberately covers the
		// display's *full* extent (it must stay visible past the body edge),
		// so it isn't subject to the clipping under test here.
		draw_subchip_displays(
			&mut geo,
			Vec2::ZERO,
			host_size,
			&displays,
			resolves_to(&led, 3),
			&FixedState { pins: HashMap::new(), internal: None },
			theme::CHIP_BODY_COL,
			false,
		);

		let clip = ClipRect::from_centre_size(Vec2::ZERO, host_size);
		for v in &geo.triangles {
			assert!(v.pos.x >= clip.min.x - 1e-4 && v.pos.x <= clip.max.x + 1e-4, "vertex x {} escapes clip", v.pos.x);
			assert!(v.pos.y >= clip.min.y - 1e-4 && v.pos.y <= clip.max.y + 1e-4, "vertex y {} escapes clip", v.pos.y);
		}
	}

	#[test]
	fn non_display_and_unresolvable_ids_are_skipped() {
		let nand = ChipDescription::new("NAND", ChipType::Nand);
		let displays = [DisplayDescription::new(1, Vec2::ZERO, 1.0), DisplayDescription::new(99, Vec2::ZERO, 1.0)];

		let mut geo = SceneGeometry::default();
		draw_subchip_displays(&mut geo, Vec2::ZERO, Vec2::splat(4.0), &displays, resolves_to(&nand, 1), &AllLow, theme::CHIP_BODY_COL, true);

		assert_eq!(rects_drawn(&geo), 0, "neither entry may produce geometry");
	}

	#[test]
	fn empty_displays_draw_nothing_at_all() {
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(
			&mut geo,
			Vec2::ZERO,
			Vec2::splat(1.0),
			&[],
			|_| unreachable!("resolver must not be consulted when there are no displays"),
			&AllLow,
			theme::CHIP_BODY_COL,
			true,
		);
		assert!(geo.triangles.is_empty());
	}

	#[test]
	fn rgb_pixel_content_reads_internal_buffer_with_packed_nibbles() {
		let rgb = rgb_desc();
		let mut internal = vec![0u32; 256];
		internal[0] = 0xF; // full red at pixel (0, 0)

		let displays = [DisplayDescription::new(2, Vec2::ZERO, 2.0)];
		let mut geo = SceneGeometry::default();
		draw_subchip_displays(
			&mut geo,
			Vec2::ZERO,
			Vec2::splat(4.0),
			&displays,
			resolves_to(&rgb, 2),
			&FixedState { pins: HashMap::new(), internal: Some(internal) },
			theme::CHIP_BODY_COL,
			false,
		);

		let colours: std::collections::HashSet<_> = geo.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect();
		assert!(colours.contains(&[1.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "the written pixel must decode to full red");
	}
}
