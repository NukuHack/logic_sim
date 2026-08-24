//! Default palette contents for a freshly-created project, ported from
//! `DLS.Game.BuiltinCollectionCreator`.

use crate::builtins::name_for;
use crate::description::ChipType;
use crate::json::{ChipCollection, StarredItem};

/// Mirrors `BuiltinCollectionCreator.GetDefaultStarredList`.
pub fn default_starred_list() -> Vec<StarredItem> {
	vec![StarredItem::new("IN/OUT", true), StarredItem::new(name_for(ChipType::Nand), false)]
}

/// Mirrors `BuiltinCollectionCreator.CreateDefaultChipCollections`. The
/// dev-only builtins (`dev.RAM-8`, the bus termini -- see
/// `ChipType::is_dev_only`) are only filed into a fresh palette in debug
/// builds; release projects never list them.
pub fn default_chip_collections() -> Vec<ChipCollection> {
	let mut bus = vec![ChipType::Bus1Bit, ChipType::Bus4Bit, ChipType::Bus8Bit];
	let mut memory = vec![ChipType::Rom256x16];
	if cfg!(debug_assertions) {
		bus.extend([ChipType::BusTerminus1Bit, ChipType::BusTerminus4Bit, ChipType::BusTerminus8Bit]);
		memory.push(ChipType::DevRam8Bit);
	}

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
		collection("BUS", &bus),
		collection("DISPLAY", &[ChipType::SevenSegmentDisplay, ChipType::DisplayDot, ChipType::DisplayRgb, ChipType::DisplayLed]),
		collection("MEMORY", &memory),
	]
}

fn collection(name: &str, chip_types: &[ChipType]) -> ChipCollection {
	ChipCollection::new(name, chip_types.iter().map(|t| name_for(*t)))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Locks in the palette coupling: fresh projects list the dev-only
	/// builtins only in debug builds (release keeps them out of every
	/// listing -- see `ChipType::is_dev_only` / `viewer::library`), while
	/// the regular chips are there either way.
	#[test]
	fn dev_builtins_are_palette_entries_in_debug_builds_only() {
		let names: Vec<String> = default_chip_collections().iter().flat_map(|c| c.chips.iter().cloned()).collect();
		let listed = |name: &str| names.iter().any(|n| n.eq_ignore_ascii_case(name));

		assert_eq!(listed("dev.RAM-8"), cfg!(debug_assertions));
		assert_eq!(listed("BUS-TERMINUS-1"), cfg!(debug_assertions));
		assert_eq!(listed("BUS-TERMINUS-4"), cfg!(debug_assertions));
		assert_eq!(listed("BUS-TERMINUS-8"), cfg!(debug_assertions));
		assert!(listed("BUS-1") && listed("BUS-4") && listed("BUS-8"));
		assert!(listed("ROM 256\u{d7}16"));
	}
}
