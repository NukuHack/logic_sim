//! Default palette contents for a freshly-created project, ported from
//! `DLS.Game.BuiltinCollectionCreator`.

use crate::builtins::name_for;
use crate::description::ChipType;
use crate::json::{ChipCollection, StarredItem};

/// Mirrors `BuiltinCollectionCreator.GetDefaultStarredList`.
pub fn default_starred_list() -> Vec<StarredItem> {
	vec![StarredItem::new("IN/OUT", true), StarredItem::new(name_for(ChipType::Nand), false)]
}

/// Mirrors `BuiltinCollectionCreator.CreateDefaultChipCollections`.
pub fn default_chip_collections() -> Vec<ChipCollection> {
	vec![
		collection(
			"BASIC",
			&[ChipType::Nand, ChipType::Clock, ChipType::Pulse, ChipType::Key, ChipType::KeyMods, ChipType::Buzzer, ChipType::TriStateBuffer],
		),
		collection("IN/OUT", &[ChipType::In1Bit, ChipType::In4Bit, ChipType::In8Bit, ChipType::Out1Bit, ChipType::Out4Bit, ChipType::Out8Bit]),
		collection(
			"MERGE/SPLIT",
			&[
				ChipType::Merge1To4Bit,
				ChipType::Merge1To8Bit,
				ChipType::Merge4To8Bit,
				ChipType::Split4To1Bit,
				ChipType::Split8To4Bit,
				ChipType::Split8To1Bit,
			],
		),
		collection(
			"BUS",
			&[
				ChipType::Bus1Bit,
				ChipType::BusTerminus1Bit,
				ChipType::Bus4Bit,
				ChipType::BusTerminus4Bit,
				ChipType::Bus8Bit,
				ChipType::BusTerminus8Bit,
			],
		),
		collection("DISPLAY", &[ChipType::SevenSegmentDisplay, ChipType::DisplayDot, ChipType::DisplayRgb, ChipType::DisplayLed]),
		collection("MEMORY", &[ChipType::Rom256x16, ChipType::DevRam8Bit]),
	]
}

fn collection(name: &str, chip_types: &[ChipType]) -> ChipCollection {
	ChipCollection::new(name, chip_types.iter().map(|t| name_for(*t)))
}
