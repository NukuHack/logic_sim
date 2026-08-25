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

#[test]
fn corresponding_origin_is_the_inverse_lookup() {
	use logic_sim::ChipType;
	assert_eq!(ChipType::BusTerminus1Bit.corresponding_bus_origin(), Some(ChipType::Bus1Bit));
	assert_eq!(ChipType::BusTerminus4Bit.corresponding_bus_origin(), Some(ChipType::Bus4Bit));
	assert_eq!(ChipType::BusTerminus8Bit.corresponding_bus_origin(), Some(ChipType::Bus8Bit));
	assert_eq!(ChipType::Nand.corresponding_bus_origin(), None);
	assert_eq!(ChipType::Bus4Bit.corresponding_bus_origin(), None, "an origin has no inverse pair of its own");
}

// ---- resolve_bus_pair_completion: any-bus-to-any-bus completions ----

/// Two independently-placed pairs: origins A (id 1) / B (id 3), termini
/// A' (id 2) / B' (id 4), each linked to its own partner by default.
fn two_pairs() -> (ChipLibrary, ChipDescription) {
	let library = bus_library();
	let mut chip = ChipDescription::new("PAIRS", ChipType::Custom);
	for (name, id, partner) in [("BUS-4", 1, 2), ("BUS-TERMINUS-4", 2, 1), ("BUS-4", 3, 4), ("BUS-TERMINUS-4", 4, 3)] {
		chip.sub_chips.push(SubChipDescription {
			name: name.into(),
			id,
			internal_data: Some(vec![partner]),
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
	}
	(library, chip)
}

fn data_of(chip: &ChipDescription, id: i32) -> Vec<u32> {
	chip.sub_chips.iter().find(|s| s.id == id).expect("subchip exists").internal_data.clone().expect("has data")
}

fn name_of(chip: &ChipDescription, id: i32) -> String {
	chip.sub_chips.iter().find(|s| s.id == id).expect("subchip exists").name.clone()
}

/// Wiring an origin onto another origin converts the second one into a
/// terminus -- flip inverted so its (formerly right-side) output pin stays
/// physically on that side -- and links the pair instantly, clearing the
/// orphaned previous partners' pointers.
#[test]
fn bus_to_bus_completes_by_converting_the_second_into_a_linked_terminus() {
	let (library, mut chip) = two_pairs();

	let (source, target) = bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 1, 3).expect("bus->bus completes");

	assert_eq!(source, PinAddress::new(1, 1), "the wire runs from the surviving origin's visible output...");
	assert_eq!(target, PinAddress::new(3, 0), "...to the converted half's input");
	assert_eq!(name_of(&chip, 3), "BUS-TERMINUS-4", "the second bus became a terminus");
	assert_eq!(data_of(&chip, 3), vec![1, 1], "linked back to the first, flip inverted (was unflipped)");
	assert_eq!(data_of(&chip, 1), vec![3, 0], "the first links forward, its own flip untouched");
	assert!(bus_wiring::bus_pair_linked(&chip, &library, 1, 3));

	// The halves' previous partners are orphaned: their pointers are
	// cleared (not left dangling at a chip that's now paired elsewhere),
	// so deletes don't cascade across the old pairs.
	assert_eq!(data_of(&chip, 2), vec![0, 0]);
	assert_eq!(data_of(&chip, 4), vec![0, 0]);
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 1, 2));
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 3, 4));
}

/// Wiring a terminus onto another terminus converts the second one into a
/// bus origin -- again flip-inverted -- and the finished wire runs from
/// the NEW origin's output to the first terminus' input.
#[test]
fn terminus_to_terminus_completes_by_converting_the_second_into_a_linked_origin() {
	let (library, mut chip) = two_pairs();

	let (source, target) = bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 2, 4).expect("terminus->terminus completes");

	assert_eq!(source, PinAddress::new(4, 1), "the converted half is the wire's source");
	assert_eq!(target, PinAddress::new(2, 0));
	assert_eq!(name_of(&chip, 4), "BUS-4", "the second terminus became an origin");
	assert_eq!(data_of(&chip, 4), vec![2, 1], "flip inverted across the conversion");
	assert_eq!(data_of(&chip, 2), vec![4, 0]);
	assert!(bus_wiring::bus_pair_linked(&chip, &library, 2, 4));
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 1, 2));
	assert!(!bus_wiring::bus_pair_linked(&chip, &library, 3, 4));
}

/// An already-compatible bus->terminus completion needs no conversion but
/// still links instantly -- and preserves both halves' existing flip
/// states.
#[test]
fn bus_to_terminus_links_without_conversion_or_flip_changes() {
	let (library, mut chip) = two_pairs();
	// Make both halves flipped to prove flips survive untouched.
	for id in [1, 4] {
		if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == id) {
			let mut data = sub.internal_data.clone().unwrap_or_default();
			data.resize(2, 0);
			data[1] = 1;
			sub.internal_data = Some(data);
		}
	}

	let (source, target) = bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 1, 4).expect("origin->terminus completes");

	assert_eq!((source, target), (PinAddress::new(1, 1), PinAddress::new(4, 0)));
	assert_eq!(name_of(&chip, 4), "BUS-TERMINUS-4", "no conversion needed");
	assert_eq!(data_of(&chip, 1), vec![4, 1], "relinked, flip kept");
	assert_eq!(data_of(&chip, 4), vec![1, 1], "relinked, flip kept");
	assert!(bus_wiring::bus_pair_linked(&chip, &library, 1, 4));
	// The reversed start order resolves identically (start keeps its type).
	let (library, mut chip) = two_pairs();
	let reversed = bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 4, 1).expect("terminus-start order works too");
	assert_eq!(reversed, (PinAddress::new(1, 1), PinAddress::new(4, 0)));
}

/// The flip inversion is exactly what keeps the visible pin on its
/// physical side: whatever side a component's visible pin sat on before
/// the conversion, it sits on after.
#[test]
fn conversions_invert_the_flip_so_the_visible_pin_keeps_its_side() {
	let (library, mut chip) = two_pairs();
	// Flip origin B (visible pin moves to the LEFT).
	if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == 3) {
		sub.internal_data = Some(vec![4, 1]);
	}

	bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 1, 3).expect("completes");

	assert_eq!(data_of(&chip, 3), vec![1, 0], "flipped origin -> unflipped terminus (left stays left)");

	// And the mirror case: a flipped terminus converting to an origin.
	let (library, mut chip) = two_pairs();
	if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == 4) {
		sub.internal_data = Some(vec![3, 1]);
	}
	bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 2, 4).expect("completes");
	assert_eq!(data_of(&chip, 4), vec![2, 0], "flipped terminus (pin left) -> unflipped origin (pin left)");
}

/// Non-bus endpoints never reach the resolver successfully.
#[test]
fn resolver_rejects_non_bus_and_unknown_endpoints() {
	let (library, mut chip) = two_pairs();
	chip.sub_chips.push(SubChipDescription {
		name: "NAND".into(),
		id: 7,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});

	assert!(bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 1, 7).is_err(), "plain gate end rejected");
	assert!(bus_wiring::resolve_bus_pair_completion(&mut chip, &library, 1, 99).is_err(), "unknown end rejected");
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
