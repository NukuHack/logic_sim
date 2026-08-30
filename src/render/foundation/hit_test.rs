//! Point-in-shape tests mirroring the drawing primitives in
//! [`crate::render::foundation::geometry`] exactly, so hover/click
//! hit-testing lines up with what's actually on screen instead of
//! assuming a simpler shape than the one that was drawn.

use crate::structs::Vec2;

/// A point-in-shape test matching `SceneGeometry::add_rounded_rect`'s actual drawn geometry
/// exactly (same corner-rounding rules), so hover hit-testing lines up with what's on screen
/// instead of assuming every pin is a plain circle.
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
	// A point only falls within a rounded corner's own carved-out region when it's simultaneously past
	// the corner threshold on both axes (dx AND dy, not either alone) -- gating on dx alone wrongly
	// treats a pill's whole flat side as corner territory, breaking hover hit-testing on those points.
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
/// pin's connection dot -- see `scene::pins::draw_pin_shape`).
pub fn point_in_circle(point: Vec2, centre: Vec2, radius: f32) -> bool {
	let dx = point.x - centre.x;
	let dy = point.y - centre.y;
	dx * dx + dy * dy <= radius * radius
}

/// A plain axis-aligned rectangle hit-test -- for a subchip's body (which,
/// unlike its pins, is never rounded) and any other centred-rect region.
pub fn point_in_rect(point: Vec2, centre: Vec2, size: Vec2) -> bool {
	(point.x - centre.x).abs() <= size.x / 2.0 && (point.y - centre.y).abs() <= size.y / 2.0
}
