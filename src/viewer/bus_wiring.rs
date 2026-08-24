//! Bus-linking and wire-tap completion rules, ported from the wire-related
//! halves of `DLS.Game.ChipInteractionController` (its
//! `CanCompleteWireConnection` family) and `WireInstance`
//! (`IsBusWire` / `TargetPin_BusCorrected`). Pure chip-description queries:
//! plain data in, plain data out, so both the editor flow and the tests can
//! drive them without any viewer state.

use crate::description::{ChipDescription, ChipLibrary, ChipType, PinAddress, WireDescription};

/// The placed subchip type of owner `owner_id` within `chip`, or `None` for
/// boundary dev-pins (which share the id space but aren't subchips).
pub fn owner_chip_type(chip: &ChipDescription, library: &ChipLibrary, owner_id: i32) -> Option<ChipType> {
	let sub = chip.sub_chips.iter().find(|s| s.id == owner_id)?;
	library.try_get(&sub.name).map(|desc| desc.chip_type)
}

/// Whether `wire` is a *bus wire*: a wire whose two endpoints are a linked
/// bus origin's output and its terminus' input. Only bus wires may receive
/// connections from output pins (`CanCompleteWireConnection`'s exception),
/// because their merged signal has one well-defined source net.
pub fn is_bus_wire(chip: &ChipDescription, library: &ChipLibrary, wire: &WireDescription) -> bool {
	let source_type = owner_chip_type(chip, library, wire.source_pin_address.pin_owner_id);
	let target_type = owner_chip_type(chip, library, wire.target_pin_address.pin_owner_id);
	source_type.is_some_and(|t| t.is_bus_origin_type()) && target_type.is_some_and(|t| t.is_bus_terminus_type())
}

/// The electrical target of a connection landing on `wire` -- mirrors
/// `WireInstance.TargetPin_BusCorrected`: on a bus wire this is the *bus
/// origin's input pin* (so everything wired into the bus merges into the
/// origin's source net), everywhere else it's simply the wire's own target.
pub fn bus_corrected_target(chip: &ChipDescription, library: &ChipLibrary, wire: &WireDescription) -> PinAddress {
	if !is_bus_wire(chip, library, wire) {
		return wire.target_pin_address;
	}

	// A bus wire always runs origin-output -> terminus-input, so the origin
	// owns the source end; feed its (hidden) input pin.
	let owner_id = wire.source_pin_address.pin_owner_id;
	if let Some(sub) = chip.sub_chips.iter().find(|s| s.id == owner_id) {
		if let Some(desc) = library.try_get(&sub.name) {
			if let Some(input_pin) = desc.input_pins.first() {
				return PinAddress::new(owner_id, input_pin.id);
			}
		}
	}
	wire.target_pin_address
}

/// Whether the bus chips owned by `owner_a`/`owner_b` are linked into a
/// pair: each stores the other's instance id in `internal_data[0]` (both
/// written together when the pair is placed, and both required here --
/// stricter than the original's single-direction check, and satisfied by
/// everything the original writes). Non-bus owners are never linked.
pub fn bus_pair_linked(chip: &ChipDescription, library: &ChipLibrary, owner_a: i32, owner_b: i32) -> bool {
	let links_to = |a: i32, b: i32| {
		chip.sub_chips
			.iter()
			.find(|s| s.id == a)
			.filter(|s| library.try_get(&s.name).is_some_and(|d| d.chip_type.is_bus_type()))
			.is_some_and(|s| s.internal_data.as_ref().and_then(|d| d.first()).is_some_and(|&v| v as i32 == b))
	};
	links_to(owner_a, owner_b) && links_to(owner_b, owner_a)
}

/// Whether the partner of bus component `owner_id` (the subchip whose id
/// sits in its `internal_data[0]`) exists and is itself a bus chip --
/// used to keep pairs together when one side is deleted or moved.
pub fn bus_partner_id(chip: &ChipDescription, library: &ChipLibrary, owner_id: i32) -> Option<i32> {
	let sub = chip.sub_chips.iter().find(|s| s.id == owner_id)?;
	if !library.try_get(&sub.name).is_some_and(|d| d.chip_type.is_bus_type()) {
		return None;
	}
	let partner_id = sub.internal_data.as_ref().and_then(|d| d.first()).map(|&v| v as i32)?;
	chip.sub_chips
		.iter()
		.find(|s| s.id == partner_id)
		.filter(|s| library.try_get(&s.name).is_some_and(|d| d.chip_type.is_bus_type()))
		.map(|_| partner_id)
}

/// Outcome of finishing a pending wire on existing wire `wire` -- mirrors
/// `CanCompleteWireConnection(wireToConnectTo, out endPin)` plus the
/// restrictions around it:
///
/// - wire-to-wire completions are rejected (ambiguous signal source),
/// - an *output*-pin end may only land on a bus wire (two outputs driving
///   one normal wire would disagree on its state),
/// - an *input*-pin end may land on any wire,
/// - electrically the new wire connects to the tapped wire's resolved pins
///   (bus-corrected on the target side).
pub fn resolve_completion_on_wire(
	chip: &ChipDescription,
	library: &ChipLibrary,
	wire_index: usize,
	started_from_wire: bool,
	start_is_source: bool,
	start_owner_id: i32,
	start_pin_id: i32,
) -> Result<(PinAddress, PinAddress), &'static str> {
	if started_from_wire {
		return Err("Can't connect a wire to another wire");
	}

	let wire = chip.wires.get(wire_index).ok_or("That wire no longer exists")?;

	if start_is_source {
		// Completing an output-pin-started wire onto `wire`: only bus wires
		// may take extra driven inputs, which merge into the origin's input.
		if !is_bus_wire(chip, library, wire) {
			return Err("Only inputs can be wired into a normal wire (outputs need a bus)");
		}
		Ok((start_pin_address(start_owner_id, start_pin_id), bus_corrected_target(chip, library, wire)))
	} else {
		// Completing an input-pin-started wire onto `wire`: it inherits the
		// tapped wire's source as its own signal source.
		Ok((wire.source_pin_address, start_pin_address(start_owner_id, start_pin_id)))
	}
}

fn start_pin_address(owner_id: i32, pin_id: i32) -> PinAddress {
	PinAddress::new(owner_id, pin_id)
}
