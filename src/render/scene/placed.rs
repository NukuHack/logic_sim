//! Resolved world-space placement of a chip's subchip instances: the body
//! rectangles and pin rows every scene layer (wires, pins, components) and
//! the interaction hit-tests share.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::description::{ChipDescription, ChipLibrary, ChipType, Color, NameLocation, PinBitCount};
use crate::render::layout;
use crate::render::theme;
use crate::structs::Vec2;

/// Per-chip-TYPE layout (body size + pin y-offsets), the part of a
/// `PlacedSubChip` that only depends on the referenced chip's own
/// definition, never on where/how many times it's instantiated. Cached in
/// [`TYPE_LAYOUT_CACHE`] and shared across every instance of the same chip
/// type, keyed by chip name.
#[derive(Debug, Clone)]
struct TypeLayout {
	size: Vec2,
	input_pin_y: Vec<f32>,
	output_pin_y: Vec<f32>,
	/// Snapshot of the `ChipDescription` fields that feed into `size`/
	/// `*_pin_y` above, cheap to compare against the live chip on every
	/// call so a stale entry (the chip's own definition changed since it
	/// was cached -- pins added/removed, resized, renamed, etc.) is
	/// detected and recomputed instead of silently reused.
	fingerprint: Fingerprint,
}

#[derive(Debug, Clone, PartialEq)]
struct Fingerprint {
	chip_type: ChipType,
	name_location: NameLocation,
	name: String,
	desc_size: Vec2,
	input_bits: Vec<PinBitCount>,
	output_bits: Vec<PinBitCount>,
}

impl Fingerprint {
	fn of(desc: &ChipDescription) -> Self {
		Self {
			chip_type: desc.chip_type,
			name_location: desc.name_location,
			name: desc.name.clone(),
			desc_size: desc.size,
			input_bits: desc.input_pins.iter().map(|p| p.bit_count).collect(),
			output_bits: desc.output_pins.iter().map(|p| p.bit_count).collect(),
		}
	}
}

thread_local! {
	/// Cache of [`TypeLayout`] keyed by chip name (case-sensitive, matching
	/// `ChipDescription::name` as stored -- `ChipLibrary` itself keys
	/// case-insensitively, but names are treated as opaque here, just used
	/// to correlate cache entries with `Fingerprint` checks).
	static TYPE_LAYOUT_CACHE: RefCell<HashMap<String, TypeLayout>> = RefCell::new(HashMap::new());
}

/// Returns the cached body size + pin y-offsets for `desc`, recomputing
/// (and refreshing the cache entry) only if this is the first time `desc`'s
/// chip has been seen or its definition has changed since it was cached.
fn type_layout(desc: &ChipDescription) -> (Vec2, Vec<f32>, Vec<f32>) {
	TYPE_LAYOUT_CACHE.with(|cache| {
		let mut cache = cache.borrow_mut();

		// Cheap staleness check first (a handful of field/slice comparisons,
		// no allocation) so the common case -- this chip type was already
		// cached and hasn't changed -- never touches font metrics or
		// allocates `input_bits`/`output_bits` at all.
		let unchanged = cache.get(&desc.name).is_some_and(|cached| {
			cached.fingerprint.chip_type == desc.chip_type
				&& cached.fingerprint.name_location == desc.name_location
				&& cached.fingerprint.desc_size == desc.size
				&& cached.fingerprint.name == desc.name
				&& cached.fingerprint.input_bits.len() == desc.input_pins.len()
				&& cached.fingerprint.output_bits.len() == desc.output_pins.len()
				&& cached.fingerprint.input_bits.iter().zip(desc.input_pins.iter()).all(|(a, p)| *a == p.bit_count)
				&& cached.fingerprint.output_bits.iter().zip(desc.output_pins.iter()).all(|(a, p)| *a == p.bit_count)
		});
		if unchanged {
			let cached = &cache[&desc.name];
			return (cached.size, cached.input_pin_y.clone(), cached.output_pin_y.clone());
		}

		let fingerprint = Fingerprint::of(desc);

		// Prefer the size actually saved on disk (`ChipDescription::size`) -- computed by the
		// original via `CalculateMinChipSize` with real font metrics, more accurate than anything
		// derivable here. Fall back to the pins+name-estimate heuristic only when nothing is saved
		// (size == (0,0)).
		let size = if desc.size != Vec2::ZERO {
			Vec2::new(desc.size.x, desc.size.y)
		} else {
			layout::calculate_min_chip_size(&fingerprint.input_bits, &fingerprint.output_bits, desc, theme::FONT_SIZE_CHIP_NAME)
		};
		let (_, input_pin_y) = layout::calculate_default_pin_layout(&fingerprint.input_bits);
		let (_, output_pin_y) = layout::calculate_default_pin_layout(&fingerprint.output_bits);

		let entry = TypeLayout { size, input_pin_y, output_pin_y, fingerprint };
		let result = (entry.size, entry.input_pin_y.clone(), entry.output_pin_y.clone());
		cache.insert(desc.name.clone(), entry);
		result
	})
}

/// Resolved placement of one subchip instance within the scene, in world
/// space.
#[derive(Debug, Clone)]
pub struct PlacedSubChip<'a> {
	pub id: i32,
	pub desc: &'a ChipDescription,
	pub centre: Vec2,
	pub size: Vec2,
	pub input_pin_y: Vec<f32>,
	pub output_pin_y: Vec<f32>,
	/// Label, borrowed from this placed instance's
	/// `SubChipDescription::label`.
	pub label: &'a Option<String>,
	/// Per-instance output pin colour overrides, borrowed from this placed
	/// instance's `SubChipDescription::pin_colour_info`.
	pub pin_colour_info: &'a [(i32, Color)],
	/// Borrowed from this placed instance's
	/// `SubChipDescription::internal_data` (empty slice if the subchip has
	/// none).
	pub internal_data: &'a [u32],
}

impl<'a> PlacedSubChip<'a> {
	/// Effective palette index for this instance's output pin `pin_id`,
	/// falling back to `default_colour` (the chip-level pin colour) if this
	/// instance has no override for it.
	pub fn output_pin_colour(&self, pin_id: i32, default_colour: Color) -> Color {
		self.pin_colour_info.iter().find(|(id, _)| *id == pin_id).map(|(_, colour)| *colour).unwrap_or(default_colour)
	}
}

/// Computes the world-space placement (body rect + pin y-offsets) of every
/// subchip in `chip`, resolving each subchip's own pin layout against
/// `library`. Subchips referencing an unknown chip name are skipped.
pub fn place_sub_chips<'a>(chip: &'a ChipDescription, library: &'a ChipLibrary) -> Vec<PlacedSubChip<'a>> {
	let mut placed = Vec::with_capacity(chip.sub_chips.len());
	place_sub_chips_into(chip, library, &mut placed);
	placed
}

/// Same as [`place_sub_chips`], but appends into a caller-owned `out`
/// buffer instead of allocating a fresh `Vec` -- `out` is *not* cleared
/// first, so callers that want a full refresh (the usual per-frame case)
/// should `out.clear()` before calling. Reusing the same buffer frame over
/// frame means its backing allocation only grows, never gets freed and
/// reallocated, once it reaches the subchip count's steady state.
pub fn place_sub_chips_into<'a>(chip: &'a ChipDescription, library: &'a ChipLibrary, out: &mut Vec<PlacedSubChip<'a>>) {
	out.reserve(chip.sub_chips.len());

	for sub in &chip.sub_chips {
		let Some(desc) = library.try_get(&sub.name) else { continue };

		let (size, input_pin_y, output_pin_y) = type_layout(desc);

		out.push(PlacedSubChip {
			id: sub.id,
			desc,
			centre: sub.position,
			size,
			label: &sub.label,
			input_pin_y,
			output_pin_y,
			pin_colour_info: &sub.pin_colour_info,
			internal_data: sub.internal_data.as_deref().unwrap_or(&[]),
		});
	}
}

/// Drops every cached [`TypeLayout`] entry. Not required for correctness (`type_layout` self-
/// invalidates via `Fingerprint` comparison whenever a chip's definition actually changes),
/// but callers that swap out or reload a whole `ChipLibrary` wholesale (e.g. loading a
/// different project) can call this to release memory held for chip names that no longer
/// exist rather than letting them linger until each name happens to get reused.
pub fn clear_type_layout_cache() {
	TYPE_LAYOUT_CACHE.with(|cache| cache.borrow_mut().clear());
}

/// A [`place_sub_chips_into`] `out` buffer that can live inside a long-lived struct (e.g.
/// `ViewerState`), despite `PlacedSubChip` being generic over the borrowed lifetime of
/// whichever `chip`/`library` it was last filled from -- a lifetime that's different (and
/// unrelated) every single call, since a fresh `chip`/`library` borrow is taken each frame.
/// [`place_sub_chips_into`] itself can't be reused this way as a *stored* field for the same
/// reason a `Vec<PlacedSubChip<'a>>` can't be a struct field without infecting the whole
/// struct with `'a`: only [`Self::fill`] (called fresh each frame, with that frame's actual
/// `'a` in scope) can name the real lifetime. That's sound *only* because the vec is always
/// emptied at the start of every [`Self::fill`] call before anything is reinterpreted -- see
/// the safety comment there.
#[derive(Default)]
pub struct PlacedBuf(Vec<PlacedSubChip<'static>>);

impl PlacedBuf {
	pub fn new() -> Self {
		Self(Vec::new())
	}

	/// Refills this buffer from scratch for `chip`/`library` -- equivalent to calling
	/// [`place_sub_chips_into`] against a cleared `out` -- and returns it borrowed at
	/// `chip`/`library`'s lifetime.
	pub fn fill<'a>(&mut self, chip: &'a ChipDescription, library: &'a ChipLibrary) -> &mut Vec<PlacedSubChip<'a>> {
		self.0.clear();

		debug_assert!(self.0.is_empty());
		// SAFETY: `self.0` was just cleared, so at this point it holds zero
		// `PlacedSubChip<'static>` values to reinterpret -- only spare backing capacity, whose
		// byte layout (pointer/len/cap) doesn't depend on `PlacedSubChip`'s lifetime parameter at
		// all.
		let out: &mut Vec<PlacedSubChip<'a>> = unsafe { std::mem::transmute(&mut self.0) };
		place_sub_chips_into(chip, library, out);
		out
	}
}
