//! JSON loading/saving that matches the on-disk save format used by the original Digital Logic Sim
//! (see DLS.Description.Serialization.Serializer and the `*.json` files under a project's `Chips/` folder).
//! This is a straight structural port: same field names (PascalCase, via serde rename), same
//! enum-as-integer encoding for ChipType, same nested shape for pins/subchips/wires. Position/Colour/Points
//! are kept as plain structs so a chip file can be re-saved without losing editor layout data.

use crate::description::{
	ChipDescription, ChipLibrary, ChipType, Color, DisplayDescription, NameLocation, PinAddress, PinBitCount, PinDescription, SubChipDescription,
	ValueDisplayMode, WireConnectionType, WireDescription,
};
use crate::structs::Vec2;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default)]
pub struct JsonColour {
	pub r: f32,
	pub g: f32,
	pub b: f32,
	pub a: f32,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct JsonPinAddress {
	#[serde(rename = "PinID")]
	pin_id: i32,
	#[serde(rename = "PinOwnerID")]
	pin_owner_id: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct JsonPinDescription {
	#[serde(rename = "Name")]
	name: String,
	#[serde(rename = "ID")]
	id: i32,
	#[serde(rename = "Position", default)]
	position: Vec2,
	#[serde(rename = "BitCount")]
	bit_count: PinBitCount,
	#[serde(rename = "Colour", default)]
	colour: Color,
	#[serde(rename = "ValueDisplayMode", default)]
	value_display_mode: ValueDisplayMode,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct JsonPinColourInfo {
	#[serde(rename = "PinColour")]
	pin_colour: Color,
	#[serde(rename = "PinID")]
	pin_id: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct JsonSubChipDescription {
	#[serde(rename = "Name")]
	name: String,
	#[serde(rename = "ID")]
	id: i32,
	#[serde(rename = "Label", default)]
	label: Option<String>,
	#[serde(rename = "Position", default)]
	position: Vec2,
	#[serde(rename = "OutputPinColourInfo", default)]
	pin_colour_info: Option<Vec<JsonPinColourInfo>>,
	#[serde(rename = "InternalData", default)]
	internal_data: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct JsonWireDescription {
	#[serde(rename = "SourcePinAddress")]
	source_pin_address: JsonPinAddress,
	#[serde(rename = "TargetPinAddress")]
	target_pin_address: JsonPinAddress,
	#[serde(rename = "ConnectionType", default)]
	connection_type: WireConnectionType,
	#[serde(rename = "ConnectedWireIndex", default)]
	connected_wire_index: i32,
	#[serde(rename = "ConnectedWireSegmentIndex", default)]
	connected_wire_segment_index: i32,
	#[serde(rename = "Points", default)]
	points: Vec<Vec2>,
}

macro_rules! impl_serde_via_int {
	($ty:ty) => {
		impl serde::Serialize for $ty {
			fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
			where
				S: serde::Serializer,
			{
				self.to_int().serialize(serializer)
			}
		}

		impl<'de> serde::Deserialize<'de> for $ty {
			fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
			where
				D: serde::Deserializer<'de>,
			{
				let v = i32::deserialize(deserializer)?;
				Ok(Self::from_int(v))
			}
		}
	};
}

// Usage – after your enum definition:
impl_serde_via_int!(ChipType);
impl_serde_via_int!(ValueDisplayMode);
impl_serde_via_int!(NameLocation);
impl_serde_via_int!(PinBitCount);
impl_serde_via_int!(Color);
impl_serde_via_int!(WireConnectionType);

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct JsonDisplayDescription {
	#[serde(rename = "SubChipID")]
	id: i32,
	#[serde(rename = "Position", default)]
	position: Vec2,
	#[serde(rename = "Scale", default)]
	scale: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct JsonChipDescription {
	#[serde(rename = "DLSVersion", default)]
	dls_version: Option<String>,
	#[serde(rename = "Name")]
	name: String,
	#[serde(rename = "NameLocation", default)]
	name_location: NameLocation,
	#[serde(rename = "ChipType")]
	chip_type: ChipType,
	#[serde(rename = "Size", default)]
	size: Vec2,
	#[serde(rename = "Colour", default)]
	colour: JsonColour,
	#[serde(rename = "InputPins", default)]
	input_pins: Vec<JsonPinDescription>,
	#[serde(rename = "OutputPins", default)]
	output_pins: Vec<JsonPinDescription>,
	#[serde(rename = "SubChips", default)]
	sub_chips: Vec<JsonSubChipDescription>,
	#[serde(rename = "Wires", default)]
	wires: Vec<JsonWireDescription>,
	#[serde(rename = "Displays", default)]
	displays: Option<Vec<JsonDisplayDescription>>,
}

/// Parse a single chip's JSON text (the contents of e.g. `Chips/NOT.json`)
/// into the simulation-ready ChipDescription.
pub fn parse_chip_description(json: &str) -> serde_json::Result<ChipDescription> {
	let raw: JsonChipDescription = serde_json::from_str(json)?;
	Ok(to_chip_description(&raw))
}

fn to_chip_description(raw: &JsonChipDescription) -> ChipDescription {
	let mut desc = ChipDescription::new(raw.name.clone(), raw.chip_type);

	desc.colour = [raw.colour.r, raw.colour.g, raw.colour.b, raw.colour.a];
	desc.name_location = raw.name_location;
	desc.size = raw.size;
	desc.dls_version = raw.dls_version.clone();

	desc.input_pins = raw
		.input_pins
		.iter()
		.map(|p| PinDescription::from_saved(p.name.clone(), p.id, p.position, p.bit_count, p.colour, p.value_display_mode))
		.collect();

	desc.output_pins = raw
		.output_pins
		.iter()
		.map(|p| PinDescription::from_saved(p.name.clone(), p.id, p.position, p.bit_count, p.colour, p.value_display_mode))
		.collect();

	desc.sub_chips = raw
		.sub_chips
		.iter()
		.map(|s| SubChipDescription {
			name: s.name.clone(),
			id: s.id,
			internal_data: s.internal_data.clone(),
			label: s.label.clone(),
			position: s.position,
			pin_colour_info: s.pin_colour_info.as_ref().map(|infos| infos.iter().map(|i| (i.pin_id, i.pin_colour)).collect()).unwrap_or_default(),
		})
		.collect();

	desc.wires = raw
		.wires
		.iter()
		.map(|w| {
			// `Points` is [source-endpoint, ...bends..., target-endpoint]. The first/last entries are
			// redundant for a plain pin-to-pin wire (re-resolved live), but for a wire tapping into
			// another wire's line they're the only record of the attachment point, so they're kept.
			let cached_source_point = w.points.first().copied().unwrap_or_default();
			let cached_target_point = w.points.last().copied().unwrap_or_default();
			let bends = if w.points.len() > 2 { w.points[1..w.points.len() - 1].to_vec() } else { Vec::new() };

			WireDescription {
				source_pin_address: PinAddress::new(w.source_pin_address.pin_owner_id, w.source_pin_address.pin_id),
				target_pin_address: PinAddress::new(w.target_pin_address.pin_owner_id, w.target_pin_address.pin_id),
				connection_type: w.connection_type,
				connected_wire_index: w.connected_wire_index,
				connected_wire_segment_index: w.connected_wire_segment_index,
				cached_source_point,
				cached_target_point,
				points: bends,
			}
		})
		.collect();

	desc.displays = raw.displays.iter().flatten().map(|d| DisplayDescription::new(d.id, d.position, d.scale)).collect();

	desc
}

/// Serialize back to the on-disk JSON shape. Editor-only fields not tracked
/// by `ChipDescription` (position, colour, wire points, ...) are written
/// with sensible defaults, so round-tripping through this loader will lose
/// layout info -- fine for the simulation core, not yet a full save-system
/// replacement for the editor.
///
/// Library-free: each placed subchip's `OutputPinColourInfo` is emitted as
/// its stored override list (empty array when none). Saving a *project*
/// chip should prefer [`serialize_chip_description_for_save`], which
/// resolves the Unity-exact shape against the chip library instead.
pub fn serialize_chip_description(desc: &ChipDescription) -> serde_json::Result<String> {
	serialize_chip_description_impl(desc, None)
}

/// Project-aware twin of [`serialize_chip_description`] (`DescriptionCreator.CreateSubChipDescription`):
/// every non-bus placed subchip saves one `OutputPinColourInfo` entry per
/// output pin -- its per-instance override where one exists, else the
/// library description's default colour -- and bus subchips save `null`
/// (their pin colours follow live wire state, so writing them would make
/// Unity flag phantom unsaved changes on open).
pub fn serialize_chip_description_for_save(desc: &ChipDescription, library: &ChipLibrary) -> serde_json::Result<String> {
	serialize_chip_description_impl(desc, Some(library))
}

fn serialize_chip_description_impl(desc: &ChipDescription, library: Option<&ChipLibrary>) -> serde_json::Result<String> {
	let raw = JsonChipDescription {
		// Every save is stamped with the running build's version, mirroring
		// `DescriptionCreator.CreateChipDescription`'s `DLSVersion =
		// Main.DLSVersion` -- the in-memory value may legitimately be older
		// (a just-migrated pre-2.1.5 chip), but what hits disk from here on
		// *is* current-format data. Writing anything older would make the
		// loader re-apply the one-shot migrations on every load.
		dls_version: Some(crate::DLS_VERSION.to_string()),
		name: desc.name.clone(),
		name_location: desc.name_location,
		chip_type: desc.chip_type,
		size: desc.size,
		colour: JsonColour { r: desc.colour[0], g: desc.colour[1], b: desc.colour[2], a: desc.colour[3] },
		input_pins: desc
			.input_pins
			.iter()
			.map(|p| JsonPinDescription {
				name: p.name.clone(),
				id: p.id,
				position: p.position,
				bit_count: p.bit_count,
				colour: p.colour,
				value_display_mode: p.value_display_mode,
			})
			.collect(),
		output_pins: desc
			.output_pins
			.iter()
			.map(|p| JsonPinDescription {
				name: p.name.clone(),
				id: p.id,
				position: p.position,
				bit_count: p.bit_count,
				colour: p.colour,
				value_display_mode: p.value_display_mode,
			})
			.collect(),
		sub_chips: desc
			.sub_chips
			.iter()
			.map(|s| JsonSubChipDescription {
				name: s.name.clone(),
				id: s.id,
				label: Some(s.label.clone().unwrap_or_default()),
				position: s.position,
				pin_colour_info: json_pin_colour_info(s, library),
				internal_data: s.internal_data.clone(),
			})
			.collect(),
		wires: desc
			.wires
			.iter()
			.map(|w| JsonWireDescription {
				source_pin_address: JsonPinAddress { pin_id: w.source_pin_address.pin_id, pin_owner_id: w.source_pin_address.pin_owner_id },
				target_pin_address: JsonPinAddress { pin_id: w.target_pin_address.pin_id, pin_owner_id: w.target_pin_address.pin_owner_id },
				connection_type: w.connection_type,
				connected_wire_index: w.connected_wire_index,
				connected_wire_segment_index: w.connected_wire_segment_index,
				// Re-wrap the interior bend points with the cached endpoint coordinates, mirroring the
				// on-disk [source, ...bends..., target] shape. For a `ToWireSource`/`ToWireTarget` wire
				// these cached points are load-bearing (see `WireConnectionType` docs), so they must round-trip.
				points: std::iter::once(w.cached_source_point)
					.chain(w.points.iter().copied())
					.chain(std::iter::once(w.cached_target_point))
					.collect(),
			})
			.collect(),
		displays: if desc.displays.is_empty() {
			None
		} else {
			Some(desc.displays.iter().map(|d| JsonDisplayDescription { id: d.sub_chip_id, position: d.position, scale: d.scale }).collect())
		},
	};

	// The original C# implementation pretty-prints; compact output is used here instead to keep file size down.

	serde_json::to_string(&raw)
}

/// The on-disk `OutputPinColourInfo` for placed subchip `s`:
///
/// - with no library context (or an unresolvable chip name): the stored
///   override list as-is;
/// - a bus subchip: `None` -- serialized as JSON `null`, exactly what
///   `DescriptionCreator.CreateSubChipDescription` writes (a bus' pin
///   colours follow live wire state, so persisting them would make Unity's
///   `UnsavedChangeDetector` flag phantom edits right after opening);
/// - anything else: one entry per output pin of the library description --
///   the subchip's own override where one exists, else that pin's default
///   colour.
fn json_pin_colour_info(s: &SubChipDescription, library: Option<&ChipLibrary>) -> Option<Vec<JsonPinColourInfo>> {
	let stored = || s.pin_colour_info.iter().map(|(pin_id, colour)| JsonPinColourInfo { pin_colour: *colour, pin_id: *pin_id }).collect::<Vec<_>>();
	let Some(library) = library else { return Some(stored()) };
	let Some(chip_desc) = library.try_get(&s.name) else { return Some(stored()) };
	if chip_desc.chip_type.is_bus_type() {
		return None;
	}
	Some(
		chip_desc
			.output_pins
			.iter()
			.map(|pin| {
				let colour = s.pin_colour_info.iter().find(|(id, _)| *id == pin.id).map(|(_, c)| *c).unwrap_or(pin.colour);
				JsonPinColourInfo { pin_colour: colour, pin_id: pin.id }
			})
			.collect(),
	)
}

/// Tolerance (absolute difference) within which two JSON numbers count as
/// equal -- mirrors `UnsavedChangeDetector`'s epsilon, so a float that
/// drifted slightly through a save/load round trip doesn't read as an edit.
const JSON_FLOAT_EPSILON: f64 = 0.0001;

/// Test if two json strings are equivalent. Unlike directly comparing the
/// strings, this ignores things like formatting and property order, and
/// compares numbers approximately (within [`JSON_FLOAT_EPSILON`]). Port of
/// `DLS.Description.UnsavedChangeDetector.IsEquivalentJson`, backing the
/// unsaved-changes prompt's "did this chip actually change" check.
pub fn is_equivalent_json(json_a: &str, json_b: &str) -> bool {
	match (serde_json::from_str::<serde_json::Value>(json_a), serde_json::from_str::<serde_json::Value>(json_b)) {
		(Ok(token_a), Ok(token_b)) => is_equivalent_token(&token_a, &token_b),
		_ => false,
	}
}

fn is_equivalent_token(a: &serde_json::Value, b: &serde_json::Value) -> bool {
	use serde_json::Value;
	match (a, b) {
		(Value::Object(obj_a), Value::Object(obj_b)) => {
			obj_a.len() == obj_b.len()
				&& obj_a.iter().all(|(key, value_a)| obj_b.get(key).is_some_and(|value_b| is_equivalent_token(value_a, value_b)))
		}
		(Value::Array(arr_a), Value::Array(arr_b)) => {
			arr_a.len() == arr_b.len() && arr_a.iter().zip(arr_b).all(|(value_a, value_b)| is_equivalent_token(value_a, value_b))
		}
		(Value::Number(num_a), Value::Number(num_b)) => match (num_a.as_f64(), num_b.as_f64()) {
			(Some(x), Some(y)) => (x - y).abs() < JSON_FLOAT_EPSILON,
			_ => num_a == num_b,
		},
		_ => a == b,
	}
}

/// Load every `*.json` chip file directly inside `chips_dir` into a
/// ChipLibrary. Mirrors DLS.SaveSystem.Loader's project-chip loading step
/// (minus builtin chips, which aren't stored as files -- see
/// `builtins::register_all` to add those to the library too).
pub fn load_chip_library_from_dir(chips_dir: &Path) -> std::io::Result<(ChipLibrary, Vec<String>)> {
	let mut library = ChipLibrary::new();
	let mut errors = Vec::new();

	// A project with no custom chips yet (e.g. one that was just created)
	// may not have a `Chips/` directory on disk at all -- that's not an
	// error, it just means there's nothing to load.
	let dir_iter = match fs::read_dir(chips_dir) {
		Ok(iter) => iter,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((library, errors)),
		Err(e) => return Err(e),
	};

	let mut entries: Vec<_> = dir_iter.filter_map(|e| e.ok()).filter(|e| e.path().extension().map(|ext| ext == "json").unwrap_or(false)).collect();
	entries.sort_by_key(|e| e.path());

	for entry in entries {
		let path = entry.path();
		match fs::read_to_string(&path) {
			Ok(text) => match parse_chip_description(&text) {
				Ok(desc) => library.add(desc),
				Err(e) => errors.push(format!("{}: parse error: {e}", path.display())),
			},
			Err(e) => errors.push(format!("{}: read error: {e}", path.display())),
		}
	}

	Ok((library, errors))
}

/// A starred chip/collection shortcut, shown in the editor's bottom bar.
/// Mirrors `DLS.Description.StarredItem` (the two `[JsonIgnore]` cached
/// display strings on the C# side aren't serialized there either, so
/// they're simply not represented here).
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct StarredItem {
	#[serde(rename = "Name", default)]
	pub name: String,
	#[serde(rename = "IsCollection", default)]
	pub is_collection: bool,
}

impl StarredItem {
	pub fn new(name: impl Into<String>, is_collection: bool) -> Self {
		Self { name: name.into(), is_collection }
	}
}

/// A named, collapsible group of chips in the chip palette. Mirrors
/// `DLS.Description.ChipCollection`.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ChipCollection {
	#[serde(rename = "Name", default)]
	pub name: String,
	#[serde(rename = "IsToggledOpen", default)]
	pub is_toggled_open: bool,
	#[serde(rename = "Chips", default)]
	pub chips: Vec<String>,
}

impl ChipCollection {
	pub fn new(name: impl Into<String>, chips: impl IntoIterator<Item = impl Into<String>>) -> Self {
		Self { name: name.into(), is_toggled_open: false, chips: chips.into_iter().map(Into::into).collect() }
	}
}

/// Full mirror of `DLS.Description.ProjectDescription` -- the per-project
/// metadata file saved at `<project>/ProjectDescription.json`. Field names
/// match the original exactly (via `serde(rename)`) so files written by
/// either the C# game or this port are interchangeable.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ProjectDescription {
	#[serde(rename = "ProjectName", default)]
	pub project_name: String,
	#[serde(rename = "DLSVersion_LastSaved", default)]
	pub dls_version_last_saved: String,
	#[serde(rename = "DLSVersion_EarliestCompatible", default)]
	pub dls_version_earliest_compatible: String,
	#[serde(rename = "CreationTime", default)]
	pub creation_time: String,
	#[serde(rename = "LastSaveTime", default)]
	pub last_save_time: String,

	// ---- Prefs ----
	#[serde(rename = "Prefs_MainPinNamesDisplayMode", default)]
	pub prefs_main_pin_names_display_mode: i32,
	#[serde(rename = "Prefs_ChipPinNamesDisplayMode", default)]
	pub prefs_chip_pin_names_display_mode: i32,
	#[serde(rename = "Prefs_GridDisplayMode", default)]
	pub prefs_grid_display_mode: i32,
	#[serde(rename = "Prefs_Snapping", default)]
	pub prefs_snapping: i32,
	#[serde(rename = "Prefs_StraightWires", default)]
	pub prefs_straight_wires: i32,
	/// Whether `CanCompleteWireConnection`'s bit-width check gates wire
	/// completion at all. Defaults to `true` (on), matching the
	/// previously-hardcoded always-on behaviour; exposed in the
	/// preferences UI so it can be turned off to allow connecting
	/// mismatched-width pins.
	#[serde(rename = "Prefs_CanCompleteWireConnection", default)]
	pub prefs_can_complete_wire_connection: i32,
	#[serde(rename = "Prefs_SimPaused", default)]
	pub prefs_sim_paused: bool,
	#[serde(rename = "Prefs_SimTargetStepsPerSecond", default)]
	pub prefs_sim_target_steps_per_second: i32,
	#[serde(rename = "Prefs_SimStepsPerClockTick", default)]
	pub prefs_sim_steps_per_clock_tick: i32,

	/// All player-created chips, in order of creation (oldest first).
	#[serde(rename = "AllCustomChipNames", default)]
	pub all_custom_chip_names: Vec<String>,

	#[serde(rename = "StarredList", default)]
	pub starred_list: Vec<StarredItem>,
	#[serde(rename = "ChipCollections", default)]
	pub chip_collections: Vec<ChipCollection>,
}

impl ProjectDescription {
	/// Mirrors `ProjectDescription.IsStarred`.
	pub fn is_starred(&self, chip_name: &str, is_collection: bool) -> bool {
		self.starred_list.iter().any(|item| item.is_collection == is_collection && item.name.eq_ignore_ascii_case(chip_name))
	}

	/// Mirrors the `StarredList`-editing half of `Project.SetStarred` (the
	/// rest of that method -- `chipLibrary`-driven "collapse the collection
	/// this chip's in" bookkeeping -- has no counterpart here since this
	/// port's library panel doesn't auto-collapse collections). Adding an
	/// already-starred name, or removing one that isn't starred, is a no-op
	/// either way.
	pub fn set_starred(&mut self, name: &str, starred: bool, is_collection: bool) {
		if !starred {
			self.starred_list.retain(|item| !(item.is_collection == is_collection && item.name.eq_ignore_ascii_case(name)));
		} else if !self.is_starred(name, is_collection) {
			self.starred_list.push(StarredItem::new(name, is_collection));
		}
	}
}

pub fn parse_project_description(json: &str) -> serde_json::Result<ProjectDescription> {
	serde_json::from_str(json)
}

pub fn serialize_project_description(desc: &ProjectDescription) -> serde_json::Result<String> {
	serde_json::to_string_pretty(desc)
}

/// Convenience: load `<project_dir>/ProjectDescription.json` plus every chip
/// under `<project_dir>/Chips/`.
pub fn load_project(project_dir: &Path) -> std::io::Result<(ProjectDescription, ChipLibrary, Vec<String>)> {
	let desc_path = project_dir.join("ProjectDescription.json");
	let project = match fs::read_to_string(&desc_path) {
		Ok(text) => parse_project_description(&text).unwrap_or_default(),
		Err(_) => ProjectDescription::default(),
	};

	let (library, errors) = load_chip_library_from_dir(&project_dir.join("Chips"))?;
	Ok((project, library, errors))
}
