//! Rendering-primitive integration tests: flat-colour geometry builders
//! and their miter-join polyline maths, point-in-shape hit tests, camera
//! transforms, the colour palette, chip/pin layout math, and the CPU-side
//! GPU-vertex conversion -- everything reachable without a GPU device.

use logic_sim::description::{Color, PinBitCount};
use logic_sim::pin_state::LogicState;
use logic_sim::render::camera::Camera;
use logic_sim::render::foundation::{
	apply_alpha, bounding_box, offset_polyline, point_in_rect, point_in_rounded_rect, RoundCorners, SceneGeometry, SceneVertex, TextLabel,
};
use logic_sim::render::gpu::{scene_to_vertices, upload_ready_bytes, Vertex};
use logic_sim::render::layout::{
	calculate_default_pin_layout, calculate_min_chip_size_for_pins, estimate_text_width, grid_line_thickness, min_chip_height_for_pins,
	pin_world_position, snap_to_grid, snap_to_grid_scalar, AVG_CHAR_WIDTH_RATIO, GRID_MIN_PIXEL_THICKNESS, GRID_SIZE, GRID_THICKNESS,
	SUB_CHIP_PIN_INSET,
};
use logic_sim::render::theme::{self, dim, state_colour, text_colour_for_background, COLORS, STATE_DISCONNECTED_COL};
use logic_sim::Vec2;

#[test]
fn rect_produces_two_triangles_six_verts() {
	let mut geo = SceneGeometry::default();
	geo.add_rect(Vec2::ZERO, Vec2::new(2.0, 1.0), theme::CHIP_BODY_COL);
	assert_eq!(geo.triangles.len(), 6);
}

#[test]
fn apply_alpha_scales_triangle_and_label_alpha_leaving_rgb_untouched() {
	let mut geo = SceneGeometry::default();
	geo.add_rect(Vec2::ZERO, Vec2::new(2.0, 1.0), [0.5, 0.4, 0.3, 1.0]);
	geo.labels.push(TextLabel { pos: Vec2::ZERO, text: "AND".into(), colour: [0.1, 0.2, 0.3, 0.8], font_size: 12.0, width: 20.0 });

	apply_alpha(&mut geo, 0.75);

	assert!(geo.triangles.iter().all(|v| v.colour[0] == 0.5 && v.colour[1] == 0.4 && v.colour[2] == 0.3 && (v.colour[3] - 0.75).abs() < 1e-6));
	let label_colour = geo.labels[0].colour;
	assert!(label_colour[0] == 0.1 && label_colour[1] == 0.2 && label_colour[2] == 0.3 && (label_colour[3] - 0.6).abs() < 1e-6);
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
fn outlined_rect_draws_full_size_outline_with_fill_inset_by_border() {
	let mut geo = SceneGeometry::default();
	geo.add_outlined_rect(Vec2::ZERO, Vec2::new(4.0, 2.0), 0.5, theme::PIN_COL, theme::CHIP_OUTLINE_COL);
	// Outline quad (6 verts) + inset fill quad (6 verts).
	assert_eq!(geo.triangles.len(), 12);

	// First quad is the full-size outline; second is the inset fill.
	let outline_max_x = geo.triangles[..6].iter().map(|v| v.pos.x).fold(f32::MIN, f32::max);
	let fill_max_x = geo.triangles[6..].iter().map(|v| v.pos.x).fold(f32::MIN, f32::max);
	assert_eq!(outline_max_x, 2.0);
	assert_eq!(fill_max_x, 1.5);

	assert!(geo.triangles[..6].iter().all(|v| v.colour == theme::CHIP_OUTLINE_COL));
	assert!(geo.triangles[6..].iter().all(|v| v.colour == theme::PIN_COL));
}

#[test]
fn outlined_rect_clamps_border_to_the_shapes_half_size() {
	let mut geo = SceneGeometry::default();
	geo.add_outlined_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), 5.0, theme::PIN_COL, theme::CHIP_OUTLINE_COL);
	// The fill's inset collapses to a degenerate zero-size quad at the
	// centre (same as the inline border-then-fill code this helper
	// replaced always produced), so only the outline is visible.
	assert_eq!(geo.triangles.len(), 12);
	assert!(geo.triangles[..6].iter().all(|v| v.colour == theme::CHIP_OUTLINE_COL));
	assert!(geo.triangles[6..].iter().all(|v| v.pos == Vec2::ZERO && v.colour == theme::PIN_COL));
}

#[test]
fn outlined_circle_matches_add_outlined_rects_layering() {
	let mut geo = SceneGeometry::default();
	geo.add_outlined_circle(Vec2::ZERO, 1.0, 0.25, theme::PIN_COL, theme::CHIP_OUTLINE_COL, 12);
	// Outline fan (12 segments * 3 verts) + inset fill fan.
	assert_eq!(geo.triangles.len(), 24 * 3);

	let outline_radius = geo.triangles[..36].iter().map(|v| v.pos.x.abs()).fold(0.0_f32, f32::max);
	let fill_radius = geo.triangles[36..].iter().map(|v| v.pos.x.abs()).fold(0.0_f32, f32::max);
	assert!((outline_radius - 1.0).abs() < 1e-6);
	assert!((fill_radius - 0.75).abs() < 1e-6);
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

/// A square (`round_left = round_right = false`) rounded-rect degenerates
/// to a plain rectangle: 4 corner points, fan-triangulated into 4
/// triangles, regardless of the radius passed in.
#[test]
fn add_rounded_rect_with_no_rounded_side_is_a_plain_rectangle() {
	let mut geo = SceneGeometry::default();
	geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, RoundCorners::NONE, 8);
	assert_eq!(geo.triangles.len(), 4 * 3);
}

/// Rounding one side (but not the other) adds `segments + 1` arc points
/// per rounded corner (2 corners) on top of the 2 remaining square
/// corners, all fan-triangulated from the centre.
#[test]
fn add_rounded_rect_with_one_rounded_side_has_expected_triangle_count() {
	let mut geo = SceneGeometry::default();
	let segments = 8;
	geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, RoundCorners { left: true, right: false }, segments);
	let expected_points = 2 * (segments + 1) + 2;
	assert_eq!(geo.triangles.len(), expected_points as usize * 3);
}

/// A radius bigger than the shape's own half-width/half-height must be
/// clamped rather than overshooting into a self-intersecting bowtie --
/// the call should still produce a well-formed (non-empty, multiple of
/// 3) triangle list instead of garbage geometry.
#[test]
fn add_rounded_rect_clamps_radius_larger_than_shape() {
	let mut geo = SceneGeometry::default();
	geo.add_rounded_rect(Vec2::ZERO, Vec2::new(0.2, 0.2), theme::PIN_COL, 5.0, RoundCorners::BOTH, 8);
	assert!(!geo.triangles.is_empty());
	assert_eq!(geo.triangles.len() % 3, 0);
}

/// Mirrors `add_rounded_rect`'s actual drawn shape: a corner flagged as rounded excludes its own square corner
/// area (a point out past the arc, still inside the raw bounding box,
/// must NOT count as a hit), while a corner left square still counts
/// its whole box as a hit.
#[test]
fn point_in_rounded_rect_respects_rounded_vs_square_corners() {
	let centre = Vec2::ZERO;
	let size = Vec2::new(1.0, 1.0);
	let radius = 0.3;

	// Top-right corner, rounded (round_right = true): the exact bounding-box corner (0.5, 0.5) is
	// well outside the arc (arc centre (0.2, 0.2), radius 0.3 -> corner is ~0.424 from the arc centre).
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
	// Only left rounded: mirrored -- the *left* corners are now the
	// excluded ones, the right side stays square (hit).
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

#[test]
fn screen_to_world_round_trips_through_world_to_screen() {
	let cam = Camera::new(Vec2::new(1920.0, 1080.0));
	let world = Vec2::new(3.5, -2.25);
	let screen = cam.world_to_screen(world);
	let back = cam.screen_to_world(screen);
	assert!((back.x - world.x).abs() < 1e-4);
	assert!((back.y - world.y).abs() < 1e-4);
}

#[test]
fn viewport_centre_maps_to_camera_position() {
	let mut cam = Camera::new(Vec2::new(800.0, 600.0));
	cam.position = Vec2::new(10.0, -5.0);
	let centre_world = cam.screen_to_world(Vec2::new(400.0, 300.0));
	assert!((centre_world.x - 10.0).abs() < 1e-4);
	assert!((centre_world.y + 5.0).abs() < 1e-4);
}

#[test]
fn zoom_at_keeps_world_point_under_cursor_fixed() {
	let mut cam = Camera::new(Vec2::new(800.0, 600.0));
	let anchor = Vec2::new(200.0, 150.0);
	let world_before = cam.screen_to_world(anchor);
	cam.zoom_at(anchor, 2.0);
	let world_after = cam.screen_to_world(anchor);
	assert!((world_before.x - world_after.x).abs() < 1e-3);
	assert!((world_before.y - world_after.y).abs() < 1e-3);
	assert_eq!(cam.zoom, 2.0);
}

#[test]
fn zoom_is_clamped_to_valid_range() {
	let mut cam = Camera::new(Vec2::new(800.0, 600.0));
	cam.zoom_at(Vec2::new(400.0, 300.0), 0.0001);
	assert_eq!(cam.zoom, Camera::MIN_ZOOM);
	cam.zoom_at(Vec2::new(400.0, 300.0), 1_000_000.0);
	assert_eq!(cam.zoom, Camera::MAX_ZOOM);
}

#[test]
fn pan_moves_position_by_world_delta() {
	let mut cam = Camera::new(Vec2::new(800.0, 600.0));
	cam.pan(Vec2::new(1.0, 2.0));
	assert_eq!(cam.position, Vec2::new(1.0, 2.0));
}

#[test]
fn fit_to_bounds_centres_and_zooms_to_show_whole_box() {
	let mut cam = Camera::new(Vec2::new(800.0, 400.0));
	cam.fit_to_bounds(Vec2::new(-2.0, -1.0), Vec2::new(2.0, 1.0), 0.0);
	assert_eq!(cam.position, Vec2::ZERO);
	// Both corners of the box should now land inside the viewport.
	let top_left = cam.world_to_screen(Vec2::new(-2.0, 1.0));
	let bottom_right = cam.world_to_screen(Vec2::new(2.0, -1.0));
	assert!(top_left.x >= -0.5 && top_left.x <= cam.viewport.x + 0.5);
	assert!(bottom_right.x >= -0.5 && bottom_right.x <= cam.viewport.x + 0.5);
}

#[test]
fn fit_to_bounds_with_padding_zooms_out_further() {
	let mut cam_tight = Camera::new(Vec2::new(800.0, 400.0));
	cam_tight.fit_to_bounds(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), 0.0);

	let mut cam_padded = Camera::new(Vec2::new(800.0, 400.0));
	cam_padded.fit_to_bounds(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), 0.2);

	assert!(cam_padded.zoom < cam_tight.zoom);
}

#[test]
fn fit_to_bounds_does_not_produce_nan_for_degenerate_box() {
	let mut cam = Camera::new(Vec2::new(800.0, 400.0));
	cam.fit_to_bounds(Vec2::ZERO, Vec2::ZERO, 0.1);
	assert!(cam.zoom.is_finite());
	assert!(cam.zoom > 0.0);
}

#[test]
fn state_colour_picks_high_or_dimmed_low() {
	assert_eq!(state_colour(LogicState::High, Color::default()), COLORS[0]);
	assert_eq!(state_colour(LogicState::Low, Color::default()), dim(COLORS[0]));
}

#[test]
fn state_colour_disconnected_is_always_black_regardless_of_index() {
	assert_eq!(state_colour(LogicState::Disconnected, Color::from_int(0)), STATE_DISCONNECTED_COL);
	assert_eq!(state_colour(LogicState::Disconnected, Color::from_int(3)), STATE_DISCONNECTED_COL);
}

#[test]
fn state_colour_clamps_out_of_range_index() {
	assert_eq!(state_colour(LogicState::High, Color::White), COLORS[7]);
}

#[test]
fn dim_darkens_but_preserves_hue_ratio_and_alpha() {
	let c = [0.8, 0.4, 0.2, 1.0];
	let d = dim(c);
	assert!(d[0] < c[0] && d[1] < c[1] && d[2] < c[2]);
	assert_eq!(d[3], c[3]);
	// Hue ratio preserved (uniform scale factor across channels).
	assert!((d[0] / c[0] - d[1] / c[1]).abs() < 1e-6);
}

#[test]
fn text_colour_is_black_on_light_background() {
	assert_eq!(text_colour_for_background([1.0, 1.0, 1.0, 1.0]), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn text_colour_is_white_on_dark_background() {
	assert_eq!(text_colour_for_background([0.05, 0.05, 0.05, 1.0]), [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn scene_vertex_converts_to_gpu_vertex() {
	let sv = SceneVertex { pos: Vec2::new(1.0, 2.0), colour: theme::PIN_COL };
	let v: Vertex = sv.into();
	assert_eq!(v.position, [1.0, 2.0]);
	assert_eq!(v.colour, theme::PIN_COL);
}

#[test]
fn scene_to_vertices_preserves_triangle_count() {
	let mut geo = SceneGeometry::default();
	geo.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::CHIP_BODY_COL);
	let verts = scene_to_vertices(&geo);
	assert_eq!(verts.len(), 6);
}

#[test]
fn vertex_bytes_round_trip_through_bytemuck() {
	let verts = vec![Vertex { position: [0.0, 0.0], colour: [1.0, 0.0, 0.0, 1.0] }];
	let bytes = upload_ready_bytes(&verts);
	assert_eq!(bytes.len(), std::mem::size_of::<Vertex>());
}

#[test]
fn single_1bit_pin_centres_on_chip_middle() {
	let (height, ys) = calculate_default_pin_layout(&[PinBitCount::Bit1]);
	assert_eq!(ys.len(), 1);
	// Raw (pre-centring) offset is -1, total stack is 2 grid units ->
	// shift by +1 to centre -> 0, i.e. dead centre of the chip body.
	assert_eq!(ys[0], 0.0);
	assert_eq!(height, 2.0 * GRID_SIZE);
}

#[test]
fn two_1bit_pins_stack_without_overlap() {
	let (height, ys) = calculate_default_pin_layout(&[PinBitCount::Bit1, PinBitCount::Bit1]);
	// Raw offsets [-1, -3], total stack 4 grid units -> shift by +2 ->
	// [1, -1]: symmetric around the chip's centre, matching a body
	// rect that spans [-2, +2].
	assert_eq!(ys, vec![1.0, -1.0]);
	assert_eq!(height, 4.0 * GRID_SIZE);
}

#[test]
fn mixed_bit_widths_use_correct_grid_heights() {
	// 1-bit (h=2), 4-bit (h=3), 8-bit (h=4); raw offsets before
	// centring are [-1, -3.5, -7], total stack 9 grid units -> shift
	// by +4.5.
	let (height, ys) = calculate_default_pin_layout(&[PinBitCount::Bit1, PinBitCount::Bit4, PinBitCount::Bit8]);
	assert_eq!(ys[0], 3.5);
	assert_eq!(ys[1], 1.0);
	assert_eq!(ys[2], -2.5);
	assert_eq!(height, 9.0 * GRID_SIZE);
}

#[test]
fn min_chip_height_takes_the_taller_stack() {
	let inputs = [PinBitCount::Bit1];
	let outputs = [PinBitCount::Bit1, PinBitCount::Bit1, PinBitCount::Bit1];
	let h = min_chip_height_for_pins(&inputs, &outputs);
	assert_eq!(h, calculate_default_pin_layout(&outputs).0);
}

#[test]
fn chip_with_no_pins_has_grid_size_minimum() {
	let size = calculate_min_chip_size_for_pins(&[], &[]);
	assert_eq!(size, Vec2::new(GRID_SIZE, GRID_SIZE));
}

#[test]
fn chip_with_pins_gets_double_grid_min_width() {
	let size = calculate_min_chip_size_for_pins(&[PinBitCount::Bit1], &[]);
	assert_eq!(size.x, GRID_SIZE * 2.0);
}

#[test]
fn input_pin_sits_left_of_chip_output_right() {
	let centre = Vec2::new(0.0, 0.0);
	let size = Vec2::new(2.0, 1.0);
	let input_pos = pin_world_position(centre, size, 0.0, true);
	let output_pos = pin_world_position(centre, size, 0.0, false);
	assert!(input_pos.x < centre.x);
	assert!(output_pos.x > centre.x);
	assert_eq!(input_pos.x, -1.0 - SUB_CHIP_PIN_INSET);
	assert_eq!(output_pos.x, 1.0 + SUB_CHIP_PIN_INSET);
}

#[test]
fn estimate_text_width_scales_with_length_and_font_size() {
	assert_eq!(estimate_text_width("", 0.25), 0.0);
	let short = estimate_text_width("AB", 0.25);
	let long = estimate_text_width("ABCDEFGH", 0.25);
	assert!(long > short);
	assert_eq!(long, "ABCDEFGH".chars().count() as f32 * 0.25 * AVG_CHAR_WIDTH_RATIO);
	// Doubling font size should double the estimated width.
	assert_eq!(estimate_text_width("AB", 0.5), short * 2.0);
}

#[test]
fn grid_line_thickness_stays_at_base_constant_when_zoomed_in() {
	// At a high zoom the base GRID_THICKNESS is already several
	// screen pixels wide, so no widening should occur.
	assert_eq!(grid_line_thickness(1000.0), GRID_THICKNESS);
}

#[test]
fn grid_line_thickness_widens_when_zoomed_out() {
	let zoom = 2.0;
	let thickness = grid_line_thickness(zoom);
	assert!(thickness > GRID_THICKNESS, "should widen past the sub-pixel base thickness");
	// The widened thickness should render at exactly the configured
	// minimum pixel width, not something inconsistent.
	assert!((thickness * zoom - GRID_MIN_PIXEL_THICKNESS).abs() < 1e-4);
}

#[test]
fn grid_line_thickness_never_renders_thinner_than_the_pixel_minimum() {
	for &zoom in &[0.05, 0.5, 1.0, 5.0, 50.0, 500.0, 4096.0] {
		let thickness = grid_line_thickness(zoom);
		let screen_px = thickness * zoom;
		assert!(screen_px >= GRID_MIN_PIXEL_THICKNESS - 1e-4, "zoom {zoom}: {screen_px}px thick, below the {GRID_MIN_PIXEL_THICKNESS}px minimum");
	}
}

#[test]
fn grid_line_thickness_handles_non_positive_zoom_without_panicking_or_nan() {
	assert_eq!(grid_line_thickness(0.0), GRID_THICKNESS);
	assert_eq!(grid_line_thickness(-5.0), GRID_THICKNESS);
}

#[test]
fn snapping_rounds_to_nearest_grid_line() {
	assert_eq!(snap_to_grid_scalar(0.05), 0.0);
	assert_eq!(snap_to_grid_scalar(0.07), GRID_SIZE);
	assert_eq!(snap_to_grid(Vec2::new(0.2, 0.3)), Vec2::new(GRID_SIZE * 2.0, GRID_SIZE * 2.0));
}
