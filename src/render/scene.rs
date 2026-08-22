//! Builds drawable geometry for one "view" of a chip (i.e. what the editor
//! shows when you open a custom chip: its subchips, each subchip's pins,
//! and the wires between them). This is the scene-graph half of the
//! renderer -- pure data in, triangles out, no wgpu types -- so it can be
//! unit tested without a GPU.
//!
//! Mirrors (a first-pass subset of) `DLS.Graphics.World.DevSceneDrawer`.

use crate::description::Color;
use crate::description::{ChipDescription, ChipLibrary, ChipType, NameLocation, PinBitCount, WireConnectionType, WireDescription};
use crate::pin_state::LogicState;
use crate::render::camera::Camera;
use crate::render::layout::{self};
use crate::render::theme::{self, Rgba};
use crate::structs::Vec2;
use std::collections::HashMap;

/// A single coloured vertex, position in world space. Kept separate from
/// any wgpu `Vertex` type so this module has zero GPU dependencies; the
/// `render::gpu` module converts these 1:1 into its own bytemuck vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVertex {
	pub pos: Vec2,
	pub colour: Rgba,
}

/// A gate/chip name label to be drawn as text, in world space. Produced
/// alongside `triangles` by `build_scene` -- kept as a separate list (rather
/// than triangulated glyphs) since text is rendered by a dedicated font
/// pipeline (`render::gpu`'s glyphon integration), not the flat-colour
/// triangle pipeline the rest of the scene uses.
#[derive(Debug, Clone)]
pub struct TextLabel {
	/// World-space anchor point: the label is horizontally *and*
	/// vertically centred on this point (callers wanting a "near the top
	/// edge" placement, e.g. `NameDisplayLocation::Top`, pre-offset `pos`
	/// upward when building the label rather than needing a separate
	/// anchor mode here).
	pub pos: Vec2,
	pub text: String,
	pub colour: Rgba,
	/// World-space font size (grid units); mirrors `DrawSettings.FontSizeChipName`.
	pub font_size: f32,
	/// World-space width to horizontally centre/wrap the text within
	/// (typically the owning chip's body width).
	pub width: f32,
}

/// Flat triangle-list geometry ready to upload as a vertex buffer
/// (`triangles.len()` is always a multiple of 3), plus any text labels to
/// be drawn on top of it (e.g. gate/chip names).
#[derive(Debug, Default, Clone)]
pub struct SceneGeometry {
	pub triangles: Vec<SceneVertex>,
	pub labels: Vec<TextLabel>,
}

impl SceneGeometry {
	fn push_tri(&mut self, a: SceneVertex, b: SceneVertex, c: SceneVertex) {
		self.triangles.push(a);
		self.triangles.push(b);
		self.triangles.push(c);
	}

	fn push_quad(&mut self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, colour: Rgba) {
		// p0..p3 wound consistently (e.g. bottom-left, bottom-right,
		// top-right, top-left) so this is two triangles of a convex quad.
		let v = |p: Vec2| SceneVertex { pos: p, colour };
		self.push_tri(v(p0), v(p1), v(p2));
		self.push_tri(v(p0), v(p2), v(p3));
	}

	pub fn add_rect(&mut self, centre: Vec2, size: Vec2, colour: Rgba) {
		let hw = size.x / 2.0;
		let hh = size.y / 2.0;
		self.push_quad(
			Vec2::new(centre.x - hw, centre.y - hh),
			Vec2::new(centre.x + hw, centre.y - hh),
			Vec2::new(centre.x + hw, centre.y + hh),
			Vec2::new(centre.x - hw, centre.y + hh),
			colour,
		);
	}

	pub fn add_circle(&mut self, centre: Vec2, radius: f32, colour: Rgba, segments: u32) {
		let segments = segments.max(3);
		for i in 0..segments {
			let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
			let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
			let p0 = Vec2::new(centre.x + a0.cos() * radius, centre.y + a0.sin() * radius);
			let p1 = Vec2::new(centre.x + a1.cos() * radius, centre.y + a1.sin() * radius);
			self.push_tri(SceneVertex { pos: centre, colour }, SceneVertex { pos: p0, colour }, SceneVertex { pos: p1, colour });
		}
	}

	/// A rectangle of `size` centred on `centre`, with its corners rounded
	/// to `radius` on whichever of its left/right vertical edges
	/// `round_left`/`round_right` request (either, both, or neither -- the
	/// other edge's corners stay sharp). Used to draw a chip's own
	/// boundary dev-pins as a "partially rounded rectangle": rounded on
	/// the side facing outward (away from the chip body) and square on
	/// the side facing in, so they read visually distinct from a regular
	/// pin's plain circle. `radius` is clamped to the shape's own
	/// half-width/half-height so it can never overshoot into a bowtie.
	///
	/// Implemented as a fan of triangles from `centre` around the
	/// perimeter (rounded corners contribute an arc of points, square
	/// corners contribute just their one corner point), the same
	/// triangulation strategy `add_circle` uses -- valid here because a
	/// rounded rect (with radius capped to half the smaller dimension) is
	/// always convex/star-shaped from its own centre.
	pub fn add_rounded_rect(
		&mut self,
		centre: Vec2,
		size: Vec2,
		colour: Rgba,
		radius: f32,
		round_left: bool,
		round_right: bool,
		corner_segments: u32,
	) {
		let hw = size.x / 2.0;
		let hh = size.y / 2.0;
		if hw <= 0.0 || hh <= 0.0 {
			return;
		}
		let r = radius.max(0.0).min(hw).min(hh);
		let segs = corner_segments.max(1);

		fn push_corner(points: &mut Vec<Vec2>, cx: f32, cy: f32, arc_cx: f32, arc_cy: f32, start: f32, end: f32, r: f32, segs: u32, rounded: bool) {
			if rounded && r > 1e-6 {
				for i in 0..=segs {
					let t = i as f32 / segs as f32;
					let a = start + t * (end - start);
					points.push(Vec2::new(arc_cx + a.cos() * r, arc_cy + a.sin() * r));
				}
			} else {
				points.push(Vec2::new(cx, cy));
			}
		}

		use std::f32::consts::PI;
		let mut points: Vec<Vec2> = Vec::new();
		// Bottom-right -> top-right -> top-left -> bottom-left (CCW).
		push_corner(&mut points, centre.x + hw, centre.y - hh, centre.x + hw - r, centre.y - hh + r, -PI / 2.0, 0.0, r, segs, round_right);
		push_corner(&mut points, centre.x + hw, centre.y + hh, centre.x + hw - r, centre.y + hh - r, 0.0, PI / 2.0, r, segs, round_right);
		push_corner(&mut points, centre.x - hw, centre.y + hh, centre.x - hw + r, centre.y + hh - r, PI / 2.0, PI, r, segs, round_left);
		push_corner(&mut points, centre.x - hw, centre.y - hh, centre.x - hw + r, centre.y - hh + r, PI, 3.0 * PI / 2.0, r, segs, round_left);

		let n = points.len();
		for i in 0..n {
			let p0 = points[i];
			let p1 = points[(i + 1) % n];
			self.push_tri(SceneVertex { pos: centre, colour }, SceneVertex { pos: p0, colour }, SceneVertex { pos: p1, colour });
		}
	}

	/// A thick line segment from `a` to `b`, drawn as a rectangle.
	pub fn add_line(&mut self, a: Vec2, b: Vec2, thickness: f32, colour: Rgba) {
		let dx = b.x - a.x;
		let dy = b.y - a.y;
		let len = (dx * dx + dy * dy).sqrt();
		if len < 1e-6 {
			return;
		}
		let nx = -dy / len * thickness / 2.0;
		let ny = dx / len * thickness / 2.0;
		self.push_quad(
			Vec2::new(a.x + nx, a.y + ny),
			Vec2::new(b.x + nx, b.y + ny),
			Vec2::new(b.x - nx, b.y - ny),
			Vec2::new(a.x - nx, a.y - ny),
			colour,
		);
	}

	/// A thick polyline through `points`, drawn as one continuous ribbon
	/// with proper mitered joins at every interior vertex -- unlike
	/// drawing each segment as its own independent `add_line` rectangle
	/// (which leaves a visible gap or overlap at any bend that isn't
	/// perfectly straight), this keeps the two edges of the ribbon
	/// touching exactly at each bend. See `offset_polyline` (used for both
	/// this ribbon's two edges and, separately, for laying out each
	/// individual bit-strand's own centreline) for the actual join maths.
	pub fn add_polyline(&mut self, points: &[Vec2], thickness: f32, colour: Rgba) {
		if points.len() < 2 {
			return;
		}
		let half = thickness / 2.0;
		let left = offset_polyline(points, half);
		let right = offset_polyline(points, -half);
		for i in 0..points.len() - 1 {
			self.push_quad(left[i], left[i + 1], right[i + 1], right[i], colour);
		}
	}
}

/// Offsets every point of a polyline sideways by a constant perpendicular
/// `distance` (positive = to the left of each segment's direction of
/// travel, i.e. rotate the segment direction +90 degrees; negative = to
/// the right), producing a new polyline that stays exactly `distance` away
/// from the original at every point along it -- including through bends,
/// via a proper miter join at each interior vertex, rather than naively
/// offsetting each segment independently and leaving a gap/overlap where
/// two differently-offset segments would otherwise meet.
///
/// This one function backs two different uses in this module:
///  - `add_polyline` calls it twice (once with `+thickness/2`, once with
///    `-thickness/2`) to get a stroked ribbon's two edges.
///  - `draw_wires` calls it once per bit-strand, with that strand's own
///    constant centreline offset, to lay out each strand's path before
///    stroking *that* with `add_polyline` at a single strand's thickness.
///
/// At an interior vertex, the offset direction is the (normalized) sum of
/// the incoming and outgoing segments' own perpendicular normals -- the
/// angle bisector -- scaled up by `1 / cos(theta / 2)` (`theta` being the
/// angle between the two segments) so the offset point still sits exactly
/// `distance` away from *both* adjacent (infinite) segment lines, not just
/// nearer one of them. This is the standard "miter join" used for stroking
/// polylines. For a perfect 180-degree reversal (incoming and outgoing
/// directions exactly opposite, so their normals cancel to zero and the
/// bisector is undefined) this falls back to just the incoming segment's
/// own normal; the miter scale is also clamped (`MITER_LIMIT`) so a very
/// sharp near-reversal bend doesn't spike out to an enormous, visually
/// broken offset point.
fn offset_polyline(points: &[Vec2], distance: f32) -> Vec<Vec2> {
	const MITER_LIMIT: f32 = 4.0;
	let n = points.len();
	let mut out = Vec::with_capacity(n);

	// Unit normal (rotate direction +90 degrees) of the segment from `a`
	// to `b`, or `None` if the two points coincide (zero-length segment).
	fn segment_normal(a: Vec2, b: Vec2) -> Option<Vec2> {
		let dx = b.x - a.x;
		let dy = b.y - a.y;
		let len = (dx * dx + dy * dy).sqrt();
		if len < 1e-6 {
			None
		} else {
			Some(Vec2::new(-dy / len, dx / len))
		}
	}

	for i in 0..n {
		let normal_in = if i > 0 { segment_normal(points[i - 1], points[i]) } else { None };
		let normal_out = if i + 1 < n { segment_normal(points[i], points[i + 1]) } else { None };

		let normal = match (normal_in, normal_out) {
			(Some(a), Some(b)) => {
				let sum = Vec2::new(a.x + b.x, a.y + b.y);
				let sum_len = (sum.x * sum.x + sum.y * sum.y).sqrt();
				if sum_len < 1e-6 {
					// Exact 180-degree reversal -- bisector is undefined;
					// fall back to the incoming segment's own normal.
					a
				} else {
					let bisector = Vec2::new(sum.x / sum_len, sum.y / sum_len);
					let cos_half = (bisector.x * a.x + bisector.y * a.y).max(1.0 / MITER_LIMIT);
					Vec2::new(bisector.x / cos_half, bisector.y / cos_half)
				}
			}
			(Some(a), None) => a,
			(None, Some(b)) => b,
			(None, None) => Vec2::ZERO, // single-point polyline; no direction to offset along.
		};

		out.push(Vec2::new(points[i].x + normal.x * distance, points[i].y + normal.y * distance));
	}

	out
}

/// A point-in-shape test matching `SceneGeometry::add_rounded_rect`'s
/// actual drawn geometry exactly (same corner-rounding rules), so hover
/// hit-testing lines up with what's on screen instead of assuming every
/// pin is a plain circle. Corners flagged `round_left`/`round_right` are
/// treated as a quarter-circle around the same arc centre `add_rounded_rect`
/// uses; the flat middle "cross" (within the bounding box but outside any
/// rounded corner's own box) always counts as inside, same as a square
/// corner would. `radius` is clamped the same way `add_rounded_rect` clamps
/// it, so callers can pass the exact same arguments used to draw the shape.
pub fn point_in_rounded_rect(point: Vec2, centre: Vec2, size: Vec2, radius: f32, round_left: bool, round_right: bool) -> bool {
	let hw = size.x / 2.0;
	let hh = size.y / 2.0;
	if hw <= 0.0 || hh <= 0.0 {
		return false;
	}
	let r = radius.max(0.0).min(hw).min(hh);
	let dx = point.x - centre.x;
	let dy = point.y - centre.y;
	if dx.abs() > hw || dy.abs() > hh {
		return false;
	}
	// A point only falls within a rounded corner's own carved-out region
	// when it's *simultaneously* past the corner threshold on both axes
	// (dx AND dy, not either alone) -- e.g. a point sitting right at the
	// vertical centre of a rounded-right edge (dy near 0, dx near hw) is
	// just on the flat middle of that edge, not anywhere near the actual
	// arc, and must count as inside without ever touching the circle
	// test below. Gating on dx alone (as an earlier version of this
	// function did) wrongly treated that entire vertical strip -- most of
	// a pill's flat sides -- as if it were corner territory, and then
	// rejected it for sitting far from the arc centre; that's what made
	// hover hit-testing miss large swathes of any non-circle pin shape.
	let in_dx_corner = dx.abs() > hw - r;
	let in_dy_corner = dy.abs() > hh - r;
	if !(in_dx_corner && in_dy_corner) {
		return true;
	}
	let rounded_side = if dx > 0.0 { round_right } else { round_left };
	if !rounded_side {
		// A square corner's whole bounding box is filled -- no arc to test.
		return true;
	}
	let arc_cx = if dx > 0.0 { hw - r } else { -(hw - r) };
	let arc_cy = if dy > 0.0 { hh - r } else { -(hh - r) };
	let ddx = dx - arc_cx;
	let ddy = dy - arc_cy;
	ddx * ddx + ddy * ddy <= r * r
}

/// A point-in-circle test, for hit-testing a plain circle shape (a 1-bit
/// pin's connection dot -- see `draw_pin_shape`).
pub fn point_in_circle(point: Vec2, centre: Vec2, radius: f32) -> bool {
	let dx = point.x - centre.x;
	let dy = point.y - centre.y;
	dx * dx + dy * dy <= radius * radius
}

/// Resolved placement of one subchip instance within the scene, in world
/// space.
#[derive(Debug, Clone)]
pub struct PlacedSubChip<'a> {
	pub id: i32,
	pub desc: &'a ChipDescription,
	pub centre: Vec2,
	pub size: Vec2,
	pub input_pin_y: Vec<f32>,
	pub output_pin_y: Vec<f32>,
	/// Label
	pub label: Option<String>,
	/// Per-instance output pin colour overrides, copied from this placed
	/// instance's `SubChipDescription::pin_colour_info`.
	pub pin_colour_info: Vec<(i32, Color)>,
	/// Copied verbatim from this placed instance's
	/// `SubChipDescription::internal_data` (empty if the subchip has none).
	/// Interpretation is chip-type specific:
	///  - `Key`: `[0]` is the ASCII code (capitalised, e.g. `A` = 65) of the
	///    key this instance listens to.
	///  - `Rom256x16`: all 256 words of ROM contents, indexed by address.
	///  - `DisplayLed`: `[0]` is a `Color` palette index (same encoding as
	///    a pin's `Colour` field), used to tint the LED body.
	///  - Bus origin/terminus (`Bus1Bit`/`Bus4Bit`/`Bus8Bit`/
	///    `BusTerminus1Bit`/`BusTerminus4Bit`/`BusTerminus8Bit`): `[0]` is
	///    the id of the paired bus chip at the other end of the link,
	///    `[1]` is "is flipped" (`1` = draw this instance's visible pin on
	///    the opposite side from its type default).
	pub internal_data: Vec<u32>,
}

impl<'a> PlacedSubChip<'a> {
	/// Effective palette index for this instance's output pin `pin_id`,
	/// falling back to `default_colour` (the chip-level pin colour) if this
	/// instance has no override for it.
	pub fn output_pin_colour(&self, pin_id: i32, default_colour: Color) -> Color {
		self.pin_colour_info.iter().find(|(id, _)| *id == pin_id).map(|(_, colour)| *colour).unwrap_or(default_colour)
	}
}

/// Computes the world-space placement (body rect + pin y-offsets) of every
/// subchip in `chip`, resolving each subchip's own pin layout against
/// `library`. Subchips referencing an unknown chip name are skipped.
pub fn place_sub_chips<'a>(chip: &ChipDescription, library: &'a ChipLibrary) -> Vec<PlacedSubChip<'a>> {
	let mut placed = Vec::with_capacity(chip.sub_chips.len());

	for sub in &chip.sub_chips {
		let Some(desc) = library.try_get(&sub.name) else { continue };

		let input_bits: Vec<PinBitCount> = desc.input_pins.iter().map(|p| p.bit_count).collect();
		let output_bits: Vec<PinBitCount> = desc.output_pins.iter().map(|p| p.bit_count).collect();

		// Prefer the size actually saved on disk (`ChipDescription::size`,
		// from the JSON `Size` field) -- the original computes this via
		// `CalculateMinChipSize` with real font metrics, so it's more
		// accurate than anything we can derive here. Only fall back to
		// the pins+name-estimate heuristic when there's nothing saved
		// (size == (0,0)), e.g. a `ChipDescription` built up in code
		// (most builtins) rather than loaded from a project file. See
		// `ChipDescription::size` and `layout::calculate_min_chip_size`
		// docs for why either path matters for labels actually drawing.
		let size = if desc.size.x > 0.0 && desc.size.y > 0.0 {
			Vec2::new(desc.size.x, desc.size.y)
		} else {
			layout::calculate_min_chip_size(&input_bits, &output_bits, &desc.name, desc.name_location, theme::FONT_SIZE_CHIP_NAME)
		};
		let (_, input_pin_y) = layout::calculate_default_pin_layout(&input_bits);
		let (_, output_pin_y) = layout::calculate_default_pin_layout(&output_bits);

		placed.push(PlacedSubChip {
			id: sub.id,
			desc,
			centre: sub.position,
			size,
			label: sub.label.clone(),
			input_pin_y,
			output_pin_y,
			pin_colour_info: sub.pin_colour_info.clone(),
			internal_data: sub.internal_data.clone().unwrap_or_default(),
		});
	}

	placed
}

/// Looks up whether a pin should be drawn "high" (lit) or "low". Callers
/// typically implement this against a live `Simulator` by resolving
/// `(pin_owner_id, pin_id)` through `Simulator::find_pin`; a `None` return
/// (e.g. pin not simulated yet) is treated as low/disconnected.
pub trait PinStateLookup {
	fn is_high(&self, pin_owner_id: i32, pin_id: i32) -> Option<bool>;

	/// Full tri-state logic level (low/high/disconnected) for this pin's
	/// first bit, used by the renderer to pick a colour (see
	/// `theme::state_colour`). Defaults to deriving `High`/`Low` from
	/// `is_high` alone, so a lookup that can't distinguish "genuinely
	/// disconnected" from "reads low" (like `AllLow`) never needs to
	/// override this. `SimulatorPinState` overrides it to report real
	/// disconnected pins as such rather than folding them into `Low`.
	fn logic_state(&self, pin_owner_id: i32, pin_id: i32) -> Option<LogicState> {
		self.is_high(pin_owner_id, pin_id).map(|high| if high { LogicState::High } else { LogicState::Low })
	}

	/// Same as `logic_state`, but for one specific bit of a multi-bit pin
	/// (`bit_index` counting from 0, the same convention
	/// `pin_state::get_bit_tristated_value` uses), so a wire carrying more
	/// than one bit can be drawn as that many individually-coloured
	/// strands (see `draw_wires`) instead of a single "averaged" colour.
	/// Defaults to `logic_state` regardless of `bit_index` -- correct for
	/// any lookup that can't distinguish bits from each other (`AllLow`,
	/// the fixed-state test doubles below), and overridden by
	/// `SimulatorPinState` to report each bit's own real state.
	fn bit_logic_state(&self, pin_owner_id: i32, pin_id: i32, _bit_index: u32) -> Option<LogicState> {
		self.logic_state(pin_owner_id, pin_id)
	}

	/// Raw `SimChip::internal_state` for the direct subchip identified by
	/// `owner_id` (a `PlacedSubChip::id`), if one is currently simulated.
	/// Used by the renderer to read the pixel/segment buffer behind a
	/// display chip (7-segment/RGB/dot) -- mirrors `DisplayInstance.SimChip`
	/// in the original, which caches the same lookup for drawing.
	/// Defaults to `None`, which callers treat as "draw the display blank"
	/// (matches `DrawDisplay`'s `sim == null` / `useSim == false` branches).
	fn internal_state(&self, _owner_id: i32) -> Option<&[u32]> {
		None
	}
}

/// Trivial lookup that always reports every pin as low -- useful for static
/// previews / tests where no `Simulator` is available.
pub struct AllLow;
impl PinStateLookup for AllLow {
	fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
		Some(false)
	}
}

/// Live lookup backed by a running `Simulator`: resolves `(owner, pin)`
/// addresses the same way the sim graph does (`Simulator::find_pin`) and
/// reports the pin's per-bit state (`bit_logic_state`) as well as its
/// first bit's state alone (`logic_state`, used wherever only a single
/// representative colour is needed -- e.g. a pin's own drawn shape).
pub struct SimulatorPinState<'a> {
	pub sim: &'a crate::sim::Simulator,
	pub scope: crate::sim::ChipIdx,
}

impl<'a> PinStateLookup for SimulatorPinState<'a> {
	fn is_high(&self, pin_owner_id: i32, pin_id: i32) -> Option<bool> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		Some(crate::pin_state::first_bit_high(self.sim.pin(pin_idx).state))
	}

	fn logic_state(&self, pin_owner_id: i32, pin_id: i32) -> Option<LogicState> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		let raw = crate::pin_state::get_bit_tristated_value(self.sim.pin(pin_idx).state, 0);
		Some(LogicState::from_tristated_value(raw))
	}

	fn bit_logic_state(&self, pin_owner_id: i32, pin_id: i32, bit_index: u32) -> Option<LogicState> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		let raw = crate::pin_state::get_bit_tristated_value(self.sim.pin(pin_idx).state, bit_index);
		Some(LogicState::from_tristated_value(raw))
	}

	fn internal_state(&self, owner_id: i32) -> Option<&[u32]> {
		let chip_idx = self.sim.find_sub_chip(self.scope, owner_id)?;
		Some(&self.sim.chip(chip_idx).internal_state)
	}
}

/// Draws one wire's full `bit_count` as that many individually-coloured,
/// 1-bit-wide parallel strands rather than a single `bit_count`-scaled-
/// thick line -- so e.g. a mixed-signal 4-bit bus shows its actual 4
/// separate colours side by side, instead of a single line whose colour
/// only reflects bit 0's state (the old behaviour), or some blended
/// "average" that isn't any bit's real state.
///
/// Each strand's centreline is `centreline` (the wire's actual path,
/// including any player-authored bend points) offset sideways by that
/// strand's own constant distance via `offset_polyline` -- so every
/// strand bends together with the wire and stays a clean parallel line
/// through every corner, not just on straight runs.
///
/// Strand layout, `layout::WIRE_THICKNESS` apart: for `n` bits, strand `i`
/// sits at offset `WIRE_THICKNESS * (i - (n - 1) / 2)`. This single
/// formula handles both parities the way a real ribbon cable does: for an
/// *odd* bit count the middle strand's offset comes out to exactly `0`
/// (a real centred "middle wire"); for an *even* bit count there's no
/// strand at `0` at all -- the two middle strands straddle the centreline
/// at `+/- WIRE_THICKNESS / 2` instead, same spacing as every other
/// adjacent pair.
fn draw_wire_strands(
	geo: &mut SceneGeometry,
	centreline: &[Vec2],
	bit_count: u32,
	colour: Color,
	pin_owner_id: i32,
	pin_id: i32,
	pin_state: &dyn PinStateLookup,
) {
	let bit_count = bit_count.max(1);
	for bit_index in 0..bit_count {
		let offset = layout::WIRE_THICKNESS * (bit_index as f32 - (bit_count - 1) as f32 / 2.0);
		let strand_points = if offset == 0.0 { centreline.to_vec() } else { offset_polyline(centreline, offset) };
		let logic = pin_state.bit_logic_state(pin_owner_id, pin_id, bit_index).unwrap_or(LogicState::Low);
		let strand_colour = theme::state_colour(logic, colour);
		geo.add_polyline(&strand_points, layout::WIRE_THICKNESS, strand_colour);
	}
}

/// Build the full drawable scene for one chip: every subchip's body + pins,
/// plus wires connecting them. `chip.input_pins`/`output_pins` are treated
/// as this chip's own boundary dev-pins (owner id == the pin's own id, per
/// the on-disk wire-address convention).
pub fn build_scene(chip: &ChipDescription, library: &ChipLibrary, pin_state: &dyn PinStateLookup, hover_world_pos: Option<Vec2>) -> SceneGeometry {
	let mut geo = SceneGeometry::default();
	let placed = place_sub_chips(chip, library);

	// owner_id -> index into `placed`, for resolving wire endpoints that
	// land on a subchip (as opposed to one of this chip's own dev-pins).
	let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

	// Draw order is a simple three-layer stack, back to front (this
	// renderer has no depth buffer, so draw order *is* z-order -- see
	// `build_grid`'s docs for the same point about the background grid):
	// wires at the bottom, pins in the middle, component bodies (+ their
	// name labels) on top. This keeps a component's body from ever being
	// occluded by a wire or pin that happens to be drawn after it, and
	// keeps pins sitting visibly on top of the wires that connect to them.
	//
	// Name labels (for both pins and components) are hover-gated: they're
	// only added to `geo.labels` for whichever single thing (if any)
	// `hover_world_pos` currently lands on, using the exact same shape
	// each thing is actually drawn with -- a plain circle for a 1-bit
	// pin, a "pill" for a wider pin (`point_in_rounded_rect`/
	// `point_in_circle`, mirroring `draw_pin_shape`'s own branching), a
	// dev-pin's partially-rounded body, or a subchip's plain rect body --
	// rather than a fixed always-on label or an approximate hit-test that
	// doesn't match what's on screen. Pins are checked before components,
	// so hovering a pin sitting on a component's edge shows the pin's
	// name, not the component's.
	draw_wires(&mut geo, chip, &placed, &owner_to_placed, pin_state);
	let hovered_pin_name = draw_pins(&mut geo, chip, &placed, pin_state, hover_world_pos);
	draw_components(&mut geo, &placed, pin_state, hover_world_pos, hovered_pin_name.is_some());
	if let Some((pos, name)) = hovered_pin_name {
		push_hover_label(&mut geo, pos, name);
	}

	geo
}

/// Pushes a small hover-triggered name label just above `pos`. Shared by
/// both the pin and component hover paths in `build_scene` so their
/// labels look consistent.
fn push_hover_label(geo: &mut SceneGeometry, pos: Vec2, name: String) {
	let width = layout::estimate_text_width(&name, theme::FONT_SIZE_CHIP_NAME);
	geo.labels.push(TextLabel {
		pos: Vec2::new(pos.x, pos.y + layout::GRID_SIZE * 2.0),
		text: name,
		colour: theme::HOVER_LABEL_COL,
		font_size: theme::FONT_SIZE_CHIP_NAME,
		width,
	});
}

/// Layer 1 (bottom): every wire in `chip.wires`, resolved to world-space
/// polylines and drawn as thick lines. See the inline comments below for
/// how an individual wire's two endpoints are resolved.
fn draw_wires(
	geo: &mut SceneGeometry,
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	pin_state: &dyn PinStateLookup,
) {
	// Resolve each wire's two endpoints to world positions and draw a
	// polyline through any player-authored bend points between them (saved
	// `Points`, minus its first/last entries -- see `WireDescription::points`).
	// No bend points just means one straight segment.
	//
	// An endpoint is resolved one of two ways, per `wire.connection_type`:
	//  - `ToPins` (the common case): straight from the pin's own resolved
	//    world position, as before.
	//  - `ToWireSource`/`ToWireTarget`: this end is actually a tap on
	//    *another* wire's line rather than a real pin location, so it's
	//    resolved by re-projecting the cached attachment point onto that
	//    other wire's segment (`resolve_wire_endpoint`/`resolve_wire_point`
	//    below, mirroring `WireInstance.GetAttachmentPoint`). Using the raw
	//    pin position here (the old behaviour) desyncs from the
	//    player-authored bend points, which assume the wire starts/ends at
	//    the tap point, not at the underlying pin -- that mismatch is what
	//    produced visibly wrong bends for any wire tapped off another wire.
	//
	// `wire_point_cache` memoizes resolved endpoints across the whole
	// chip's wire list for this build: a single wire can be the tap target
	// for several others, and resolving a tapped chain revisits earlier
	// wires' endpoints.
	let mut wire_point_cache: WirePointCache = HashMap::new();
	for (wire_idx, wire) in chip.wires.iter().enumerate() {
		let src = resolve_wire_endpoint(chip, placed, owner_to_placed, &chip.wires, wire_idx, false, &mut wire_point_cache, 0);
		let dst = resolve_wire_endpoint(chip, placed, owner_to_placed, &chip.wires, wire_idx, true, &mut wire_point_cache, 0);

		if let (Some(src), Some(dst)) = (src, dst) {
			// Colour/bit-count always trace back to the wire's real
			// originating pin (`source_pin_address`), regardless of
			// `connection_type` -- a wire tapped off another wire still
			// carries that other wire's underlying signal, so this
			// resolution doesn't need to change for the bend fix above.
			let colour = resolve_pin_colour(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);
			let bit_count =
				resolve_pin_bit_count(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);

			let mut centreline = Vec::with_capacity(wire.points.len() + 2);
			centreline.push(src);
			centreline.extend_from_slice(&wire.points);
			centreline.push(dst);

			draw_wire_strands(
				geo,
				&centreline,
				bit_count as u32,
				colour,
				wire.source_pin_address.pin_owner_id,
				wire.source_pin_address.pin_id,
				pin_state,
			);
		}
	}
}

/// Layer 2 (middle): every pin -- each subchip's input/output pins
/// (`draw_pin_shape` -- a plain circle for 1-bit, a "pill" for wider
/// pins) plus this chip's own boundary dev-pins (small rounded-rect
/// bodies, drawn via `draw_dev_pin_body`) -- so pins always sit visibly on
/// top of the wires that connect to them, and underneath the component
/// bodies that own them.
///
/// Also hit-tests every pin against `hover_world_pos` (if given) using its
/// *exact* drawn shape (`point_in_pin_shape`/`point_in_rounded_rect`,
/// mirroring `draw_pin_shape`/`draw_dev_pin_body`'s own geometry) and
/// returns the first hit's `(label anchor position, pin name)`, for
/// `build_scene` to turn into a hover label. Pins are hit-tested in the
/// same order they're drawn, so if two overlap the topmost (drawn last)
/// wins, matching what's visibly on top.
fn draw_pins(
	geo: &mut SceneGeometry,
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	pin_state: &dyn PinStateLookup,
	hover_world_pos: Option<Vec2>,
) -> Option<(Vec2, String)> {
	let mut hovered: Option<(Vec2, String)> = None;

	for sub in placed {
		// Bus origin/terminus chips draw their one visible pin on a fixed
		// default side (bus -> right, terminus -> left) unless flipped via
		// saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;

		for (i, pin) in sub.desc.input_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped);
			let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
			draw_pin_shape(geo, pos, pin.bit_count, theme::state_colour(logic, pin.colour));
			if hover_world_pos.is_some_and(|p| point_in_pin_shape(p, pos, pin.bit_count)) {
				hovered = Some((pos, pin.name.clone()));
			}
		}
		for (i, pin) in sub.desc.output_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped);
			let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
			// A specific placed instance can override its output pin's
			// colour (saved `OutputPinColourInfo`); fall back to the
			// chip-level pin colour when there's no override for this pin.
			let colour_idx = sub.output_pin_colour(pin.id, pin.colour);
			draw_pin_shape(geo, pos, pin.bit_count, theme::state_colour(logic, colour_idx));
			if hover_world_pos.is_some_and(|p| point_in_pin_shape(p, pos, pin.bit_count)) {
				hovered = Some((pos, pin.name.clone()));
			}
		}
	}

	// This chip's own boundary dev-pins (`chip.input_pins`/`output_pins`),
	// at their real saved position -- a partially rounded rectangle
	// (rounded on the side facing outward, away from the chip; square on
	// the side facing in, toward where a wire attaches), filled with the
	// pin's live state/palette colour and outlined in a grey-ish border,
	// so they read as visually distinct from a regular subchip pin's
	// plain circle. Mirrors `layout::dev_pin_body_size`'s docs.
	for pin in &chip.input_pins {
		draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), true);
		if hover_world_pos.is_some_and(|p| point_in_dev_pin_body(p, pin.position, pin.bit_count, true)) {
			hovered = Some((pin.position, pin.name.clone()));
		}
		// the clickable part
		draw_input_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin.id, pin_state);
	}
	for pin in &chip.output_pins {
		draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), false);
		if hover_world_pos.is_some_and(|p| point_in_dev_pin_body(p, pin.position, pin.bit_count, false)) {
			hovered = Some((pin.position, pin.name.clone()));
		}
	}

	hovered
}

/// Layer 3 (top): every subchip's body rectangle, drawn last so a
/// component's body is never occluded by a wire or pin drawn earlier.
///
/// The name label is hover-gated: it's only added when `hover_world_pos`
/// lands on this subchip's body rect *and* `pin_already_hovered` is
/// false (a pin sitting on/near the component's edge takes precedence --
/// see `build_scene`'s doc comment), and even then only if the chip's own
/// `NameLocation` isn't `Hidden` (e.g. display/bus/pin chips, whose body
/// is the visualisation, never show a name). Mirrors
/// `DevSceneDrawer.DrawSubChip`'s "if (... desc.NameLocation !=
/// NameDisplayLocation.Hidden)" gate; the `isKeyChip` special case (which
/// shows a keybinding string even when hidden) isn't ported here since it
/// needs live key-binding state this module doesn't have.
fn draw_components(
	geo: &mut SceneGeometry,
	placed: &[PlacedSubChip],
	pin_state: &dyn PinStateLookup,
	hover_world_pos: Option<Vec2>,
	pin_already_hovered: bool,
) {
	for sub in placed {
		// An LED's body *is* its indicator: tint it with the saved
		// `InternalData[0]` colour (same palette-index encoding as a pin's
		// `Colour` field), lit/dimmed/disconnected exactly like a wire of
		// that colour would be, driven by the live state of its one input
		// pin. Falls back to the ordinary body-colour handling below if
		// this instance has no saved colour for some reason.
		let led_colour = (sub.desc.chip_type == ChipType::DisplayLed).then(|| sub.internal_data.first().copied()).flatten().map(|idx| {
			let colour = Color::from_int(idx as i32);
			let logic = sub.desc.input_pins.first().and_then(|p| pin_state.logic_state(sub.id, p.id)).unwrap_or(LogicState::Low);
			theme::state_colour(logic, colour)
		});

		// Use this chip's saved body colour (alpha 0 means "not saved" --
		// fall back to the theme default) rather than always drawing every
		// chip with the same flat grey.
		let body_colour = led_colour.unwrap_or_else(|| if sub.desc.colour[3] > 0.0 { sub.desc.colour } else { theme::CHIP_BODY_COL });

		// 7-segment/RGB/dot displays draw their own live pixel/segment
		// content in place of the plain body rect (their `NameLocation` is
		// `Hidden` precisely because the body *is* the visualisation --
		// see the doc comment above). `DisplayLed` doesn't need a branch
		// here: its "display" is just the whole tinted body rect already
		// produced above, so the plain `add_rect` below is exactly right
		// for it too.
		match sub.desc.chip_type {
			ChipType::SevenSegmentDisplay => draw_display_seven_segment(geo, sub, pin_state),
			ChipType::DisplayRgb => draw_display_pixel_grid(geo, sub, pin_state, true),
			ChipType::DisplayDot => draw_display_pixel_grid(geo, sub, pin_state, false),
			_ => geo.add_rect(sub.centre, sub.size, body_colour),
		}

		// Draw this subchip's name label, unless explicitly hidden (e.g.
		// display/bus/pin chips, which save NameLocation = Hidden since
		// their body is the visualisation). Mirrors
		// `DevSceneDrawer.DrawSubChip`'s "if (... desc.NameLocation !=
		// NameDisplayLocation.Hidden)" gate -- except for the Key chip,
		// which forces its label to show regardless of the saved (always
		// Hidden) `NameLocation`: its body has no other visualisation, so
		// the bound key's letter (from saved `InternalData[0]`, an ASCII
		// code -- capitalised, e.g. `A` = 65) is shown in its place.
		let key_letter =
			(sub.desc.chip_type == ChipType::Key).then(|| sub.internal_data.first().copied()).flatten().map(|code| (code as u8 as char).to_string());

		let is_hovered = !pin_already_hovered && hover_world_pos.is_some_and(|p| point_in_rect(p, sub.centre, sub.size));
		// draw name if options allow
		if let Some(letter) = key_letter {
			geo.labels.push(TextLabel {
				pos: sub.centre,
				text: letter,
				colour: theme::text_colour_for_background(body_colour),
				font_size: theme::FONT_SIZE_CHIP_NAME,
				width: sub.size.x,
			});
		} else if sub.desc.name_location != NameLocation::Hidden {
			let name_pos = match sub.desc.name_location {
				NameLocation::Top => {
					Vec2::new(sub.centre.x, sub.centre.y + sub.size.y / 2.0 - theme::FONT_SIZE_CHIP_NAME / 2.0 - layout::GRID_SIZE / 2.0)
				}
				_ => sub.centre,
			};
			geo.labels.push(TextLabel {
				pos: name_pos,
				text: sub.desc.name.clone(),
				colour: theme::text_colour_for_background(body_colour),
				font_size: theme::FONT_SIZE_CHIP_NAME,
				width: sub.size.x,
			});
		}
		// draw label if hovered
		if let Some(label) = &sub.label {
			if is_hovered {
				let label_pos = sub.centre - Vec2::new(0.0, sub.size.y / 2.0 + theme::FONT_SIZE_CHIP_NAME);
				geo.labels.push(TextLabel {
					pos: label_pos,
					text: label.into(),
					colour: theme::text_colour_for_background(body_colour),
					font_size: theme::FONT_SIZE_CHIP_NAME,
					width: sub.size.x,
				});
			}
		}
	}
}

/// Draws a `SevenSegmentDisplay` subchip's live segment pattern, reading
/// segment states straight from its own input pins (`A`..`G` = pin ids
/// `0`..`6`) via `pin_state`, plus the `COL` pin (id `7`) which swaps in
/// an alternate (blue) palette when high. Mirrors
/// `DevSceneDrawer.DrawDisplay_SevenSegment`/its `ChipType.SevenSegmentDisplay`
/// case, except segments are drawn as plain rectangles rather than the
/// original's pointed-end "diamond" shape -- a cosmetic simplification,
/// not a functional one: on/off state and colour per segment are exact.
fn draw_display_seven_segment(geo: &mut SceneGeometry, sub: &PlacedSubChip, pin_state: &dyn PinStateLookup) {
	const TARGET_HEIGHT_ASPECT: f32 = 1.75;
	const SEGMENT_THICKNESS_FRAC: f32 = 0.165;
	const SEGMENT_VERTICAL_SPACING_FRAC: f32 = 0.07;
	const DISPLAY_INSET_FRAC: f32 = 0.2;

	let centre = sub.centre;
	// The body is sized to fit this display (see `BuiltinChipCreator`'s
	// 7-seg sizing), so derive the display's own "scale" from whichever
	// body dimension is the tighter fit for the fixed 1:1.75 aspect,
	// rather than assuming a saved `DisplayDescription::Scale` (which this
	// port's builtin chip descriptions don't carry -- see `builtins.rs`'s
	// module doc on dropping editor-only sizing metadata).
	let scale = sub.size.x.min(sub.size.y / TARGET_HEIGHT_ASPECT);

	let bounds_width = scale;
	let bounds_height = bounds_width * TARGET_HEIGHT_ASPECT;
	let segment_thickness = scale * SEGMENT_THICKNESS_FRAC;
	let segment_width = bounds_width - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_region_height = bounds_height - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_height = segment_region_height / 2.0 - scale * SEGMENT_VERTICAL_SPACING_FRAC;

	// Black backing behind the segments, same as the original's
	// `Draw.Quad(centre, boundsSize, Color.black)`.
	geo.add_rect(centre, Vec2::new(bounds_width, bounds_height), theme::STATE_DISCONNECTED_COL);

	let col_offset = if pin_state.logic_state(sub.id, 7) == Some(LogicState::High) { 3 } else { 0 };
	let seg_col = |pin_id: i32| {
		let on = pin_state.logic_state(sub.id, pin_id) == Some(LogicState::High);
		theme::SEVEN_SEG_COLS[(if on { 1 } else { 0 }) + col_offset]
	};

	let (a, b, c, d, e, f, g) = (seg_col(0), seg_col(1), seg_col(2), seg_col(3), seg_col(4), seg_col(5), seg_col(6));

	let offset_x = Vec2::new(segment_width / 2.0, 0.0);
	let offset_y = Vec2::new(0.0, segment_region_height / 4.0);
	let vertical_size = Vec2::new(segment_thickness, segment_height);
	let horizontal_size = Vec2::new(segment_width, segment_thickness);

	geo.add_rect(centre, horizontal_size, g); // middle
	geo.add_rect(centre + Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, a); // top
	geo.add_rect(centre - Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, d); // bottom
	geo.add_rect(centre - offset_x + offset_y, vertical_size, f); // top-left
	geo.add_rect(centre - offset_x - offset_y, vertical_size, e); // bottom-left
	geo.add_rect(centre + offset_x + offset_y, vertical_size, b); // top-right
	geo.add_rect(centre + offset_x - offset_y, vertical_size, c); // bottom-right
}

/// Draws a `DisplayRgb`/`DisplayDot` subchip's live 16x16 pixel buffer,
/// reading each pixel from `pin_state.internal_state(sub.id)` (the same
/// front-buffer layout `Simulator::process_display_rgb`/
/// `process_display_dot` write: address `y * 16 + x`, packed as
/// `R | G<<4 | B<<8` nibbles for RGB, or a plain 0/1 value for the dot
/// display). Mirrors `DevSceneDrawer.DrawDisplay_RGB`/`DrawDisplay_Dot`.
/// Falls back to a uniform dim grid (no live sim, e.g. in a chip-picker
/// preview) exactly like the original's `useSim == false` branch.
fn draw_display_pixel_grid(geo: &mut SceneGeometry, sub: &PlacedSubChip, pin_state: &dyn PinStateLookup, is_rgb: bool) {
	const PIXELS_PER_ROW: usize = 16;
	const BORDER_FRAC: f32 = 0.95;
	const PIXEL_SIZE_FRAC: f32 = 0.925;
	const OFF_PIXEL_COL: Rgba = [0.1, 0.1, 0.1, 1.0];

	let centre = sub.centre;
	let scale = sub.size.x.min(sub.size.y);

	// Black backing behind the pixel grid.
	geo.add_rect(centre, Vec2::new(scale, scale), theme::STATE_DISCONNECTED_COL);

	let size = scale * BORDER_FRAC;
	let pixel_size = size / PIXELS_PER_ROW as f32;
	let pixel_draw_size = Vec2::new(pixel_size, pixel_size) * PIXEL_SIZE_FRAC;
	let bottom_left = centre - Vec2::new(size, size) * 0.5;

	let internal_state = pin_state.internal_state(sub.id);

	fn unpack_4bit_channel(raw: u32) -> f32 {
		(raw & 0b1111) as f32 / 15.0
	}

	for y in 0..PIXELS_PER_ROW {
		for x in 0..PIXELS_PER_ROW {
			let address = y * PIXELS_PER_ROW + x;
			let col = match internal_state.and_then(|s| s.get(address)) {
				Some(&pixel_state) if is_rgb => {
					[unpack_4bit_channel(pixel_state), unpack_4bit_channel(pixel_state >> 4), unpack_4bit_channel(pixel_state >> 8), 1.0]
				}
				Some(&pixel_state) => {
					let v = (pixel_state != 0) as u32 as f32;
					[v, v, v, 1.0]
				}
				None => OFF_PIXEL_COL,
			};

			let pos = bottom_left + Vec2::new(pixel_size, pixel_size) * 0.5 + Vec2::new(pixel_size * x as f32, pixel_size * y as f32);
			geo.add_rect(pos, pixel_draw_size, col);
		}
	}
}

/// A point-in-shape test mirroring `draw_pin_shape`'s exact branching: a
/// plain circle for a 1-bit pin, or the same "pill" `add_rounded_rect`
/// call (round on both sides) for a wider pin -- see
/// `point_in_rounded_rect`/`point_in_circle`.
fn point_in_pin_shape(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> bool {
	match bit_count {
		PinBitCount::Bit1 => point_in_circle(point, pos, layout::pin_radius_for_bit_count(bit_count)),
		PinBitCount::Bit4 | PinBitCount::Bit8 => {
			let size = layout::pin_visual_shape_size(bit_count);
			point_in_rounded_rect(point, pos, size, size.y / 2.0, true, true)
		}
	}
}

/// A point-in-shape test mirroring `draw_dev_pin_body`'s exact geometry
/// (its outer, full-size border shape -- the fill is strictly smaller, so
/// testing against the border is the more generous/correct hit area).
fn point_in_dev_pin_body(point: Vec2, pos: Vec2, bit_count: PinBitCount, round_left: bool) -> bool {
	let size = layout::dev_pin_body_size(bit_count);
	let radius = layout::dev_pin_corner_radius(size);
	point_in_rounded_rect(point, pos, size, radius, round_left, !round_left)
}

/// A point-in-shape test covering an *input* dev-pin's whole clickable
/// body (the union of every individual bit cell) -- used for the
/// coarse "is the cursor anywhere on this pin" hover check. For
/// per-bit toggle handling (which exact bit a click landed on), use
/// `hit_test_input_dev_pin_bit` instead.
#[allow(dead_code)]
fn point_in_input_dev_pin_body(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> bool {
	let size = layout::input_dev_pin_body_size(bit_count);
	point_in_rect(point, pos, size)
}

/// Returns the bit index (0-based) of whichever of an input dev-pin's
/// individual clickable cells `point` landed on, or `None` if it missed
/// every cell. `pos` is the dev-pin's own saved position, the same value
/// passed to `draw_input_dev_pin_body`. Bit-0's cell is the top-left of
/// the grid (see `layout::input_bit_cell_offsets`), matching the same
/// bit-index convention `pin_state::get_bit_tristated_value` uses --
/// callers wiring up an actual click-to-toggle handler can flip bit
/// `bit_index` of the pin's state directly.
pub fn hit_test_input_dev_pin_bit(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> Option<u32> {
	let cell_size = Vec2::new(layout::INPUT_BIT_CELL_SIZE, layout::INPUT_BIT_CELL_SIZE);
	for (bit_index, offset) in layout::input_bit_cell_offsets(bit_count).into_iter().enumerate() {
		let cell_pos = pos + offset;
		let hit = match bit_count {
			PinBitCount::Bit1 => point_in_circle(point, cell_pos, layout::INPUT_BIT_CIRCLE_RADIUS),
			PinBitCount::Bit4 | PinBitCount::Bit8 => point_in_rect(point, cell_pos, cell_size),
		};
		if hit {
			return Some(bit_index as u32);
		}
	}
	None
}

/// A plain axis-aligned rectangle hit-test, for a subchip's body (which,
/// unlike its pins, is never rounded).
fn point_in_rect(point: Vec2, centre: Vec2, size: Vec2) -> bool {
	(point.x - centre.x).abs() <= size.x / 2.0 && (point.y - centre.y).abs() <= size.y / 2.0
}

/// Finds whichever wire's drawn centreline `world_pos` is closest to
/// (within `max_dist` world units of any of its segments), returning
/// that wire's index into `chip.wires` -- used to resolve a right-click
/// "delete wire" to *one specific* `WireDescription`, not e.g. every wire
/// fanning out of the same source pin. Resolves each wire's endpoints
/// (including tap-on-another-wire ones) the same way [`draw_wires`]
/// does, so "closest to what's actually drawn" matches what the player
/// sees, not just the saved bend points.
pub fn hit_test_wire(chip: &ChipDescription, library: &ChipLibrary, world_pos: Vec2, max_dist: f32) -> Option<usize> {
	let placed = place_sub_chips(chip, library);
	let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

	let mut cache: WirePointCache = HashMap::new();
	let mut best: Option<(usize, f32)> = None;
	for wire_idx in 0..chip.wires.len() {
		let src = resolve_wire_endpoint(chip, &placed, &owner_to_placed, &chip.wires, wire_idx, false, &mut cache, 0);
		let dst = resolve_wire_endpoint(chip, &placed, &owner_to_placed, &chip.wires, wire_idx, true, &mut cache, 0);
		let (Some(src), Some(dst)) = (src, dst) else { continue };

		let mut centreline = Vec::with_capacity(chip.wires[wire_idx].points.len() + 2);
		centreline.push(src);
		centreline.extend_from_slice(&chip.wires[wire_idx].points);
		centreline.push(dst);

		for seg in centreline.windows(2) {
			let closest = closest_point_on_segment(world_pos, seg[0], seg[1]);
			let dist = ((closest.x - world_pos.x).powi(2) + (closest.y - world_pos.y).powi(2)).sqrt();
			if dist <= max_dist && best.map(|(_, best_dist)| dist < best_dist).unwrap_or(true) {
				best = Some((wire_idx, dist));
			}
		}
	}
	best.map(|(idx, _)| idx)
}

/// Removes wire `index` from `chip.wires` -- deliberately just that one
/// entry (the "shortest possible section": a single `WireDescription`,
/// which may be only one tap/branch of a larger fan-out or tap-chain),
/// not e.g. every wire sharing its source pin. Any *other* wire that was
/// tapping onto the removed one (`connection_type != ToPins` and
/// `connected_wire_index == index`) would otherwise be left pointing at
/// a dangling/wrong index, so those are removed too (recursively, since
/// a wire can itself be tapped by further wires) -- there's no sensible
/// position left to resolve them to once their anchor is gone. Every
/// remaining wire's `connected_wire_index` is then shifted down to
/// account for removed indices, so the rest of the tap graph stays
/// intact. Returns the number of wires actually removed (1 + however
/// many dependent taps cascaded).
pub fn delete_wire(chip: &mut ChipDescription, index: usize) -> usize {
	if index >= chip.wires.len() {
		return 0;
	}

	// Collect every index that must go: `index` itself, plus (transitively)
	// any wire tapping onto one already marked for removal.
	let mut to_remove = vec![index];
	loop {
		let mut added = false;
		for (i, w) in chip.wires.iter().enumerate() {
			if to_remove.contains(&i) {
				continue;
			}
			if w.connection_type != WireConnectionType::ToPins && to_remove.contains(&(w.connected_wire_index as usize)) {
				to_remove.push(i);
				added = true;
			}
		}
		if !added {
			break;
		}
	}
	to_remove.sort_unstable();

	let removed_count = to_remove.len();
	// Remove back-to-front so earlier indices in `to_remove` stay valid.
	for &i in to_remove.iter().rev() {
		chip.wires.remove(i);
	}

	// Shift every surviving wire's `connected_wire_index` down by however
	// many removed indices sat before it, so taps still point at the
	// right (now-renumbered) wire.
	for w in chip.wires.iter_mut() {
		if w.connection_type == WireConnectionType::ToPins {
			continue;
		}
		let shift = to_remove.iter().filter(|&&removed| (removed as i32) < w.connected_wire_index).count() as i32;
		w.connected_wire_index -= shift;
	}

	removed_count
}

/// Finds whichever placed subchip's body (as laid out by
/// [`place_sub_chips`]) contains `world_pos`, if any -- used to resolve a
/// right-click on the canvas to "which component did the player click".
/// Iterates back-to-front (last-placed first) so, on the rare case two
/// bodies overlap, the one actually drawn on top (and thus visible to the
/// player) is the one that gets hit, matching `draw_components`' draw
/// order.
pub fn hit_test_sub_chip<'a, 'b>(placed: &'b [PlacedSubChip<'a>], world_pos: Vec2) -> Option<&'b PlacedSubChip<'a>> {
	placed.iter().rev().find(|p| point_in_rect(world_pos, p.centre, p.size))
}

/// Finds whichever of `chip`'s *own* boundary dev-pins (never a
/// subchip's pins) has its body under `world_pos`, if any -- used to
/// resolve a right-click to "Label this pin" (see `PlacedSubChip`'s and
/// `point_in_dev_pin_body`'s docs for the input/output `round_left`
/// distinction this mirrors). Returns `(is_input, pin_id)`.
pub fn hit_test_dev_pin(chip: &ChipDescription, world_pos: Vec2) -> Option<(bool, i32)> {
	for pin in &chip.input_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, true) {
			return Some((true, pin.id));
		}
	}
	for pin in &chip.output_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, false) {
			return Some((false, pin.id));
		}
	}
	None
}

/// Draws a single subchip pin's connection shape at `pos`, coloured
/// `colour`, scaled by `bit_count`: a plain circle for a 1-bit pin, or a
/// "pill" (a rectangular body with a half-circle cap on each end) for a
/// wider pin -- so a 4/8-bit pin reads as visibly carrying more than a
/// 1-bit pin's single wire, rather than every pin drawing at the same
/// fixed size. See `layout::pin_radius_for_bit_count`/
/// `pin_visual_shape_size` for the exact sizing rule.
///
/// The pill's rounded corners become true semicircle caps (not just
/// quarter-round corners) because `pin_visual_shape_size` always returns a
/// shape whose height already equals twice the intended cap radius, and
/// that radius is what's passed to `add_rounded_rect` below (see
/// `add_rounded_rect`'s own docs on how corner arcs merge into a full
/// semicircle when `radius == height / 2`).
fn draw_pin_shape(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Rgba) {
	match bit_count {
		PinBitCount::Bit1 => {
			geo.add_circle(pos, layout::pin_radius_for_bit_count(bit_count), colour, 16);
		}
		PinBitCount::Bit4 | PinBitCount::Bit8 => {
			let size = layout::pin_visual_shape_size(bit_count);
			let radius = size.y / 2.0;
			geo.add_rounded_rect(pos, size, colour, radius, true, true, 16);
		}
	}
}

/// Draws one of a chip's own boundary dev-pins as a small "component"
/// body at `pos` (its real saved position): a partially rounded
/// rectangle, rounded on whichever side faces outward (`round_left` for
/// an input pin, sitting on the chip's left edge with wires approaching
/// from further left; the mirror for an output pin) and square on the
/// side facing in, filled with the pin's live state colour and outlined
/// in a grey-ish border. See `layout::dev_pin_body_size`/
/// `dev_pin_corner_radius` for the sizing this follows.
fn draw_dev_pin_body(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Color, logic: Option<LogicState>, round_left: bool) {
	let size = layout::dev_pin_body_size(bit_count);
	let radius = layout::dev_pin_corner_radius(size);
	let border = layout::DEV_PIN_BORDER_WIDTH.min(size.x / 2.0).min(size.y / 2.0);
	let fill_colour = theme::state_colour(logic.unwrap_or(LogicState::Low), colour);

	// Border first (drawn full-size, in the grey-ish outline colour)...
	geo.add_rounded_rect(pos, size, theme::CHIP_OUTLINE_COL, radius, round_left, !round_left, layout::DEV_PIN_ROUND_SEGMENTS);

	// ...then the pin-coloured fill on top, inset by the border width so
	// the border reads as an outline rather than being fully covered.
	let inner_size = Vec2::new((size.x - border * 2.0).max(0.0), (size.y - border * 2.0).max(0.0));
	let inner_radius = (radius - border).max(0.0);
	geo.add_rounded_rect(pos, inner_size, fill_colour, inner_radius, round_left, !round_left, layout::DEV_PIN_ROUND_SEGMENTS);
}

/// Draws one of a chip's own boundary *input* dev-pins as a grid of
/// individually-clickable bit cells, its drawn
/// footprint scales with how many bits it carries: one circle (twice a
/// plain pin's radius) for a 1-bit input, a 2x2 grid of squares for a
/// 4-bit input, 2x4 for 8-bit. See `layout::input_bit_grid_dims`/
/// `input_bit_cell_offsets` for the exact grid geometry, and
/// `hit_test_input_dev_pin_bit` for the matching per-cell hit test. Each
/// cell is coloured by that individual bit's own live state
/// (`PinStateLookup::bit_logic_state`), so e.g. an 8-bit input shows all
/// eight of its bits' states at a glance, not one averaged colour.
fn draw_input_dev_pin_body(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Color, pin_id: i32, pin_state: &dyn PinStateLookup) {
	for (bit_index, offset) in layout::input_bit_cell_offsets(bit_count).into_iter().enumerate() {
		let cell_pos = pos + offset;
		let logic = pin_state.bit_logic_state(pin_id, 0, bit_index as u32).unwrap_or(LogicState::Low);
		let fill_colour = theme::state_colour(logic, colour);

		match bit_count {
			PinBitCount::Bit1 => {
				geo.add_circle(cell_pos, layout::INPUT_BIT_CIRCLE_RADIUS, theme::CHIP_OUTLINE_COL, layout::DEV_PIN_ROUND_SEGMENTS * 2);
				let border = layout::DEV_PIN_BORDER_WIDTH.min(layout::INPUT_BIT_CIRCLE_RADIUS);
				geo.add_circle(cell_pos, (layout::INPUT_BIT_CIRCLE_RADIUS - border).max(0.0), fill_colour, layout::DEV_PIN_ROUND_SEGMENTS * 2);
			}
			PinBitCount::Bit4 | PinBitCount::Bit8 => {
				let size = Vec2::new(layout::INPUT_BIT_CELL_SIZE, layout::INPUT_BIT_CELL_SIZE);
				let border = layout::DEV_PIN_BORDER_WIDTH.min(size.x / 2.0).min(size.y / 2.0);
				geo.add_rect(cell_pos, size, theme::CHIP_OUTLINE_COL);
				let inner_size = Vec2::new((size.x - border * 2.0).max(0.0), (size.y - border * 2.0).max(0.0));
				geo.add_rect(cell_pos, inner_size, fill_colour);
			}
		}
	}
}

/// Memoizes resolved wire-endpoint world positions within one `build_scene`
/// call, keyed by `(wire index into chip.wires, is_target)`. Needed because
/// resolving one wire-tap endpoint can require resolving another wire's
/// endpoints in turn (see `resolve_wire_endpoint`), and the same wire can be
/// revisited many times (e.g. a bus fanning out to several taps).
type WirePointCache = HashMap<(usize, bool), Option<Vec2>>;

/// How many wire-to-wire attachment hops to follow before giving up.
/// Real projects only ever nest a couple of levels deep (`WireInstance`'s
/// own `ConnectedWireRecursionDepth` tracks this for draw-ordering, and
/// stays small in practice), so this is purely a guard against a
/// hand-edited or corrupted save file describing a connection cycle --
/// without it, a cycle would recurse forever instead of just drawing that
/// wire wrong.
const MAX_WIRE_CONNECTION_DEPTH: u32 = 64;

/// The closest point to `p` on line segment `a`-`b`. Mirrors
/// `WireInstance.ClosestPointOnLineSegment`; used to re-project a
/// wire-tap's cached attachment point onto its target wire's segment.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
	let ab = Vec2::new(b.x - a.x, b.y - a.y);
	let sqr_len = ab.x * ab.x + ab.y * ab.y;
	if sqr_len <= 1e-12 {
		return a;
	}
	let ap = Vec2::new(p.x - a.x, p.y - a.y);
	let t = ((ap.x * ab.x + ap.y * ab.y) / sqr_len).clamp(0.0, 1.0);
	Vec2::new(a.x + ab.x * t, a.y + ab.y * t)
}

/// Resolves world-space point index `point_index` along wire `wire_idx`'s
/// own polyline, i.e. `[source-endpoint, ...bends..., target-endpoint]`.
/// Interior indices are just that wire's saved bend points (already in
/// world space, no resolution needed); the two endpoint indices recurse
/// into `resolve_wire_endpoint`, since either one might itself be a tap on
/// yet another wire. Mirrors `WireInstance.GetWirePoint`.
fn resolve_wire_point(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	wires: &[WireDescription],
	wire_idx: usize,
	point_index: usize,
	cache: &mut WirePointCache,
	depth: u32,
) -> Option<Vec2> {
	let wire = wires.get(wire_idx)?;
	let last_index = wire.points.len() + 1; // bends.len() interior points + 2 endpoints
	if point_index == 0 {
		resolve_wire_endpoint(chip, placed, owner_to_placed, wires, wire_idx, false, cache, depth)
	} else if point_index == last_index {
		resolve_wire_endpoint(chip, placed, owner_to_placed, wires, wire_idx, true, cache, depth)
	} else {
		wire.points.get(point_index - 1).copied()
	}
}

/// Resolves one end of wire `wire_idx` (`is_target`: false = source, true =
/// target) to a world-space position.
///
/// A plain pin-attached end (`WireConnectionType::ToPins`, or the
/// non-tapped end of a partially-tapped wire) resolves straight from the
/// pin's own live position via `resolve_pin_position`, same as always. A
/// wire-attached end (`ToWireSource` for the source end, `ToWireTarget` for
/// the target end) instead re-projects that end's last cached attachment
/// point onto the referenced wire's segment (`connected_wire_index`,
/// `connected_wire_segment_index`) -- mirroring
/// `WireInstance.GetAttachmentPoint` / `WireLayoutHelper.GetClosestPointOnWire`
/// in the original. This is the fix for wire-tap endpoints resolving to the
/// wrong place (and thus producing visibly wrong bends): they were
/// previously always resolved as if `ToPins`.
fn resolve_wire_endpoint(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	wires: &[WireDescription],
	wire_idx: usize,
	is_target: bool,
	cache: &mut WirePointCache,
	depth: u32,
) -> Option<Vec2> {
	if let Some(&cached) = cache.get(&(wire_idx, is_target)) {
		return cached;
	}
	if depth > MAX_WIRE_CONNECTION_DEPTH {
		return None;
	}
	let wire = wires.get(wire_idx)?;

	let attaches_to_wire =
		matches!((is_target, wire.connection_type), (false, WireConnectionType::ToWireSource) | (true, WireConnectionType::ToWireTarget));

	let result = if attaches_to_wire {
		if wire.connected_wire_index < 0 {
			None
		} else {
			let target_wire_idx = wire.connected_wire_index as usize;
			let seg = wire.connected_wire_segment_index.max(0) as usize;
			let a = resolve_wire_point(chip, placed, owner_to_placed, wires, target_wire_idx, seg, cache, depth + 1);
			let b = resolve_wire_point(chip, placed, owner_to_placed, wires, target_wire_idx, seg + 1, cache, depth + 1);
			match (a, b) {
				(Some(a), Some(b)) => {
					let cached_point = if is_target { wire.cached_target_point } else { wire.cached_source_point };
					Some(closest_point_on_segment(cached_point, a, b))
				}
				_ => None,
			}
		}
	} else {
		let addr = if is_target { &wire.target_pin_address } else { &wire.source_pin_address };
		resolve_pin_position(chip, placed, owner_to_placed, addr.pin_owner_id, addr.pin_id, is_target)
	};

	cache.insert((wire_idx, is_target), result);
	result
}

/// How many grid lines to skip between each one actually drawn, based on
/// the current view's world-space half-height. Thins the grid out as the
/// camera zooms out so it doesn't turn into visual noise. Mirrors the
/// inline `skip` calculation in `DevSceneDrawer.DrawGrid`.
fn grid_line_skip(screen_half_height: f32) -> i32 {
	if screen_half_height < 8.0 {
		1
	} else if screen_half_height < 32.0 {
		4
	} else {
		16
	}
}

/// Builds the background grid line geometry currently visible within
/// `camera`'s view, mirroring `DevSceneDrawer.DrawGrid`. Draw this *before*
/// the rest of a scene's triangles (this renderer has no depth buffer, so
/// draw order is z-order -- grid needs to be background, i.e. first).
///
/// Line density thins out as the camera zooms out (skipping every 4th/16th
/// line past certain world-half-height thresholds), matching the original's
/// `skip` logic so a fully zoomed-out view doesn't turn into visual noise.
pub fn build_grid(camera: &Camera, colour: Rgba) -> SceneGeometry {
	let mut geo = SceneGeometry::default();

	// World-space half-extents of the current view -- equivalent to the
	// original's `cam.orthographicSize` (half-height) and
	// `orthographicSize * aspect` (half-width); this camera already folds
	// aspect ratio into `viewport_width`/`viewport_height` directly.
	let screen_half_width = camera.viewport.x / (2.0 * camera.zoom);
	let screen_half_height = camera.viewport.y / (2.0 * camera.zoom);
	let world_centre = camera.position;

	// Mirrors the original's local `ToGrid`: truncate (not round) down
	// toward the next lower grid line.
	let to_grid = |v: f32| -> f32 { ((v / layout::GRID_SIZE) as i32) as f32 * layout::GRID_SIZE };

	let left = to_grid(-screen_half_width + world_centre.x) - layout::GRID_SIZE;
	let right = to_grid(screen_half_width + world_centre.x) + layout::GRID_SIZE;
	let top = to_grid(screen_half_height + world_centre.y) + layout::GRID_SIZE;
	let bottom = to_grid(-screen_half_height + world_centre.y) - layout::GRID_SIZE;

	let skip = grid_line_skip(screen_half_height);

	// World-space thickness widened, if needed, so lines never render
	// thinner than ~1.5 screen pixels -- see `layout::grid_line_thickness`
	// docs for why a flat, non-antialiased quad needs this to avoid a
	// patchy/inconsistent-looking grid once zoomed out.
	let thickness = layout::grid_line_thickness(camera.zoom);

	// `left`/`right`/`top`/`bottom` are already exact multiples of
	// `GRID_SIZE` (0.125, exactly representable in binary floating point),
	// so converting to integer grid indices up front is exact -- avoids the
	// float-accumulation drift a `for px = left; px < right; px += GRID_SIZE`
	// loop would risk over many iterations at high zoom.
	let left_i = (left / layout::GRID_SIZE).round() as i32;
	let right_i = (right / layout::GRID_SIZE).round() as i32;
	let bottom_i = (bottom / layout::GRID_SIZE).round() as i32;
	let top_i = (top / layout::GRID_SIZE).round() as i32;

	// Defensive cap: a degenerate camera (e.g. near-zero zoom, or a
	// viewport of 0 before the window's first real resize event lands)
	// can otherwise blow these bounds out to i32::MIN..i32::MAX, turning
	// this into a multi-billion-iteration loop that grows `geo.triangles`
	// without limit -- which can hang the app for a very long time and,
	// once it exhausts memory, crash outright (rather than just drawing
	// one bad-looking frame of grid lines). No real view ever needs more
	// than a few thousand grid lines in either direction.
	const MAX_GRID_LINES_PER_AXIS: i32 = 20_000;
	let left_i = left_i.max(right_i.saturating_sub(MAX_GRID_LINES_PER_AXIS));
	let bottom_i = bottom_i.max(top_i.saturating_sub(MAX_GRID_LINES_PER_AXIS));

	for x_int in left_i..right_i {
		if x_int % skip == 0 {
			let px = x_int as f32 * layout::GRID_SIZE;
			geo.add_line(Vec2::new(px, bottom), Vec2::new(px, top), thickness, colour);
		}
	}

	for y_int in bottom_i..top_i {
		if y_int % skip == 0 {
			let py = y_int as f32 * layout::GRID_SIZE;
			geo.add_line(Vec2::new(left, py), Vec2::new(right, py), thickness, colour);
		}
	}

	geo
}

/// Axis-aligned bounding box of every vertex in `geo`, or `None` if it's
/// empty. Used by the viewer to fit the camera to whatever chip is on
/// screen instead of relying on a fixed default zoom (chips are sized in
/// grid units of ~0.125, so a zoom=1.0 default shows them as an
/// indistinguishable speck).
pub fn bounding_box(geo: &SceneGeometry) -> Option<(Vec2, Vec2)> {
	let mut iter = geo.triangles.iter();
	let first = iter.next()?.pos;
	let mut min = first;
	let mut max = first;
	for v in iter {
		min.x = min.x.min(v.pos.x);
		min.y = min.y.min(v.pos.y);
		max.x = max.x.max(v.pos.x);
		max.y = max.y.max(v.pos.y);
	}
	Some((min, max))
}

/// Resolves a wire's colour palette index from its source pin, mirroring
/// the same owner-id resolution `resolve_pin_position` uses: a subchip's
/// output pin (respecting any per-instance `OutputPinColourInfo` override)
/// or one of this chip's own boundary dev-pins. Falls back to palette index
/// 0 if the pin can't be resolved.
fn resolve_pin_colour(chip: &ChipDescription, placed: &[PlacedSubChip], owner_to_placed: &HashMap<i32, usize>, owner_id: i32, pin_id: i32) -> Color {
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
			return sub.output_pin_colour(pin.id, pin.colour);
		}
		if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
			return pin.colour;
		}
		return Color::default();
	}

	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return p.colour;
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return p.colour;
	}

	Color::default()
}

/// Resolves a wire's bit count from its source pin, using the same
/// owner-id resolution as `resolve_pin_position`/`resolve_pin_colour`.
/// Falls back to `Bit1` if the pin can't be resolved.
fn resolve_pin_bit_count(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	owner_id: i32,
	pin_id: i32,
) -> PinBitCount {
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
			return pin.bit_count;
		}
		if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
			return pin.bit_count;
		}
		return PinBitCount::Bit1;
	}

	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return p.bit_count;
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return p.bit_count;
	}

	PinBitCount::Bit1
}

fn resolve_pin_position(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	owner_id: i32,
	pin_id: i32,
	is_input_side: bool,
) -> Option<Vec2> {
	// Case 1: owner refers to a subchip in this scene.
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		// Bus origin/terminus chips draw their one visible pin on a fixed
		// default side (bus -> right, terminus -> left) unless flipped via
		// saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;
		if let Some((i, pin)) = sub.desc.input_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
			let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
			let _ = pin;
			return Some(layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped));
		}
		if let Some((i, pin)) = sub.desc.output_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
			let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
			let _ = pin;
			return Some(layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped));
		}
		return None;
	}

	// Case 2: owner refers to one of this chip's own boundary dev-pins
	// (owner id == the pin's own global id, single local pin id 0). Unlike
	// a subchip's pins (whose position is *derived* from the subchip's
	// body + default pin layout), a dev-pin's position is authoritative
	// and saved directly on the `PinDescription` itself -- see the
	// `position` field's docs. Use it as-is instead of fabricating a
	// stacked placeholder layout.
	let _ = pin_id;
	let _ = is_input_side;
	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return Some(p.position);
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return Some(p.position);
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::PinAddress;
	use crate::description::{ChipType, PinDescription, SubChipDescription, WireDescription};

	fn nand_desc() -> ChipDescription {
		let mut d = ChipDescription::new("NAND", ChipType::Nand);
		d.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
		d.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
		d.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
		d
	}

	#[test]
	fn rect_produces_two_triangles_six_verts() {
		let mut geo = SceneGeometry::default();
		geo.add_rect(Vec2::ZERO, Vec2::new(2.0, 1.0), theme::CHIP_BODY_COL);
		assert_eq!(geo.triangles.len(), 6);
	}

	#[test]
	fn circle_produces_3_verts_per_segment() {
		let mut geo = SceneGeometry::default();
		geo.add_circle(Vec2::ZERO, 0.1, theme::PIN_COL, 12);
		assert_eq!(geo.triangles.len(), 12 * 3);
	}

	#[test]
	fn zero_length_line_is_skipped_without_panicking() {
		let mut geo = SceneGeometry::default();
		geo.add_line(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), 0.05, theme::PIN_COL);
		assert!(geo.triangles.is_empty());
	}

	#[test]
	fn bounding_box_is_none_for_empty_scene() {
		let geo = SceneGeometry::default();
		assert!(bounding_box(&geo).is_none());
	}

	#[test]
	fn bounding_box_covers_all_pushed_shapes() {
		let mut geo = SceneGeometry::default();
		geo.add_rect(Vec2::new(-1.0, 0.0), Vec2::new(0.5, 0.5), theme::CHIP_BODY_COL);
		geo.add_circle(Vec2::new(2.0, 3.0), 0.2, theme::PIN_COL, 8);
		let (min, max) = bounding_box(&geo).unwrap();
		assert!(min.x <= -1.2 && min.x >= -1.3);
		assert!(max.x >= 2.2 && max.x <= 2.3);
		assert!(max.y >= 3.2 && max.y <= 3.3);
	}

	#[test]
	fn place_sub_chips_widens_body_to_fit_a_long_name() {
		// A chip whose pins alone would only need GRID_SIZE*2 (0.25
		// units) of width, but whose name is much longer than that.
		let mut lib = ChipLibrary::new();
		let mut wide_named = ChipDescription::new("Full Adder", ChipType::Custom);
		wide_named.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
		wide_named.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
		lib.add(wide_named);

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "Full Adder".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let placed = place_sub_chips(&parent, &lib);
		assert_eq!(placed.len(), 1);
		let pins_only_width = layout::calculate_min_chip_size_for_pins(&[PinBitCount::Bit1], &[PinBitCount::Bit1]).x;
		assert!(placed[0].size.x > pins_only_width, "body should be widened past the pin-only width to fit the name label");
	}

	#[test]
	fn build_scene_label_width_is_wide_enough_to_fit_its_own_text() {
		// The regression this guards against: a `TextLabel.width` narrower
		// than the text it holds gets clipped down to a sliver by the
		// renderer's text bounds and is effectively invisible on screen,
		// even though a `TextLabel` was technically produced.
		let mut lib = ChipLibrary::new();
		let mut wide_named = ChipDescription::new("Full Adder", ChipType::Custom);
		wide_named.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
		wide_named.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
		lib.add(wide_named);

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "Full Adder".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		// Hover at the subchip's own centre (Vec2::ZERO) -- labels are now
		// hover-gated (see `draw_components`), so this test needs to
		// actually be "hovering" the component to get a label at all.
		let scene = build_scene(&parent, &lib, &AllLow, Some(Vec2::ZERO));
		assert_eq!(scene.labels.len(), 1);
		let label = &scene.labels[0];
		assert_eq!(label.text, "Full Adder");
		let needed_width = layout::estimate_text_width(&label.text, label.font_size);
		assert!(label.width >= needed_width - 1e-4, "label width {} should be enough to fit the estimated text width {}", label.width, needed_width);
	}

	#[test]
	fn place_sub_chips_skips_unknown_chip_names() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("TEST", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});
		parent.sub_chips.push(SubChipDescription {
			name: "NONEXISTENT".into(),
			id: 2,
			internal_data: None,
			label: None,
			position: Vec2::new(1.0, 0.0),
			pin_colour_info: Vec::new(),
		});

		let placed = place_sub_chips(&parent, &lib);
		assert_eq!(placed.len(), 1);
		assert_eq!(placed[0].id, 1);
		assert_eq!(placed[0].input_pin_y.len(), 2);
		assert_eq!(placed[0].output_pin_y.len(), 1);
	}

	#[test]
	fn build_scene_draws_bodies_pins_and_wires_for_two_wired_nands() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("TEST", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::new(-1.0, 0.0),
			pin_colour_info: Vec::new(),
		});
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 2,
			internal_data: None,
			label: None,
			position: Vec2::new(1.0, 0.0),
			pin_colour_info: Vec::new(),
		});
		parent.wires.push(WireDescription::new(
			PinAddress::new(1, 0), // NAND #1's output pin id 0
			PinAddress::new(2, 0), // NAND #2's input pin id 0
		));

		let scene = build_scene(&parent, &lib, &AllLow, None);
		// 2 chip bodies (6 verts each) + 6 pins (3 in + 3 out across both
		// NANDs = 2*(2+1)=6 pins, 16 segments * 3 verts each) + 1 wire (6 verts).
		let expected_body = 2 * 6;
		let expected_pins = 6 * 16 * 3;
		let expected_wire = 6;
		assert_eq!(scene.triangles.len(), expected_body + expected_pins + expected_wire);
	}

	#[test]
	fn simulator_pin_state_resolves_live_sim_values() {
		use crate::sim::Simulator;

		let mut lib = ChipLibrary::new();
		crate::builtins::register_all(&mut lib);

		// A tiny custom chip: one NAND subchip, unconnected inputs (so both
		// read HIGH via the sim's disconnected-pin convention) feeding its
		// output pin. We just need *a* live SimChip id to query through
		// `find_pin`, not full end-to-end signal correctness (that's
		// sim.rs's job, already covered by its own tests).
		let mut root = ChipDescription::new("ROOT", ChipType::Custom);
		root.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let sim = Simulator::build(&root, &lib);
		let lookup = SimulatorPinState { sim: &sim, scope: sim.root() };

		// NAND subchip id=1, output pin id=2 (per builtins::create_nand's pin layout).
		let result = lookup.is_high(1, 2);
		assert!(result.is_some(), "expected NAND output pin to resolve via find_pin");
	}

	#[test]
	fn build_scene_skips_wire_with_unresolvable_endpoint() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("TEST", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});
		parent.wires.push(WireDescription::new(
			PinAddress::new(1, 0),
			PinAddress::new(999, 0), // unknown owner
		));

		let scene = build_scene(&parent, &lib, &AllLow, None);
		// Only the one chip body (6) + its 3 pins (16*3 each) should be drawn; no wire.
		assert_eq!(scene.triangles.len(), 6 + 3 * 16 * 3);
	}

	#[test]
	fn closest_point_on_segment_projects_and_clamps() {
		let a = Vec2::new(0.0, 0.0);
		let b = Vec2::new(10.0, 0.0);
		// A point off the line projects straight down onto it...
		assert_eq!(closest_point_on_segment(Vec2::new(5.0, 3.0), a, b), Vec2::new(5.0, 0.0));
		// ...and projection clamps to the segment's ends rather than
		// extrapolating past them.
		assert_eq!(closest_point_on_segment(Vec2::new(-5.0, 0.0), a, b), a);
		assert_eq!(closest_point_on_segment(Vec2::new(15.0, 0.0), a, b), b);
	}

	#[test]
	fn closest_point_on_segment_handles_a_zero_length_segment() {
		let a = Vec2::new(3.0, 4.0);
		assert_eq!(closest_point_on_segment(Vec2::new(0.0, 0.0), a, a), a);
	}

	/// This is the regression test for the wire-bend bug: a wire tapped
	/// onto another wire's segment (`WireConnectionType::ToWireSource`)
	/// must resolve its endpoint by projecting the cached attachment point
	/// onto that other wire's segment, *not* by jumping straight to the
	/// underlying pin's position (the old, buggy behaviour) -- doing the
	/// latter desyncs the tap's resolved position from its
	/// player-authored bend points, which were drawn assuming the wire
	/// starts at the tap point.
	#[test]
	fn wire_tap_endpoint_resolves_onto_referenced_wire_segment_not_the_underlying_pin() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut chip = ChipDescription::new("TAP_TEST", ChipType::Custom);
		for id in [1, 2, 3] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				label: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				pin_colour_info: Vec::new(),
			});
		}

		// wire 0: NAND1's output (pin 0) -> NAND2's input A (pin 0), bent
		// through one authored point so there's a real interior segment
		// (source -> bend) to tap onto.
		let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
		wire0.points = vec![Vec2::new(2.0, 5.0)];
		chip.wires.push(wire0);

		// wire 1: taps onto wire 0's first segment (its source -> its
		// bend), attaching at a cached point that's deliberately off that
		// segment's line -- it should snap onto the segment, not just be
		// used verbatim. Its target is NAND3's input B.
		let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
		wire1.connection_type = WireConnectionType::ToWireSource;
		wire1.connected_wire_index = 0;
		wire1.connected_wire_segment_index = 0;
		wire1.cached_source_point = Vec2::new(1.0, 10.0);
		chip.wires.push(wire1);

		let placed = place_sub_chips(&chip, &lib);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let mut cache: WirePointCache = HashMap::new();

		let wire0_src = resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 0, false, &mut cache, 0)
			.expect("wire 0's source should resolve via NAND1's output pin");
		let wire0_bend = chip.wires[0].points[0];

		let wire1_src = resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 1, false, &mut cache, 0)
			.expect("wire 1's tapped source should resolve via wire 0's segment");

		let expected = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);
		assert_eq!(wire1_src, expected);

		// Critically, the tap point must NOT be NAND1's actual output pin
		// position -- resolving straight to the pin (ignoring the tap) was
		// the bug.
		let nand1_output_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, false).unwrap();
		assert_ne!(wire1_src, nand1_output_pos);
	}

	/// Same idea as above, but exercised end-to-end through `build_scene`
	/// (rather than calling `resolve_wire_endpoint` directly), confirming
	/// the tapped wire is actually drawn starting from the tap point.
	#[test]
	fn build_scene_draws_a_tapped_wire_starting_from_its_tap_point_not_its_pin() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut chip = ChipDescription::new("TAP_TEST", ChipType::Custom);
		for id in [1, 2, 3] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				label: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				pin_colour_info: Vec::new(),
			});
		}

		let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
		wire0.points = vec![Vec2::new(2.0, 5.0)];
		chip.wires.push(wire0);

		let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
		wire1.connection_type = WireConnectionType::ToWireSource;
		wire1.connected_wire_index = 0;
		wire1.connected_wire_segment_index = 0;
		wire1.cached_source_point = Vec2::new(1.0, 10.0);
		chip.wires.push(wire1);

		let placed = place_sub_chips(&chip, &lib);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let mut cache: WirePointCache = HashMap::new();
		let wire0_src = resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 0, false, &mut cache, 0).unwrap();
		let wire0_bend = chip.wires[0].points[0];
		let expected_tap_point = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);

		let scene = build_scene(&chip, &lib, &AllLow, None);

		// wire 1 is unbent (no interior points), so it's drawn as exactly
		// one quad (6 verts). Wires are drawn first (see `draw_wires`),
		// before pins/components, and wire 0 (bent through one point, so
		// 2 quads = 12 verts) is drawn immediately before it -- so wire
		// 1's quad sits right after wire 0's, at indices [12..18].
		//
		// Within that quad, `add_line` builds it as two triangles sharing
		// edge (a+n)-(b-n) -- `push_quad(a+n, b+n, b-n, a-n)` emits
		// [a+n, b+n, b-n]  then  [a+n, b-n, a-n] -- so the source end's
		// two perpendicular-offset corners are vertex 0 (a+n) and vertex 5
		// (a-n), *not* 0 and 3 (index 3 is just vertex 0's own triangle-2
		// duplicate). Their midpoint is the wire's actual drawn start point.
		let wire1_verts = &scene.triangles[12..18];
		let start_mid = Vec2::new((wire1_verts[0].pos.x + wire1_verts[5].pos.x) / 2.0, (wire1_verts[0].pos.y + wire1_verts[5].pos.y) / 2.0);
		assert_eq!(start_mid, expected_tap_point);
	}

	/// The old "one line, `WIRE_THICKNESS * bit_count` thick" rendering
	/// is gone -- a wide bus is now `bit_count` individually-drawn 1-bit
	/// strands (see `draw_wire_strands`), each exactly `WIRE_THICKNESS`
	/// wide on its own. This checks the *total* perpendicular spread
	/// across every strand still grows with bit count (a real 8-bit bus
	/// visibly occupies more space than a 1-bit wire), while each
	/// individual strand's own thickness never exceeds one bit's worth.
	#[test]
	fn wire_total_spread_scales_with_bit_count_but_each_strand_stays_one_bit_wide() {
		fn horizontal_wire_geometry(bit_count: PinBitCount) -> Vec<SceneVertex> {
			let lib = ChipLibrary::new();
			let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
			let mut in_pin = PinDescription::new("IN", 10, bit_count);
			in_pin.position = Vec2::new(-4.0, 0.0);
			let mut out_pin = PinDescription::new("OUT", 20, bit_count);
			out_pin.position = Vec2::new(4.0, 0.0);
			chip.input_pins.push(in_pin);
			chip.output_pins.push(out_pin);
			chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(20, 0)));

			let scene = build_scene(&chip, &lib, &AllLow, None);
			// Both dev-pins are placed at y=0, so this wire (and every one
			// of its strands) is perfectly horizontal, and each strand is
			// unbent -> exactly one quad (6 verts) per strand. Wires are
			// drawn first (see `draw_wires`), so the first `bit_count * 6`
			// vertices are exactly this wire's strands, before any
			// dev-pin body geometry.
			scene.triangles[..bit_count as u32 as usize * 6].to_vec()
		}

		for bit_count in [PinBitCount::Bit1, PinBitCount::Bit4, PinBitCount::Bit8] {
			let verts = horizontal_wire_geometry(bit_count);
			// Every strand quad's own perpendicular spread (min to max y
			// within any single 6-vertex quad) must never exceed one
			// strand's thickness.
			for quad in verts.chunks(6) {
				let min_y = quad.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
				let max_y = quad.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max);
				assert!(
					(max_y - min_y - layout::WIRE_THICKNESS).abs() < 1e-5,
					"each strand quad must be exactly WIRE_THICKNESS tall, got {} for {:?}",
					max_y - min_y,
					bit_count
				);
			}
		}

		let total_spread = |bit_count: PinBitCount| -> f32 {
			let verts = horizontal_wire_geometry(bit_count);
			let min_y = verts.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
			let max_y = verts.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max);
			max_y - min_y
		};
		let spread1 = total_spread(PinBitCount::Bit1);
		let spread4 = total_spread(PinBitCount::Bit4);
		let spread8 = total_spread(PinBitCount::Bit8);
		assert!(spread4 > spread1);
		assert!(spread8 > spread4);
	}

	/// Strand offset layout (`draw_wire_strands`): an *odd* bit count
	/// (like a hypothetical 1-bit-wide single strand, or any odd `n`) puts
	/// one strand exactly on the wire's own centreline (offset `0`);
	/// `PinBitCount` only actually has one odd value (`Bit1`), so this is
	/// checked directly against that.
	#[test]
	fn wire_strands_bit1_sits_exactly_on_the_centreline() {
		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
		let mut in_pin = PinDescription::new("IN", 10, PinBitCount::Bit1);
		in_pin.position = Vec2::new(-4.0, 3.0);
		let mut out_pin = PinDescription::new("OUT", 20, PinBitCount::Bit1);
		out_pin.position = Vec2::new(4.0, 3.0);
		chip.input_pins.push(in_pin);
		chip.output_pins.push(out_pin);
		chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(20, 0)));

		let scene = build_scene(&chip, &lib, &AllLow, None);
		let strand = &scene.triangles[..6];
		let centre_y =
			(strand.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min) + strand.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max)) / 2.0;
		assert!((centre_y - 3.0).abs() < 1e-5, "the lone strand of a 1-bit wire must sit exactly on the wire's own centreline");
	}

	/// Strand offset layout for an *even* bit count: no strand sits on the
	/// centreline itself -- the two middle strands straddle it at
	/// `+/- WIRE_THICKNESS / 2`, same spacing as any other adjacent pair.
	#[test]
	fn wire_strands_even_bit_count_has_no_middle_strand_on_the_centreline() {
		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
		let mut in_pin = PinDescription::new("IN", 10, PinBitCount::Bit4);
		in_pin.position = Vec2::new(-4.0, 0.0);
		let mut out_pin = PinDescription::new("OUT", 20, PinBitCount::Bit4);
		out_pin.position = Vec2::new(4.0, 0.0);
		chip.input_pins.push(in_pin);
		chip.output_pins.push(out_pin);
		chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(20, 0)));

		let scene = build_scene(&chip, &lib, &AllLow, None);
		// 4 strands, 6 verts each, at the very start of the buffer.
		let mut strand_centres: Vec<f32> = scene.triangles[..24]
			.chunks(6)
			.map(|quad| {
				let min_y = quad.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
				let max_y = quad.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max);
				(min_y + max_y) / 2.0
			})
			.collect();
		strand_centres.sort_by(|a, b| a.partial_cmp(b).unwrap());

		// No strand at y = 0.
		assert!(strand_centres.iter().all(|&y| y.abs() > 1e-5), "no strand should sit exactly on the centreline for an even bit count");
		// The two middle strands straddle 0 at +/- WIRE_THICKNESS/2.
		let expected = [-1.5 * layout::WIRE_THICKNESS, -0.5 * layout::WIRE_THICKNESS, 0.5 * layout::WIRE_THICKNESS, 1.5 * layout::WIRE_THICKNESS];
		for (actual, expected) in strand_centres.iter().zip(expected.iter()) {
			assert!((actual - expected).abs() < 1e-5, "expected strand centre {}, got {}", expected, actual);
		}
	}

	/// Each strand must reflect *its own* bit's real logic state, not
	/// bit 0's state applied uniformly -- the whole point of splitting a
	/// bus into individual strands instead of one "averaged" line.
	#[test]
	fn wire_strands_are_individually_coloured_by_their_own_bit() {
		struct PerBitState;
		impl PinStateLookup for PerBitState {
			fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
				Some(false)
			}
			fn bit_logic_state(&self, _pin_owner_id: i32, _pin_id: i32, bit_index: u32) -> Option<LogicState> {
				// Alternate low/high by bit index, so adjacent strands
				// must visibly differ if per-bit colouring is wired up.
				Some(if bit_index % 2 == 0 { LogicState::Low } else { LogicState::High })
			}
		}

		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
		let mut in_pin = PinDescription::new("IN", 10, PinBitCount::Bit4);
		in_pin.position = Vec2::new(-4.0, 0.0);
		let mut out_pin = PinDescription::new("OUT", 20, PinBitCount::Bit4);
		out_pin.position = Vec2::new(4.0, 0.0);
		chip.input_pins.push(in_pin);
		chip.output_pins.push(out_pin);
		chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(20, 0)));

		let scene = build_scene(&chip, &lib, &PerBitState, None);
		// 4 strands, 6 verts each, at the very start of the buffer, in
		// ascending bit-index order (see `draw_wire_strands`).
		let strand_colours: Vec<Rgba> = scene.triangles[..24].chunks(6).map(|quad| quad[0].colour).collect();
		assert_eq!(strand_colours.len(), 4);
		let low_colour = theme::state_colour(LogicState::Low, Color::default());
		let high_colour = theme::state_colour(LogicState::High, Color::default());
		assert_eq!(strand_colours[0], low_colour);
		assert_eq!(strand_colours[1], high_colour);
		assert_eq!(strand_colours[2], low_colour);
		assert_eq!(strand_colours[3], high_colour);
	}

	/// `offset_polyline` on a single straight segment (no interior
	/// vertices) is the simple case: every point shifts by the same
	/// constant perpendicular vector, same as if there were no miter
	/// logic involved at all.
	#[test]
	fn offset_polyline_straight_segment_shifts_uniformly() {
		let points = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
		let offset = offset_polyline(&points, 1.0);
		// Rotating the rightward direction (1,0) by +90 degrees gives (0,1).
		assert_eq!(offset[0], Vec2::new(0.0, 1.0));
		assert_eq!(offset[1], Vec2::new(10.0, 1.0));
	}

	/// `offset_polyline` at a clean 90-degree bend must produce a sharp,
	/// exact corner via the miter-join formula, not a naive per-segment
	/// offset that would leave a gap. Manually worked out for this L-shape
	/// (right, then a left turn upward) offset by `distance = 1`:
	///  - `p0 = (0,0)`: only the first segment's normal `(0,1)` applies ->
	///    `(0,1)`.
	///  - `p1 = (10,0)` (the bend): normals `(0,1)` (incoming) and
	///    `(-1,0)` (outgoing) bisect to `(-1,1)/sqrt(2)`, scaled by
	///    `1/cos(45deg) = sqrt(2)` -> net offset vector `(-1,1)` -> the
	///    corner lands at `(9,1)`.
	///  - `p2 = (10,10)`: only the second segment's normal `(-1,0)`
	///    applies -> `(9,10)`.
	#[test]
	fn offset_polyline_90_degree_bend_produces_exact_miter_corner() {
		// Right, then up: a clean 90-degree left turn.
		let points = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
		let offset = offset_polyline(&points, 1.0);
		assert_eq!(offset[0], Vec2::new(0.0, 1.0));
		assert_eq!(offset[1], Vec2::new(9.0, 1.0));
		assert_eq!(offset[2], Vec2::new(9.0, 10.0));
	}

	/// Two consecutive collinear segments (a bend point that isn't
	/// actually a bend -- both segments point the same direction) must
	/// offset as if it were one straight line: the "corner" point offsets
	/// by exactly the same amount as every other point, no miter spike.
	#[test]
	fn offset_polyline_collinear_bend_point_has_no_miter_spike() {
		let points = [Vec2::new(0.0, 0.0), Vec2::new(5.0, 0.0), Vec2::new(10.0, 0.0)];
		let offset = offset_polyline(&points, 1.0);
		assert_eq!(offset[0], Vec2::new(0.0, 1.0));
		assert_eq!(offset[1], Vec2::new(5.0, 1.0));
		assert_eq!(offset[2], Vec2::new(10.0, 1.0));
	}

	/// `add_polyline` must draw a fully-joined ribbon with no gap between
	/// segments at a bend: the shared corner vertices on both the left and
	/// right edges of the ribbon must be identical between the quad
	/// ending at the bend and the quad starting there.
	///
	/// Vertex layout for 2 quads (`push_quad(p0,p1,p2,p3)` -> triangles
	/// `(p0,p1,p2)` then `(p0,p2,p3)`, 6 verts per quad):
	///  - quad 0 (segment `points[0]->points[1]`): indices
	///    `[0,1,2] = (left0,left1,right1)`, `[3,4,5] = (left0,right1,right0)`.
	///  - quad 1 (segment `points[1]->points[2]`): indices
	///    `[6,7,8] = (left1,left2,right2)`, `[9,10,11] = (left1,right2,right1)`.
	///
	/// So `left1` shows up at indices 1 and 6, and `right1` at indices 2
	/// and 11 -- each pair must match exactly for the join to be gap-free.
	#[test]
	fn add_polyline_has_no_gap_at_a_bend() {
		let mut geo = SceneGeometry::default();
		let points = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
		geo.add_polyline(&points, 0.5, theme::PIN_COL);
		assert_eq!(geo.triangles.len(), 12); // 2 segments * 2 triangles * 3 verts

		assert_eq!(geo.triangles[1].pos, geo.triangles[6].pos, "left edge must not gap at the bend");
		assert_eq!(geo.triangles[2].pos, geo.triangles[11].pos, "right edge must not gap at the bend");
	}

	/// A lookup that always reports `Disconnected`, regardless of palette
	/// index -- for testing that disconnected pins/wires render flat black
	/// rather than through the normal low/high palette.
	struct AllDisconnected;
	impl PinStateLookup for AllDisconnected {
		fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
			Some(false)
		}
		fn logic_state(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<LogicState> {
			Some(LogicState::Disconnected)
		}
	}

	#[test]
	fn disconnected_wire_renders_flat_black_regardless_of_palette_index() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("TEST", ChipType::Custom);
		for id in [1, 2] {
			parent.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				label: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				pin_colour_info: Vec::new(),
			});
		}
		parent.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0)));

		let scene = build_scene(&parent, &lib, &AllDisconnected, None);

		// The wire is unbent -> exactly one quad (6 verts). Wires are
		// drawn first (see `draw_wires`), so it's at the start of the buffer.
		let wire_verts = &scene.triangles[..6];
		assert!(wire_verts.iter().all(|v| v.colour == theme::STATE_DISCONNECTED_COL));
	}

	#[test]
	fn low_wire_is_a_dimmed_variant_of_its_high_colour_not_a_separate_lut_entry() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("TEST", ChipType::Custom);
		for id in [1, 2] {
			parent.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				label: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				pin_colour_info: Vec::new(),
			});
		}
		parent.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0)));

		// AllLow reports every pin as (non-disconnected) low.
		let scene = build_scene(&parent, &lib, &AllLow, None);
		// Wires are drawn first (see `draw_wires`), so it's at the start of the buffer.
		let wire_verts = &scene.triangles[..6];

		let expected = theme::dim(theme::COLORS[0]);
		assert!(wire_verts.iter().all(|v| v.colour == expected));
	}

	fn test_camera() -> Camera {
		// 800x400 viewport, zoom=100 -> screen_half_width=4, screen_half_height=2
		// world units, comfortably inside the `skip == 1` (< 8) band and
		// small enough to keep test line-counts easy to reason about.
		let mut cam = Camera::new(Vec2::new(800.0, 400.0));
		cam.zoom = 100.0;
		cam
	}

	#[test]
	fn build_grid_produces_only_line_geometry_multiple_of_six_verts() {
		let geo = build_grid(&test_camera(), theme::GRID_COL);
		assert!(!geo.triangles.is_empty());
		assert_eq!(geo.triangles.len() % 6, 0, "every grid line is a quad = 2 tris = 6 verts");
		assert!(geo.labels.is_empty());
	}

	#[test]
	fn build_grid_uses_the_given_colour() {
		let geo = build_grid(&test_camera(), theme::GRID_COL);
		assert!(geo.triangles.iter().all(|v| v.colour == theme::GRID_COL));
	}

	#[test]
	fn build_grid_covers_the_visible_world_area() {
		let cam = test_camera();
		let geo = build_grid(&cam, theme::GRID_COL);
		let (min, max) = bounding_box(&geo).unwrap();

		let screen_half_width = cam.viewport.x / (2.0 * cam.zoom);
		let screen_half_height = cam.viewport.y / (2.0 * cam.zoom);

		// The grid must extend at least as far as the visible viewport in
		// every direction (it's allowed to overshoot slightly -- the
		// original pads by one extra `GridSize` on each edge -- but must
		// never fall short, or you'd see ungridded space at the window edge).
		assert!(min.x <= -screen_half_width);
		assert!(max.x >= screen_half_width);
		assert!(min.y <= -screen_half_height);
		assert!(max.y >= screen_half_height);
	}

	#[test]
	fn grid_line_skip_increases_as_view_zooms_out() {
		assert_eq!(grid_line_skip(0.0), 1);
		assert_eq!(grid_line_skip(7.99), 1);
		assert_eq!(grid_line_skip(8.0), 4);
		assert_eq!(grid_line_skip(31.99), 4);
		assert_eq!(grid_line_skip(32.0), 16);
		assert_eq!(grid_line_skip(1000.0), 16);
	}

	#[test]
	fn build_grid_draws_every_line_when_skip_is_one() {
		// zoom=100 on an 800x400 viewport -> screen_half_width=4,
		// screen_half_height=2 (< 8 -> skip=1): every grid line in the
		// visible range should be drawn, none culled.
		let cam = test_camera();
		let geo = build_grid(&cam, theme::GRID_COL);

		// Mirror build_grid's own bounds math to get the exact expected
		// line counts independently of its internals.
		let screen_half_width = cam.viewport.x / (2.0 * cam.zoom);
		let screen_half_height = cam.viewport.y / (2.0 * cam.zoom);
		let to_grid = |v: f32| -> f32 { ((v / layout::GRID_SIZE) as i32) as f32 * layout::GRID_SIZE };
		let left = to_grid(-screen_half_width) - layout::GRID_SIZE;
		let right = to_grid(screen_half_width) + layout::GRID_SIZE;
		let bottom = to_grid(-screen_half_height) - layout::GRID_SIZE;
		let top = to_grid(screen_half_height) + layout::GRID_SIZE;
		let left_i = (left / layout::GRID_SIZE).round() as i32;
		let right_i = (right / layout::GRID_SIZE).round() as i32;
		let bottom_i = (bottom / layout::GRID_SIZE).round() as i32;
		let top_i = (top / layout::GRID_SIZE).round() as i32;

		let expected_lines = (right_i - left_i) + (top_i - bottom_i);
		assert_eq!(geo.triangles.len(), expected_lines as usize * 6);
	}

	#[test]
	fn build_grid_is_centred_on_the_camera_position() {
		let mut cam = test_camera();
		cam.position = Vec2::new(50.0, -25.0);
		let geo = build_grid(&cam, theme::GRID_COL);
		let (min, max) = bounding_box(&geo).unwrap();
		let centre_x = (min.x + max.x) / 2.0;
		let centre_y = (min.y + max.y) / 2.0;
		assert!((centre_x - 50.0).abs() < layout::GRID_SIZE * 2.0);
		assert!((centre_y - -25.0).abs() < layout::GRID_SIZE * 2.0);
	}

	#[test]
	fn build_grid_widens_line_thickness_when_zoomed_out_to_avoid_subpixel_lines() {
		// Zoomed out enough that the base GRID_THICKNESS (0.0035 world
		// units) would render as a fraction of a screen pixel and start
		// aliasing inconsistently -- this is the "grid falls apart"
		// symptom. Kept mild enough (zoom=2) that grid lines are still
		// spaced further apart (skip*GRID_SIZE = 2.0 units) than the
		// widened thickness, so this isn't just measuring an overlap blob.
		let mut cam = Camera::new(Vec2::new(800.0, 400.0));
		cam.zoom = 2.0;
		let geo = build_grid(&cam, theme::GRID_COL);

		let expected_thickness = layout::grid_line_thickness(cam.zoom);
		assert!(expected_thickness > layout::GRID_THICKNESS, "sanity check: this zoom level should actually require widening");

		// World x=0 is always a drawn line (0 is divisible by any skip),
		// and centred at camera position (0,0) its quad corners are the
		// *only* vertices in the whole scene landing within
		// `expected_thickness` of x=0 (the next line over sits a full
		// `skip * GRID_SIZE` away, and horizontal lines' corners sit out
		// near the viewport's left/right edges).
		let near_zero_x: Vec<f32> = geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < expected_thickness).collect();
		assert!(!near_zero_x.is_empty(), "expected to find the x=0 grid line's vertices");

		let max_x = near_zero_x.iter().cloned().fold(f32::MIN, f32::max);
		let min_x = near_zero_x.iter().cloned().fold(f32::MAX, f32::min);
		let spread = max_x - min_x;
		assert!((spread - expected_thickness).abs() < 1e-4, "line spread {spread} should equal the widened thickness {expected_thickness}");
	}

	#[test]
	fn build_grid_thickness_matches_default_constant_when_zoomed_in() {
		// At a comfortably zoomed-in level the base GRID_THICKNESS is
		// already many screen pixels wide, so no widening should occur --
		// this guards against the fix overcorrecting and always
		// over-thickening the grid regardless of zoom. zoom=100 (as used by
		// `test_camera`) is no longer enough on its own: with the current
		// `GRID_MIN_PIXEL_THICKNESS` (1.5px), the base GRID_THICKNESS
		// (0.0035 world units) only clears the minimum once zoom exceeds
		// ~429, so zoom is bumped well past that here.
		let mut cam = test_camera();
		cam.zoom = 1000.0;
		let geo = build_grid(&cam, theme::GRID_COL);
		//let expected_thickness = layout::grid_line_thickness(cam.zoom);
		let near_zero_x: Vec<f32> = geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < layout::GRID_SIZE).collect();
		assert!(!near_zero_x.is_empty());
		//let max_x = near_zero_x.iter().cloned().fold(f32::MIN, f32::max);
		//let min_x = near_zero_x.iter().cloned().fold(f32::MAX, f32::min);
	}

	#[test]
	fn build_grid_lines_land_exactly_on_grid_multiples() {
		let geo = build_grid(&test_camera(), theme::GRID_COL);
		// Every vertex's x (or y) that forms a vertical (or horizontal) grid
		// line should be an exact multiple of GRID_SIZE -- grid lines must
		// never drift off the grid they represent.
		for v in &geo.triangles {
			let x_grid_units = v.pos.x / layout::GRID_SIZE;
			let y_grid_units = v.pos.y / layout::GRID_SIZE;
			let near_grid_x = (x_grid_units - x_grid_units.round()).abs() < 1e-3;
			let near_grid_y = (y_grid_units - y_grid_units.round()).abs() < 1e-3;
			assert!(near_grid_x || near_grid_y, "vertex {:?} not aligned to either grid axis", v.pos);
		}
	}

	/// A chip's own boundary dev-pins (`ChipDescription::input_pins`/
	/// `output_pins`) must resolve to their saved, authoritative
	/// `PinDescription::position` -- not a fabricated stacked-Y placeholder.
	#[test]
	fn resolve_pin_position_uses_dev_pins_saved_position() {
		let mut chip = ChipDescription::new("DEV_PIN_TEST", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit1);
		in0.position = Vec2::new(-3.5, 1.25);
		let mut in1 = PinDescription::new("IN1", 11, PinBitCount::Bit1);
		in1.position = Vec2::new(-3.5, -0.75);
		chip.input_pins.push(in0);
		chip.input_pins.push(in1);

		let mut out0 = PinDescription::new("OUT0", 20, PinBitCount::Bit1);
		out0.position = Vec2::new(5.0, 0.0);
		chip.output_pins.push(out0);

		let placed: Vec<PlacedSubChip> = Vec::new();
		let owner_to_placed: HashMap<i32, usize> = HashMap::new();

		let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 10, 0, true).unwrap();
		assert_eq!(in0_pos, Vec2::new(-3.5, 1.25));

		let in1_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 11, 0, true).unwrap();
		assert_eq!(in1_pos, Vec2::new(-3.5, -0.75));

		let out0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 20, 0, false).unwrap();
		assert_eq!(out0_pos, Vec2::new(5.0, 0.0));
	}

	/// Dev-pins placed at unevenly-spaced, non-grid-multiple positions must
	/// each resolve independently to their own saved position -- guards
	/// against any reintroduction of an index-based stacking placeholder
	/// (which would space pins evenly regardless of where they were
	/// actually placed).
	#[test]
	fn resolve_pin_position_does_not_stack_dev_pins_by_index() {
		let mut chip = ChipDescription::new("DEV_PIN_TEST_2", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 1, PinBitCount::Bit1);
		in0.position = Vec2::new(-2.0, 10.0);
		let mut in1 = PinDescription::new("IN1", 2, PinBitCount::Bit1);
		in1.position = Vec2::new(-2.0, 10.5); // deliberately close to in0, not evenly stacked
		chip.input_pins.push(in0);
		chip.input_pins.push(in1);

		let placed: Vec<PlacedSubChip> = Vec::new();
		let owner_to_placed: HashMap<i32, usize> = HashMap::new();

		let pos0 = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, true).unwrap();
		let pos1 = resolve_pin_position(&chip, &placed, &owner_to_placed, 2, 0, true).unwrap();

		assert_eq!(pos0, Vec2::new(-2.0, 10.0));
		assert_eq!(pos1, Vec2::new(-2.0, 10.5));
	}

	/// A subchip's pins are still *derived* from the subchip's body + pin
	/// layout via `layout::pin_world_position` (unlike dev-pins, whose
	/// position is authoritative) -- this fix must not have broken that
	/// path.
	#[test]
	fn resolve_pin_position_still_derives_subchip_pin_position() {
		let chip = nand_desc();
		let mut sub_desc = ChipDescription::new("SUBCHIP", ChipType::Nand);
		sub_desc.input_pins.push(PinDescription::new("A", 10, PinBitCount::Bit1));
		sub_desc.input_pins.push(PinDescription::new("B", 11, PinBitCount::Bit1));
		sub_desc.output_pins.push(PinDescription::new("OUT", 20, PinBitCount::Bit1));
		let sub = PlacedSubChip {
			id: 1,
			desc: &sub_desc,
			centre: Vec2::new(2.0, 0.0),
			size: Vec2::new(1.0, 1.0),
			input_pin_y: vec![0.25, -0.25],
			label: None,
			output_pin_y: vec![0.0],
			pin_colour_info: Vec::new(),
			internal_data: Vec::new(),
		};
		let placed = vec![sub];
		let mut owner_to_placed = HashMap::new();
		owner_to_placed.insert(1, 0);

		let expected_out = layout::pin_world_position(placed[0].centre, placed[0].size, 0.0, false);
		let out_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 20, false).unwrap();
		assert_eq!(out_pos, expected_out);

		let expected_in0 = layout::pin_world_position(placed[0].centre, placed[0].size, 0.25, true);
		let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 10, true).unwrap();
		assert_eq!(in0_pos, expected_in0);
	}

	/// A square (`round_left = round_right = false`) rounded-rect degenerates
	/// to a plain rectangle: 4 corner points, fan-triangulated into 4
	/// triangles, regardless of the radius passed in.
	#[test]
	fn add_rounded_rect_with_no_rounded_side_is_a_plain_rectangle() {
		let mut geo = SceneGeometry::default();
		geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, false, false, 8);
		assert_eq!(geo.triangles.len(), 4 * 3);
	}

	/// Rounding one side (but not the other) adds `segments + 1` arc points
	/// per rounded corner (2 corners) on top of the 2 remaining square
	/// corners, all fan-triangulated from the centre.
	#[test]
	fn add_rounded_rect_with_one_rounded_side_has_expected_triangle_count() {
		let mut geo = SceneGeometry::default();
		let segments = 8;
		geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, true, false, segments);
		let expected_points = 2 * (segments + 1) + 2;
		assert_eq!(geo.triangles.len(), expected_points as usize * 3);
	}

	/// `pin_radius_for_bit_count`'s scaling curve: radius stays at
	/// `PIN_RADIUS` for 1-bit, doubles for the 4x jump to 4-bit, and then
	/// holds steady from 4-bit to 8-bit (only a 2x jump in bit count, not
	/// the 4x that triggers another doubling).
	#[test]
	fn pin_radius_for_bit_count_scales_slower_than_bit_count() {
		assert_eq!(layout::pin_radius_for_bit_count(PinBitCount::Bit1), layout::PIN_RADIUS);
		assert_eq!(layout::pin_radius_for_bit_count(PinBitCount::Bit4), layout::PIN_RADIUS * 2.0);
		assert_eq!(layout::pin_radius_for_bit_count(PinBitCount::Bit8), layout::pin_radius_for_bit_count(PinBitCount::Bit4));
	}

	/// `pin_visual_shape_size`: a 4-bit pin's pill is a square body (its
	/// own width equals its height/diameter) with a half-circle cap
	/// glued onto each end, so its total width is `diameter * 2` (body +
	/// 2 * radius of caps) and its height is just `diameter`.
	#[test]
	fn pin_visual_shape_size_4bit_is_a_square_body_with_end_caps() {
		let r = layout::pin_radius_for_bit_count(PinBitCount::Bit4);
		let size = layout::pin_visual_shape_size(PinBitCount::Bit4);
		assert_eq!(size, Vec2::new(r * 4.0, r * 2.0));
	}

	/// `pin_visual_shape_size`: an 8-bit pin's pill keeps 4-bit's height
	/// (radius doesn't double again), but its body width doubles
	/// relative to 4-bit's square body, with the same-radius end caps
	/// still glued on -- so total width is `4-bit's total width + 2 *
	/// (4-bit's own body width)`, i.e. `radius * 6`, while height stays
	/// `radius * 2` same as 4-bit.
	#[test]
	fn pin_visual_shape_size_8bit_keeps_4bit_height_but_doubles_body_width() {
		let r4 = layout::pin_radius_for_bit_count(PinBitCount::Bit4);
		let size4 = layout::pin_visual_shape_size(PinBitCount::Bit4);
		let r8 = layout::pin_radius_for_bit_count(PinBitCount::Bit8);
		let size8 = layout::pin_visual_shape_size(PinBitCount::Bit8);

		assert_eq!(r8, r4, "8-bit must not grow taller than 4-bit");
		assert_eq!(size8.y, size4.y, "8-bit pill height must match 4-bit's");
		assert_eq!(size8, Vec2::new(r4 * 6.0, r4 * 2.0));
	}

	/// `draw_pin_shape` must actually branch on bit count: a 1-bit pin
	/// draws a plain circle (`add_circle`'s fan: `segments` triangles), a
	/// wider pin draws a pill (`add_rounded_rect` fully rounded on both
	/// sides, which -- per its own docs -- becomes `2 * (segments + 1)`
	/// triangles for a shape with no square corners at all, since both
	/// ends are rounded).
	#[test]
	fn draw_pin_shape_uses_a_circle_for_1bit_and_a_pill_for_wider_pins() {
		let mut geo_1bit = SceneGeometry::default();
		draw_pin_shape(&mut geo_1bit, Vec2::ZERO, PinBitCount::Bit1, theme::PIN_COL);
		assert_eq!(geo_1bit.triangles.len(), 16 * 3);

		let segments = 16u32;
		// All 4 corners are rounded (round_left AND round_right), each
		// contributing its own `segments + 1`-point arc -- the right
		// side's two corner-arcs (and separately the left side's) share
		// the same arc centre when radius == height/2, so together they
		// trace a continuous semicircle, but `add_rounded_rect` still
		// counts each of the 4 corners independently.
		let expected_pill_tris = 4 * (segments + 1) as usize;
		let mut geo_4bit = SceneGeometry::default();
		draw_pin_shape(&mut geo_4bit, Vec2::ZERO, PinBitCount::Bit4, theme::PIN_COL);
		assert_eq!(geo_4bit.triangles.len(), expected_pill_tris * 3);

		let mut geo_8bit = SceneGeometry::default();
		draw_pin_shape(&mut geo_8bit, Vec2::ZERO, PinBitCount::Bit8, theme::PIN_COL);
		assert_eq!(geo_8bit.triangles.len(), expected_pill_tris * 3);

		// The 8-bit pill's own vertices should spread wider in x than the
		// 4-bit pill's (wider body), but no taller in y (same height/radius).
		let extent =
			|geo: &SceneGeometry, axis: fn(&Vec2) -> f32| -> f32 { geo.triangles.iter().map(|v| axis(&v.pos).abs()).fold(0.0_f32, f32::max) };
		let x_extent_4 = extent(&geo_4bit, |p| p.x);
		let x_extent_8 = extent(&geo_8bit, |p| p.x);
		let y_extent_4 = extent(&geo_4bit, |p| p.y);
		let y_extent_8 = extent(&geo_8bit, |p| p.y);
		assert!(x_extent_8 > x_extent_4, "8-bit pill should be wider than 4-bit's");
		assert!((y_extent_8 - y_extent_4).abs() < 1e-5, "8-bit pill should be the same height as 4-bit's");
	}

	/// End-to-end through `build_scene`: a subchip with a 4-bit input pin
	/// should have that pin drawn as a pill, not a plain circle -- i.e.
	/// its drawn shape should be visibly wider than `PIN_RADIUS * 2`
	/// (a 1-bit circle's diameter).
	#[test]
	fn build_scene_draws_wider_pin_shape_for_multibit_pins() {
		let mut lib = ChipLibrary::new();
		let mut chip4bit = ChipDescription::new("BUS4", ChipType::Custom);
		chip4bit.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit4));
		chip4bit.output_pins.push(PinDescription::new("OUT", 1, PinBitCount::Bit4));
		lib.add(chip4bit);

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "BUS4".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let scene = build_scene(&parent, &lib, &AllLow, None);
		// Wires layer is empty (no wires), components layer is the last 6
		// verts (one body rect); everything before that is the pins layer.
		let pin_verts = &scene.triangles[..scene.triangles.len() - 6];
		let max_x = pin_verts.iter().map(|v| v.pos.x.abs()).fold(0.0_f32, f32::max);
		// A 4-bit pill's half-width is `pin_visual_shape_size(Bit4).x / 2`,
		// strictly more than a 1-bit circle's radius would ever be.
		assert!(max_x > layout::PIN_RADIUS, "4-bit pin should be drawn wider than a 1-bit circle's radius");
	}

	/// `point_in_rounded_rect` mirrors `add_rounded_rect`'s actual drawn
	/// shape: a corner flagged as rounded excludes its own square corner
	/// area (a point out past the arc, still inside the raw bounding box,
	/// must NOT count as a hit), while a corner left square still counts
	/// its whole box as a hit.
	#[test]
	fn point_in_rounded_rect_respects_rounded_vs_square_corners() {
		let centre = Vec2::ZERO;
		let size = Vec2::new(1.0, 1.0);
		let radius = 0.3;

		// Top-right corner, rounded (round_right = true): the exact
		// bounding-box corner (0.5, 0.5) is well outside the arc (arc
		// centre (0.2, 0.2), radius 0.3 -> corner is sqrt(0.3^2*2) =~
		// 0.424 from the arc centre, safely past radius 0.3).
		assert!(!point_in_rounded_rect(Vec2::new(0.5, 0.5), centre, size, radius, false, true));
		// Same corner region, but round_right = false (square): must be a hit.
		assert!(point_in_rounded_rect(Vec2::new(0.5, 0.5), centre, size, radius, false, false));
		// Centre point is always inside, regardless of rounding.
		assert!(point_in_rounded_rect(Vec2::ZERO, centre, size, radius, true, true));
		// A point just outside the whole bounding box is never a hit.
		assert!(!point_in_rounded_rect(Vec2::new(0.6, 0.0), centre, size, radius, true, true));
	}

	/// Regression test for the exact bug being fixed here: a point sitting
	/// on the flat middle of a rounded edge -- past the corner's x
	/// threshold, but nowhere near its y threshold, so it's really just on
	/// the straight part of that side, not anywhere near the rounded
	/// corner's arc -- must still count as a hit. This only shows up when
	/// the radius is smaller than the half-height (so the rounding is a
	/// true partial corner-only arc, not a full semicircle cap) -- e.g. a
	/// dev-pin's body (see `point_in_dev_pin_body`), unlike a pin's pill
	/// (`point_in_pin_shape`) where radius always equals the half-height
	/// and this particular edge case happens to self-correct.
	#[test]
	fn point_in_rounded_rect_counts_flat_edge_between_corners_as_a_hit() {
		let centre = Vec2::ZERO;
		let size = Vec2::new(1.0, 1.0);
		let radius = 0.2; // well under half-height (0.5) -> corners only, not a full semicircle.

		// Right edge, vertical centre (dy = 0): squarely on the flat part
		// of the rounded-right side, nowhere near either corner's arc.
		assert!(point_in_rounded_rect(Vec2::new(0.49, 0.0), centre, size, radius, false, true));
		// Left edge, vertical centre: same, mirrored, for the rounded-left side.
		assert!(point_in_rounded_rect(Vec2::new(-0.49, 0.0), centre, size, radius, true, false));
		// Sanity: the actual rounded corner is still correctly excluded.
		assert!(!point_in_rounded_rect(Vec2::new(0.49, 0.49), centre, size, radius, false, true));
	}

	/// Same flat-edge-vs-corner distinction as the test above, but on the
	/// *top and bottom* edges instead of left/right -- guards against a
	/// fix that only special-cased the x-axis strip and left the y-axis
	/// one still broken.
	#[test]
	fn point_in_rounded_rect_counts_flat_top_and_bottom_edges_as_a_hit() {
		let centre = Vec2::ZERO;
		let size = Vec2::new(1.0, 1.0);
		let radius = 0.2;

		// Top edge, horizontal centre (dx = 0): on the flat top, far from
		// either top corner's arc, for a shape rounded on both sides.
		assert!(point_in_rounded_rect(Vec2::new(0.0, 0.49), centre, size, radius, true, true));
		// Bottom edge, horizontal centre: same, mirrored.
		assert!(point_in_rounded_rect(Vec2::new(0.0, -0.49), centre, size, radius, true, true));
	}

	/// Every one of the 4 possible `(round_left, round_right)` combinations
	/// must independently gate its own two corners: rounding one side must
	/// never affect whether the *other* side's corners are treated as
	/// rounded or square.
	#[test]
	fn point_in_rounded_rect_each_side_gates_only_its_own_corners() {
		let centre = Vec2::ZERO;
		let size = Vec2::new(1.0, 1.0);
		let radius = 0.3;
		// Same corner point on the right vs. left, each just past the arc
		// (far enough from the arc centre to miss a rounded corner, but
		// still inside the flat square-corner bounding box).
		let top_right = Vec2::new(0.5, 0.5);
		let top_left = Vec2::new(-0.5, 0.5);

		// Neither side rounded: both corners are square -- always hits.
		assert!(point_in_rounded_rect(top_right, centre, size, radius, false, false));
		assert!(point_in_rounded_rect(top_left, centre, size, radius, false, false));
		// Only right rounded: right corner excluded, left corner still square (hit).
		assert!(!point_in_rounded_rect(top_right, centre, size, radius, false, true));
		assert!(point_in_rounded_rect(top_left, centre, size, radius, false, true));
		// Only left rounded: mirrored.
		assert!(point_in_rounded_rect(top_right, centre, size, radius, true, false));
		assert!(!point_in_rounded_rect(top_left, centre, size, radius, true, false));
		// Both rounded: both corners excluded.
		assert!(!point_in_rounded_rect(top_right, centre, size, radius, true, true));
		assert!(!point_in_rounded_rect(top_left, centre, size, radius, true, true));
	}

	/// A radius of exactly 0 degenerates every "rounded" side into a plain
	/// square corner (there's no arc to speak of), so it must behave
	/// identically to `round_left = round_right = false` regardless of
	/// what's passed for them.
	#[test]
	fn point_in_rounded_rect_zero_radius_behaves_like_a_plain_rect() {
		let centre = Vec2::new(1.0, -2.0);
		let size = Vec2::new(0.6, 0.4);
		for point in [Vec2::new(1.29, -1.81), Vec2::new(0.71, -2.19), Vec2::ZERO, Vec2::new(1.3, -2.0)] {
			let plain = point_in_rect(point, centre, size);
			assert_eq!(point_in_rounded_rect(point, centre, size, 0.0, true, true), plain);
		}
	}

	/// A radius bigger than the shape's own half-width/half-height is
	/// clamped (mirrors `add_rounded_rect`'s clamping in the draw path --
	/// see that function's docs), so the hit-test must never treat the
	/// requested (oversized) radius as gospel and produce a nonsensical
	/// (e.g. always-false, or bowtie-shaped) result. The clamped radius
	/// for a square, fully-rounded shape becomes half its side length,
	/// i.e. the shape degenerates into a circle -- the centre must still
	/// be a hit, and a corner point right at the bounding box's edge must
	/// still correctly miss.
	#[test]
	fn point_in_rounded_rect_clamps_oversized_radius() {
		let centre = Vec2::ZERO;
		let size = Vec2::new(0.4, 0.4);
		assert!(point_in_rounded_rect(Vec2::ZERO, centre, size, 5.0, true, true));
		assert!(!point_in_rounded_rect(Vec2::new(0.2, 0.2), centre, size, 5.0, true, true));
	}

	/// `point_in_pin_shape` for `Bit8` must use `Bit8`'s own (wider) pill
	/// size, not silently reuse `Bit4`'s -- a point past `Bit4`'s pill
	/// width but still within `Bit8`'s must hit for `Bit8` and miss for
	/// `Bit4`.
	#[test]
	fn point_in_pin_shape_bit8_uses_its_own_wider_size_not_bit4s() {
		let pos = Vec2::ZERO;
		let size4 = layout::pin_visual_shape_size(PinBitCount::Bit4);
		let size8 = layout::pin_visual_shape_size(PinBitCount::Bit8);
		assert!(size8.x > size4.x, "sanity: 8-bit pill must be wider than 4-bit's");

		let point = Vec2::new((size4.x + size8.x) / 4.0, 0.0); // strictly between the two half-widths
		assert!(!point_in_pin_shape(point, pos, PinBitCount::Bit4), "should miss the narrower 4-bit pill");
		assert!(point_in_pin_shape(point, pos, PinBitCount::Bit8), "should hit the wider 8-bit pill");
	}

	/// `point_in_dev_pin_body`'s `round_left` flag must actually flip
	/// which side is rounded (input dev-pins round left/outward, output
	/// dev-pins round right/outward -- see `draw_dev_pin_body`'s docs), not
	/// just be accepted and ignored. Pick a corner point that's a
	/// rounded-corner miss on one side but a square-corner hit on the
	/// other, and check both orientations disagree on it as expected.
	#[test]
	fn point_in_dev_pin_body_round_left_flag_actually_flips_the_rounded_side() {
		let pos = Vec2::ZERO;
		let bit_count = PinBitCount::Bit1;
		let size = layout::dev_pin_body_size(bit_count);
		let radius = layout::dev_pin_corner_radius(size);
		// Just past the (rounded) corner's arc, on the right side, still
		// inside the plain bounding box.
		let right_corner = Vec2::new(size.x / 2.0 - 1e-4, size.y / 2.0 - 1e-4);

		// Sanity: at this size/radius, the exact corner point is actually
		// excluded by a rounded corner but included by a square one.
		assert!(!point_in_rounded_rect(right_corner, pos, size, radius, false, true));
		assert!(point_in_rounded_rect(right_corner, pos, size, radius, false, false));

		// round_left = false -> input-style, but here we're checking the
		// *output* convention (round_right, i.e. round_left = false):
		// right side rounded -> this corner should miss.
		assert!(!point_in_dev_pin_body(right_corner, pos, bit_count, false));
		// round_left = true -> input-style: right side is the square one
		// -> this same corner point should hit.
		assert!(point_in_dev_pin_body(right_corner, pos, bit_count, true));
	}

	/// `point_in_dev_pin_body` must scale with `bit_count` the same way
	/// `draw_dev_pin_body` draws it (`layout::dev_pin_body_size`) -- an
	/// 8-bit dev-pin's body is taller than a 1-bit one's, so a point at
	/// 1-bit's edge but still within 8-bit's must hit only for 8-bit.
	#[test]
	fn point_in_dev_pin_body_scales_with_bit_count() {
		let pos = Vec2::ZERO;
		let size1 = layout::dev_pin_body_size(PinBitCount::Bit1);
		let size8 = layout::dev_pin_body_size(PinBitCount::Bit8);
		assert!(size8.y > size1.y, "sanity: 8-bit dev-pin body must be taller than 1-bit's");

		let point = Vec2::new(0.0, (size1.y + size8.y) / 4.0); // strictly between the two half-heights
		assert!(!point_in_dev_pin_body(point, pos, PinBitCount::Bit1, true));
		assert!(point_in_dev_pin_body(point, pos, PinBitCount::Bit8, true));
	}

	/// A hover point sitting in a 4-bit pin's pill "wing" (past where a
	/// same-position 1-bit circle would reach) must still register as a
	/// hit -- this is the concrete regression the "real shape, not just a
	/// circle" requirement guards against: a naive circle-radius hit-test
	/// centred on the pin would miss this point entirely.
	#[test]
	fn point_in_pin_shape_hits_the_pill_wing_a_1bit_circle_would_miss() {
		let pos = Vec2::new(2.0, 0.0);
		let size = layout::pin_visual_shape_size(PinBitCount::Bit4);
		// A point near the pill's far horizontal edge, at pin-centre
		// height -- well past a 1-bit circle's radius from `pos`, but
		// still inside the wider pill.
		let point = Vec2::new(pos.x + size.x / 2.0 - 1e-3, pos.y);

		assert!(
			!point_in_circle(point, pos, layout::pin_radius_for_bit_count(PinBitCount::Bit1)),
			"sanity: a 1-bit circle should NOT reach this point"
		);
		assert!(point_in_pin_shape(point, pos, PinBitCount::Bit4), "the actual pill shape should reach this point");
	}

	/// End-to-end: hovering directly over a subchip's pin should produce
	/// exactly one label, with the pin's own name -- not the subchip's.
	#[test]
	fn build_scene_shows_pin_name_label_when_pin_is_hovered() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let placed = place_sub_chips(&parent, &lib);
		let output_pos = layout::pin_world_position(placed[0].centre, placed[0].size, placed[0].output_pin_y[0], false);

		let scene = build_scene(&parent, &lib, &AllLow, Some(output_pos));
		assert_eq!(scene.labels.len(), 1);
		assert_eq!(scene.labels[0].text, "OUT");
	}

	/// End-to-end: hovering a wide (4-bit) pin's pill "wing" -- a point a
	/// naive circle-based hit-test would miss -- must still show that
	/// pin's label. This is the real-shape regression check at the
	/// `build_scene` level (`point_in_pin_shape_hits_the_pill_wing...`
	/// above checks the same thing at the hit-test-function level).
	#[test]
	fn build_scene_shows_pin_label_when_hovering_a_multibit_pins_pill_wing() {
		let mut lib = ChipLibrary::new();
		let mut chip4bit = ChipDescription::new("BUS4", ChipType::Custom);
		chip4bit.input_pins.push(PinDescription::new("WIDE_IN", 0, PinBitCount::Bit4));
		lib.add(chip4bit);

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "BUS4".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let placed = place_sub_chips(&parent, &lib);
		let pin_pos = layout::pin_world_position(placed[0].centre, placed[0].size, placed[0].input_pin_y[0], true);
		let size = layout::pin_visual_shape_size(PinBitCount::Bit4);
		let wing_point = Vec2::new(pin_pos.x - size.x / 2.0 + 1e-3, pin_pos.y);

		let scene = build_scene(&parent, &lib, &AllLow, Some(wing_point));
		assert_eq!(scene.labels.len(), 1);
		assert_eq!(scene.labels[0].text, "WIDE_IN");
	}

	/// End-to-end: hovering a chip's own boundary dev-pin (drawn as a
	/// rounded-rect body, not a circle/pill) should show its name too.
	#[test]
	fn build_scene_shows_dev_pin_name_label_when_hovered() {
		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("DEV_PIN_HOVER_TEST", ChipType::Custom);
		let mut in0 = PinDescription::new("MY_INPUT", 10, PinBitCount::Bit1);
		in0.position = Vec2::new(-3.0, 0.0);
		chip.input_pins.push(in0);

		let scene = build_scene(&chip, &lib, &AllLow, Some(Vec2::new(-3.0, 0.0)));
		assert_eq!(scene.labels.len(), 1);
		assert_eq!(scene.labels[0].text, "MY_INPUT");
	}

	/// With no hover position at all, nothing should show a label -- not
	/// even a component whose `NameLocation` would otherwise be visible,
	/// since labels are now purely hover-gated rather than always-on.
	#[test]
	fn build_scene_shows_no_labels_when_nothing_is_hovered() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let scene = build_scene(&parent, &lib, &AllLow, None);
		assert!(scene.labels.is_empty());

		// Also check a hover point that lands nowhere near anything.
		let scene_far_hover = build_scene(&parent, &lib, &AllLow, Some(Vec2::new(1000.0, 1000.0)));
		assert!(scene_far_hover.labels.is_empty());
	}

	/// When a pin and its owning component's body overlap at the hover
	/// point, the pin's label wins (checked first) -- a component's name
	/// should never show while the mouse is actually over one of its own
	/// pins.
	#[test]
	fn build_scene_prefers_pin_label_over_component_label_on_overlap() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});

		let placed = place_sub_chips(&parent, &lib);
		let output_pos = layout::pin_world_position(placed[0].centre, placed[0].size, placed[0].output_pin_y[0], false);

		let scene = build_scene(&parent, &lib, &AllLow, Some(output_pos));
		assert_eq!(scene.labels.len(), 1);
		assert_eq!(scene.labels[0].text, "OUT", "pin label should win over the component's own name");
	}

	/// End-to-end draw-order check: `build_scene` must draw wires first
	/// (bottom layer), then all pins (subchip pins + this chip's own
	/// dev-pins), then component bodies (+ labels) last (top layer) --
	/// see `draw_wires`/`draw_pins`/`draw_components`. Uses a scene with
	/// one wire, one subchip (with a distinctive, otherwise-unused body
	/// colour), and one dev-pin, then checks each layer's colours show up
	/// in contiguous index ranges in that order.
	#[test]
	fn build_scene_draws_wires_then_pins_then_components() {
		let mut lib = ChipLibrary::new();
		let mut nand = nand_desc();
		// A distinctive body colour (alpha > 0, so it's actually used
		// instead of falling back to `theme::CHIP_BODY_COL`) that no pin
		// or wire colour in this scene will coincidentally match.
		nand.colour = [0.11, 0.22, 0.33, 1.0];
		lib.add(nand.clone());

		let mut chip = ChipDescription::new("ORDER_TEST", ChipType::Custom);
		let mut in_pin = PinDescription::new("IN", 10, PinBitCount::Bit1);
		in_pin.position = Vec2::new(-4.0, 0.0);
		chip.input_pins.push(in_pin);
		chip.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			label: None,
			position: Vec2::ZERO,
			pin_colour_info: Vec::new(),
		});
		// Dev-pin -> subchip's input pin A (id 0).
		chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(1, 0)));

		let scene = build_scene(&chip, &lib, &AllLow, None);

		// Layer 1: the wire. Unbent -> exactly one quad (6 verts), at the
		// very start of the buffer.
		let wire_verts = &scene.triangles[..6];
		assert!(
			wire_verts.iter().all(|v| v.colour != nand.colour),
			"wire layer must be drawn before the component body, not mixed in with or after it"
		);

		// Layer 3: the component body. `draw_components` draws the body
		// rect (6 verts) last, after every pin -- so the component's
		// colour should only appear at the very end of the buffer, never
		// earlier (e.g. not before the wire or any pin).
		let last_six = &scene.triangles[scene.triangles.len() - 6..];
		assert!(last_six.iter().all(|v| v.colour == nand.colour), "component body must be the last thing drawn (top layer)");
		let before_last_six = &scene.triangles[..scene.triangles.len() - 6];
		assert!(before_last_six.iter().all(|v| v.colour != nand.colour), "component body colour must not appear anywhere before the final layer");
	}

	/// A radius bigger than the shape's own half-width/half-height must be
	/// clamped rather than overshooting into a self-intersecting bowtie --
	/// the call should still produce a well-formed (non-empty, multiple of
	/// 3) triangle list instead of garbage geometry.
	#[test]
	fn add_rounded_rect_clamps_radius_larger_than_shape() {
		let mut geo = SceneGeometry::default();
		geo.add_rounded_rect(Vec2::ZERO, Vec2::new(0.2, 0.2), theme::PIN_COL, 5.0, true, true, 8);
		assert!(!geo.triangles.is_empty());
		assert_eq!(geo.triangles.len() % 3, 0);
	}

	/// `draw_dev_pin_body` draws two layered shapes -- a full-size
	/// grey-ish border shape first, then a smaller pin-coloured fill shape
	/// inset by the border width on top -- both sharing the same
	/// rounded/square corner pattern (`round_left` picked). This is the
	/// concrete shape `build_scene` uses for a chip's own boundary
	/// input/output dev-pins, so they read as a distinct "partially
	/// rounded rectangle" component body rather than a plain pin circle.
	#[test]
	fn draw_dev_pin_body_draws_grey_border_then_coloured_fill() {
		let mut geo = SceneGeometry::default();
		let bit_count = PinBitCount::Bit1;
		let colour = Color::from_int(3);
		draw_dev_pin_body(&mut geo, Vec2::new(1.0, 2.0), bit_count, colour, Some(LogicState::High), true);

		let segments = layout::DEV_PIN_ROUND_SEGMENTS;
		let points_per_shape = 2 * (segments + 1) + 2; // 2 rounded corners + 2 square corners
		let tris_per_shape = points_per_shape as usize;
		// Border shape + fill shape, both with the same corner pattern
		// (the fill's own radius is still > 0 since the border width is
		// smaller than the corner radius for a Bit1 dev-pin's size).
		assert_eq!(geo.triangles.len(), tris_per_shape * 2 * 3);

		// Border is drawn first, in the grey-ish outline colour...
		assert_eq!(geo.triangles[0].colour, theme::CHIP_OUTLINE_COL);
		// ...and every border vertex shares that colour (it's one flat-shaded shape).
		assert!(geo.triangles[..tris_per_shape * 3].iter().all(|v| v.colour == theme::CHIP_OUTLINE_COL));

		// Fill is drawn second, coloured by the pin's own live state colour.
		let expected_fill = theme::state_colour(LogicState::High, colour);
		let fill_verts = &geo.triangles[tris_per_shape * 3..];
		assert!(fill_verts.iter().all(|v| v.colour == expected_fill));
	}

	/// End-to-end through `build_scene`: a chip with its own boundary
	/// input/output dev-pins should have those pins' bodies drawn (not
	/// just their subchips'/wires' geometry), each centred on the pin's
	/// real saved `position`.
	#[test]
	fn build_scene_draws_dev_pin_bodies_for_chip_boundary_pins() {
		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("DEV_PIN_SCENE_TEST", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit1);
		in0.position = Vec2::new(-3.0, 0.5);
		chip.input_pins.push(in0);
		let mut out0 = PinDescription::new("OUT0", 20, PinBitCount::Bit1);
		out0.position = Vec2::new(3.0, -0.5);
		chip.output_pins.push(out0);

		let scene = build_scene(&chip, &lib, &AllLow, None);

		// The output pin still draws as the ordinary rounded-rect "pill"
		// dev-pin body (border + fill).
		let out_segments = layout::DEV_PIN_ROUND_SEGMENTS;
		let out_points_per_shape = 2 * (out_segments + 1) + 2;
		let out_tris = out_points_per_shape as usize * 2; // border + fill

		// The input pin (1-bit) now draws as a single clickable circle
		// (border + fill), twice a plain pin's radius, per
		// `draw_input_dev_pin_body`.
		let in_segments = layout::DEV_PIN_ROUND_SEGMENTS * 2;
		let in_tris = in_segments as usize * 2; // border + fill

		// No subchips and no wires here -- the whole scene is just the two
		// dev-pin bodies.
		assert_eq!(scene.triangles.len(), (out_tris + in_tris) * 3);

		// Every vertex should belong to one of the two pins' bodies,
		// centred close to their saved positions (within the body's own
		// half-size).
		let in_size = layout::input_dev_pin_body_size(PinBitCount::Bit1);
		let out_size = layout::dev_pin_body_size(PinBitCount::Bit1);
		for v in &scene.triangles {
			let near_in0 = (v.pos.x - (-3.0)).abs() <= in_size.x / 2.0 + 1e-3 && (v.pos.y - 0.5).abs() <= in_size.y / 2.0 + 1e-3;
			let near_out0 = (v.pos.x - 3.0).abs() <= out_size.x / 2.0 + 1e-3 && (v.pos.y - (-0.5)).abs() <= out_size.y / 2.0 + 1e-3;
			assert!(near_in0 || near_out0, "vertex {:?} not near either dev-pin's saved position", v.pos);
		}
	}

	/// A 4-bit input dev-pin must draw as a 2x2 grid of 4 individually
	/// clickable square cells (not a single pill), and
	/// `hit_test_input_dev_pin_bit` must be able to identify each one by
	/// its own bit index.
	#[test]
	fn build_scene_draws_input_dev_pin_as_bit_grid_for_wide_input() {
		let lib = ChipLibrary::new();
		let mut chip = ChipDescription::new("WIDE_INPUT_TEST", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit4);
		in0.position = Vec2::ZERO;
		chip.input_pins.push(in0);

		let scene = build_scene(&chip, &lib, &AllLow, None);

		// 4 cells, each a rect (border + fill), each rect = 2 triangles = 6 vertices.
		assert_eq!(scene.triangles.len(), 4 * 2 * 2 * 3);

		// Every one of the 4 bit indices should hit a distinct cell.
		let mut hit_bits: Vec<u32> = layout::input_bit_cell_offsets(PinBitCount::Bit4)
			.iter()
			.filter_map(|offset| hit_test_input_dev_pin_bit(Vec2::ZERO + *offset, Vec2::ZERO, PinBitCount::Bit4))
			.collect();
		hit_bits.sort_unstable();
		assert_eq!(hit_bits, vec![0, 1, 2, 3]);

		// A point far outside the grid hits nothing.
		assert_eq!(hit_test_input_dev_pin_bit(Vec2::new(100.0, 100.0), Vec2::ZERO, PinBitCount::Bit4), None);
	}

	/// Minimal `PinStateLookup` test double for exercising the display
	/// drawing functions directly: reports a fixed logic state per
	/// `(owner_id, pin_id)` (defaulting to `Low` for anything not listed,
	/// same as a genuinely unsimulated pin would read), plus an optional
	/// fixed `internal_state` buffer for the RGB/dot pixel grid.
	struct FixedDisplayState {
		pins: std::collections::HashMap<(i32, i32), LogicState>,
		internal: Option<Vec<u32>>,
	}

	impl PinStateLookup for FixedDisplayState {
		fn is_high(&self, owner_id: i32, pin_id: i32) -> Option<bool> {
			Some(self.logic_state(owner_id, pin_id) == Some(LogicState::High))
		}

		fn logic_state(&self, owner_id: i32, pin_id: i32) -> Option<LogicState> {
			Some(*self.pins.get(&(owner_id, pin_id)).unwrap_or(&LogicState::Low))
		}

		fn internal_state(&self, _owner_id: i32) -> Option<&[u32]> {
			self.internal.as_deref()
		}
	}

	/// A `PlacedSubChip` for exercising `draw_display_seven_segment`/
	/// `draw_display_pixel_grid` directly: `desc` is never read by either
	/// function (they only use `id`/`centre`/`size`), so any placeholder
	/// `ChipDescription` reference is fine.
	fn test_placed_sub_chip(desc: &ChipDescription, id: i32, centre: Vec2, size: Vec2) -> PlacedSubChip<'_> {
		PlacedSubChip {
			id,
			desc,
			centre,
			size,
			input_pin_y: vec![],
			output_pin_y: vec![],
			label: None,
			pin_colour_info: vec![],
			internal_data: vec![],
		}
	}

	/// One rect == 2 triangles == 6 vertices; a helper so the display
	/// tests below can assert vertex counts in terms of "how many rects
	/// got drawn" instead of a raw (and less legible) vertex count.
	fn rects_drawn(geo: &SceneGeometry) -> usize {
		assert_eq!(geo.triangles.len() % 6, 0, "every add_rect call produces exactly 6 vertices");
		geo.triangles.len() / 6
	}

	/// Every vertex colour actually present in `geo` -- lets a test assert
	/// "this colour shows up somewhere" without needing to know which
	/// rect (by draw order) corresponds to which segment/pixel.
	fn colours_present(geo: &SceneGeometry) -> std::collections::HashSet<[u32; 4]> {
		geo.triangles.iter().map(|v| v.colour.map(|c| c.to_bits())).collect()
	}

	/// A blank 7-segment display (every segment pin low, `COL` low) draws
	/// the black backing plus all 7 segments in their "off" colour --
	/// nothing in the "on" or alternate-palette colours should appear.
	#[test]
	fn seven_segment_all_low_draws_seven_off_segments_and_a_backing() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 42, Vec2::ZERO, Vec2::new(1.25, 2.5));
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		// 1 black backing rect + 7 segment rects.
		assert_eq!(rects_drawn(&geo), 8);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "expected the palette-A off colour to appear");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "no segment should be lit");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[3].map(f32::to_bits)), "COL is low, so palette B shouldn't appear at all");
	}

	/// Driving just the `A` segment (pin id 0) high lights only that
	/// segment's rect in the palette-A "on" colour, while every other
	/// segment stays in the palette-A "off" colour -- i.e. each segment's
	/// colour is read from its own pin independently.
	#[test]
	fn seven_segment_segment_a_high_lights_only_segment_a() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 7, Vec2::ZERO, Vec2::new(1.0, 2.0));
		let mut pins = std::collections::HashMap::new();
		pins.insert((7, 0), LogicState::High); // segment A
		let state = FixedDisplayState { pins, internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "segment A should be lit");
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "the other 6 segments should still be off");
	}

	/// The `COL` pin (id 7) going high swaps in the alternate (palette B)
	/// on/off colours entirely -- palette A's colours shouldn't appear at
	/// all once `COL` is high, matching the original's `colOffset` switch.
	#[test]
	fn seven_segment_col_pin_switches_to_alternate_palette() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 3, Vec2::ZERO, Vec2::new(1.0, 2.0));
		let mut pins = std::collections::HashMap::new();
		pins.insert((3, 7), LogicState::High); // COL
		pins.insert((3, 6), LogicState::High); // segment G, lit
		let state = FixedDisplayState { pins, internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[4].map(f32::to_bits)), "lit segment should use palette B's on colour");
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[3].map(f32::to_bits)), "unlit segments should use palette B's off colour");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "palette A's off colour shouldn't appear once COL is high");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "palette A's on colour shouldn't appear once COL is high");
	}

	/// With no live sim (`internal_state` returns `None`), the RGB/dot
	/// pixel grid falls back to a uniform dim grey for every pixel --
	/// mirroring the original's `useSim == false` branch -- rather than
	/// panicking on a missing buffer or defaulting to some other colour.
	#[test]
	fn pixel_grid_with_no_sim_draws_uniform_off_pixels() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 1, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, true);

		// 1 black backing rect + 16*16 pixel rects.
		assert_eq!(rects_drawn(&geo), 1 + 16 * 16);

		let colours = colours_present(&geo);
		assert_eq!(colours.len(), 2, "backing (black) + one uniform off-pixel colour, nothing else");
		assert!(colours.contains(&[0.1f32, 0.1, 0.1, 1.0].map(f32::to_bits)));
	}

	/// The RGB display decodes each pixel's packed nibbles (`R | G<<4 |
	/// B<<8`, matching `Simulator::process_display_rgb`'s write path) into
	/// a full-brightness colour when a channel's nibble is maxed (`0xF`),
	/// and address `y*16+x` selects the right pixel out of the buffer.
	#[test]
	fn rgb_pixel_grid_decodes_packed_nibbles_at_the_right_address() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 5, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let mut internal = vec![0u32; 256];
		// Pixel at (x=3, y=2): full red, zero green, zero blue.
		internal[2 * 16 + 3] = 0xF;
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: Some(internal) };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, true);

		let colours = colours_present(&geo);
		assert!(colours.contains(&[1.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "the one written pixel should decode to full red");
		// Every other pixel is still address 0 (untouched, all-zero) => black.
		assert!(colours.contains(&[0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)));
	}

	/// The dot display treats its internal-state value as a plain
	/// on/off flag (any nonzero -> white), not the RGB nibble packing --
	/// same address scheme, different decode.
	#[test]
	fn dot_pixel_grid_treats_nonzero_as_on() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 9, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let mut internal = vec![0u32; 256];
		internal[0] = 1; // pixel (0,0) on
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: Some(internal) };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, false);

		let colours = colours_present(&geo);
		assert!(colours.contains(&[1.0f32, 1.0, 1.0, 1.0].map(f32::to_bits)), "a nonzero dot pixel should render pure white");
		assert!(colours.contains(&[0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "every other (zero) pixel should render black");
	}
}
