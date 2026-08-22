use logic_sim::{
	load_chip_library_from_dir, load_project, parse_chip_description, serialize_chip_description, ChipCollection, ChipDescription, ChipLibrary,
	ChipType, Color, ExternalInput, NameLocation, PinAddress, PinBitCount, PinDescription, ProjectDescription, Simulator, StarredItem,
	SubChipDescription, ValueDisplayMode, Vec2, WireConnectionType, WireDescription,
};
use std::path::Path;

fn gol_fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/GOL")
}

fn fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/ZHT90")
}

// ============================================================================
// DESERIALIZATION TESTS
// ============================================================================

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
	let (_project, library, errors) = load_chip_library_from_dir(&gol_fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty(), "parse errors: {errors:?}");

	let cell = library.get("CELL");
	let bent_wire = cell.wires.iter().find(|w| w.points.len() == 4).expect("expected to find the bent wire with 4 interior points");

	let expected = [(1.625, -1.875), (1.625, -1.0), (-2.875, -1.0), (-2.875, -1.5)];
	for (p, (ex, ey)) in bent_wire.points.iter().zip(expected.iter()) {
		assert!((p.x - ex).abs() < 1e-4 && (p.y - ey).abs() < 1e-4, "point {p:?} != ({ex}, {ey})");
	}

	assert!(cell.wires.iter().any(|w| w.points.is_empty()), "expected at least one unbent wire");
}

#[test]
fn parses_minimal_chip_description() {
	let json = r#"{
        "Name": "TestChip",
        "ChipType": 0,
        "Size": { "x": 2.0, "y": 2.0 },
        "Colour": { "r": 0.5, "g": 0.5, "b": 0.5, "a": 1.0 },
        "NameLocation": 0,
        "InputPins": [],
        "OutputPins": [],
        "SubChips": [],
        "Wires": []
    }"#;

	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.name, "TestChip");
	assert_eq!(desc.chip_type, ChipType::Custom);
	assert_eq!(desc.colour, [0.5, 0.5, 0.5, 1.0]);
	assert_eq!(desc.name_location, NameLocation::Centre);
	assert!(desc.input_pins.is_empty());
	assert!(desc.output_pins.is_empty());
	assert!(desc.sub_chips.is_empty());
	assert!(desc.wires.is_empty());
}

#[test]
fn parses_chip_with_pins() {
	let json = r#"{
        "Name": "PinTest",
        "ChipType": 1,
        "InputPins": [
            { "Name": "A", "ID": 0, "Position": { "x": -2.0, "y": 0.0 }, "BitCount": 1, "Colour": 1, "ValueDisplayMode": 1 },
            { "Name": "B", "ID": 1, "Position": { "x": -2.0, "y": 1.0 }, "BitCount": 4, "Colour": 2, "ValueDisplayMode": 3 }
        ],
        "OutputPins": [
            { "Name": "OUT", "ID": 2, "Position": { "x": 2.0, "y": 0.0 }, "BitCount": 8, "Colour": 3, "ValueDisplayMode": 2 }
        ],
        "SubChips": [],
        "Wires": []
    }"#;

	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.chip_type, ChipType::Nand);

	assert_eq!(desc.input_pins.len(), 2);
	let pin_a = &desc.input_pins[0];
	assert_eq!(pin_a.name, "A");
	assert_eq!(pin_a.id, 0);
	assert_eq!(pin_a.bit_count, PinBitCount::Bit1);
	assert_eq!(pin_a.colour, Color::Orange);
	assert_eq!(pin_a.value_display_mode, ValueDisplayMode::Decimal);

	let pin_b = &desc.input_pins[1];
	assert_eq!(pin_b.name, "B");
	assert_eq!(pin_b.id, 1);
	assert_eq!(pin_b.bit_count, PinBitCount::Bit4);
	assert_eq!(pin_b.colour, Color::Yellow);
	assert_eq!(pin_b.value_display_mode, ValueDisplayMode::Hex);

	assert_eq!(desc.output_pins.len(), 1);
	let out_pin = &desc.output_pins[0];
	assert_eq!(out_pin.name, "OUT");
	assert_eq!(out_pin.id, 2);
	assert_eq!(out_pin.bit_count, PinBitCount::Bit8);
	assert_eq!(out_pin.colour, Color::Green);
	assert_eq!(out_pin.value_display_mode, ValueDisplayMode::SignedDecimal);
}

#[test]
fn parses_chip_with_subchips() {
	let json = r#"{
        "Name": "Parent",
        "ChipType": 0,
        "InputPins": [],
        "OutputPins": [],
        "SubChips": [
            {
                "Name": "NAND",
                "ID": 1,
                "Position": { "x": 1.0, "y": 2.0 },
                "OutputPinColourInfo": [
                    { "PinID": 0, "PinColour": 4 }
                ],
                "InternalData": [1, 2, 3, 4]
            }
        ],
        "Wires": []
    }"#;

	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.sub_chips.len(), 1);

	let sub = &desc.sub_chips[0];
	assert_eq!(sub.name, "NAND");
	assert_eq!(sub.id, 1);
	assert_eq!(sub.position, Vec2::new(1.0, 2.0));
	assert_eq!(sub.internal_data, Some(vec![1, 2, 3, 4]));
	assert_eq!(sub.pin_colour_info.len(), 1);
	assert_eq!(sub.pin_colour_info[0], (0, Color::Blue));
}

#[test]
fn parses_chip_with_wires() {
	let json = r#"{
        "Name": "WireTest",
        "ChipType": 0,
        "InputPins": [
            { "Name": "IN", "ID": 0, "Position": { "x": -2.0, "y": 0.0 }, "BitCount": 1, "Colour": 0, "ValueDisplayMode": 0 }
        ],
        "OutputPins": [
            { "Name": "OUT", "ID": 1, "Position": { "x": 2.0, "y": 0.0 }, "BitCount": 1, "Colour": 0, "ValueDisplayMode": 0 }
        ],
        "SubChips": [],
        "Wires": [
            {
                "SourcePinAddress": { "PinID": 0, "PinOwnerID": 0 },
                "TargetPinAddress": { "PinID": 1, "PinOwnerID": 1 },
                "ConnectionType": 0,
                "ConnectedWireIndex": -1,
                "ConnectedWireSegmentIndex": -1,
                "Points": [
                    { "x": -2.0, "y": 0.0 },
                    { "x": -1.0, "y": 0.0 },
                    { "x": 0.0, "y": 1.0 },
                    { "x": 1.0, "y": 1.0 },
                    { "x": 2.0, "y": 0.0 }
                ]
            }
        ]
    }"#;

	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.wires.len(), 1);

	let wire = &desc.wires[0];
	assert_eq!(wire.source_pin_address.pin_id, 0);
	assert_eq!(wire.source_pin_address.pin_owner_id, 0);
	assert_eq!(wire.target_pin_address.pin_id, 1);
	assert_eq!(wire.target_pin_address.pin_owner_id, 1);
	assert_eq!(wire.connection_type, WireConnectionType::ToPins);
	assert_eq!(wire.connected_wire_index, -1);
	assert_eq!(wire.connected_wire_segment_index, -1);
	assert_eq!(wire.cached_source_point.x, -2.0);
	assert_eq!(wire.cached_source_point.y, 0.0);
	assert_eq!(wire.cached_target_point.x, 2.0);
	assert_eq!(wire.cached_target_point.y, 0.0);
	assert_eq!(wire.points.len(), 3); // Interior points only
	assert_eq!(wire.points[0].x, -1.0);
	assert_eq!(wire.points[1].y, 1.0);
}

#[test]
fn parses_wire_with_to_wire_source_connection() {
	let json = r#"{
        "Name": "WireTapTest",
        "ChipType": 0,
        "InputPins": [],
        "OutputPins": [],
        "SubChips": [],
        "Wires": [
            {
                "SourcePinAddress": { "PinID": 0, "PinOwnerID": 0 },
                "TargetPinAddress": { "PinID": 1, "PinOwnerID": 1 },
                "ConnectionType": 1,
                "ConnectedWireIndex": 2,
                "ConnectedWireSegmentIndex": 3,
                "Points": [
                    { "x": 1.5, "y": 2.5 },
                    { "x": 2.5, "y": 3.5 }
                ]
            }
        ]
    }"#;

	let desc = parse_chip_description(json).unwrap();
	let wire = &desc.wires[0];
	assert_eq!(wire.connection_type, WireConnectionType::ToWireSource);
	assert_eq!(wire.connected_wire_index, 2);
	assert_eq!(wire.connected_wire_segment_index, 3);
	assert_eq!(wire.cached_source_point.x, 1.5);
	assert_eq!(wire.cached_source_point.y, 2.5);
	assert_eq!(wire.cached_target_point.x, 2.5);
	assert_eq!(wire.cached_target_point.y, 3.5);
	assert!(wire.points.is_empty()); // No interior points
}

#[test]
fn parses_all_enum_values_correctly() {
	// Test all ChipType values
	for i in 0..=30 {
		let json = format!(
			r#"{{
            "Name": "EnumTest",
            "ChipType": {},
            "InputPins": [],
            "OutputPins": [],
            "SubChips": [],
            "Wires": []
        }}"#,
			i
		);
		let desc = parse_chip_description(&json).unwrap();
		assert_eq!(desc.chip_type.to_int(), i);
	}

	// Test invalid ChipType falls back to Custom
	let json = r#"{
        "Name": "Invalid",
        "ChipType": 999,
        "InputPins": [],
        "OutputPins": [],
        "SubChips": [],
        "Wires": []
    }"#;
	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.chip_type, ChipType::Custom);
}

#[test]
fn parses_project_description_correctly() {
	let json = r#"{
        "ProjectName": "MyProject",
        "DLSVersion_LastSaved": "0.8.2",
        "DLSVersion_EarliestCompatible": "0.8.0",
        "CreationTime": "2024-01-01T00:00:00",
        "LastSaveTime": "2024-01-02T00:00:00",
        "Prefs_MainPinNamesDisplayMode": 1,
        "Prefs_ChipPinNamesDisplayMode": 2,
        "Prefs_GridDisplayMode": 1,
        "Prefs_Snapping": 1,
        "Prefs_StraightWires": 0,
        "Prefs_SimPaused": false,
        "Prefs_SimTargetStepsPerSecond": 60,
        "Prefs_SimStepsPerClockTick": 1,
        "AllCustomChipNames": ["NOT", "AND", "OR"],
        "StarredList": [
            { "Name": "NOT", "IsCollection": false },
            { "Name": "MyCollection", "IsCollection": true }
        ],
        "ChipCollections": [
            { "Name": "Basic", "IsToggledOpen": true, "Chips": ["NOT", "AND"] },
            { "Name": "Advanced", "IsToggledOpen": false, "Chips": ["ADDER", "MUX"] }
        ]
    }"#;

	let project: ProjectDescription = serde_json::from_str(json).unwrap();
	assert_eq!(project.project_name, "MyProject");
	assert_eq!(project.dls_version_last_saved, "0.8.2");
	assert_eq!(project.dls_version_earliest_compatible, "0.8.0");
	assert_eq!(project.prefs_sim_target_steps_per_second, 60);
	assert_eq!(project.all_custom_chip_names, vec!["NOT", "AND", "OR"]);
	assert_eq!(project.starred_list.len(), 2);
	assert_eq!(project.starred_list[0].name, "NOT");
	assert!(!project.starred_list[0].is_collection);
	assert_eq!(project.chip_collections.len(), 2);
	assert_eq!(project.chip_collections[0].name, "Basic");
	assert!(project.chip_collections[0].is_toggled_open);
	assert_eq!(project.chip_collections[0].chips, vec!["NOT", "AND"]);
}

// ============================================================================
// FAILING DESERIALIZATION TESTS
// ============================================================================

#[test]
fn fails_to_parse_invalid_json() {
	let invalid_json = r#"{
        "Name": "Broken",
        "ChipType": 0,
        "InputPins": [{
            "Name": "A",
            "ID": "not_an_int",  // Should be integer
            "BitCount": 1,
            "Colour": 0,
            "ValueDisplayMode": 0
        }],
        "OutputPins": [],
        "SubChips": [],
        "Wires": []
    }"#;

	let result = parse_chip_description(invalid_json);
	assert!(result.is_err());
}

#[test]
fn fails_to_parse_missing_required_fields() {
	let json = r#"{
        "ChipType": 0,
        "InputPins": [],
        "OutputPins": [],
        "SubChips": [],
        "Wires": []
    }"#;

	let result = parse_chip_description(json);
	assert!(result.is_err());
}

#[test]
fn fails_to_parse_invalid_bit_count() {
	let json = r#"{
        "Name": "BadBitCount",
        "ChipType": 0,
        "InputPins": [
            { "Name": "A", "ID": 0, "Position": { "x": 0.0, "y": 0.0 }, "BitCount": 2, "Colour": 0, "ValueDisplayMode": 0 }
        ],
        "OutputPins": [],
        "SubChips": [],
        "Wires": []
    }"#;

	// Invalid bit count should default to Bit1
	let desc = parse_chip_description(json).unwrap();
	assert_eq!(desc.input_pins[0].bit_count, PinBitCount::Bit1);
}

#[test]
fn fails_to_parse_malformed_project_description() {
	let json = r#"{
        "ProjectName": "Broken",
        "StarredList": "not_an_array"  // Should be array
    }"#;

	let result: Result<ProjectDescription, _> = serde_json::from_str(json);
	assert!(result.is_err());
}

// ============================================================================
// SERIALIZATION TESTS
// ============================================================================

#[test]
fn serializes_basic_chip_description() {
	let mut desc = ChipDescription::new("TestChip", ChipType::Custom);
	desc.colour = [0.2, 0.3, 0.4, 0.5];
	desc.name_location = NameLocation::Top;

	let json = serialize_chip_description(&desc).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

	assert_eq!(parsed["Name"].as_str().unwrap(), "TestChip");
	assert_eq!(parsed["ChipType"].as_i64().unwrap(), 0);
	assert_eq!(parsed["NameLocation"].as_i64().unwrap(), 1);
	assert_eq!(parsed["Colour"]["r"].as_f64().unwrap(), 0.2);
	assert_eq!(parsed["Colour"]["g"].as_f64().unwrap(), 0.3);
	assert_eq!(parsed["Colour"]["b"].as_f64().unwrap(), 0.4);
	assert_eq!(parsed["Colour"]["a"].as_f64().unwrap(), 0.5);
	assert_eq!(parsed["DLSVersion"].as_str().unwrap(), "0.0.0");
}

#[test]
fn serializes_chip_with_pins() {
	let mut desc = ChipDescription::new("PinChip", ChipType::Nand);

	desc.input_pins.push(PinDescription {
		name: "A".to_string(),
		id: 0,
		position: Default::default(),
		bit_count: PinBitCount::Bit1,
		colour: Color::Red,
		value_display_mode: ValueDisplayMode::Decimal,
		driven_state: 0,
	});

	desc.input_pins.push(PinDescription {
		name: "B".to_string(),
		id: 1,
		position: Default::default(),
		bit_count: PinBitCount::Bit4,
		colour: Color::Yellow,
		value_display_mode: ValueDisplayMode::Hex,
		driven_state: 0,
	});

	desc.output_pins.push(PinDescription {
		name: "OUT".to_string(),
		id: 2,
		position: Default::default(),
		bit_count: PinBitCount::Bit8,
		colour: Color::Green,
		value_display_mode: ValueDisplayMode::SignedDecimal,
		driven_state: 0,
	});

	let json = serialize_chip_description(&desc).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

	let inputs = parsed["InputPins"].as_array().unwrap();
	assert_eq!(inputs.len(), 2);
	assert_eq!(inputs[0]["Name"].as_str().unwrap(), "A");
	assert_eq!(inputs[0]["BitCount"].as_i64().unwrap(), 1);
	assert_eq!(inputs[0]["Colour"].as_i64().unwrap(), 0);
	assert_eq!(inputs[0]["ValueDisplayMode"].as_i64().unwrap(), 1);
	assert_eq!(inputs[1]["BitCount"].as_i64().unwrap(), 4);
	assert_eq!(inputs[1]["Colour"].as_i64().unwrap(), 2);
	assert_eq!(inputs[1]["ValueDisplayMode"].as_i64().unwrap(), 3);

	let outputs = parsed["OutputPins"].as_array().unwrap();
	assert_eq!(outputs.len(), 1);
	assert_eq!(outputs[0]["Name"].as_str().unwrap(), "OUT");
	assert_eq!(outputs[0]["BitCount"].as_i64().unwrap(), 8);
	assert_eq!(outputs[0]["Colour"].as_i64().unwrap(), 3);
	assert_eq!(outputs[0]["ValueDisplayMode"].as_i64().unwrap(), 2);
}

#[test]
fn serializes_chip_with_subchips() {
	let mut desc = ChipDescription::new("Parent", ChipType::Custom);

	let sub = SubChipDescription {
		name: "NAND".to_string(),
		id: 1,
		internal_data: Some(vec![1, 2, 3, 4]),
		label: None,
		position: Vec2::new(1.0, 2.0),
		pin_colour_info: vec![(0, Color::Blue), (1, Color::Pink)],
	};
	desc.sub_chips.push(sub);

	let json = serialize_chip_description(&desc).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

	let subchips = parsed["SubChips"].as_array().unwrap();
	assert_eq!(subchips.len(), 1);
	assert_eq!(subchips[0]["Name"].as_str().unwrap(), "NAND");
	assert_eq!(subchips[0]["ID"].as_i64().unwrap(), 1);
	assert_eq!(subchips[0]["Position"]["x"].as_f64().unwrap(), 1.0);
	assert_eq!(subchips[0]["Position"]["y"].as_f64().unwrap(), 2.0);

	let colour_info = subchips[0]["OutputPinColourInfo"].as_array().unwrap();
	assert_eq!(colour_info.len(), 2);
	assert_eq!(colour_info[0]["PinID"].as_i64().unwrap(), 0);
	assert_eq!(colour_info[0]["PinColour"].as_i64().unwrap(), 4);
	assert_eq!(colour_info[1]["PinID"].as_i64().unwrap(), 1);
	assert_eq!(colour_info[1]["PinColour"].as_i64().unwrap(), 6);

	let internal_data = subchips[0]["InternalData"].as_array().unwrap();
	assert_eq!(internal_data.len(), 4);
	assert_eq!(internal_data[0].as_u64().unwrap(), 1);
}

#[test]
fn serializes_chip_with_wires() {
	let mut desc = ChipDescription::new("WireChip", ChipType::Custom);

	let wire = WireDescription {
		source_pin_address: PinAddress::new(0, 1),
		target_pin_address: PinAddress::new(2, 3),
		connection_type: WireConnectionType::ToWireSource,
		connected_wire_index: 4,
		connected_wire_segment_index: 5,
		cached_source_point: Vec2 { x: 1.5, y: 2.5 },
		cached_target_point: Vec2 { x: 3.5, y: 4.5 },
		points: vec![Vec2 { x: 2.0, y: 3.0 }, Vec2 { x: 2.5, y: 3.5 }],
	};
	desc.wires.push(wire);

	let json = serialize_chip_description(&desc).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

	let wires = parsed["Wires"].as_array().unwrap();
	assert_eq!(wires.len(), 1);

	assert_eq!(wires[0]["SourcePinAddress"]["PinID"].as_i64().unwrap(), 1);
	assert_eq!(wires[0]["SourcePinAddress"]["PinOwnerID"].as_i64().unwrap(), 0);
	assert_eq!(wires[0]["TargetPinAddress"]["PinID"].as_i64().unwrap(), 3);
	assert_eq!(wires[0]["TargetPinAddress"]["PinOwnerID"].as_i64().unwrap(), 2);
	assert_eq!(wires[0]["ConnectionType"].as_i64().unwrap(), 1);
	assert_eq!(wires[0]["ConnectedWireIndex"].as_i64().unwrap(), 4);
	assert_eq!(wires[0]["ConnectedWireSegmentIndex"].as_i64().unwrap(), 5);

	let points = wires[0]["Points"].as_array().unwrap();
	assert_eq!(points.len(), 4); // source + 2 bends + target
	assert_eq!(points[0]["x"].as_f64().unwrap(), 1.5);
	assert_eq!(points[0]["y"].as_f64().unwrap(), 2.5);
	assert_eq!(points[1]["x"].as_f64().unwrap(), 2.0);
	assert_eq!(points[2]["y"].as_f64().unwrap(), 3.5);
	assert_eq!(points[3]["x"].as_f64().unwrap(), 3.5);
	assert_eq!(points[3]["y"].as_f64().unwrap(), 4.5);
}

#[test]
fn serializes_project_description() {
	let mut project = ProjectDescription::default();
	project.project_name = "TestProject".to_string();
	project.dls_version_last_saved = "0.8.2".to_string();
	project.dls_version_earliest_compatible = "0.8.0".to_string();
	project.prefs_sim_target_steps_per_second = 120;
	project.all_custom_chip_names = vec!["CHIP1".to_string(), "CHIP2".to_string()];
	project.starred_list = vec![StarredItem::new("CHIP1", false), StarredItem::new("Collection", true)];
	project.chip_collections = vec![ChipCollection::new("Basic", vec!["CHIP1", "CHIP2"])];

	let json = serde_json::to_string_pretty(&project).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

	assert_eq!(parsed["ProjectName"].as_str().unwrap(), "TestProject");
	assert_eq!(parsed["DLSVersion_LastSaved"].as_str().unwrap(), "0.8.2");
	assert_eq!(parsed["Prefs_SimTargetStepsPerSecond"].as_i64().unwrap(), 120);

	let starred = parsed["StarredList"].as_array().unwrap();
	assert_eq!(starred.len(), 2);
	assert_eq!(starred[0]["Name"].as_str().unwrap(), "CHIP1");
	assert!(!starred[0]["IsCollection"].as_bool().unwrap());

	let collections = parsed["ChipCollections"].as_array().unwrap();
	assert_eq!(collections.len(), 1);
	assert_eq!(collections[0]["Name"].as_str().unwrap(), "Basic");
	let chips = collections[0]["Chips"].as_array().unwrap();
	assert_eq!(chips.len(), 2);
	assert_eq!(chips[0].as_str().unwrap(), "CHIP1");
}

// ============================================================================
// ROUNDTRIP TESTS
// ============================================================================

#[test]
fn roundtrip_basic_chip() {
	let original = ChipDescription::new("RoundtripTest", ChipType::Clock);

	let json = serialize_chip_description(&original).unwrap();
	let roundtrip = parse_chip_description(&json).unwrap();

	assert_eq!(roundtrip.name, original.name);
	assert_eq!(roundtrip.chip_type, original.chip_type);
	assert_eq!(roundtrip.input_pins.len(), 0);
	assert_eq!(roundtrip.output_pins.len(), 0);
	assert_eq!(roundtrip.sub_chips.len(), 0);
	assert_eq!(roundtrip.wires.len(), 0);
}

#[test]
fn roundtrip_chip_with_all_features() {
	let mut original = ChipDescription::new("FullChip", ChipType::Custom);
	original.colour = [0.1, 0.2, 0.3, 0.4];
	original.name_location = NameLocation::Hidden;
	original.size = Vec2 { x: 3.0, y: 4.0 };

	// Input pins
	original.input_pins.push(PinDescription {
		name: "IN1".to_string(),
		id: 0,
		position: Vec2 { x: -2.0, y: 0.0 },
		bit_count: PinBitCount::Bit1,
		colour: Color::Red,
		value_display_mode: ValueDisplayMode::Decimal,
		driven_state: 0,
	});
	original.input_pins.push(PinDescription {
		name: "IN4".to_string(),
		id: 1,
		position: Vec2 { x: -2.0, y: 1.0 },
		bit_count: PinBitCount::Bit4,
		colour: Color::Orange,
		value_display_mode: ValueDisplayMode::Hex,
		driven_state: 0,
	});

	// Output pins
	original.output_pins.push(PinDescription {
		name: "OUT8".to_string(),
		id: 2,
		position: Vec2 { x: 2.0, y: 0.0 },
		bit_count: PinBitCount::Bit8,
		colour: Color::Green,
		value_display_mode: ValueDisplayMode::SignedDecimal,
		driven_state: 0,
	});

	// Subchips
	original.sub_chips.push(SubChipDescription {
		name: "Sub1".to_string(),
		id: 3,
		internal_data: Some(vec![5, 6, 7]),
		label: None,
		position: Vec2::new(0.5, 1.5),
		pin_colour_info: vec![(0, Color::Purple), (1, Color::Pink)],
	});

	// Wires
	original.wires.push(WireDescription {
		source_pin_address: PinAddress::new(0, 0),
		target_pin_address: PinAddress::new(3, 2),
		connection_type: WireConnectionType::ToPins,
		connected_wire_index: -1,
		connected_wire_segment_index: -1,
		cached_source_point: Vec2 { x: -2.0, y: 0.0 },
		cached_target_point: Vec2 { x: 0.5, y: 1.5 },
		points: vec![Vec2 { x: -1.0, y: 0.5 }, Vec2 { x: 0.0, y: 1.0 }],
	});

	original.wires.push(WireDescription {
		source_pin_address: PinAddress::new(3, 1),
		target_pin_address: PinAddress::new(2, 2),
		connection_type: WireConnectionType::ToWireSource,
		connected_wire_index: 0,
		connected_wire_segment_index: 1,
		cached_source_point: Vec2 { x: 0.0, y: 1.0 },
		cached_target_point: Vec2 { x: 2.0, y: 0.0 },
		points: vec![],
	});

	let json = serialize_chip_description(&original).unwrap();
	print!("{}", json);
	let roundtrip = parse_chip_description(&json).unwrap();

	// Compare everything
	assert_eq!(roundtrip.name, original.name);
	assert_eq!(roundtrip.chip_type, original.chip_type);
	assert_eq!(roundtrip.colour, original.colour);
	assert_eq!(roundtrip.name_location, original.name_location);
	assert_eq!(roundtrip.size, original.size);

	// Pins
	assert_eq!(roundtrip.input_pins.len(), original.input_pins.len());
	for (r, o) in roundtrip.input_pins.iter().zip(original.input_pins.iter()) {
		assert_eq!(r.name, o.name);
		assert_eq!(r.id, o.id);
		assert_eq!(r.bit_count, o.bit_count);
		assert_eq!(r.colour, o.colour);
		assert_eq!(r.value_display_mode, o.value_display_mode);
		// Position is not preserved in serialization (set to default)
	}

	assert_eq!(roundtrip.output_pins.len(), original.output_pins.len());
	for (r, o) in roundtrip.output_pins.iter().zip(original.output_pins.iter()) {
		assert_eq!(r.name, o.name);
		assert_eq!(r.id, o.id);
		assert_eq!(r.bit_count, o.bit_count);
		assert_eq!(r.colour, o.colour);
		assert_eq!(r.value_display_mode, o.value_display_mode);
	}

	// Subchips
	assert_eq!(roundtrip.sub_chips.len(), original.sub_chips.len());
	for (r, o) in roundtrip.sub_chips.iter().zip(original.sub_chips.iter()) {
		assert_eq!(r.name, o.name);
		assert_eq!(r.id, o.id);
		assert_eq!(r.internal_data, o.internal_data);
		assert_eq!(r.position, o.position);
		assert_eq!(r.pin_colour_info, o.pin_colour_info);
	}

	// Wires
	assert_eq!(roundtrip.wires.len(), original.wires.len());
	for (r, o) in roundtrip.wires.iter().zip(original.wires.iter()) {
		assert_eq!(r.source_pin_address.pin_id, o.source_pin_address.pin_id);
		assert_eq!(r.source_pin_address.pin_owner_id, o.source_pin_address.pin_owner_id);
		assert_eq!(r.target_pin_address.pin_id, o.target_pin_address.pin_id);
		assert_eq!(r.target_pin_address.pin_owner_id, o.target_pin_address.pin_owner_id);
		assert_eq!(r.connection_type, o.connection_type);
		assert_eq!(r.connected_wire_index, o.connected_wire_index);
		assert_eq!(r.connected_wire_segment_index, o.connected_wire_segment_index);
		assert_eq!(r.cached_source_point, o.cached_source_point);
		assert_eq!(r.cached_target_point, o.cached_target_point);
		assert_eq!(r.points, o.points);
	}
}

#[test]
fn roundtrip_project_description() {
	let mut original = ProjectDescription::default();
	original.project_name = "RoundtripProject".to_string();
	original.dls_version_last_saved = "0.8.2".to_string();
	original.dls_version_earliest_compatible = "0.8.0".to_string();
	original.creation_time = "2024-01-01T00:00:00".to_string();
	original.last_save_time = "2024-01-02T00:00:00".to_string();
	original.prefs_main_pin_names_display_mode = 1;
	original.prefs_chip_pin_names_display_mode = 2;
	original.prefs_grid_display_mode = 1;
	original.prefs_snapping = 1;
	original.prefs_straight_wires = 0;
	original.prefs_sim_paused = false;
	original.prefs_sim_target_steps_per_second = 60;
	original.prefs_sim_steps_per_clock_tick = 1;
	original.all_custom_chip_names = vec!["NOT".to_string(), "AND".to_string(), "OR".to_string()];
	original.starred_list = vec![StarredItem::new("NOT", false), StarredItem::new("Collection", true)];
	original.chip_collections = vec![ChipCollection::new("Basic", vec!["NOT", "AND"]), ChipCollection::new("Advanced", vec!["ADDER"])];

	let json = serde_json::to_string_pretty(&original).unwrap();
	let roundtrip: ProjectDescription = serde_json::from_str(&json).unwrap();

	assert_eq!(roundtrip.project_name, original.project_name);
	assert_eq!(roundtrip.dls_version_last_saved, original.dls_version_last_saved);
	assert_eq!(roundtrip.dls_version_earliest_compatible, original.dls_version_earliest_compatible);
	assert_eq!(roundtrip.creation_time, original.creation_time);
	assert_eq!(roundtrip.last_save_time, original.last_save_time);
	assert_eq!(roundtrip.prefs_main_pin_names_display_mode, original.prefs_main_pin_names_display_mode);
	assert_eq!(roundtrip.prefs_chip_pin_names_display_mode, original.prefs_chip_pin_names_display_mode);
	assert_eq!(roundtrip.prefs_grid_display_mode, original.prefs_grid_display_mode);
	assert_eq!(roundtrip.prefs_snapping, original.prefs_snapping);
	assert_eq!(roundtrip.prefs_straight_wires, original.prefs_straight_wires);
	assert_eq!(roundtrip.prefs_sim_paused, original.prefs_sim_paused);
	assert_eq!(roundtrip.prefs_sim_target_steps_per_second, original.prefs_sim_target_steps_per_second);
	assert_eq!(roundtrip.prefs_sim_steps_per_clock_tick, original.prefs_sim_steps_per_clock_tick);
	assert_eq!(roundtrip.all_custom_chip_names, original.all_custom_chip_names);
	assert_eq!(roundtrip.starred_list, original.starred_list);
	assert_eq!(roundtrip.chip_collections, original.chip_collections);
}

#[test]
fn roundtrip_chip_with_all_enum_variants() {
	let mut original = ChipDescription::new("EnumTest", ChipType::Buzzer);
	original.name_location = NameLocation::Top;

	// Test all color variants
	for color in [Color::Red, Color::Orange, Color::Yellow, Color::Green, Color::Blue, Color::Purple, Color::Pink, Color::White] {
		original.input_pins.push(PinDescription {
			name: format!("{:?}", color),
			id: color.to_int(),
			position: Default::default(),
			bit_count: PinBitCount::Bit1,
			colour: color,
			value_display_mode: ValueDisplayMode::None,
			driven_state: 0,
		});
	}

	// Test all ValueDisplayMode variants
	for mode in [ValueDisplayMode::None, ValueDisplayMode::Decimal, ValueDisplayMode::SignedDecimal, ValueDisplayMode::Hex] {
		original.output_pins.push(PinDescription {
			name: format!("{:?}", mode),
			id: mode.to_int(),
			position: Default::default(),
			bit_count: PinBitCount::Bit1,
			colour: Color::Red,
			value_display_mode: mode,
			driven_state: 0,
		});
	}

	// Test all WireConnectionType variants
	for conn_type in [WireConnectionType::ToPins, WireConnectionType::ToWireSource, WireConnectionType::ToWireTarget] {
		original.wires.push(WireDescription {
			source_pin_address: PinAddress::new(0, 0),
			target_pin_address: PinAddress::new(1, 1),
			connection_type: conn_type,
			connected_wire_index: 0,
			connected_wire_segment_index: 0,
			cached_source_point: Vec2::default(),
			cached_target_point: Vec2::default(),
			points: vec![],
		});
	}

	let json = serialize_chip_description(&original).unwrap();
	let roundtrip = parse_chip_description(&json).unwrap();

	// Check colors roundtrip
	for (r, o) in roundtrip.input_pins.iter().zip(original.input_pins.iter()) {
		assert_eq!(r.colour, o.colour);
		assert_eq!(r.value_display_mode, o.value_display_mode);
	}

	// Check display modes roundtrip
	for (r, o) in roundtrip.output_pins.iter().zip(original.output_pins.iter()) {
		assert_eq!(r.value_display_mode, o.value_display_mode);
	}

	// Check connection types roundtrip
	for (r, o) in roundtrip.wires.iter().zip(original.wires.iter()) {
		assert_eq!(r.connection_type, o.connection_type);
	}
}

// ============================================================================
// SIMULATION TESTS
// ============================================================================

#[test]
fn simulates_the_loaded_not_chip_correctly() {
	let (_project, mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty());

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

#[test]
fn simulates_loaded_not_chip_with_serialization_roundtrip() {
	// Load NOT chip
	let (_project, mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).map(|(lib, errs)| ((), lib, errs)).unwrap();
	assert!(errors.is_empty());

	// Add NAND builtin
	let mut nand = ChipDescription::new("NAND", ChipType::Nand);
	nand.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
	nand.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
	nand.output_pins.push(PinDescription::new("OUT", 2, PinBitCount::Bit1));
	library.add(nand.clone());

	// Get NOT description, serialize and reload it
	let original_not = library.get("NOT").clone();
	let json = serialize_chip_description(&original_not).unwrap();
	let roundtrip_not = parse_chip_description(&json).unwrap();

	// Add the roundtripped chip to a new library
	let mut new_library = ChipLibrary::new();
	new_library.add(roundtrip_not.clone());
	new_library.add(nand);

	// Test simulation on the roundtripped chip
	let in_pin_id = roundtrip_not.input_pins[0].id;
	let out_pin_id = roundtrip_not.output_pins[0].id;

	let mut sim = Simulator::build(&roundtrip_not, &new_library);

	for &input_val in &[0u32, 1] {
		let inputs = vec![ExternalInput { address: PinAddress::new(in_pin_id, in_pin_id), state: input_val }];
		for _ in 0..3 {
			sim.run_simulation_step(&inputs);
		}

		let out_pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("output pin should resolve");
		let out_state = sim.pin(out_pin).state & 1;

		assert_eq!(out_state, 1 - input_val, "Roundtripped NOT({input_val}) should invert");
	}
}

#[test]
fn chip_library_is_starred_functionality() {
	let mut project = ProjectDescription::default();
	project.starred_list = vec![StarredItem::new("NOT", false), StarredItem::new("Collection", true)];

	assert!(project.is_starred("NOT", false));
	assert!(!project.is_starred("AND", false));
	assert!(project.is_starred("Collection", true));
	assert!(!project.is_starred("Collection", false));
	assert!(project.is_starred("not", false)); // Case insensitive
}
