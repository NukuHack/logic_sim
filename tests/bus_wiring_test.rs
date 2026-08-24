//! Bus-wiring rules: linked-pair detection, bus-wire recognition,
//! bus-corrected targets, and the wire-tap completion matrix
//! (`CanCompleteWireConnection`'s restrictions), exercised through the
//! public `viewer::bus_wiring` API.

use logic_sim::description::{ChipDescription, ChipType, PinAddress, SubChipDescription, WireConnectionType, WireDescription};
use logic_sim::register_all_builtins;
use logic_sim::viewer::bus_wiring;
use logic_sim::ChipLibrary;
use logic_sim::Vec2;

fn bus_library() -> ChipLibrary {
	let mut lib = ChipLibrary::new();
	register_all_builtins(&mut lib);
	lib
}

/// BUS-4 origin (id 5) linked to BUS-TERMINUS-4 (id 10), plus a plain NAND
/// (id 7) for the non-bus cases.
fn fixture() -> (ChipLibrary, ChipDescription) {
	let library = bus_library();

	let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
	chip.sub_chips.push(SubChipDescription {
		name: "BUS-4".into(),
		id: 5,
		internal_data: Some(vec![10]),
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});
	chip.sub_chips.push(SubChipDescription {
		name: "BUS-TERMINUS-4".into(),
		id: 10,
		internal_data: Some(vec![5]),
		position: Vec2::new(6.0, 0.0),
		label: None,
		pin_colour_info: vec![],
	});
	chip.sub_chips.push(SubChipDescription {
		name: "NAND".into(),
		id: 7,
		internal_data: None,
		position: Vec2::new(-6.0, 0.0),
		label: None,
		pin_colour_info: vec![],
	});

	// The bus wire itself: origin output (pin 1) -> terminus input (pin 0).
	let bus_wire = WireDescription::new(PinAddress::new(5, 1), PinAddress::new(10, 0));
	chip.wires.push(bus_wire);

	// A plain gate-to-gate wire.
	chip.wires.push(WireDescription::new(PinAddress::new(7, 0), PinAddress::new(7, 1)));

	(library, chip)
}

#[test]
fn owner_chip_type_resolves_subchips_but_not_dev_pins() {
	let (library, chip) = fixture();
	assert_eq!(bus_wiring::owner_chip_type(&chip, &library, 5), Some(ChipType::Bus4Bit));
	assert_eq!(bus_wiring::owner_chip_type(&chip, &library, 10), Some(ChipType::BusTerminus4Bit));
	assert_eq!(bus_wiring::owner_chip_type(&chip, &library, 7), Some(ChipType::Nand));
	assert_eq!(bus_wiring::owner_chip_type(&chip, &library, 999), None, "unknown id");
}

#[test]
fn bus_pair_linking_requires_mutual_ids_and_bus_types() {
	let (library, mut chip) = fixture();

	assert!(bus_wiring::bus_pair_linked(&chip, &library, 5, 10));
	assert!(bus_wiring::bus_pair_linked(&chip, &library, 10, 5), "order-independent");
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 5, 7), "non-bus owners are never linked");

	// One-sided link data isn't a pair.
	if let Some(terminus) = chip.sub_chips.iter_mut().find(|s| s.id == 10) {
		terminus.internal_data = Some(vec![999]);
	}
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 5, 10));

	// Missing link data entirely likewise.
	if let Some(origin) = chip.sub_chips.iter_mut().find(|s| s.id == 5) {
		origin.internal_data = None;
	}
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 5, 10));
}

#[test]
fn bus_partner_id_finds_the_other_half_only_for_bus_chips() {
	let (library, chip) = fixture();
	assert_eq!(bus_wiring::bus_partner_id(&chip, &library, 5), Some(10));
	assert_eq!(bus_wiring::bus_partner_id(&chip, &library, 10), Some(5));
	assert_eq!(bus_wiring::bus_partner_id(&chip, &library, 7), None, "non-bus chips have no partner");
}

#[test]
fn only_origin_to_terminus_wires_count_as_bus_wires() {
	let (library, chip) = fixture();

	assert!(bus_wiring::is_bus_wire(&chip, &library, &chip.wires[0]));
	assert!(!bus_wiring::is_bus_wire(&chip, &library, &chip.wires[1]), "gate-to-gate wire");

	// Same endpoints reversed (terminus "output" -> origin "input") isn't a
	// valid bus wire -- the origin owns the source end by construction.
	let reversed = WireDescription::new(PinAddress::new(10, 0), PinAddress::new(5, 1));
	assert!(!bus_wiring::is_bus_wire(&chip, &library, &reversed));
}

#[test]
fn bus_corrected_target_lands_on_the_origins_input_pin() {
	let (library, chip) = fixture();

	// On the bus wire, connections merge into the ORIGIN's hidden input (id 0),
	// not the terminus input the wire visibly points at.
	assert_eq!(bus_wiring::bus_corrected_target(&chip, &library, &chip.wires[0]), PinAddress::new(5, 0));

	// Everywhere else it's just the wire's own target.
	assert_eq!(bus_wiring::bus_corrected_target(&chip, &library, &chip.wires[1]), chip.wires[1].target_pin_address);
}

#[test]
fn completion_matrix_inputs_anywhere_outputs_bus_only_never_wire_to_wire() {
	let (library, chip) = fixture();

	// An INPUT completing onto a normal wire inherits that wire's source.
	let (source, target) =
		bus_wiring::resolve_completion_on_wire(&chip, &library, 1, false, false, 8, 3).expect("input into a normal wire is allowed");
	assert_eq!((source, target), (PinAddress::new(7, 0), PinAddress::new(8, 3)));

	// An OUTPUT completing onto a normal wire is rejected (two drivers).
	assert!(bus_wiring::resolve_completion_on_wire(&chip, &library, 1, false, true, 9, 2).is_err());

	// An OUTPUT completing onto the BUS wire merges into the origin's input.
	let (source, target) = bus_wiring::resolve_completion_on_wire(&chip, &library, 0, false, true, 11, 2).expect("output into a bus wire is allowed");
	assert_eq!((source, target), (PinAddress::new(11, 2), PinAddress::new(5, 0)));

	// An INPUT completing onto the BUS wire takes the tapped wire's source
	// verbatim (only output-started wires get the bus-corrected form --
	// `CanCompleteWireConnection`'s asymmetry).
	let (source, target) = bus_wiring::resolve_completion_on_wire(&chip, &library, 0, false, false, 12, 3).expect("input into a bus wire");
	assert_eq!((source, target), (PinAddress::new(5, 1), PinAddress::new(12, 3)));

	// Wire-to-wire completions are always rejected as ambiguous.
	assert!(bus_wiring::resolve_completion_on_wire(&chip, &library, 0, true, true, 0, 0).expect_err("wire->wire must fail").contains("another wire"));

	// A stale wire index (e.g. the wire was deleted mid-placement) errors cleanly.
	assert!(bus_wiring::resolve_completion_on_wire(&chip, &library, 99, false, false, 8, 3).is_err());
}

#[test]
fn corresponding_terminus_maps_each_bus_width_and_nothing_else() {
	use logic_sim::ChipType;
	assert_eq!(ChipType::Bus1Bit.corresponding_bus_terminus(), Some(ChipType::BusTerminus1Bit));
	assert_eq!(ChipType::Bus4Bit.corresponding_bus_terminus(), Some(ChipType::BusTerminus4Bit));
	assert_eq!(ChipType::Bus8Bit.corresponding_bus_terminus(), Some(ChipType::BusTerminus8Bit));
	assert_eq!(ChipType::Nand.corresponding_bus_terminus(), None);
	assert_eq!(ChipType::BusTerminus4Bit.corresponding_bus_terminus(), None, "a terminus has no further pair");
}

/// The electrical payoff of "wiring into a bus wire": two NAND outputs
/// completed onto the bus wire both merge into the origin's hidden input
/// (via `TargetPin_BusCorrected`), and the terminus side reads the result.
/// Only deterministic driver states are asserted (the sim resolves
/// conflicting drivers randomly, mirroring the original).
#[test]
fn simulator_feeds_multiple_inputs_into_a_bus_wire_through_the_origin() {
	let (library, mut chip) = fixture();

	// Two NAND drivers (ids 8 and 9) whose outputs complete ONTO the bus
	// wire -- exactly what the editor builds for an output-onto-bus-wire
	// completion: target bus-corrected to the origin's input (pin 0).
	for id in [8, 9] {
		chip.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		chip.wires.push(WireDescription::new_tapped_target(PinAddress::new(id, 2), PinAddress::new(5, 0), 0, 0, Vec2::ZERO));
	}

	let mut sim = logic_sim::Simulator::build(&chip, &library);

	// Drive both NANDs so their outputs sit HIGH deterministically
	// (NAND(IN B=0, IN A=anything) = 1): identical states on the shared
	// net, so the random conflict resolution can't flip the outcome.
	let drive = |sim: &mut logic_sim::Simulator, audio: &mut logic_sim::audio::SimAudio| {
		let mut inputs = Vec::new();
		for id in [8u32, 9] {
			inputs.push(logic_sim::ExternalInput { address: PinAddress::new(id as i32, 0), state: 0 }); // IN B low
			inputs.push(logic_sim::ExternalInput { address: PinAddress::new(id as i32, 1), state: 1 });
		}
		sim.run_simulation_step(&inputs, audio);
	};

	let mut audio = logic_sim::audio::SimAudio::new();
	for _ in 0..3 {
		drive(&mut sim, &mut audio);
	}

	let terminus_input = sim.find_pin(sim.root(), PinAddress::new(10, 0)).expect("terminus input pin exists");
	assert_eq!(sim.pin(terminus_input).state & 1, 1, "merged bus signal reaches the terminus");
}

#[test]
fn tapped_target_wires_round_trip_through_the_save_format() {
	let mut wire = WireDescription::new_tapped_target(PinAddress::new(1, 0), PinAddress::new(2, 1), 0, 2, Vec2::new(3.0, 4.0));
	wire.points = vec![Vec2::new(1.0, 1.0)];

	let json = logic_sim::serialize_chip_description(&{
		let mut chip = ChipDescription::new("T", ChipType::Custom);
		chip.wires.push(wire.clone());
		chip
	})
	.unwrap();
	let parsed = logic_sim::parse_chip_description(&json).unwrap();

	assert_eq!(parsed.wires.len(), 1);
	let back = &parsed.wires[0];
	assert_eq!(back.connection_type, WireConnectionType::ToWireTarget);
	assert_eq!(back.connected_wire_index, 0);
	assert_eq!(back.connected_wire_segment_index, 2);
	assert_eq!(back.cached_target_point, Vec2::new(3.0, 4.0));
	assert_eq!(back.points, vec![Vec2::new(1.0, 1.0)]);
	assert_eq!(back.source_pin_address, PinAddress::new(1, 0));
}
