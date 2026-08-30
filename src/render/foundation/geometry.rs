//! Flat-colour triangle geometry: the drawable primitives every scene/UI builder composes from.
//! Pure data in, triangles out, zero GPU dependencies -- `render::gpu` converts these 1:1 into its
//! own bytemuck vertex, and everything above this layer stays unit-testable without a GPU.

use crate::render::foundation::polyline::offset_polyline;
use crate::render::theme::Rgba;
use crate::structs::Vec2;

/// A single coloured vertex, position in world space. Kept separate from
/// any wgpu `Vertex` type so this module has zero GPU dependencies; the
/// `render::gpu` module converts these 1:1 into its own bytemuck vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVertex {
	pub pos: Vec2,
	pub colour: Rgba,
}

/// A gate/chip name label to be drawn as text, in world space. Produced
/// alongside `triangles` by scene builders -- kept as a separate list (rather
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

/// Which of a rounded rectangle's left/right vertical edges get rounded
/// corners (see [`SceneGeometry::add_rounded_rect`]). Bundled into a
/// struct so the call reads as a single "corners" concept instead of two
/// loose booleans.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RoundCorners {
	pub left: bool,
	pub right: bool,
}

impl RoundCorners {
	pub const NONE: Self = Self { left: false, right: false };
	pub const BOTH: Self = Self { left: true, right: true };
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

	/// A filled rectangle with an outline rendered behind it: an `outline_colour` rect at the
	/// full `size`, with the `fill_colour` rect inset by `border` on every side drawn on top, so
	/// the outline reads as a border of that thickness rather than being fully covered.
	pub fn add_outlined_rect(&mut self, centre: Vec2, size: Vec2, border: f32, fill_colour: Rgba, outline_colour: Rgba) {
		self.add_rect(centre, size, outline_colour);
		let inner = Vec2::new((size.x - border * 2.0).max(0.0), (size.y - border * 2.0).max(0.0));
		self.add_rect(centre, inner, fill_colour);
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

	/// [`Self::add_outlined_rect`]'s circle counterpart: an
	/// `outline_colour` disc at the full `radius` with a `fill_colour`
	/// disc of `radius - border` on top.
	pub fn add_outlined_circle(&mut self, centre: Vec2, radius: f32, border: f32, fill_colour: Rgba, outline_colour: Rgba, segments: u32) {
		self.add_circle(centre, radius, outline_colour, segments);
		self.add_circle(centre, (radius - border).max(0.0), fill_colour, segments);
	}

	/// A rectangle of `size` centred on `centre`, with its corners rounded to `radius` on
	/// whichever of its left/right vertical edges `corners` selects (either, both, or neither --
	/// the other edge's corners stay sharp). Implemented as a fan of triangles from `centre`
	/// around the perimeter (rounded corners contribute an arc of points, square corners
	/// contribute just their one corner point), the same triangulation strategy `add_circle`
	/// uses -- valid here because a rounded rect (with radius capped to half the smaller
	/// dimension) is always convex/star-shaped from its own centre.
	pub fn add_rounded_rect(&mut self, centre: Vec2, size: Vec2, colour: Rgba, radius: f32, corners: RoundCorners, corner_segments: u32) {
		let hw = size.x / 2.0;
		let hh = size.y / 2.0;
		if hw <= 0.0 || hh <= 0.0 {
			return;
		}
		let r = radius.max(0.0).min(hw).min(hh);
		let segs = corner_segments.max(1);

		struct CornerSpec {
			centre: Vec2,
			arc_centre: Vec2,
			start: f32,
			end: f32,
			rounded: bool,
		}
		fn push_corner(points: &mut Vec<Vec2>, spec: &CornerSpec, r: f32, segs: u32) {
			if spec.rounded && r > 1e-6 {
				for i in 0..=segs {
					let t = i as f32 / segs as f32;
					let a = spec.start + t * (spec.end - spec.start);
					points.push(Vec2::new(spec.arc_centre.x + a.cos() * r, spec.arc_centre.y + a.sin() * r));
				}
			} else {
				points.push(spec.centre);
			}
		}

		use std::f32::consts::PI;
		let mut points: Vec<Vec2> = Vec::new();
		// Bottom-right -> top-right -> top-left -> bottom-left (CCW).
		push_corner(
			&mut points,
			&CornerSpec {
				centre: Vec2::new(centre.x + hw, centre.y - hh),
				arc_centre: Vec2::new(centre.x + hw - r, centre.y - hh + r),
				start: -PI / 2.0,
				end: 0.0,
				rounded: corners.right,
			},
			r,
			segs,
		);
		push_corner(
			&mut points,
			&CornerSpec {
				centre: Vec2::new(centre.x + hw, centre.y + hh),
				arc_centre: Vec2::new(centre.x + hw - r, centre.y + hh - r),
				start: 0.0,
				end: PI / 2.0,
				rounded: corners.right,
			},
			r,
			segs,
		);
		push_corner(
			&mut points,
			&CornerSpec {
				centre: Vec2::new(centre.x - hw, centre.y + hh),
				arc_centre: Vec2::new(centre.x - hw + r, centre.y + hh - r),
				start: PI / 2.0,
				end: PI,
				rounded: corners.left,
			},
			r,
			segs,
		);
		push_corner(
			&mut points,
			&CornerSpec {
				centre: Vec2::new(centre.x - hw, centre.y - hh),
				arc_centre: Vec2::new(centre.x - hw + r, centre.y - hh + r),
				start: PI,
				end: 3.0 * PI / 2.0,
				rounded: corners.left,
			},
			r,
			segs,
		);

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

	/// A thick polyline through `points`, drawn as one continuous ribbon with proper mitered
	/// joins at every interior vertex -- unlike drawing each segment as its own independent
	/// `add_line` rectangle (which leaves a visible gap or overlap at any bend that isn't
	/// perfectly straight), this keeps the two edges of the ribbon touching exactly at each
	/// bend.
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

/// Scales every triangle vertex's and label's alpha channel by `alpha`, leaving RGB untouched --
/// used to fade an already-built scene (e.g. a chip pending placement, floating translucently at
/// the cursor) without needing a second draw path just for blending.
pub fn apply_alpha(geo: &mut SceneGeometry, alpha: f32) {
	for v in &mut geo.triangles {
		v.colour[3] *= alpha;
	}
	for l in &mut geo.labels {
		l.colour[3] *= alpha;
	}
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
