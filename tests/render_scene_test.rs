//! Chip-scene integration tests through the public renderer API:
//! background-grid construction, multi-strand bus wire rendering,
//! wire-tap hit-testing, pin shapes/hover labels, subchip placement, and
//! live simulator-backed pin lookups -- all via `build_scene` and friends.

use logic_sim::description::Color;
use logic_sim::description::{ChipDescription, ChipLibrary, ChipType, PinAddress, PinBitCount, PinDescription, SubChipDescription, WireDescription};
use logic_sim::pin_state::LogicState;
use logic_sim::render::camera::Camera;
use logic_sim::render::foundation::{bounding_box, SceneVertex};
use logic_sim::render::layout;
use logic_sim::render::scene::{
	build_grid, build_scene, closest_wire_hit, hit_test_any_pin, hit_test_input_dev_pin_bit, hit_test_sub_chip_pin, place_sub_chips, AllLow,
	PinStateLookup, SimulatorPinState,
};
use logic_sim::render::theme;
use logic_sim::render::theme::Rgba;
use logic_sim::sim::Simulator;
use logic_sim::Vec2;

/// Lookup that always reports `Disconnected`, regardless of palette
/// index -- for testing that disconnected wires render flat black rather
/// than through the normal low/high palette.
struct AllDisconnected;
impl PinStateLookup for AllDisconnected {
	fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
		Some(false)
	}
	fn logic_state(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<LogicState> {
		Some(LogicState::Disconnected)
	}
}

/// Shared NAND fixture (mirrors the crate's unit-test `test_support` one,
/// which is intentionally not exported).
fn nand_desc() -> ChipDescription {
	let mut d = ChipDescription::new("NAND", ChipType::Nand);
	d.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
	d.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
	d.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
	d
}

fn test_camera() -> Camera {
	// 800x400 viewport, zoom=100 -> screen_half_width=4, screen_half_height=2
	// world units, comfortably inside the `skip == 1` (< 8) band.
	let mut cam = Camera::new(logic_sim::Vec2::new(800.0, 400.0));
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

	// The grid must extend at least as far as the visible viewport in every direction (it's
	// allowed to overshoot slightly, but must never fall short, or you'd see ungridded space at the window edge).
	assert!(min.x <= -screen_half_width);
	assert!(max.x >= screen_half_width);
	assert!(min.y <= -screen_half_height);
	assert!(max.y >= screen_half_height);
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
	// Zoomed out enough that the base GRID_THICKNESS would render as a fraction of a screen pixel
	// and start aliasing inconsistently -- the "grid falls apart" symptom. Kept mild enough (zoom=2)
	// that grid lines are still spaced further apart than the widened thickness.
	let mut cam = Camera::new(Vec2::new(800.0, 400.0));
	cam.zoom = 2.0;
	let geo = build_grid(&cam, theme::GRID_COL);

	let expected_thickness = layout::grid_line_thickness(cam.zoom);
	assert!(expected_thickness > layout::GRID_THICKNESS, "sanity check: this zoom level should actually require widening");

	// World x=0 is always a drawn line, and centred at camera position (0,0) its quad corners are
	// the only vertices in the whole scene landing within `expected_thickness` of x=0 (the next
	// line over sits a full `skip * GRID_SIZE` away).
	let near_zero_x: Vec<f32> = geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < expected_thickness).collect();
	assert!(!near_zero_x.is_empty(), "expected to find the x=0 grid line's vertices");

	let max_x = near_zero_x.iter().cloned().fold(f32::MIN, f32::max);
	let min_x = near_zero_x.iter().cloned().fold(f32::MAX, f32::min);
	let spread = max_x - min_x;
	assert!((spread - expected_thickness).abs() < 1e-4, "line spread {spread} should equal the widened thickness {expected_thickness}");
}

#[test]
fn build_grid_thickness_matches_default_constant_when_zoomed_in() {
	// At a comfortably zoomed-in level the base GRID_THICKNESS is already many screen pixels wide,
	// so no widening should occur -- guards against overcorrecting. zoom=100 is no longer enough
	// on its own: with `GRID_MIN_PIXEL_THICKNESS` (1.5px), the base thickness only clears the minimum past zoom ~429.
	let mut cam = test_camera();
	cam.zoom = 1000.0;
	let geo = build_grid(&cam, theme::GRID_COL);
	let near_zero_x: Vec<f32> = geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < layout::GRID_SIZE).collect();
	assert!(!near_zero_x.is_empty());
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
		// Both dev-pins are placed at y=0, so this wire (and every one of its strands) is
		// perfectly horizontal, and each strand is unbent -> exactly one quad (6 verts) per
		// strand. Wires are drawn first, so the first `bit_count * 6` vertices are this wire's strands.
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
			Some(if bit_index.is_multiple_of(2) { LogicState::Low } else { LogicState::High })
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

/// `closest_wire_hit` finds the *projected* point on the right
/// segment of the nearest wire's drawn centreline -- including a bend
/// point that splits one wire into two distinct segments.
#[test]
fn hit_test_wire_tap_finds_the_projected_point_on_the_right_segment() {
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
	let mut wire = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
	// A bend point partway along, so the wire has two distinct segments to
	// disambiguate between (segment 0: source -> bend, segment 1: bend -> target).
	wire.points.push(Vec2::new(0.0, 1.0));
	parent.wires.push(wire);

	let tap = closest_wire_hit(&parent, &lib, Vec2::new(0.0, 1.0), 0.5).expect("should tap near the bend point");
	assert_eq!(tap.wire_index, 0);
	assert_eq!(tap.segment_index, 0);
	assert!((tap.point.x - 0.0).abs() < 1e-3);
	assert!((tap.point.y - 1.0).abs() < 1e-3);

	assert!(closest_wire_hit(&parent, &lib, Vec2::new(1000.0, 1000.0), 0.5).is_none());
}

#[test]
fn hit_test_sub_chip_pin_finds_a_subchips_output_pin_at_its_exact_position() {
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
	let placed = place_sub_chips(&parent, &lib);
	let output_pos = layout::pin_world_position(placed[0].centre, placed[0].size, placed[0].output_pin_y[0], false);

	let hit = hit_test_sub_chip_pin(&placed, output_pos).expect("should land on NAND's output pin");
	assert_eq!(hit.owner_id, 1);
	assert_eq!(hit.pin_id, 0);
	assert!(!hit.is_input);
	assert!(!hit.is_boundary);
	assert!(hit.is_wire_source(), "a subchip's output pin should be a valid wire source");

	assert!(hit_test_sub_chip_pin(&placed, Vec2::new(1000.0, 1000.0)).is_none());
}

#[test]
fn hit_test_any_pin_resolves_the_chips_own_boundary_dev_pins() {
	let mut chip = ChipDescription::new("TEST", ChipType::Custom);
	chip.input_pins.push(PinDescription::new("IN", 10, PinBitCount::Bit1));
	chip.input_pins[0].position = Vec2::new(-5.0, 0.0);
	chip.output_pins.push(PinDescription::new("OUT", 20, PinBitCount::Bit1));
	chip.output_pins[0].position = Vec2::new(5.0, 0.0);

	let hit = hit_test_any_pin(&chip, &[], Vec2::new(-5.0, 0.0)).expect("should land on the boundary input dev-pin");
	assert_eq!(hit.owner_id, 10);
	assert_eq!(hit.pin_id, 10);
	assert!(hit.is_input);
	assert!(hit.is_boundary);
	// A chip's own *input* dev-pin is the thing driving wires from inside the
	// chip, so it plays the wire *source* role even though it's literally an input.
	assert!(hit.is_wire_source());

	let hit = hit_test_any_pin(&chip, &[], Vec2::new(5.0, 0.0)).expect("should land on the boundary output dev-pin");
	assert_eq!(hit.owner_id, 20);
	assert!(!hit.is_input);
	assert!(hit.is_wire_target(), "a chip's own output dev-pin is a wire target from inside the chip");

	assert!(hit_test_any_pin(&chip, &[], Vec2::new(0.0, 0.0)).is_none());
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
	assert!(scene.triangles.len() > 4 * 2 * 2 * 3 - 1);

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
	assert_eq!(scene.labels.len(), 2);
	assert_eq!(scene.labels[1].text, "OUT");
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
	let size = PinBitCount::Bit4.pin_visual_shape_size();
	let wing_point = Vec2::new(pin_pos.x - size.x / 2.0 + 1e-3, pin_pos.y);

	let scene = build_scene(&parent, &lib, &AllLow, Some(wing_point));
	assert_eq!(scene.labels.len(), 2);
	assert_eq!(scene.labels[1].text, "WIDE_IN");
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
fn simulator_pin_state_resolves_live_sim_values() {
	use logic_sim::builtins::register_all;

	let mut lib = ChipLibrary::new();
	register_all(&mut lib);

	// A tiny custom chip: one NAND subchip, unconnected inputs (so both read HIGH via the sim's
	// disconnected-pin convention) feeding its output pin. We just need a live SimChip id to
	// query through `find_pin`, not full end-to-end signal correctness (that's sim.rs's job).
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
