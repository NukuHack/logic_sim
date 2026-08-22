use logic_sim::{
	load_chip_library_from_dir, load_project, ChipDescription, ChipType, ExternalInput, PinAddress, PinBitCount, PinDescription, Simulator,
};
use std::path::Path;

fn gol_fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/GOL")
}

fn fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/ZHT90")
}

#[test]
fn loads_real_project_chips_without_errors() {
	let (project, library, errors) = load_project(&fixture_dir()).expect("io error");

	assert!(errors.is_empty(), "parse errors: {errors:?}");
	assert_eq!(project.project_name, "ZHT90");
	assert!(library.try_get("NOT").is_some());
	assert!(library.try_get("NAND").is_none()); // builtin, not a file -- expected
}

#[test]
fn parses_not_chip_structure_correctly() {
	let (_project, library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty());

	let not_chip = library.get("NOT");
	assert_eq!(not_chip.input_pins.len(), 1);
	assert_eq!(not_chip.output_pins.len(), 1);
	assert_eq!(not_chip.sub_chips.len(), 1);
	assert_eq!(not_chip.sub_chips[0].name, "NAND");
	assert_eq!(not_chip.wires.len(), 3);
}

#[test]
fn parses_wire_bend_points_from_saved_points_stripping_placeholder_ends() {
	// `CELL.json` (in the GOL fixture project) has a wire whose saved `Points` is [source placeholder,
	// 4 real bends, target placeholder] -- the interior points should survive parsing, and the
	// placeholder first/last entries should not.
	let (_project, library, errors) = load_chip_library_from_dir(&gol_fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty(), "parse errors: {errors:?}");

	let cell = library.get("CELL");
	let bent_wire = cell.wires.iter().find(|w| w.points.len() == 4).expect("expected to find the bent wire with 4 interior points");

	let expected = [(1.625, -1.875), (1.625, -1.0), (-2.875, -1.0), (-2.875, -1.5)];
	for (p, (ex, ey)) in bent_wire.points.iter().zip(expected.iter()) {
		assert!((p.x - ex).abs() < 1e-4 && (p.y - ey).abs() < 1e-4, "point {p:?} != ({ex}, {ey})");
	}

	// Straight (unbent) wires in the same chip should still parse with no
	// bend points at all.
	assert!(cell.wires.iter().any(|w| w.points.is_empty()), "expected at least one unbent wire");
}

#[test]
fn simulates_the_loaded_not_chip_correctly() {
	let (_project, mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty());

	// The library doesn't include builtins (NAND) since they aren't saved
	// as files -- register a minimal NAND description so NOT (which is
	// built from one NAND) can be resolved.
	let mut nand = ChipDescription::new("NAND", ChipType::Nand);
	nand.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
	nand.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
	nand.output_pins.push(PinDescription::new("OUT", 2, PinBitCount::Bit1));
	library.add(nand);

	let not_desc = library.get("NOT").clone();
	let in_pin_id = not_desc.input_pins[0].id;
	let out_pin_id = not_desc.output_pins[0].id;

	let mut sim = Simulator::build(&not_desc, &library);

	for &input_val in &[0u32, 1] {
		let inputs = vec![ExternalInput { address: PinAddress::new(in_pin_id, in_pin_id), state: input_val }];
		for _ in 0..3 {
			sim.run_simulation_step(&inputs);
		}

		let out_pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("output pin should resolve");
		let out_state = sim.pin(out_pin).state & 1;

		assert_eq!(out_state, 1 - input_val, "NOT({input_val}) should invert");
	}
}
