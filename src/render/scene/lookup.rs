//! Pin-state lookups used while drawing a scene: the trait scene builders
//! query for "what colour is this pin/wire right now", plus the two
//! implementations the app actually uses -- a live [`Simulator`]-backed
//! one and the all-low static preview.

use crate::pin_state::LogicState;

/// Looks up whether a pin should be drawn "high" (lit) or "low". Callers
/// typically implement this against a live `Simulator` by resolving
/// `(pin_owner_id, pin_id)` through `Simulator::find_pin`; a `None` return
/// (e.g. pin not simulated yet) is treated as low/disconnected.
pub trait PinStateLookup {
	fn is_high(&self, pin_owner_id: i32, pin_id: i32) -> Option<bool>;

	/// Full tri-state logic level (low/high/disconnected) for this pin's
	/// first bit, used by the renderer to pick a colour (see
	/// `theme::state_colour`). Defaults to deriving `High`/`Low` from
	/// `is_high` alone, so a lookup that can't distinguish "genuinely
	/// disconnected" from "reads low" (like `AllLow`) never needs to
	/// override this. `SimulatorPinState` overrides it to report real
	/// disconnected pins as such rather than folding them into `Low`.
	fn logic_state(&self, pin_owner_id: i32, pin_id: i32) -> Option<LogicState> {
		self.is_high(pin_owner_id, pin_id).map(|high| if high { LogicState::High } else { LogicState::Low })
	}

	/// Same as `logic_state`, but for one specific bit of a multi-bit pin
	/// (`bit_index` counting from 0, the same convention
	/// `pin_state::get_bit_tristated_value` uses), so a wire carrying more
	/// than one bit can be drawn as that many individually-coloured
	/// strands (see `draw_wires`) instead of a single "averaged" colour.
	/// Defaults to `logic_state` regardless of `bit_index` -- correct for
	/// any lookup that can't distinguish bits from each other (`AllLow`,
	/// the fixed-state test doubles below), and overridden by
	/// `SimulatorPinState` to report each bit's own real state.
	fn bit_logic_state(&self, pin_owner_id: i32, pin_id: i32, _bit_index: u32) -> Option<LogicState> {
		self.logic_state(pin_owner_id, pin_id)
	}

	/// Raw `SimChip::internal_state` for the direct subchip identified by
	/// `owner_id` (a `PlacedSubChip::id`), if one is currently simulated.
	/// Used by the renderer to read the pixel/segment buffer behind a
	/// display chip (7-segment/RGB/dot) -- mirrors `DisplayInstance.SimChip`
	/// in the original, which caches the same lookup for drawing.
	/// Defaults to `None`, which callers treat as "draw the display blank"
	/// (matches `DrawDisplay`'s `sim == null` / `useSim == false` branches).
	fn internal_state(&self, _owner_id: i32) -> Option<&[u32]> {
		None
	}

	/// Descends into the simulation scope of the direct subchip identified
	/// by `owner_id`, so addresses that were resolved against the *parent*
	/// scope can keep resolving one level deeper (used when walking into a
	/// custom chip's own embedded displays -- see
	/// `render::scene::displays`). `None` means this lookup can't (or the
	/// simulator doesn't) model that sub-scope; callers fall back to
	/// drawing the nested content blank, mirroring the original's
	/// `sim == null` branches.
	fn enter_scope(&self, _owner_id: i32) -> Option<Box<dyn PinStateLookup + '_>> {
		None
	}
}

/// Trivial lookup that always reports every pin as low -- useful for static
/// previews / tests where no `Simulator` is available.
pub struct AllLow;
impl PinStateLookup for AllLow {
	fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
		Some(false)
	}
}

/// Live lookup backed by a running `Simulator`: resolves `(owner, pin)`
/// addresses the same way the sim graph does (`Simulator::find_pin`) and
/// reports the pin's per-bit state (`bit_logic_state`) as well as its
/// first bit's state alone (`logic_state`, used wherever only a single
/// representative colour is needed -- e.g. a pin's own drawn shape).
pub struct SimulatorPinState<'a> {
	pub sim: &'a crate::sim::Simulator,
	pub scope: crate::sim::ChipIdx,
}

impl<'a> PinStateLookup for SimulatorPinState<'a> {
	fn is_high(&self, pin_owner_id: i32, pin_id: i32) -> Option<bool> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		Some(crate::pin_state::first_bit_high(self.sim.pin(pin_idx).state))
	}

	fn logic_state(&self, pin_owner_id: i32, pin_id: i32) -> Option<LogicState> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		let raw = crate::pin_state::get_bit_tristated_value(self.sim.pin(pin_idx).state, 0);
		Some(LogicState::from_int(raw as u8))
	}

	fn bit_logic_state(&self, pin_owner_id: i32, pin_id: i32, bit_index: u32) -> Option<LogicState> {
		let addr = crate::description::PinAddress::new(pin_owner_id, pin_id);
		let pin_idx = self.sim.find_pin(self.scope, addr)?;
		let raw = crate::pin_state::get_bit_tristated_value(self.sim.pin(pin_idx).state, bit_index);
		Some(LogicState::from_int(raw as u8))
	}

	fn internal_state(&self, owner_id: i32) -> Option<&[u32]> {
		let chip_idx = self.sim.find_sub_chip(self.scope, owner_id)?;
		Some(&self.sim.chip(chip_idx).internal_state)
	}

	fn enter_scope(&self, owner_id: i32) -> Option<Box<dyn PinStateLookup + '_>> {
		let chip_idx = self.sim.find_sub_chip(self.scope, owner_id)?;
		Some(Box::new(SimulatorPinState { sim: self.sim, scope: chip_idx }))
	}
}

#[cfg(test)]
mod tests {}
