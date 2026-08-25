use logic_sim::{load_chip_library_from_dir, pin_state::PinState, register_all_builtins, ExternalInput, PinAddress, Simulator};
use std::path::Path;

fn fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/MainTest")
}

#[test]
fn all_builtins_have_unique_valid_names() {
	let chips = logic_sim::create_all_builtins();
	// 6 IO pins + key + key mods + 4 basic + 2 memory + 6 merge/split + 4 display + 6 bus + 1 buzzer = 31
	assert_eq!(chips.len(), 31);

	let mut names = std::collections::HashSet::new();
	for c in &chips {
		assert!(names.insert(c.name.clone()), "duplicate builtin chip name: {}", c.name);
	}
}

#[test]
fn nand_builtin_matches_original_pin_layout() {
	let chips = logic_sim::create_all_builtins();
	let nand = chips.iter().find(|c| c.name == "NAND").unwrap();
	assert_eq!(nand.input_pins.len(), 2);
	assert_eq!(nand.output_pins.len(), 1);
	assert_eq!(nand.input_pins[0].name, "IN B");
	assert_eq!(nand.input_pins[0].id, 0);
	assert_eq!(nand.input_pins[1].name, "IN A");
	assert_eq!(nand.input_pins[1].id, 1);
	assert_eq!(nand.output_pins[0].id, 2);
}

/// This is the real end-to-end path: load a saved project's custom chips
/// from disk, register the builtin chips (as the app does on startup), and
/// simulate a loaded custom chip that's built out of a real builtin (NAND) --
/// with zero hand-written stub chips.
#[test]
fn loaded_not_chip_simulates_correctly_using_real_builtins() {
	let (mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).unwrap();
	assert!(errors.is_empty());

	register_all_builtins(&mut library);
	assert!(library.try_get("NAND").is_some());

	let not_desc = library.get("NOT").clone();
	let in_pin_id = not_desc.input_pins[0].id;
	let out_pin_id = not_desc.output_pins[0].id;

	let mut sim = Simulator::build(&not_desc, &library);

	for &input_val in &[0u32, 1] {
		let inputs = vec![ExternalInput { address: PinAddress::new(in_pin_id, in_pin_id), state: PinState::from_raw(input_val) }];
		for _ in 0..3 {
			sim.run_simulation_step(&inputs, &mut logic_sim::audio::SimAudio::new());
		}

		let out_pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("output pin should resolve");
		let out_state = (sim.pin(out_pin).state.bit_states() & 1) as u32;

		assert_eq!(out_state, 1 - input_val, "NOT({input_val}) should invert");
	}
}

/// Same idea, but for a bigger real chip from the project: OR, built from
/// NANDs (De Morgan's), to sanity check multi-subchip wiring via builtins.
#[test]
fn loaded_or_chip_simulates_correctly() {
	let (mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).unwrap();
	assert!(errors.is_empty());
	register_all_builtins(&mut library);

	let or_desc = library.get("OR").clone();
	assert_eq!(or_desc.input_pins.len(), 2);
	let a_id = or_desc.input_pins[0].id;
	let b_id = or_desc.input_pins[1].id;
	let out_id = or_desc.output_pins[0].id;

	let mut sim = Simulator::build(&or_desc, &library);

	for &a in &[0u32, 1] {
		for &b in &[0u32, 1] {
			let inputs = vec![
				ExternalInput { address: PinAddress::new(a_id, a_id), state: PinState::from_raw(a) },
				ExternalInput { address: PinAddress::new(b_id, b_id), state: PinState::from_raw(b) },
			];
			for _ in 0..4 {
				sim.run_simulation_step(&inputs, &mut logic_sim::audio::SimAudio::new());
			}
			let out_pin = sim.find_pin(sim.root(), PinAddress::new(out_id, out_id)).unwrap();
			let out_state = (sim.pin(out_pin).state.bit_states() & 1) as u32;
			assert_eq!(out_state, a | b, "OR({a},{b}) should be {}", a | b);
		}
	}
}

/// Every non-display builtin (spot-checked via NAND) keeps the default
/// zero size, relying on `place_sub_chips`'s pins-based fallback -- only
/// the display chips need an explicit size.
#[test]
fn non_display_builtins_keep_default_zero_size() {
	let chips = logic_sim::create_all_builtins();
	let nand = chips.iter().find(|c| c.name == "NAND").unwrap();
	assert_eq!(nand.size, logic_sim::Vec2::default());
}

/// The 3-state buffer passes its input through while enabled, and its
/// output must go genuinely *disconnected* (tristate flag set, not a
/// connected LOW) when disabled -- that floating wire is the whole point
/// of the component.
#[test]
fn tri_state_buffer_output_floats_when_disabled() {
	use logic_sim::pin_state::LogicState;

	let mut wrapper = logic_sim::ChipDescription::new("TSB HOST", logic_sim::ChipType::Custom);
	const DATA: i32 = 10;
	const ENABLE: i32 = 11;
	const OUT: i32 = 12;
	wrapper.input_pins = vec![
		logic_sim::PinDescription::new("DATA", DATA, logic_sim::PinBitCount::Bit1),
		logic_sim::PinDescription::new("ENABLE", ENABLE, logic_sim::PinBitCount::Bit1),
	];
	wrapper.output_pins = vec![logic_sim::PinDescription::new("OUT", OUT, logic_sim::PinBitCount::Bit1)];
	wrapper.sub_chips = vec![logic_sim::SubChipDescription {
		name: "3-STATE BUFFER".to_string(),
		id: 1,
		internal_data: None,
		position: logic_sim::Vec2::ZERO,
		label: None,
		pin_colour_info: Vec::new(),
	}];
	wrapper.wires = vec![
		logic_sim::WireDescription::new(logic_sim::PinAddress::new(DATA, DATA), logic_sim::PinAddress::new(1, 0)),
		logic_sim::WireDescription::new(logic_sim::PinAddress::new(ENABLE, ENABLE), logic_sim::PinAddress::new(1, 1)),
		logic_sim::WireDescription::new(logic_sim::PinAddress::new(1, 2), logic_sim::PinAddress::new(OUT, OUT)),
	];

	let mut library = logic_sim::ChipLibrary::new();
	register_all_builtins(&mut library);
	library.add(wrapper.clone());
	let mut sim = Simulator::build(&wrapper, &library);

	let out_pin = sim.find_pin(sim.root(), logic_sim::PinAddress::new(OUT, OUT)).expect("wrapper output pin should resolve");
	let step_with = |sim: &mut Simulator, data: u32, enable: u32| {
		let inputs = vec![
			ExternalInput { address: logic_sim::PinAddress::new(DATA, DATA), state: PinState::from_raw(data) },
			ExternalInput { address: logic_sim::PinAddress::new(ENABLE, ENABLE), state: PinState::from_raw(enable) },
		];
		for _ in 0..3 {
			sim.run_simulation_step(&inputs, &mut logic_sim::audio::SimAudio::new());
		}
	};

	step_with(&mut sim, 1, 1);
	assert_eq!(sim.pin(out_pin).state.bit(0), LogicState::High, "enabled buffer passes a HIGH through");

	step_with(&mut sim, 0, 1);
	assert_eq!(sim.pin(out_pin).state.bit(0), LogicState::Low, "enabled buffer passes a LOW through");

	step_with(&mut sim, 1, 0);
	assert_eq!(sim.pin(out_pin).state.bit(0), LogicState::Disconnected, "disabled buffer's output floats instead of sitting LOW");
}
