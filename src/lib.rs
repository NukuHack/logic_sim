//! Crate root for the logic simulator: wires together simulation, chip
//! description/serialization, save-system persistence, rendering, and the
//! viewer UI, and re-exports the public API surface used by the binary.

#![cfg_attr(docsrs, feature(doc_cfg))]
//#![warn(missing_docs)]
#![warn(
	clippy::pedantic, // strict-er clippy and optionalizations
	clippy::nursery, // strict-er clippy and optionalizations
	clippy::let_unit_value, // function that returns ()
	clippy::print_stdout, // side effect
	clippy::print_stderr, // side effect
	unsafe_code, // unsafe code
	clippy::panic,
	clippy::unwrap_used,
	clippy::expect_used,
)]
#![allow(
    clippy::cast_possible_truncation, // num as other num
    clippy::cast_possible_wrap, // num as other num
    clippy::cast_sign_loss, // signed to unsigned
    clippy::cast_precision_loss, // float precision loss
	clippy::too_long_first_doc_paragraph, // should be long ...
	clippy::wildcard_imports, // should only be used in testing
	clippy::format_push_string, // it's fine in this scale

	clippy::trivially_copy_pass_by_ref, // should be removed
	clippy::struct_excessive_bools, // should be removed
	clippy::significant_drop_tightening, // should be removed
	clippy::implicit_hasher, // we will use hashset, nothing else

	clippy::similar_names, // similar ...
	clippy::too_many_lines, // will correct it when i correct file lengths
)]
#![warn(
    missing_debug_implementations, // nice to debug all
	unused_qualifications, // it's useless
    rust_2018_idioms,    // Still useful for backward compatibility patterns
    rust_2021_compatibility, // Warns about things that changed in 2021
    rust_2024_compatibility,  // Warns about things that changed in 2024 (when stable)
)]

pub mod audio;
pub mod builtins;
pub mod description;
pub mod gate_op;
pub mod json;
pub mod logging;
pub mod pin_state;
pub mod render;
pub mod save_system;
pub mod settings;
pub mod sim;
pub mod ui_menu;
pub mod viewer;

pub use builtins::{create_all as create_all_builtins, register_all as register_all_builtins};
pub use description::{
	ChipDescription, ChipLibrary, ChipType, Color, DisplayDescription, NameLocation, PinAddress, PinBitCount, PinDescription, SubChipDescription,
	ValueDisplayMode, WireConnectionType, WireDescription,
};
pub use json::{
	ChipCollection, ProjectDescription, StarredItem, is_equivalent_json, load_chip_library_from_dir, load_project, parse_chip_description,
	parse_project_description, serialize_chip_description, serialize_project_description,
};
pub use save_system::{
	DLS_VERSION, DLS_VERSION_EARLIEST_COMPATIBLE, Loader, SavePaths, Saver, Version, can_open_project, create_or_load_project, create_project,
	default_chip_collections, default_starred_list,
};
pub use settings::{AppSettings, FullScreenMode};
pub use sim::{ChipIdx, ExternalInput, KeyCode, KeyboardSnapshot, PinIdx, SimChip, SimPin, Simulator, key_mods_bits};
pub use ui_menu::{MainMenu, MenuOutcome};
