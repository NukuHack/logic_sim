//! Simulator side of sequential-chip memoisation ("memory discovery"); the data structures
//! live in [`crate::gate_op`]'s `memo` module -- read its docs first.
//!
//! Model. For a custom chip `C` whose insides are pure (NAND, tri-state buffer, splitters and
//! mergers, other custom chips built from those) and have no multi-driver pins (so no random
//! conflict resolution), one simulation frame is a deterministic function of
//!
//!  * `C`'s input pins, and
//!  * its *state*: the value held by every sub-chip input pin at any depth (these are the only
//!    pins whose previous-frame value can be read before being overwritten -- that is exactly
//!    what a feedback loop is).
//!
//! While a chip is on its learned table, `SimChip::memo_state` names its state and the pins
//! inside it are **stale**. They are brought back up to date ("materialised") only when
//! something needs the real thing: a table miss, demotion, the viewer looking inside the chip,
//! or a rebuild capturing pin states.

use super::{ChipIdx, FastPath, MEMO_UNINIT, PinState, Simulator};
use crate::description::{ChipType, PinBitCount};
use crate::gate_op::memo::{MAX_MEMO_PINS, MAX_SNAPSHOT_WORDS, MemoType};
use std::sync::Arc;

#[inline]
fn enc(s: PinState) -> u32 {
	let tag = match s.width() {
		PinBitCount::Bit1 => 0u32,
		PinBitCount::Bit4 => 1,
		PinBitCount::Bit8 => 2,
	};
	(tag << 16) | u32::from(s.raw())
}

#[inline]
const fn dec(w: u32) -> PinState {
	let width = match w >> 16 {
		0 => PinBitCount::Bit1,
		1 => PinBitCount::Bit4,
		_ => PinBitCount::Bit8,
	};
	PinState::from_raw_with_width((w & 0xFFFF) as u16, width)
}

/// Builtins whose output is a pure function of their input pins (no time, memory, I/O).
const fn is_pure_builtin(t: ChipType) -> bool {
	matches!(
		t,
		ChipType::Nand
			| ChipType::TriStateBuffer
			| ChipType::Split4To1Bit
			| ChipType::Merge1To4Bit
			| ChipType::Merge1To8Bit
			| ChipType::Merge4To8Bit
			| ChipType::Split8To4Bit
			| ChipType::Split8To1Bit
	)
}

impl Simulator {
	/// Turns sequential-chip learning on/off at runtime. Turning it off first writes every
	/// learned state back into real pins.
	pub fn set_use_state_memo(&mut self, on: bool) {
		if self.use_state_memo == on {
			return;
		}
		if !on {
			self.clear_memo();
		}
		self.use_state_memo = on;
		self.caching.memo.bump();
	}

	/// Drops everything learned (tables and per-chip states), keeping simulation results intact.
	pub fn clear_memo(&mut self) {
		self.sync_memo_to_pins();
		for c in &mut self.chips {
			c.memo_state = MEMO_UNINIT;
		}
		self.caching.memo.clear();
	}

	/// Writes every memoised chip's learned state into its real internal pins. Call before
	/// reading pins *inside* a chip (e.g. capturing pin states across a rebuild). Cheap when
	/// nothing is memoised.
	pub fn sync_memo_to_pins(&mut self) {
		for ci in 0..self.chips.len() {
			let sid = self.chips[ci].memo_state;
			if sid == MEMO_UNINIT {
				continue;
			}
			if let Some(slot) = self.caching.memo.slot_of(&self.chips[ci].name) {
				self.memo_write_pins(ChipIdx(ci), slot, sid);
			}
		}
	}

	/// Tells the simulator which chip the viewer is looking inside (`None` = just the root's
	/// outside). Chips on the path from the root down to it must run for real so the pins in
	/// the viewed chip stay live; everything else may keep using learned tables.
	pub fn set_observed_scope(&mut self, scope: Option<ChipIdx>) {
		if self.observed_scope == scope {
			return;
		}
		self.observed_scope = scope;
		if let Some(o) = scope {
			for ci in o.0..self.chips.len() {
				if self.chips[ci].sub_chip_lo as usize <= o.0 && self.chips[ci].memo_state != MEMO_UNINIT {
					self.memo_materialize(ChipIdx(ci));
				}
			}
		}
	}

	/// Resolves a chip already known to be non-combinational to a memo table (analysing it the
	/// first time its name is seen).
	pub(super) fn resolve_memo_path(&mut self, chip_idx: ChipIdx, name: &Arc<str>) -> FastPath {
		if !self.use_state_memo {
			return FastPath::None;
		}
		if let Some(slot) = self.caching.memo.slot_of(name) {
			return if self.caching.memo.get(slot).active { FastPath::Memo(slot) } else { FastPath::None };
		}
		let t = self.analyze_memo(chip_idx).unwrap_or_else(MemoType::rejected);
		let active = t.active;
		let slot = self.caching.memo.add(Arc::clone(name), t);
		if active {
			log::debug!("[memo] '{name}' will run on a learned transition table");
			FastPath::Memo(slot)
		} else {
			log::debug!("[memo] '{name}' is not memoisable");
			FastPath::None
		}
	}

	/// `Some(table)` if `chip_idx`'s insides satisfy the determinism conditions in the module doc.
	fn analyze_memo(&self, chip_idx: ChipIdx) -> Option<MemoType> {
		let c = &self.chips[chip_idx.0];
		if c.is_builtin || c.input_pins.len() > MAX_MEMO_PINS || c.output_pins.len() > MAX_MEMO_PINS {
			return None;
		}
		for ci in c.sub_chip_lo as usize..chip_idx.0 {
			let sub = &self.chips[ci];
			if sub.is_builtin && !is_pure_builtin(sub.chip_type) {
				return None;
			}
		}
		let (lo, hi) = (c.sub_pin_lo as usize, c.pin_hi as usize);
		let mut snap_rel = Vec::new();
		for pi in lo..hi {
			let p = &self.pins[pi];
			let own_input = p.is_input && p.parent_chip == chip_idx;
			if !own_input && p.num_input_connections > 1 {
				return None; // several drivers: resolved randomly, not a function of the state
			}
			if p.is_input && !own_input {
				snap_rel.push((pi - lo) as u32);
			}
		}
		if snap_rel.len() > MAX_SNAPSHOT_WORDS {
			return None;
		}
		Some(MemoType::new(c.input_pins.len(), c.output_pins.len(), snap_rel))
	}

	/// Step `chip_idx` from its learned table. `false` = caller must really simulate it.
	pub(super) fn process_memo_chip(&mut self, chip_idx: ChipIdx, slot: u32, audio: &mut crate::audio::SimAudio) -> bool {
		if let Some(o) = self.observed_scope
			&& self.chips[chip_idx.0].sub_chip_lo as usize <= o.0
			&& o.0 <= chip_idx.0
		{
			return false; // on the path to the viewed chip: must stay live
		}

		let mut key = [0u32; MAX_MEMO_PINS];
		let n_in = {
			let chip = &self.chips[chip_idx.0];
			for (i, &p) in chip.input_pins.iter().enumerate() {
				key[i] = enc(self.pins[p.0].state);
			}
			chip.input_pins.len()
		};

		let mut sid = self.chips[chip_idx.0].memo_state;
		if sid == MEMO_UNINIT {
			let snap = self.snapshot(chip_idx, slot);
			sid = self.caching.memo.get_mut(slot).intern(&snap);
			self.chips[chip_idx.0].memo_state = sid;
		}

		let mt = self.caching.memo.get_mut(slot);
		mt.lookups += 1;
		if let Some((next, outs)) = mt.find(sid, &key[..n_in]) {
			let chip = &mut self.chips[chip_idx.0];
			for (&p, &w) in chip.output_pins.iter().zip(outs) {
				self.pins[p.0].state = dec(w);
			}
			chip.memo_state = next;
			return true;
		}
		mt.misses += 1;
		self.memo_compute(chip_idx, slot, sid, &key[..n_in], audio);
		true
	}

	/// Table miss: bring the real pins to `sid`, simulate the frame for real, record what happened.
	#[cold]
	fn memo_compute(&mut self, chip_idx: ChipIdx, slot: u32, sid: u32, key: &[u32], audio: &mut crate::audio::SimAudio) {
		self.memo_restore(chip_idx, slot, sid);

		let saved_reorder = self.can_dynamic_reorder_this_frame;
		self.can_dynamic_reorder_this_frame = false; // random reordering would make the frame non-deterministic
		self.suppress_caches += 1; // everything underneath runs for real, so the snapshot is complete
		self.step_chip(chip_idx, audio);
		self.suppress_caches -= 1;
		self.can_dynamic_reorder_this_frame = saved_reorder;

		let snap = self.snapshot(chip_idx, slot);
		let mut outs = [0u32; MAX_MEMO_PINS];
		let chip = &self.chips[chip_idx.0];
		for (i, &p) in chip.output_pins.iter().enumerate() {
			outs[i] = enc(self.pins[p.0].state);
		}
		let n_out = chip.output_pins.len();

		let mt = self.caching.memo.get_mut(slot);
		let next = mt.intern(&snap);
		mt.insert(sid, key, next, &outs[..n_out]);
		let over = mt.over_budget();
		self.chips[chip_idx.0].memo_state = next;

		if over {
			self.demote_memo_type(chip_idx, slot);
		}
	}

	/// This type's state space / table outgrew its budget (a wide counter, a whole RAM...):
	/// write every instance's learned state back into real pins and stop learning it.
	#[cold]
	fn demote_memo_type(&mut self, chip_idx: ChipIdx, slot: u32) {
		let name = Arc::clone(&self.chips[chip_idx.0].name);
		log::debug!("[memo] '{name}' outgrew its table budget -- back to plain simulation");
		for ci in 0..self.chips.len() {
			let sid = self.chips[ci].memo_state;
			if sid != MEMO_UNINIT && *self.chips[ci].name == *name {
				self.memo_restore(ChipIdx(ci), slot, sid);
				self.chips[ci].memo_state = MEMO_UNINIT;
			}
		}
		self.caching.memo.get_mut(slot).free();
		self.caching.memo.bump();
	}

	/// Ensures `chip_idx`'s real pins are authoritative (no-op unless it's on a learned state).
	#[cold]
	pub(super) fn memo_materialize(&mut self, chip_idx: ChipIdx) {
		let sid = self.chips[chip_idx.0].memo_state;
		if sid == MEMO_UNINIT {
			return;
		}
		if let Some(slot) = self.caching.memo.slot_of(&self.chips[chip_idx.0].name) {
			self.memo_restore(chip_idx, slot, sid);
		}
		self.chips[chip_idx.0].memo_state = MEMO_UNINIT;
	}

	/// Current internal state of `chip_idx`, read from its real pins.
	fn snapshot(&self, chip_idx: ChipIdx, slot: u32) -> Vec<u32> {
		let lo = self.chips[chip_idx.0].sub_pin_lo as usize;
		self.caching.memo.get(slot).snap_rel.iter().map(|&r| enc(self.pins[lo + r as usize].state)).collect()
	}

	/// Writes state `sid` into the chip's real pins, keeping its learned-state id valid.
	fn memo_write_pins(&mut self, chip_idx: ChipIdx, slot: u32, sid: u32) {
		let lo = self.chips[chip_idx.0].sub_pin_lo as usize;
		let mt = self.caching.memo.get(slot);
		for (&r, &w) in mt.snap_rel.iter().zip(mt.state(sid)) {
			self.pins[lo + r as usize].state = dec(w);
		}
	}

	/// Like [`Self::memo_write_pins`], and also makes everything inside the chip trust its real
	/// pins again (their learned ids, if any, described states that no longer apply).
	fn memo_restore(&mut self, chip_idx: ChipIdx, slot: u32, sid: u32) {
		self.memo_write_pins(chip_idx, slot, sid);
		for ci in self.chips[chip_idx.0].sub_chip_lo as usize..chip_idx.0 {
			self.chips[ci].memo_state = MEMO_UNINIT;
		}
	}
}
