//! Polyline offsetting with proper miter joins: the shared maths behind
//! stroked wire ribbons and per-bit bus strand layout.

use crate::structs::Vec2;

/// Offsets every point of a polyline sideways by a constant perpendicular `distance`
/// (positive = to the left of each segment's direction of travel, i.e. rotate the segment
/// direction +90 degrees; negative = to the right), producing a new polyline that stays
/// exactly `distance` away from the original at every point along it -- including through
/// bends, via a proper miter join at each interior vertex, rather than naively offsetting
/// each segment independently and leaving a gap/overlap where two differently-offset segments
/// would otherwise meet.
pub fn offset_polyline(points: &[Vec2], distance: f32) -> Vec<Vec2> {
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
