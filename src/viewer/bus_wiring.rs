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

/// Resolves a wire completion whose *two endpoints both land on bus-family
/// chips* (the caller has checked both owners are `is_bus_type`). Any bus
/// may connect to any other:
///
/// - the first-clicked half (`start_owner`) keeps its type;
/// - the second-clicked half (`end_owner`) converts to the complementary
///   type where needed (`BUS-n` <-> `BUS-TERMINUS-n`, same width), with its
///   flip flag inverted so the conversion -- which swaps which side a
///   visible pin is drawn on by default -- leaves the pin physically on
///   the same side it was when the player aimed at it;
/// - the two halves are then linked instantly: each writes the other's
///   instance id into its `internal_data[0]`. Any *previous* links are
///   replaced, and partners orphaned by the re-link have their pointer
///   cleared so deletion-cascades don't drag them along.
///
/// Returns the `(source, target)` pin addresses the finished wire must
/// use: the origin half's visible output pin and the terminus half's input
/// pin, whichever physical owner each role landed on.
pub fn resolve_bus_pair_completion(
	chip: &mut ChipDescription,
	library: &ChipLibrary,
	start_owner: i32,
	end_owner: i32,
) -> Result<(PinAddress, PinAddress), &'static str> {
	let start_type = owner_chip_type(chip, library, start_owner).ok_or("That component no longer exists")?;
	if !start_type.is_bus_type() || !owner_chip_type(chip, library, end_owner).is_some_and(|t| t.is_bus_type()) {
		return Err("Both wire ends must be bus chips");
	}

	// An origin start wants a terminus end and vice versa; the conversion
	// itself is skipped when the end chip already has the wanted type.
	let wanted_end_type =
		if start_type.is_bus_origin_type() { start_type.corresponding_bus_terminus() } else { start_type.corresponding_bus_origin() };
	if let Some(wanted) = wanted_end_type {
		convert_bus_component(chip, library, end_owner, wanted)?;
	}

	link_bus_pair(chip, library, start_owner, end_owner);

	let (origin_owner, terminus_owner) = if start_type.is_bus_origin_type() { (start_owner, end_owner) } else { (end_owner, start_owner) };
	Ok((visible_output_pin(chip, library, origin_owner)?, input_pin(chip, library, terminus_owner)?))
}

/// Renames bus component `owner_id` to whichever library chip implements
/// `to_type` and inverts its flip flag (`internal_data[1]`). Origin and
/// terminus descriptions draw their visible pin on opposite default sides,
/// so inverting the flip is what keeps the pin physically where it was
/// across the conversion. A no-op when the component already is `to_type`.
fn convert_bus_component(chip: &mut ChipDescription, library: &ChipLibrary, owner_id: i32, to_type: ChipType) -> Result<(), &'static str> {
	// Read everything the conversion needs up front so the shared
	// `library`/`chip` borrows end before the mutation below (`None` =
	// already the wanted type).
	let new_name = {
		let Some(sub) = chip.sub_chips.iter().find(|s| s.id == owner_id) else { return Err("That component no longer exists") };
		if library.try_get(&sub.name).map(|d| d.chip_type) == Some(to_type) {
			None
		} else {
			Some(library.iter().find(|d| d.chip_type == to_type).map(|d| d.name.clone()).ok_or("No matching bus chip exists")?)
		}
	};
	let Some(new_name) = new_name else { return Ok(()) };

	let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == owner_id) else { return Err("That component no longer exists") };
	sub.name = new_name;

	// Flip the "is flipped" bit: origin and terminus draw their
	// visible pin on opposite default sides, so inverting the flag is
	// what keeps it on the same physical side across the conversion.
	let mut data = sub.internal_data.clone().unwrap_or_default();
	data.resize(2, 0);
	data[1] ^= 1;
	sub.internal_data = Some(data);
	Ok(())
}

/// Writes `partner` into subchip `owner_id`'s `internal_data[0]`,
/// preserving whatever sits behind it (the flip flag at `[1]`). Partner id
/// `0` means "no partner" -- ids are always `> 0`, so no real link can
/// ever collide with it.
fn set_bus_link(chip: &mut ChipDescription, owner_id: i32, partner_id: i32) {
	if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == owner_id) {
		let mut data = sub.internal_data.clone().unwrap_or_default();
		data.resize(2, 0);
		data[0] = partner_id as u32;
		sub.internal_data = Some(data);
	}
}

/// Links bus components `a` and `b` into a mutual pair (see
/// [`resolve_bus_pair_completion`]), first clearing any *other* bus
/// component's link that still pointed at either half -- a re-link
/// orphans the previous partners, and a stale pointer would otherwise
/// make them cascade-delete together with their ex-partner.
fn link_bus_pair(chip: &mut ChipDescription, library: &ChipLibrary, a: i32, b: i32) {
	// Collect stale pointers first (shared borrows) so they're done before
	// the mutation pass.
	let stale: Vec<i32> = chip
		.sub_chips
		.iter()
		.filter(|s| s.id != a && s.id != b)
		.filter(|s| library.try_get(&s.name).is_some_and(|d| d.chip_type.is_bus_type()))
		.filter(|s| s.internal_data.as_ref().and_then(|d| d.first()).is_some_and(|&v| v as i32 == a || v as i32 == b))
		.map(|s| s.id)
		.collect();
	for orphan in stale {
		set_bus_link(chip, orphan, 0);
	}
	set_bus_link(chip, a, b);
	set_bus_link(chip, b, a);
}

/// The address of bus component `owner_id`'s visible output pin (origins'
/// pin 1) / input pin (both kinds' pin 0), resolved from the owning
/// description rather than assumed.
fn visible_output_pin(chip: &ChipDescription, library: &ChipLibrary, owner_id: i32) -> Result<PinAddress, &'static str> {
	pin_of(chip, library, owner_id, true)
}

fn input_pin(chip: &ChipDescription, library: &ChipLibrary, owner_id: i32) -> Result<PinAddress, &'static str> {
	pin_of(chip, library, owner_id, false)
}

fn pin_of(chip: &ChipDescription, library: &ChipLibrary, owner_id: i32, output: bool) -> Result<PinAddress, &'static str> {
	let sub = chip.sub_chips.iter().find(|s| s.id == owner_id).ok_or("That component no longer exists")?;
	let desc = library.try_get(&sub.name).ok_or("That bus chip no longer exists")?;
	let pin_id = if output { desc.output_pins.first() } else { desc.input_pins.first() }.map(|p| p.id).ok_or("That bus chip has no such pin")?;
	Ok(PinAddress::new(owner_id, pin_id))
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
