//! Save-format upgrades for chips saved by older versions of the game.

use crate::description::{ChipDescription, Color};
use crate::save_system::version::Version;

/// Version assumed for chips whose save file carries no parseable
/// `DLSVersion` (mirrors `ApplyVersionChanges`' `defaultVersion`).
const DEFAULT_CHIP_VERSION: Version = Version::new(2, 0, 0);

/// The last chip format that still needs [`update_chip_pre_2_1_5`].
const VERSION_PRE_2_1_5: Version = Version::new(2, 1, 4);

/// The builtin LED chip's name (`ChipTypeHelper.GetName(ChipType.DisplayLED)`),
/// matched case-insensitively like `ChipDescription.NameMatch`.
const LED_NAME: &str = "LED";

/// Mirrors `UpgradeHelper.ApplyVersionChanges`: migrates every custom chip
/// saved by a version at or before 2.1.4 in place. Builtin chips are
/// deliberately not passed in -- they're always constructed fresh by this
/// build and can never carry old data.
pub(crate) fn apply_version_changes(custom_chips: &mut [ChipDescription]) {
	for chip in custom_chips.iter_mut() {
		let chip_version = chip.dls_version.as_deref().and_then(Version::try_parse).unwrap_or(DEFAULT_CHIP_VERSION);
		if chip_version <= VERSION_PRE_2_1_5 {
			update_chip_pre_2_1_5(chip);
			chip.dls_version = Some(VERSION_PRE_2_1_5.to_string());
		}
	}
}

/// Mirrors `UpgradeHelper.UpdateChipPre_2_1_5`.
fn update_chip_pre_2_1_5(chip: &mut ChipDescription) {
	// Update input pin cols.
	for pin in &mut chip.input_pins {
		pin.colour = get_new_pin_colour(pin.colour);
	}

	for sub in &mut chip.sub_chips {
		// ---- Added LED colour option (requires instance data array size of 1 for led subchips) ----
		if sub.name.eq_ignore_ascii_case(LED_NAME) && sub.internal_data.as_ref().is_none_or(|data| data.is_empty()) {
			sub.internal_data = Some(vec![0]);
		}

		// Update subchip output pin cols.
		for (_, colour) in &mut sub.pin_colour_info {
			*colour = get_new_pin_colour(*colour);
		}
	}
}

/// ---- Inserted ORANGE as colour option at index 1, so update old indices to correct values ----
fn get_new_pin_colour(col_old: Color) -> Color {
	let colour_index = col_old.to_int();
	Color::from_int(if colour_index > 0 { colour_index + 1 } else { colour_index })
}

#[cfg(test)]
mod tests {
	//! White-box: the migrations run inside the loader over freshly-parsed
	//! descriptions before anything reaches the public API, so exercising
	//! them here against constructed `ChipDescription`s (plus one full
	//! JSON round trip) is the only way to pin their exact semantics.

	use super::*;
	use crate::description::{ChipType, PinBitCount, PinDescription, SubChipDescription};
	use crate::structs::Vec2;

	fn chip_with_input_colour(index: i32) -> ChipDescription {
		let mut chip = ChipDescription::new("OLD", ChipType::Custom);
		chip.input_pins.push(PinDescription::with_colour("IN", 1, PinBitCount::Bit1, Color::from_int(index)));
		chip.dls_version = Some("2.1.3".to_string());
		chip
	}

	#[test]
	fn orange_shift_moves_every_index_above_zero_up_by_one() {
		for (old, expected) in [(0, 0), (1, 2), (6, 7), (7, 7)] {
			let mut chip = chip_with_input_colour(old.min(6));
			update_chip_pre_2_1_5(&mut chip);
			assert_eq!(chip.input_pins[0].colour.to_int(), if old == 7 { 7 } else { expected }, "old index {old}");
		}
	}

	#[test]
	fn led_without_internal_data_gets_the_default_colour_slot() {
		let mut chip = chip_with_input_colour(0);
		chip.sub_chips.push(SubChipDescription {
			name: "led".into(), // name match is case-insensitive
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		chip.sub_chips.push(SubChipDescription {
			name: "LED".into(),
			id: 2,
			internal_data: Some(Vec::new()),
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		chip.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 3,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});

		update_chip_pre_2_1_5(&mut chip);

		assert_eq!(chip.sub_chips[0].internal_data, Some(vec![0]), "absent data defaults");
		assert_eq!(chip.sub_chips[1].internal_data, Some(vec![0]), "empty data defaults");
		assert_eq!(chip.sub_chips[2].internal_data, None, "non-LED chips are untouched");
	}

	#[test]
	fn output_pin_colour_info_shifts_with_the_palette() {
		let mut chip = chip_with_input_colour(0);
		chip.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![(0, Color::from_int(0)), (1, Color::from_int(3))],
		});

		update_chip_pre_2_1_5(&mut chip);

		assert_eq!(chip.sub_chips[0].pin_colour_info[0].1.to_int(), 0, "index 0 stays put");
		assert_eq!(chip.sub_chips[0].pin_colour_info[1].1.to_int(), 4, "index 3 shifts past ORANGE");
	}

	#[test]
	fn only_pre_2_1_5_chips_migrate_and_are_restamped() {
		let mut chips = vec![
			chip_with_input_colour(1),
			{
				let mut c = chip_with_input_colour(1);
				c.dls_version = Some("2.1.5".to_string());
				c
			},
			{
				let mut c = chip_with_input_colour(1);
				c.dls_version = Some("garbage".to_string());
				c
			},
			{
				let mut c = chip_with_input_colour(1);
				c.dls_version = None;
				c
			},
		];

		apply_version_changes(&mut chips);

		assert_eq!(chips[0].input_pins[0].colour.to_int(), 2, "2.1.3 migrates");
		assert_eq!(chips[0].dls_version.as_deref(), Some("2.1.4"));
		assert_eq!(chips[1].input_pins[0].colour.to_int(), 1, "2.1.5+ is already current");
		assert_eq!(chips[1].dls_version.as_deref(), Some("2.1.5"));
		assert_eq!(chips[2].input_pins[0].colour.to_int(), 2, "unparseable falls back to 2.0.0");
		assert_eq!(chips[3].input_pins[0].colour.to_int(), 2, "absent falls back to 2.0.0");
	}

	/// End-to-end through the real loader: a chip file written by an old
	/// build loads with the shift applied, and re-saving stamps it with
	/// the running version so a second load doesn't shift again.
	#[test]
	fn loaded_old_chips_upgrade_exactly_once_across_a_round_trip() {
		let json = r#"{
			"DLSVersion": "2.1.4",
			"Name": "OLD",
			"NameLocation": 0,
			"ChipType": 0,
			"InputPins": [ { "Name": "IN", "ID": 1, "Position": {"x":0,"y":0}, "BitCount": 1, "Colour": 2 } ]
		}"#;
		let mut chip = crate::json::parse_chip_description(json).expect("parses");
		apply_version_changes(std::slice::from_mut(&mut chip));

		assert_eq!(chip.input_pins[0].colour.to_int(), 3, "old Yellow(2) -> Green(3)");

		let resaved = crate::json::serialize_chip_description(&chip).expect("serializes");
		let reloaded = crate::json::parse_chip_description(&resaved).expect("reloads");
		assert_ne!(reloaded.dls_version.as_deref(), Some("2.1.4"), "resave stamps the current version");
		let mut again = reloaded;
		apply_version_changes(std::slice::from_mut(&mut again));
		assert_eq!(again.input_pins[0].colour.to_int(), 3, "second pass is a no-op");
	}
}
