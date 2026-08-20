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
        collection("BASIC", &[ChipType::Nand, ChipType::Clock, ChipType::Pulse, ChipType::Key, ChipType::TriStateBuffer]),
        collection("IN/OUT", &[ChipType::In1Bit, ChipType::In4Bit, ChipType::In8Bit, ChipType::Out1Bit, ChipType::Out4Bit, ChipType::Out8Bit]),
        collection(
            "MERGE/SPLIT",
            &[ChipType::Merge1To4Bit, ChipType::Merge1To8Bit, ChipType::Merge4To8Bit, ChipType::Split4To1Bit, ChipType::Split8To4Bit, ChipType::Split8To1Bit],
        ),
        collection("BUS", &[ChipType::Bus1Bit, ChipType::Bus4Bit, ChipType::Bus8Bit]),
        collection("DISPLAY", &[ChipType::SevenSegmentDisplay, ChipType::DisplayDot, ChipType::DisplayRgb, ChipType::DisplayLed]),
        collection("MEMORY", &[ChipType::Rom256x16]),
    ]
}

fn collection(name: &str, chip_types: &[ChipType]) -> ChipCollection {
    ChipCollection::new(name, chip_types.iter().map(|t| name_for(*t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_starred_list_matches_original() {
        let list = default_starred_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "IN/OUT");
        assert!(list[0].is_collection);
        assert_eq!(list[1].name, "NAND");
        assert!(!list[1].is_collection);
    }

    #[test]
    fn default_chip_collections_match_original_names_and_contents() {
        let collections = default_chip_collections();
        let names: Vec<&str> = collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["BASIC", "IN/OUT", "MERGE/SPLIT", "BUS", "DISPLAY", "MEMORY"]);

        let basic = &collections[0];
        assert_eq!(basic.chips, vec!["NAND", "CLOCK", "PULSE", "KEY", "3-STATE BUFFER"]);

        let memory = &collections[5];
        assert_eq!(memory.chips, vec!["ROM 256\u{d7}16"]);

        for c in &collections {
            assert!(!c.is_toggled_open);
        }
    }
}
