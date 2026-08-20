//! Static description of a chip (as loaded from a saved project), used to
//! build the runtime simulation graph. Mirrors DLS.Description in the
//! original C# codebase.
use crate::{render::theme::{Rgba, COLORS}, structs::Vec2};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
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
}

impl ChipType {
    /// Convert to the integer representation used on disk.
    pub fn to_int(&self) -> i32 {
        *self as i32
    }

    pub fn is_bus_origin_type(self) -> bool {
        matches!(self, ChipType::Bus1Bit | ChipType::Bus4Bit | ChipType::Bus8Bit)
    }

    pub fn is_bus_terminus_type(self) -> bool {
        matches!(
            self,
            ChipType::BusTerminus1Bit | ChipType::BusTerminus4Bit | ChipType::BusTerminus8Bit
        )
    }

    pub fn is_bus_type(self) -> bool {
        self.is_bus_origin_type() || self.is_bus_terminus_type()
    }
    /// Reconstruct from an integer, matching the original C# enum order.
    /// Invalid values fall back to `Custom`.
    pub fn from_int(v: i32) -> Self {
        match v {
            0 => Self::Custom,
            1 => Self::Nand,
            2 => Self::TriStateBuffer,
            3 => Self::Clock,
            4 => Self::Pulse,
            5 => Self::DevRam8Bit,
            6 => Self::Rom256x16,
            7 => Self::SevenSegmentDisplay,
            8 => Self::DisplayRgb,
            9 => Self::DisplayDot,
            10 => Self::DisplayLed,
            11 => Self::Merge1To4Bit,
            12 => Self::Merge1To8Bit,
            13 => Self::Merge4To8Bit,
            14 => Self::Split4To1Bit,
            15 => Self::Split8To4Bit,
            16 => Self::Split8To1Bit,
            17 => Self::In1Bit,
            18 => Self::In4Bit,
            19 => Self::In8Bit,
            20 => Self::Out1Bit,
            21 => Self::Out4Bit,
            22 => Self::Out8Bit,
            23 => Self::Key,
            24 => Self::Bus1Bit,
            25 => Self::BusTerminus1Bit,
            26 => Self::Bus4Bit,
            27 => Self::BusTerminus4Bit,
            28 => Self::Bus8Bit,
            29 => Self::BusTerminus8Bit,
            30 => Self::Buzzer,
            _ => Self::default(),
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ValueDisplayMode {
    #[default]
    None = 0,
    Decimal = 1,
    SignedDecimal = 2,
    Hex = 3,
}

impl ValueDisplayMode {
    pub fn from_int(v: i32) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Decimal,
            2 => Self::SignedDecimal,
            3 => Self::Hex,
            _ => Self::default(),
        }
    }

    pub fn to_int(&self) -> i32 {
        *self as i32
    }
}

/// Where (if anywhere) a chip's name label is drawn on its body. Mirrors
/// `DLS.Description.NameDisplayLocation`, saved on disk as `NameLocation`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NameLocation {
    #[default]
    Centre = 0,
    Top = 1,
    Hidden = 2,
}

impl NameLocation {
    /// `NameDisplayLocation` as stored on disk: a plain integer matching the
    /// original C# enum's declaration order
    pub fn from_int(v: i32) -> Self {
        match v {
            0 => Self::Centre,
            1 => Self::Top,
            2 => Self::Hidden,
            _ => Self::default(),
        }
    }

    pub fn to_int(&self) -> i32 {
        *self as i32
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
    pub fn new(pin_owner_id: i32, pin_id: i32) -> Self {
        Self { pin_id, pin_owner_id }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PinBitCount {
    #[default]
    Bit1 = 1,
    Bit4 = 4,
    Bit8 = 8,
}

impl PinBitCount {
    pub fn from_int(v: i32) -> Self {
        match v {
            1 => PinBitCount::Bit1,
            4 => PinBitCount::Bit4,
            8 => PinBitCount::Bit8,
            _ => Self::default(),
        }
    }

    pub fn to_int(&self) -> i32 {
        *self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    /// Clamp a saved pin/wire colour palette index into the valid 0..=7 range
    /// used by `render::theme`'s `COLOR` table.
    /// Negative or out-of-range values (shouldn't normally occur, but the
    /// on-disk value is an unchecked int) fall back to 0 rather than panicking
    /// or silently wrapping.
    pub fn from_int(a: i32) -> Self {
        match a {
            0 => Color::Red,
            1 => Color::Orange,
            2 => Color::Yellow,
            3 => Color::Green,
            4 => Color::Blue,
            5 => Color::Purple,
            6 => Color::Pink,
            7 => Color::White,
            _ => Self::default(),
        }
    }

    pub fn to_int(&self) -> i32 {
        *self as i32  // Works because of explicit discriminants!
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
    /// World-space position of this pin, in grid units, as saved on disk
    /// under this pin's `Position` field. For a chip's own boundary
    /// (dev-)pins (`ChipDescription::input_pins`/`output_pins`) this is the
    /// exact point at which wires attach -- unlike a subchip's pins, whose
    /// position is instead *derived* from the subchip's body + default pin
    /// layout (see `layout::pin_world_position`), a dev-pin's position is
    /// authoritative and comes straight from this field. Defaults to the
    /// origin for pins built up in code rather than loaded from disk.
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
        Self {
            name: name.into(),
            id,
            position: Vec2::default(),
            bit_count,
            colour: Color::default(),
            value_display_mode: ValueDisplayMode::None,
        }
    }

    pub fn with_colour(name: impl Into<String>, id: i32, bit_count: PinBitCount, colour: Color) -> Self {
        Self {
            name: name.into(),
            id,
            position: Vec2::default(),
            bit_count,
            colour,
            value_display_mode: ValueDisplayMode::None,
        }
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

#[derive(Debug, Clone)]
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
        self.pin_colour_info
            .iter()
            .find(|(id, _)| *id == pin_id)
            .map(|(_, colour)| *colour)
            .unwrap_or(default_colour)
    }
}

/// How a wire's source/target end attaches to the rest of the scene.
/// Mirrors `DLS.Description.WireConnectionType`. Most wires attach both
/// ends directly to a pin (`ToPins`), but a wire can instead be "tapped"
/// off of another wire's line -- its saved `SourcePinAddress`/
/// `TargetPinAddress` still names the real originating pin (needed for
/// colour/bit-count/simulation-state lookups), but that end's *position*
/// must be resolved along the referenced wire's segment instead of at the
/// pin itself. See `render::scene`'s wire-endpoint resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireConnectionType {
    #[default]
    ToPins = 0,
    ToWireSource = 1,
    ToWireTarget = 2,
}

impl WireConnectionType {
    /// `WireConnectionType` as stored on disk: `ToPins` = 0, `ToWireSource` = 1,
    /// `ToWireTarget` = 2, matching the original C# enum's declaration order.
    pub fn from_int(v: i32) -> Self {
        match v {
            0 => Self::ToPins,
            1 => Self::ToWireSource,
            2 => Self::ToWireTarget,
            _ => Self::default(),
        }
    }

    pub fn to_int(&self) -> i32 {
        *self as i32
    }
}

#[derive(Debug, Clone)]
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
    /// Cached world-space position of the source end, as last saved to
    /// disk. Only meaningful (and only used) when `connection_type ==
    /// ToWireSource`: it's the point that gets re-projected onto the
    /// referenced wire's segment to find this wire's actual source
    /// position (mirrors `WireInstance.GetAttachmentPoint`'s use of
    /// `originalWireConnectionPoint`). Ignored for pin-attached sources,
    /// since those resolve directly from the pin's live position instead.
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
    /// This chip's body colour as saved on disk (`Colour`), RGBA in 0..1.
    /// Alpha 0 (the default) means "no colour was saved" -- renderers
    /// should fall back to their own default body colour in that case.
    pub colour: [f32; 4],
    /// Where this chip's name label should be drawn on its body, as saved
    /// on disk (`NameLocation`). Defaults to `Centre`, matching the
    /// original's default for newly-created chips.
    pub name_location: NameLocation,
    /// This chip's body footprint as saved on disk (`Size`), in world/grid
    /// units. The original computes this at save time via
    /// `SubChipHelper.CalculateMinChipSize`, which folds in the actual
    /// rendered width of the chip's name label (real font metrics) as
    /// well as its pins -- info this crate can't recompute exactly since
    /// it has no font-metrics access outside `render::gpu`. `(0, 0)` (the
    /// default) means "not saved" (e.g. a chip built up in code rather
    /// than loaded from disk); renderers should fall back to computing a
    /// pins/name-estimate size in that case, e.g.
    /// `render::layout::calculate_min_chip_size`.
    pub size: Vec2,
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
            colour: [0.0, 0.0, 0.0, 0.0],
            name_location: NameLocation::default(),
            size: Vec2::default(),
        }
    }
}

/// Lookup table of all known chip descriptions (builtin + custom), keyed by
/// case-insensitive name. Mirrors DLS.Game.ChipLibrary, minus editor concerns.
#[derive(Debug, Default)]
pub struct ChipLibrary {
    by_name: std::collections::HashMap<String, ChipDescription>,
}

impl ChipLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, desc: ChipDescription) {
        self.by_name.insert(desc.name.to_ascii_lowercase(), desc);
    }

    pub fn get(&self, name: &str) -> &ChipDescription {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("Chip not found in library: {name}"))
    }

    pub fn try_get(&self, name: &str) -> Option<&ChipDescription> {
        self.by_name.get(&name.to_ascii_lowercase())
    }

    /// Iterate over every chip currently in the library (builtin + custom).
    /// Used by tooling (e.g. the viewer) that needs to pick a sensible
    /// default chip to display rather than assuming a fixed name exists.
    pub fn iter(&self) -> impl Iterator<Item = &ChipDescription> {
        self.by_name.values()
    }
}