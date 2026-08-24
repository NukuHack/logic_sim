//! Reproduces the customize-preview's display-state resolution path:
//! a panel chip whose embedded 7-segment / LED displays are wired to a
//! driven input, simulated, then queried through `SimulatorPinState`
//! exactly like `viewer::customize::build_layer` does.

use logic_sim::register_all_builtins;
use logic_sim::render::scene::{PinStateLookup, SimulatorPinState};
use logic_sim::{
	parse_chip_description, serialize_chip_description, ChipDescription, ChipLibrary, ChipType, ExternalInput, PinAddress, PinBitCount,
	PinDescription, Simulator, SubChipDescription, Vec2, WireDescription,
};

#[test]
fn embedded_display_states_resolve_through_the_live_simulator() {
	let mut library = ChipLibrary::new();
	register_all_builtins(&mut library);

	// Panel: one input dev-pin (id 1) wired into both a 7-seg (id 4, pin A
	// = 0) and an LED (id 5, IN = 0).
	let mut panel = ChipDescription::new("Panel", ChipType::Custom);
	panel.input_pins.push(PinDescription::new("IN", 1, PinBitCount::Bit1));
	panel.sub_chips.push(SubChipDescription {
		name: "7-SEGMENT".into(),
		id: 4,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});
	panel.sub_chips.push(SubChipDescription {
		name: "LED".into(),
		id: 5,
		internal_data: None,
		position: Vec2::new(2.0, 0.0),
		label: None,
		pin_colour_info: vec![],
	});
	panel.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(4, 0)));
	panel.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(5, 0)));
	panel.displays.push(logic_sim::DisplayDescription::new(4, Vec2::ZERO, 1.0));

	library.add(panel.clone());
	let mut sim = Simulator::build(&panel, &library);

	let high = vec![ExternalInput { address: PinAddress::new(1, 0), state: 1 }];
	for _ in 0..4 {
		sim.run_simulation_step(&high);
	}

	// Exactly the lookup the customize preview performs.
	let lookup = SimulatorPinState { sim: &sim, scope: sim.root() };
	assert_eq!(lookup.is_high(4, 0), Some(true), "7-seg segment A must read high");
	assert_eq!(lookup.is_high(5, 0), Some(true), "LED input must read high");

	drop(lookup);
	for _ in 0..4 {
		sim.run_simulation_step(&[ExternalInput { address: PinAddress::new(1, 0), state: 0 }]);
	}
	let lookup = SimulatorPinState { sim: &sim, scope: sim.root() };
	assert_eq!(lookup.is_high(4, 0), Some(false), "back to low after driving low");
}

/// The full save round-trip keeps displays attached to their subchip ids,
/// so a customized-and-saved panel still resolves its displays' states
/// after a reload.
#[test]
fn saved_panel_displays_survive_round_trip_with_ids_intact() {
	let mut panel = ChipDescription::new("Panel", ChipType::Custom);
	panel.input_pins.push(PinDescription::new("IN", 1, PinBitCount::Bit1));
	panel.sub_chips.push(SubChipDescription {
		name: "LED".into(),
		id: 5,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});
	panel.displays.push(logic_sim::DisplayDescription::new(5, Vec2::splat(1.0), 1.25));

	let json = serialize_chip_description(&panel).unwrap();
	let parsed = parse_chip_description(&json).unwrap();

	assert_eq!(parsed.displays.len(), 1);
	assert_eq!(parsed.displays[0].sub_chip_id, 5);
	assert!(parsed.sub_chips.iter().any(|s| s.id == 5), "subchip id the display points at must survive");
}

/// Regression: a *placed* panel's embedded display must light on the
/// canvas. The display's `(subchip id, pin id)` addresses live inside the
/// panel's own simulation scope; resolving them against the root scope
/// (the pre-fix behaviour) leaves every embedded display permanently dark.
#[test]
fn placed_panel_embedded_display_lights_on_canvas() {
	use logic_sim::render::scene::{build_scene, PinStateLookup};

	let mut library = ChipLibrary::new();
	register_all_builtins(&mut library);

	// Panel interior: input boundary pin (id 1) -> 7-seg segment A (id 4).
	let mut panel = ChipDescription::new("Panel", ChipType::Custom);
	panel.input_pins.push(PinDescription::new("IN", 1, PinBitCount::Bit1));
	panel.size = Vec2::new(3.0, 2.0);
	panel.sub_chips.push(SubChipDescription {
		name: "7-SEGMENT".into(),
		id: 4,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});
	panel.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(4, 0)));
	panel.displays.push(logic_sim::DisplayDescription::new(4, Vec2::ZERO, 1.0));

	// Host chip: just one placed Panel instance (id 9).
	let mut host = ChipDescription::new("Host", ChipType::Custom);
	host.size = Vec2::new(6.0, 5.0);
	host.sub_chips.push(SubChipDescription {
		name: "Panel".into(),
		id: 9,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: vec![],
	});

	library.add(panel);

	// Drive the panel instance's boundary pin from outside (its address in
	// the host/root scope is owner=9), then draw the host scene.
	let mut sim = Simulator::build(&host.clone(), &library);
	let driven = vec![ExternalInput { address: PinAddress::new(9, 1), state: 1 }];
	for _ in 0..4 {
		sim.run_simulation_step(&driven);
	}

	let lookup = SimulatorPinState { sim: &sim, scope: sim.root() };
	let geo = build_scene(&host, &library, &lookup, None);

	let lit_seg_col = [1.0f32, 0.32, 0.28, 1.0].map(f32::to_bits); // theme::SEVEN_SEG_COLS[1], palette-A "on"
	let colours: std::collections::HashSet<_> = geo.triangles.iter().map(|v| v.colour.map(f32::to_bits)).collect();
	assert!(colours.contains(&lit_seg_col), "the panel's embedded 7-segment must render lit once its scope is entered");
}
