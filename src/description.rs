//! Static description of a chip (as loaded from a saved project), used to
//! build the runtime simulation graph. Mirrors DLS.Description in the
//! original C# codebase.
use crate::render::theme::{COLORS, Rgba};
use glam::Vec2;
use logic_sim_macros::ConstFromPrimitive;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, ConstFromPrimitive)]
#[repr(i32)]
pub enum ChipType {
	// ---- Basic Chips ----
	#[default]
	Custom = 0,
	Nand = 1,
	TriStateBuffer = 2,
	Clock = 3,
	Pulse = 4,

	// ---- Memory ----
	DevRam8Bit = 5,
	Rom256x16 = 6,

	// ---- Displays ----
	SevenSegmentDisplay = 7,
	DisplayRgb = 8,
	DisplayDot = 9,
	DisplayLed = 10,

	// ---- Merge / Split ----
	Merge1To4Bit = 11,
	Merge1To8Bit = 12,
	Merge4To8Bit = 13,
	Split4To1Bit = 14,
	Split8To4Bit = 15,
	Split8To1Bit = 16,

	// ---- In / Out Pins ----
	In1Bit = 17,
	In4Bit = 18,
	In8Bit = 19,
	Out1Bit = 20,
	Out4Bit = 21,
	Out8Bit = 22,

	Key = 23,

	// ---- Buses ----
	Bus1Bit = 24,
	BusTerminus1Bit = 25,
	Bus4Bit = 26,
	BusTerminus4Bit = 27,
	Bus8Bit = 28,
	BusTerminus8Bit = 29,

	// ---- Audio ----
	Buzzer = 30,

	/// Outputs the host's current keyboard modifier state (shift/ctrl/alt/super)
	/// as a bitmask -- see `sim::key_mods_bits` for the bit layout. Not part of
	/// the original DLS chip set, so this discriminant has no C# counterpart.
	KeyMods = 31,
}

impl ChipType {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}

	pub const fn is_bus_origin_type(self) -> bool {
		matches!(self, Self::Bus1Bit | Self::Bus4Bit | Self::Bus8Bit)
	}

	pub const fn is_bus_terminus_type(self) -> bool {
		matches!(self, Self::BusTerminus1Bit | Self::BusTerminus4Bit | Self::BusTerminus8Bit)
	}

	pub const fn is_bus_type(self) -> bool {
		self.is_bus_origin_type() || self.is_bus_terminus_type()
	}

	pub const fn is_merge_type(self) -> bool {
		matches!(self, Self::Merge1To4Bit | Self::Merge1To8Bit | Self::Merge4To8Bit | Self::Split4To1Bit | Self::Split8To4Bit | Self::Split8To1Bit)
	}

	pub const fn is_io_type(self) -> bool {
		matches!(self, Self::In1Bit | Self::In4Bit | Self::In8Bit | Self::Out1Bit | Self::Out4Bit | Self::Out8Bit)
	}

	/// Dev-facing builtins (`dev.RAM-8` and the BUS-TERMINUS trio) that release builds keep out
	/// of every player-facing list -- palette defaults, collection syncing/rows, the bottom bar,
	/// and search (see `viewer::library::is_listed_in_current_build`).
	pub const fn is_dev_only(self) -> bool {
		matches!(self, Self::DevRam8Bit | Self::BusTerminus1Bit | Self::BusTerminus4Bit | Self::BusTerminus8Bit)
	}

	/// The bus-terminus chip type that pairs with this bus *origin* type --
	/// `ChipTypeHelper.GetCorrespondingBusTerminusType`. `None` for anything
	/// that isn't a bus origin (terminus types have no further pair of their
	/// own).
	pub const fn corresponding_bus_terminus(self) -> Option<Self> {
		match self {
			Self::Bus1Bit => Some(Self::BusTerminus1Bit),
			Self::Bus4Bit => Some(Self::BusTerminus4Bit),
			Self::Bus8Bit => Some(Self::BusTerminus8Bit),
			_ => None,
		}
	}

	/// The inverse lookup: the bus-*origin* chip type paired with this
	/// terminus type. `None` for anything that isn't a bus terminus
	/// (origins pair the other way via
	/// [`ChipType::corresponding_bus_terminus`]).
	pub const fn corresponding_bus_origin(self) -> Option<Self> {
		match self {
			Self::BusTerminus1Bit => Some(Self::Bus1Bit),
			Self::BusTerminus4Bit => Some(Self::Bus4Bit),
			Self::BusTerminus8Bit => Some(Self::Bus8Bit),
			_ => None,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, ConstFromPrimitive)]
#[repr(i32)]
pub enum ValueDisplayMode {
	#[default]
	None = 0,
	Decimal = 1,
	SignedDecimal = 2,
	Hex = 3,
}

impl ValueDisplayMode {
	/// Every display mode, in the order the pin-edit popup's "Decimal
	/// Display" option buttons list them (index == discriminant).
	pub const ALL: [Self; 4] = [Self::None, Self::Decimal, Self::SignedDecimal, Self::Hex];

	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}

	/// Label shown on the pin-edit popup's "Decimal Display" option button
	/// for this mode (mirrors `PinEditMenu.PinDecimalDisplayOptions`).
	pub const fn label(&self) -> &'static str {
		match self {
			Self::None => "Off",
			Self::Decimal => "Unsigned",
			Self::SignedDecimal => "Signed",
			Self::Hex => "HEX",
		}
	}
}

/// Where (if anywhere) a chip's name label is drawn on its body. Mirrors
/// `DLS.Description.NameDisplayLocation`, saved on disk as `NameLocation`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, ConstFromPrimitive)]
#[repr(i32)]
pub enum NameLocation {
	#[default]
	Centre = 0,
	Top = 1,
	Hidden = 2,
}

impl NameLocation {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}
}

/// Simplified, informational summary of what caching a chip ends up with once Save resolves
/// `ChipDescription::should_be_cached` (`viewer::save_flow::resolve_should_cache`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, ConstFromPrimitive)]
#[repr(i32)]
pub enum CacheKind {
	/// Not cached: either the chip isn't combinational, or
	/// `should_be_cached` came out false.
	#[default]
	Off = 0,
	None = 1,

	// native can easily have methods what would collapse the input disabled or off to a same internal value
	// so no need for different kinds, LUT only needs it because memory size
	Lut = 2,
	LutTristateOutput = 3,
	LutTristateInput = 4,
	LutTristateBoth = 5,
	Native = 6,
	NativeList = 7,
}

impl CacheKind {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}

	pub const fn is_none(&self) -> bool {
		matches!(self, Self::None)
	}

	pub const fn is_off(&self) -> bool {
		matches!(self, Self::Off)
	}

	pub const fn toggle(&mut self) {
		*self = match self {
			Self::Off => Self::None,
			_ => Self::Off,
		};
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinAddress {
	/// ID for this pin (unique within its owner, but not globally unique)
	pub pin_id: i32,
	/// ID of the dev-pin or subchip to which this pin belongs (unique within its parent)
	pub pin_owner_id: i32,
}

impl PinAddress {
	pub const fn new(pin_owner_id: i32, pin_id: i32) -> Self {
		Self { pin_id, pin_owner_id }
	}
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ConstFromPrimitive)]
#[repr(i32)]
pub enum PinBitCount {
	#[default]
	Bit1 = 1,
	Bit4 = 4,
	Bit8 = 8,
}

/// World-space height of a 1-bit pin's connection stub (its drawn circle
/// is twice this as a radius). Base of the per-bit-width pin sizing below.
pub const PIN_HEIGHT_1BIT: f32 = 0.185;
/// World-space height reserved by a 4-bit pin's connection stub
pub const PIN_HEIGHT_4BIT: f32 = PIN_HEIGHT_1BIT * 2.0;
/// World-space height reserved by an 8-bit pin's connection stub
pub const PIN_HEIGHT_8BIT: f32 = PIN_HEIGHT_1BIT * 3.0;
/// World-space radius to draw a 1-bit pin's connection circle at
pub const PIN_RADIUS: f32 = PIN_HEIGHT_1BIT / 2.0;

impl PinBitCount {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}

	/// Height (in world units) of this pin's connection stub. Mirrors
	/// `SubChipHelper.PinHeightFromBitCount`.
	pub const fn pin_height(self) -> f32 {
		match self {
			Self::Bit1 => PIN_HEIGHT_1BIT,
			Self::Bit4 => PIN_HEIGHT_4BIT,
			Self::Bit8 => PIN_HEIGHT_8BIT,
		}
	}

	/// World-space radius to draw this pin's connection circle at
	pub const fn pin_radius(self) -> f32 {
		match self {
			Self::Bit1 => PIN_RADIUS,
			Self::Bit4 => PIN_RADIUS * 1.7,
			Self::Bit8 => PIN_RADIUS * 2.5,
		}
	}

	/// World-space bounding size of this pin's drawn connection shape:
	/// Feed the result straight into `SceneGeometry::add_rounded_rect` with
	/// `radius = size.y / 2.0` and both `round_left`/`round_right = true` to
	/// get the actual pill shape (its rounded corners become true semicircle
	/// caps exactly when the radius equals half the height).
	pub const fn pin_visual_shape_size(self) -> Vec2 {
		let r = self.pin_radius();
		let body_width = match self {
			Self::Bit1 => 0.0, // unused -- Bit1 draws a plain circle, not a pill.
			Self::Bit4 => r * 0.6,
			Self::Bit8 => r,
		};
		Vec2::new(r, body_width + r)
	}

	/// Grid-height (in grid units) reserved for one pin along a chip's
	/// edge. Mirrors the inline switch inside
	/// `SubChipHelper.CalculateDefaultPinLayout`.
	pub(crate) const fn pin_grid_height(self) -> i32 {
		match self {
			Self::Bit1 => 2,
			Self::Bit4 => 3,
			Self::Bit8 => 4,
		}
	}

	/// Grid arrangement (columns, rows) of per-bit clickable cells for an
	/// *input* dev-pin's body,
	/// a single 1-bit input is one circle (no grid, 1x1); 4 bits arrange
	/// as a 2x2 grid; 8 bits as 2x4 (same 2-wide column count, twice as
	/// tall). Mirrors the `1 = 1, 4 = 2x2, 8 = 2x4` layout.
	pub const fn input_bit_grid_dims(self) -> (i32, i32) {
		match self {
			Self::Bit1 => (1, 1),
			Self::Bit4 => (2, 2),
			Self::Bit8 => (2, 4),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, ConstFromPrimitive)]
#[repr(i32)]
pub enum Color {
	#[default]
	Red = 0,
	Orange = 1,
	Yellow = 2,
	Green = 3,
	Blue = 4,
	Purple = 5,
	Pink = 6,
	White = 7,
}

impl Color {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}

	pub fn to_rgba(&self) -> Rgba {
		let idx = (self.to_int() as usize).min(COLORS.len() - 1);
		COLORS[idx]
	}
}

#[derive(Debug, Clone)]
pub struct PinDescription {
	pub name: String,
	pub id: i32,
	/// World-space position of this pin, in grid units, as saved on disk under this pin's
	/// `Position` field. Defaults to the origin for pins built up in code rather than loaded
	/// from disk.
	pub position: Vec2,
	pub bit_count: PinBitCount,
	/// Palette index (0..=7) into the state colour tables
	/// saved on disk under this pin's `Colour` field. Defaults to 0.
	pub colour: Color,
	/// How this pin's current value should be displayed alongside it (as
	/// saved on disk under this pin's `ValueDisplayMode` field). Defaults
	/// to `None` (no value shown).
	pub value_display_mode: ValueDisplayMode,
}

impl PinDescription {
	pub fn new(name: impl Into<String>, id: i32, bit_count: PinBitCount) -> Self {
		Self { name: name.into(), id, position: Vec2::default(), bit_count, colour: Color::default(), value_display_mode: ValueDisplayMode::None }
	}

	pub fn with_colour(name: impl Into<String>, id: i32, bit_count: PinBitCount, colour: Color) -> Self {
		Self { name: name.into(), id, position: Vec2::ZERO, bit_count, colour, value_display_mode: ValueDisplayMode::None }
	}

	/// Full constructor mirroring every on-disk field, used when parsing a
	/// saved chip (`json::to_chip_description`) so a dev-pin's saved
	/// position and value-display-mode aren't dropped on load.
	pub fn from_saved(
		name: impl Into<String>,
		id: i32,
		position: Vec2,
		bit_count: PinBitCount,
		colour: Color,
		value_display_mode: ValueDisplayMode,
	) -> Self {
		Self { name: name.into(), id, position, bit_count, colour, value_display_mode }
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubChipDescription {
	pub name: String,
	/// Unique within parent chip. ID > 0
	pub id: i32,
	/// Arbitrary data for specific chip types (ROM contents, bus link id, key binding, etc).
	/// None if the subchip has no persistent internal data.
	pub internal_data: Option<Vec<u32>>,
	/// World-space centre position of this subchip within its parent, in
	/// grid units (see `layout::GRID_SIZE`). Editor/layout concern only --
	/// unused by the simulation core, but needed by the renderer.
	pub position: Vec2,
	/// Label
	pub label: Option<String>,
	/// Per-instance overrides of an output pin's state-colour palette index
	/// (see `PinDescription::colour`), keyed by pin id. Lets two placed
	/// instances of the same chip type show differently-coloured output
	/// pins/wires, mirroring the saved `OutputPinColourInfo` list.
	pub pin_colour_info: Vec<(i32, Color)>,
}

impl SubChipDescription {
	/// Effective palette index for this instance's output pin `pin_id`,
	/// falling back to `default_colour` (the chip-level pin colour) if this
	/// instance has no override for it.
	pub fn output_pin_colour(&self, pin_id: i32, default_colour: Color) -> Color {
		self.pin_colour_info.iter().find(|(id, _)| *id == pin_id).map_or(default_colour, |(_, colour)| *colour)
	}
}

/// One display surface embedded in a chip's body ("customize" feature):
/// a live view of one of the chip's own subchips that is itself a
/// display-type builtin (`SevenSegmentDisplay` / `DisplayRgb` /
/// `DisplayDot` / `DisplayLed`). Mirrors `DLS.Description.DisplayDescription`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayDescription {
	/// Id of the subchip (within the owning chip's own `sub_chips`) whose
	/// content this display shows.
	pub sub_chip_id: i32,
	/// Display centre relative to the parent chip body's centre, in world
	/// units (scaled by the parent's own scale when nested -- see
	/// `render::scene::displays`).
	pub position: Vec2,
	/// World size multiplier applied to the displayed content's natural
	/// footprint (see `display_base_size`).
	pub scale: f32,
}

impl DisplayDescription {
	pub const fn new(sub_chip_id: i32, position: Vec2, scale: f32) -> Self {
		Self { sub_chip_id, position, scale }
	}
}

/// How a wire's source/target end attaches to the rest of the scene. Most wires attach both
/// ends directly to a pin (`ToPins`), but a wire can instead be "tapped" off of another
/// wire's line -- its saved `SourcePinAddress`/ `TargetPinAddress` still names the real
/// originating pin (needed for colour/bit-count/simulation-state lookups), but that end's
/// *position* must be resolved along the referenced wire's segment instead of at the pin
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ConstFromPrimitive)]
#[repr(i32)]
pub enum WireConnectionType {
	#[default]
	ToPins = 0,
	ToWireSource = 1,
	ToWireTarget = 2,
}

impl WireConnectionType {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct WireDescription {
	pub source_pin_address: PinAddress,
	pub target_pin_address: PinAddress,
	/// Whether either end of this wire is attached to another wire rather
	/// than directly to a pin, and if so, which end.
	pub connection_type: WireConnectionType,
	/// Index (into the owning chip's `wires`) of the wire this one taps
	/// into, when `connection_type != ToPins`. `-1` / unused otherwise.
	pub connected_wire_index: i32,
	/// Index of the first point of the segment (on the referenced wire)
	/// that this wire's tap-point sits on, when `connection_type !=
	/// ToPins`. `-1` / unused otherwise.
	pub connected_wire_segment_index: i32,
	/// Cached world-space position of the source end, as last saved to disk. Ignored for pin-
	/// attached sources, since those resolve directly from the pin's live position instead.
	pub cached_source_point: Vec2,
	/// Same as `cached_source_point`, but for the target end; only
	/// meaningful when `connection_type == ToWireTarget`.
	pub cached_target_point: Vec2,
	/// Player-authored bend points between the resolved source and target
	/// endpoints, in the same world/grid coordinate space as everything
	/// else in this chip. Mirrors the saved `WireDescription.Points`, minus
	/// its first and last entries (those are the cached endpoint
	/// coordinates captured above instead). Empty for a straight, unbent
	/// wire.
	pub points: Vec<Vec2>,
}

impl WireDescription {
	/// A straight wire between `source` and `target` with no bend points,
	/// both ends attached directly to a pin.
	pub fn new(source_pin_address: PinAddress, target_pin_address: PinAddress) -> Self {
		Self {
			source_pin_address,
			target_pin_address,
			connection_type: WireConnectionType::ToPins,
			connected_wire_index: -1,
			connected_wire_segment_index: -1,
			cached_source_point: Vec2::default(),
			cached_target_point: Vec2::default(),
			points: Vec::new(),
		}
	}

	/// A wire whose *source* end taps onto an existing wire's segment
	/// (`WireConnectionType::ToWireSource`) instead of attaching to a pin directly -- used when
	/// a wire is placed by starting the drag from another wire's line rather than a pin.
	/// `source_pin_address` must still be the tapped wire's own real originating pin (needed for
	/// colour/bit-count/simulation-state lookups -- see `render::scene::draw_wires`'s doc
	/// comment), not an address describing the tap point itself.
	pub fn new_tapped_source(
		source_pin_address: PinAddress,
		target_pin_address: PinAddress,
		connected_wire_index: i32,
		connected_wire_segment_index: i32,
		tap_point: Vec2,
	) -> Self {
		Self {
			source_pin_address,
			target_pin_address,
			connection_type: WireConnectionType::ToWireSource,
			connected_wire_index,
			connected_wire_segment_index,
			cached_source_point: tap_point,
			cached_target_point: Vec2::default(),
			points: Vec::new(),
		}
	}

	/// The mirror of [`Self::new_tapped_source`] for a wire whose *target* end lands on an
	/// existing wire's line (`WireConnectionType::ToWireTarget`) -- how an input is fed from the
	/// middle of another wire ("wiring into a wire").
	pub fn new_tapped_target(
		source_pin_address: PinAddress,
		target_pin_address: PinAddress,
		connected_wire_index: i32,
		connected_wire_segment_index: i32,
		tap_point: Vec2,
	) -> Self {
		Self {
			source_pin_address,
			target_pin_address,
			connection_type: WireConnectionType::ToWireTarget,
			connected_wire_index,
			connected_wire_segment_index,
			cached_source_point: Vec2::default(),
			cached_target_point: tap_point,
			points: Vec::new(),
		}
	}
}

/// Full description of a chip: either a built-in primitive (Nand, Clock, ...)
/// or a Custom chip made up of subchips and wires between them.
#[derive(Debug, Clone, Default)]
pub struct ChipDescription {
	pub name: String,
	pub chip_type: ChipType,
	pub input_pins: Vec<PinDescription>,
	pub output_pins: Vec<PinDescription>,
	pub sub_chips: Vec<SubChipDescription>,
	pub wires: Vec<WireDescription>,
	/// Display surfaces embedded in this chip's body, as saved on disk
	/// (`Displays`). Empty for chips without any (the original saves
	/// `null` in that case, which parses as empty here).
	pub displays: Vec<DisplayDescription>,
	/// This chip's body colour as saved on disk (`Colour`), RGBA in 0..1.
	/// Alpha 0 (the default) means "no colour was saved" -- renderers
	/// should fall back to their own default body colour in that case.
	pub colour: [f32; 4],
	/// Where this chip's name label should be drawn on its body, as saved
	/// on disk (`NameLocation`). Defaults to `Centre`, matching the
	/// original's default for newly-created chips.
	pub name_location: NameLocation,
	/// The `DLSVersion_LastSaved` string this chip's own save file declared,
	/// as parsed from disk (`None` when absent, e.g. for chips built up in
	/// code rather than loaded).
	pub dls_version: Option<String>,
	/// This chip's body footprint as saved on disk (`Size`), in world/grid
	/// units.
	pub size: Vec2,
	/// Simplified indicator of what caching this chip is set up for.
	pub cache_kind: CacheKind,
}

impl ChipDescription {
	pub fn new(name: impl Into<String>, chip_type: ChipType) -> Self {
		Self {
			name: name.into(),
			chip_type,
			input_pins: Vec::new(),
			output_pins: Vec::new(),
			sub_chips: Vec::new(),
			wires: Vec::new(),
			displays: Vec::new(),
			colour: [0.0, 0.0, 0.0, 0.0],
			name_location: NameLocation::default(),
			dls_version: None,
			size: Vec2::default(),
			cache_kind: CacheKind::Off,
		}
	}
}

/// Lookup table of all known chip descriptions (builtin + custom), keyed by
/// case-insensitive name. Mirrors DLS.Game.ChipLibrary, minus editor concerns.
#[derive(Debug, Default, Clone)]
pub struct ChipLibrary {
	by_name: std::collections::HashMap<String, std::sync::Arc<ChipDescription>>,
}

impl ChipLibrary {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn add(&mut self, desc: ChipDescription) {
		self.by_name.insert(desc.name.to_ascii_lowercase(), std::sync::Arc::new(desc));
	}

	/// # Panics
	/// if `try_get` woud return None
	pub fn get(&self, name: &str) -> &ChipDescription {
		self.by_name.get(&name.to_ascii_lowercase()).map_or_else(|| panic!("Chip not found in library: {name}"), std::sync::Arc::as_ref)
	}

	/// Cheap-clone counterpart to `get`
	/// # Panics
	/// if `try_get` woud return None
	pub fn get_arc(&self, name: &str) -> std::sync::Arc<ChipDescription> {
		self.by_name.get(&name.to_ascii_lowercase()).cloned().unwrap_or_else(|| panic!("Chip not found in library: {name}"))
	}

	/// Mutable counterpart to `get` -- used by the viewer for in-place
	/// edits to a chip's description (deleting components/wires, renaming
	/// pins, applying customization drafts, undo/redo restores). Note this
	/// still deep-clones if the entry's `Arc` is shared (e.g. someone is
	/// mid-`get_arc`), same as it always has -- that's `Arc::make_mut`'s
	/// normal copy-on-write behaviour, not a regression.
	/// # Panics
	/// if `try_get` woud return None
	pub fn get_mut(&mut self, name: &str) -> &mut ChipDescription {
		let entry = self.by_name.get_mut(&name.to_ascii_lowercase()).unwrap_or_else(|| panic!("Chip not found in library: {name}"));
		std::sync::Arc::make_mut(entry)
	}

	pub fn try_get(&self, name: &str) -> Option<&ChipDescription> {
		self.by_name.get(&name.to_ascii_lowercase()).map(std::sync::Arc::as_ref)
	}

	/// Removes a chip from the library (by name, same case-insensitive lookup as everything else
	/// here), returning it if it was present.
	pub fn remove(&mut self, name: &str) -> Option<ChipDescription> {
		self.by_name.remove(&name.to_ascii_lowercase()).map(|arc| std::sync::Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
	}

	/// Iterate over every chip currently in the library (builtin + custom).
	/// Used by tooling (e.g. the viewer) that needs to pick a sensible
	/// default chip to display rather than assuming a fixed name exists.
	pub fn iter(&self) -> impl Iterator<Item = &ChipDescription> {
		self.by_name.values().map(std::sync::Arc::as_ref)
	}
}
