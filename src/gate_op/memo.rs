//! "Memory discovery": learned transition tables for sequential chips.
//!
//! A combinational chip is a pure function of its inputs, so `caching` can tabulate it once. A
//! latch / flip-flop / register built from NANDs is not -- but it *is* a deterministic state
//! machine: given the chip's complete internal state and its input pins, one simulation frame
//! always produces the same next state and the same outputs. So instead of recognising "this
//! looks like a D flip-flop" (brittle, and only finds what we thought of), the simulator simply
//! *remembers what happened*: the first time a chip type sees `(state, inputs)` it really
//! simulates the frame and records `(next state, outputs)`; every later occurrence -- in this
//! instance or in any of the other instances of the same chip type, e.g. all 2048 bit cells of
//! a RAM -- is one hash probe instead of walking the gate graph.
//!
//! This module is the pure data structure (interned states + an exact-match transition
//! table). The pin I/O and the real-simulation fallback live in `sim::memo_impl`.
//!
//! Safety valves: a type whose state space or table keeps growing (a wide counter, a whole RAM
//! array) is *demoted* back to plain simulation; the caller materialises every instance's real
//! pins from its learned state first, so nothing is lost.

use std::collections::HashMap;
use std::sync::Arc;

/// Most input / output pins a memoised chip may have (keys live in stack arrays).
pub const MAX_MEMO_PINS: usize = 16;
/// Largest snapshot (state words) a memoised chip may have.
pub const MAX_SNAPSHOT_WORDS: usize = 4096;
/// A type is demoted once it has learned this many distinct states...
pub const MAX_STATES: usize = 4096;
/// ...or this many transitions...
pub const MAX_TRANSITIONS: usize = 1 << 16;
/// ...or this many words of stored snapshots.
pub const MAX_STATE_WORDS: usize = 1 << 22;
/// Demote when, after this many misses, more than 1 in 4 lookups still miss.
pub const MISS_GRACE: u64 = 256;

/// Counters for one memoised chip type (see [`MemoStore::stats`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoStats {
	pub name: String,
	pub active: bool,
	pub states: usize,
	pub transitions: usize,
	pub lookups: u64,
	pub misses: u64,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
	state: u32,
	next: u32,
	hash: u64,
}

#[inline]
fn mix(h: u64, w: u32) -> u64 {
	(h.rotate_left(5) ^ u64::from(w)).wrapping_mul(0x517c_c1b7_2722_0a95)
}

#[inline]
fn hash_key(state: u32, key: &[u32]) -> u64 {
	let mut h = mix(0, state);
	for &w in key {
		h = mix(h, w);
	}
	h ^ (h >> 29)
}

/// Everything learned about one chip type.
#[derive(Debug)]
pub struct MemoType {
	pub(crate) active: bool,
	pub(crate) n_in: usize,
	pub(crate) n_out: usize,
	/// Pins forming the chip's internal state, as offsets from its `sub_pin_lo`.
	pub(crate) snap_rel: Vec<u32>,

	states: Vec<u32>,
	state_map: HashMap<Box<[u32]>, u32>,

	entries: Vec<Entry>,
	keys: Vec<u32>,
	outs: Vec<u32>,
	slots: Vec<u32>,

	pub(crate) lookups: u64,
	pub(crate) misses: u64,
}

impl MemoType {
	pub(crate) fn new(n_in: usize, n_out: usize, snap_rel: Vec<u32>) -> Self {
		Self {
			active: true,
			n_in,
			n_out,
			snap_rel,
			states: Vec::new(),
			state_map: HashMap::new(),
			entries: Vec::new(),
			keys: Vec::new(),
			outs: Vec::new(),
			slots: vec![0; 64],
			lookups: 0,
			misses: 0,
		}
	}

	/// A permanently-declined type (impure contents, too big, ...).
	pub(crate) fn rejected() -> Self {
		let mut t = Self::new(0, 0, Vec::new());
		t.active = false;
		t.slots = Vec::new();
		t
	}

	pub(crate) const fn snap_len(&self) -> usize {
		self.snap_rel.len()
	}

	pub(crate) fn num_states(&self) -> usize {
		self.state_map.len()
	}

	pub(crate) const fn num_transitions(&self) -> usize {
		self.entries.len()
	}

	/// Interns a state snapshot, returning its stable id.
	pub(crate) fn intern(&mut self, snap: &[u32]) -> u32 {
		if let Some(&id) = self.state_map.get(snap) {
			return id;
		}
		let id = self.state_map.len() as u32;
		self.states.extend_from_slice(snap);
		self.state_map.insert(snap.into(), id);
		id
	}

	pub(crate) fn state(&self, id: u32) -> &[u32] {
		let n = self.snap_len();
		let base = id as usize * n;
		&self.states[base..base + n]
	}

	/// Looks up `(state, key)`: the next state id and the output words.
	#[inline]
	pub(crate) fn find(&self, state: u32, key: &[u32]) -> Option<(u32, &[u32])> {
		let hash = hash_key(state, key);
		let mask = self.slots.len() - 1;
		let mut i = hash as usize & mask;
		loop {
			let s = self.slots[i];
			if s == 0 {
				return None;
			}
			let idx = s as usize - 1;
			let e = self.entries[idx];
			if e.hash == hash && e.state == state && &self.keys[idx * self.n_in..(idx + 1) * self.n_in] == key {
				return Some((e.next, &self.outs[idx * self.n_out..(idx + 1) * self.n_out]));
			}
			i = (i + 1) & mask;
		}
	}

	pub(crate) fn insert(&mut self, state: u32, key: &[u32], next: u32, outs: &[u32]) {
		if (self.entries.len() + 1) * 2 > self.slots.len() {
			self.grow();
		}
		let hash = hash_key(state, key);
		self.entries.push(Entry { state, next, hash });
		self.keys.extend_from_slice(key);
		self.outs.extend_from_slice(outs);
		self.place(self.entries.len() - 1, hash);
	}

	fn place(&mut self, idx: usize, hash: u64) {
		let mask = self.slots.len() - 1;
		let mut i = hash as usize & mask;
		while self.slots[i] != 0 {
			i = (i + 1) & mask;
		}
		self.slots[i] = idx as u32 + 1;
	}

	fn grow(&mut self) {
		let new_len = (self.slots.len() * 2).max(64);
		self.slots = vec![0; new_len];
		for idx in 0..self.entries.len() {
			let h = self.entries[idx].hash;
			self.place(idx, h);
		}
	}

	/// Drops all learned data (keeps shape + counters' meaning); used on demotion.
	pub(crate) fn free(&mut self) {
		self.active = false;
		self.states = Vec::new();
		self.state_map = HashMap::new();
		self.entries = Vec::new();
		self.keys = Vec::new();
		self.outs = Vec::new();
		self.slots = Vec::new();
	}

	pub(crate) fn over_budget(&self) -> bool {
		self.num_states() > MAX_STATES
			|| self.num_transitions() > MAX_TRANSITIONS
			|| self.states.len() > MAX_STATE_WORDS
			|| (self.misses > MISS_GRACE && self.misses * 4 > self.lookups)
	}
}

/// All learned chip types, by chip name.
#[derive(Debug, Default)]
pub struct MemoStore {
	types: Vec<MemoType>,
	index: HashMap<Arc<str>, u32>,
	epoch: u64,
}

impl MemoStore {
	/// Bumps whenever a type is added, demoted or dropped.
	#[must_use]
	pub const fn epoch(&self) -> u64 {
		self.epoch
	}

	pub(crate) fn slot_of(&self, name: &str) -> Option<u32> {
		self.index.get(name).copied()
	}

	pub(crate) fn add(&mut self, name: Arc<str>, t: MemoType) -> u32 {
		self.epoch += 1;
		let slot = self.types.len() as u32;
		self.types.push(t);
		self.index.insert(name, slot);
		slot
	}

	pub(crate) fn get(&self, slot: u32) -> &MemoType {
		&self.types[slot as usize]
	}

	pub(crate) fn get_mut(&mut self, slot: u32) -> &mut MemoType {
		&mut self.types[slot as usize]
	}

	pub(crate) const fn bump(&mut self) {
		self.epoch += 1;
	}

	/// Forgets every learned type (e.g. after any chip definition changed). Chips resolved
	/// against the old tables re-resolve on their next step; callers must have synced learned
	/// states back into real pins first (`Simulator::sync_memo_to_pins`).
	pub fn clear(&mut self) {
		self.epoch += 1;
		self.types.clear();
		self.index.clear();
	}

	/// Per-type counters, for diagnostics and benchmarks.
	#[must_use]
	pub fn stats(&self) -> Vec<MemoStats> {
		let mut v: Vec<MemoStats> = self
			.index
			.iter()
			.map(|(name, &slot)| {
				let t = &self.types[slot as usize];
				MemoStats {
					name: name.to_string(),
					active: t.active,
					states: t.num_states(),
					transitions: t.num_transitions(),
					lookups: t.lookups,
					misses: t.misses,
				}
			})
			.collect();
		v.sort_by(|a, b| a.name.cmp(&b.name));
		v
	}
}
