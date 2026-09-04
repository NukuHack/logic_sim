//! Covers the `recognize`-on-build step added to `gate_op::caching::recalculate_chip_cache`:
//! once a combinational chip's full truth table (`Lut`) is swept, the cache now tries to
//! recognize it as a known closed-form gate pattern (`Native`) before falling back to storing
//! the `Lut` itself. These tests exercise that decision through the real, public
//! `recalculate_chip_cache` entry point rather than poking at `recognize` directly (already
//! covered in isolation by `recognize_test.rs`), so a regression that breaks the *wiring*
//! between the sweep and `recognize` -- not just the pattern matching itself -- would be
//! caught here too.
//!
//! `Lut`/`Native` don't expose a `downcast`/`Any` for tests to branch on, so these tests read
//! the stored `Box<dyn CachedGate>`'s `Debug` output (both derive `Debug`, and it always starts
//! with the struct's name) to tell which representation actually got stored.

use logic_sim::description::{CacheKind, ChipDescription, ChipType, PinAddress, PinBitCount, PinDescription, SubChipDescription};
use logic_sim::gate_op::recalculate_chip_cache;
use logic_sim::{load_chip_library_from_dir, register_all_builtins, ChipLibrary, Simulator, Vec2};
use std::path::Path;

fn fixture_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Projects/MainTest")
}

/// Bare NAND subchip description, positioned at the origin -- every hand-built fixture chip
/// below just needs distinct `id`s, not real layout.
fn nand_sub(id: i32) -> SubChipDescription {
	SubChipDescription { name: "NAND".into(), id, internal_data: None, position: Vec2::ZERO, label: None, pin_colour_info: vec![] }
}

/// Looks up a cached gate by chip name and returns its `Debug` string, panicking if nothing
/// got cached at all (distinct from "cached, but as the other representation").
fn cached_debug(sim: &Simulator, name: &str) -> String {
	let gate = sim.caching.combinational_chip_cache.get(name).unwrap_or_else(|| panic!("'{name}' should have ended up in the cache"));
	format!("{gate:?}")
}

/// `OR`, loaded from the real project fixture (already exercised end-to-end by
/// `builtins_test.rs`'s `loaded_or_chip_simulates_correctly`), is built from two NANDs
/// (De Morgan's) -- functionally a plain 2-input OR. Opting it into caching should now produce
/// a `Native` (matching the `OR2`/`OR_N` candidate), not a stored 4-row `Lut`.
#[test]
fn or_chip_gets_recognized_as_native_instead_of_a_stored_lut() {
	let (mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).unwrap();
	assert!(errors.is_empty());
	register_all_builtins(&mut library);

	// Only chips explicitly opted into caching (`CacheKind::None`) are eligible --
	// `recalculate_chip_cache` skips anything still at the default `Off`.
	library.get_mut("OR").cache_kind = CacheKind::None;
	let or_desc = library.get("OR").clone();

	let mut sim = Simulator::build(&or_desc, &library);
	let root = sim.root();
	recalculate_chip_cache(&mut sim, root);

	let debug_str = cached_debug(&sim, "OR");
	assert!(debug_str.starts_with("Native"), "expected OR to be recognized as a Native gate, got: {debug_str}");
	assert!(!sim.caching.not_combinational_chip_cache.contains("OR"), "OR is combinational and should never land in the negative cache");
}

/// `XOR`, same fixture project, same NAND-built story but for the `XOR2`/`XOR_N` candidate
/// instead of `OR2`/`OR_N` -- a second, independently-wired real chip so a pattern-specific
/// regression (e.g. `XOR2`'s formula) wouldn't hide behind only ever testing `OR`.
#[test]
fn xor_chip_gets_recognized_as_native_instead_of_a_stored_lut() {
	let (mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).unwrap();
	assert!(errors.is_empty());
	register_all_builtins(&mut library);

	library.get_mut("XOR").cache_kind = CacheKind::None;
	let xor_desc = library.get("XOR").clone();

	let mut sim = Simulator::build(&xor_desc, &library);
	let root = sim.root();
	recalculate_chip_cache(&mut sim, root);

	let debug_str = cached_debug(&sim, "XOR");
	assert!(debug_str.starts_with("Native"), "expected XOR to be recognized as a Native gate, got: {debug_str}");
}

/// A hand-built chip with *two* output pins: `OUT1 = NOT(IN)`, `OUT2 = NOT(NOT(IN))` (i.e.
/// `IN` buffered), wired directly from builtin NANDs with both inputs of each NAND tied
/// together. A plain `Native`'s `eval` only ever writes a single output word (see its
/// `CachedGate` impl), so this can't be represented as one `Native` -- but each output pin
/// *individually* matches a known pattern (`NOT`, `BUFFER`), so `recalculate_chip_cache` should
/// recognize both and combine them into a `NativeMulti` instead of falling back to a stored
/// `Lut`.
#[test]
fn multi_output_chip_with_every_pin_recognized_becomes_native_multi() {
	let mut library = ChipLibrary::new();
	register_all_builtins(&mut library);

	let mut chip = ChipDescription::new("MULTI_OUT_TEST", ChipType::Custom);
	chip.cache_kind = CacheKind::None;

	chip.input_pins.push(PinDescription::new("IN", 500, PinBitCount::Bit1));
	chip.output_pins.push(PinDescription::new("OUT1", 600, PinBitCount::Bit1));
	chip.output_pins.push(PinDescription::new("OUT2", 601, PinBitCount::Bit1));

	// NAND #10: both inputs tied to IN -> acts as NOT(IN), feeding OUT1.
	chip.sub_chips.push(nand_sub(10));
	// NAND #20: both inputs tied to NAND #10's output -> NOT(NOT(IN)), feeding OUT2.
	chip.sub_chips.push(nand_sub(20));

	// Dev-pin wiring convention (matches the real `OR`/`XOR` fixtures above): a chip's own
	// input/output pin is addressed with itself as the owner and local pin id 0. NAND's two
	// inputs are local ids 0/1, its output is local id 2 (see `builtins_test.rs`'s
	// `nand_builtin_matches_original_pin_layout`).
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(500, 0), PinAddress::new(10, 0)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(500, 0), PinAddress::new(10, 1)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(10, 2), PinAddress::new(600, 0)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(10, 2), PinAddress::new(20, 0)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(10, 2), PinAddress::new(20, 1)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(20, 2), PinAddress::new(601, 0)));

	let mut sim = Simulator::build(&chip, &library);
	let root = sim.root();
	recalculate_chip_cache(&mut sim, root);

	let debug_str = cached_debug(&sim, "MULTI_OUT_TEST");
	assert!(
		debug_str.starts_with("NativeMulti"),
		"every output pin here (NOT, BUFFER) is individually recognizable, expected a NativeMulti, got: {debug_str}"
	);
}

/// A hand-built chip with two 2-bit-input, 1-bit-output pins: `OUT1 = AND(A, B)` (recognizable
/// as `AND2`), `OUT2 = A` (a plain projection of one input, ignoring the other -- not a pattern
/// in the recognizer's registry). Since `OUT2` can't be matched to any known gate, the whole
/// chip must fall back to a stored `Lut`, even though `OUT1` alone would be recognizable --
/// `NativeMulti` only ever gets built when *every* output pin matches (see
/// `multi_output_chip_with_every_pin_recognized_becomes_native_multi` for the all-match case).
#[test]
fn multi_output_chip_with_one_unrecognized_pin_falls_back_to_a_stored_lut() {
	let mut library = ChipLibrary::new();
	register_all_builtins(&mut library);

	let mut chip = ChipDescription::new("MULTI_OUT_PARTIAL_TEST", ChipType::Custom);
	chip.cache_kind = CacheKind::None;

	chip.input_pins.push(PinDescription::new("A", 500, PinBitCount::Bit1));
	chip.input_pins.push(PinDescription::new("B", 501, PinBitCount::Bit1));
	chip.output_pins.push(PinDescription::new("OUT1", 600, PinBitCount::Bit1));
	chip.output_pins.push(PinDescription::new("OUT2", 601, PinBitCount::Bit1));

	// AND(A, B) via De Morgan's: NAND(A, B) then NOT.
	chip.sub_chips.push(nand_sub(10)); // NAND(A, B)
	chip.sub_chips.push(nand_sub(20)); // NOT(NAND(A,B)) == AND(A,B): both inputs tied together

	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(500, 0), PinAddress::new(10, 0)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(501, 0), PinAddress::new(10, 1)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(10, 2), PinAddress::new(20, 0)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(10, 2), PinAddress::new(20, 1)));
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(20, 2), PinAddress::new(600, 0)));

	// OUT2 = A, wired straight through with no subchip at all -- a projection of one input
	// that ignores the other, which isn't any candidate in `recognize::registry`.
	chip.wires.push(logic_sim::description::WireDescription::new(PinAddress::new(500, 0), PinAddress::new(601, 0)));

	let mut sim = Simulator::build(&chip, &library);
	let root = sim.root();
	recalculate_chip_cache(&mut sim, root);

	let debug_str = cached_debug(&sim, "MULTI_OUT_PARTIAL_TEST");
	assert!(debug_str.starts_with("Lut"), "OUT2 is an unrecognizable projection, so the whole chip should fall back to a Lut, got: {debug_str}");
}

/// A chip whose `cache_kind` is left at the default `Off` (never opted into caching) should be
/// completely untouched: not in the positive cache as either representation, and not in the
/// negative cache either -- `recalculate_chip_cache` should just skip it outright rather than
/// paying for the sweep at all.
#[test]
fn chip_not_opted_into_caching_is_left_alone() {
	let (mut library, errors) = load_chip_library_from_dir(&fixture_dir().join("Chips")).unwrap();
	assert!(errors.is_empty());
	register_all_builtins(&mut library);

	// Deliberately not setting `cache_kind` -- stays at `ChipDescription::new`'s default `Off`.
	let not_desc = library.get("NOT").clone();
	assert!(not_desc.cache_kind.is_off());

	let mut sim = Simulator::build(&not_desc, &library);
	let root = sim.root();
	recalculate_chip_cache(&mut sim, root);

	assert!(sim.caching.combinational_chip_cache.get("NOT").is_none(), "a chip that never opted into caching shouldn't be cached at all");
	assert!(
		sim.caching.not_combinational_chip_cache.contains("NOT"),
		"recalculate_chip_cache should still record that it looked at NOT and declined"
	);
}
