//! The editor canvas background: a world-space grid that thins out as the
//! camera zooms out so it never degrades into visual noise.

use crate::render::camera::Camera;
use crate::render::foundation::SceneGeometry;
use crate::render::layout;
use crate::render::theme::Rgba;
use crate::structs::Vec2;

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

	// World-space half-extents of the current view -- equivalent to the original's `orthographicSize`
	// (half-height) and `orthographicSize * aspect` (half-width); this camera already folds aspect
	// ratio into `viewport_width`/`viewport_height` directly.
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

	// World-space thickness widened, if needed, so lines never render thinner than ~1.5 screen pixels
	// -- see `layout::grid_line_thickness` docs for why a flat, non-antialiased quad needs this to
	// avoid a patchy/inconsistent-looking grid once zoomed out.
	let thickness = layout::grid_line_thickness(camera.zoom);

	// `left`/`right`/`top`/`bottom` are already exact multiples of `GRID_SIZE` (0.125, exactly
	// representable in binary floating point), so converting to integer grid indices up front is
	// exact -- avoids the float-accumulation drift a `+= GRID_SIZE` loop would risk at high zoom.
	let left_i = (left / layout::GRID_SIZE).round() as i32;
	let right_i = (right / layout::GRID_SIZE).round() as i32;
	let bottom_i = (bottom / layout::GRID_SIZE).round() as i32;
	let top_i = (top / layout::GRID_SIZE).round() as i32;

	// Defensive cap: a degenerate camera (near-zero zoom, or a 0 viewport before the first resize
	// event) can otherwise blow these bounds out to i32::MIN..i32::MAX, turning this into a
	// multi-billion-iteration loop that hangs the app and exhausts memory. No real view needs more.
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn grid_line_skip_increases_as_view_zooms_out() {
		assert_eq!(grid_line_skip(0.0), 1);
		assert_eq!(grid_line_skip(7.99), 1);
		assert_eq!(grid_line_skip(8.0), 4);
		assert_eq!(grid_line_skip(31.99), 4);
		assert_eq!(grid_line_skip(32.0), 16);
		assert_eq!(grid_line_skip(1000.0), 16);
	}
}
