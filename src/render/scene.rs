//! Builds drawable geometry for one "view" of a chip (i.e. what the editor
//! shows when you open a custom chip: its subchips, each subchip's pins,
//! and the wires between them). This is the scene-graph half of the
//! renderer -- pure data in, triangles out, no wgpu types -- so it can be
//! unit tested without a GPU.
//!
//! Mirrors (a first-pass subset of) `DLS.Graphics.World.DevSceneDrawer`.

use crate::description::{ChipDescription, ChipLibrary, ChipType, NameLocation, PinBitCount, WireConnectionType, WireDescription};
use crate::pin_state::LogicState;
use crate::render::camera::Camera;
use crate::render::layout;
use crate::structs::Vec2;
use crate::render::theme::{self, Rgba};
use crate::description::Color;
use std::collections::HashMap;

/// A single coloured vertex, position in world space. Kept separate from
/// any wgpu `Vertex` type so this module has zero GPU dependencies; the
/// `render::gpu` module converts these 1:1 into its own bytemuck vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVertex {
    pub pos: Vec2,
    pub colour: Rgba,
}

/// A gate/chip name label to be drawn as text, in world space. Produced
/// alongside `triangles` by `build_scene` -- kept as a separate list (rather
/// than triangulated glyphs) since text is rendered by a dedicated font
/// pipeline (`render::gpu`'s glyphon integration), not the flat-colour
/// triangle pipeline the rest of the scene uses.
#[derive(Debug, Clone)]
pub struct TextLabel {
    /// World-space anchor point: the label is horizontally *and*
    /// vertically centred on this point (callers wanting a "near the top
    /// edge" placement, e.g. `NameDisplayLocation::Top`, pre-offset `pos`
    /// upward when building the label rather than needing a separate
    /// anchor mode here).
    pub pos: Vec2,
    pub text: String,
    pub colour: Rgba,
    /// World-space font size (grid units); mirrors `DrawSettings.FontSizeChipName`.
    pub font_size: f32,
    /// World-space width to horizontally centre/wrap the text within
    /// (typically the owning chip's body width).
    pub width: f32,
}

/// Flat triangle-list geometry ready to upload as a vertex buffer
/// (`triangles.len()` is always a multiple of 3), plus any text labels to
/// be drawn on top of it (e.g. gate/chip names).
#[derive(Debug, Default, Clone)]
pub struct SceneGeometry {
    pub triangles: Vec<SceneVertex>,
    pub labels: Vec<TextLabel>,
}

impl SceneGeometry {
    fn push_tri(&mut self, a: SceneVertex, b: SceneVertex, c: SceneVertex) {
        self.triangles.push(a);
        self.triangles.push(b);
        self.triangles.push(c);
    }

    fn push_quad(&mut self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, colour: Rgba) {
        // p0..p3 wound consistently (e.g. bottom-left, bottom-right,
        // top-right, top-left) so this is two triangles of a convex quad.
        let v = |p: Vec2| SceneVertex { pos: p, colour };
        self.push_tri(v(p0), v(p1), v(p2));
        self.push_tri(v(p0), v(p2), v(p3));
    }

    pub fn add_rect(&mut self, centre: Vec2, size: Vec2, colour: Rgba) {
        let hw = size.x / 2.0;
        let hh = size.y / 2.0;
        self.push_quad(
            Vec2::new(centre.x - hw, centre.y - hh),
            Vec2::new(centre.x + hw, centre.y - hh),
            Vec2::new(centre.x + hw, centre.y + hh),
            Vec2::new(centre.x - hw, centre.y + hh),
            colour,
        );
    }

    pub fn add_circle(&mut self, centre: Vec2, radius: f32, colour: Rgba, segments: u32) {
        let segments = segments.max(3);
        for i in 0..segments {
            let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let p0 = Vec2::new(centre.x + a0.cos() * radius, centre.y + a0.sin() * radius);
            let p1 = Vec2::new(centre.x + a1.cos() * radius, centre.y + a1.sin() * radius);
            self.push_tri(
                SceneVertex { pos: centre, colour },
                SceneVertex { pos: p0, colour },
                SceneVertex { pos: p1, colour },
            );
        }
    }

    /// A rectangle of `size` centred on `centre`, with its corners rounded
    /// to `radius` on whichever of its left/right vertical edges
    /// `round_left`/`round_right` request (either, both, or neither -- the
    /// other edge's corners stay sharp). Used to draw a chip's own
    /// boundary dev-pins as a "partially rounded rectangle": rounded on
    /// the side facing outward (away from the chip body) and square on
    /// the side facing in, so they read visually distinct from a regular
    /// pin's plain circle. `radius` is clamped to the shape's own
    /// half-width/half-height so it can never overshoot into a bowtie.
    ///
    /// Implemented as a fan of triangles from `centre` around the
    /// perimeter (rounded corners contribute an arc of points, square
    /// corners contribute just their one corner point), the same
    /// triangulation strategy `add_circle` uses -- valid here because a
    /// rounded rect (with radius capped to half the smaller dimension) is
    /// always convex/star-shaped from its own centre.
    pub fn add_rounded_rect(
        &mut self,
        centre: Vec2,
        size: Vec2,
        colour: Rgba,
        radius: f32,
        round_left: bool,
        round_right: bool,
        corner_segments: u32,
    ) {
        let hw = size.x / 2.0;
        let hh = size.y / 2.0;
        if hw <= 0.0 || hh <= 0.0 {
            return;
        }
        let r = radius.max(0.0).min(hw).min(hh);
        let segs = corner_segments.max(1);

        fn push_corner(points: &mut Vec<Vec2>, cx: f32, cy: f32, arc_cx: f32, arc_cy: f32, start: f32, end: f32, r: f32, segs: u32, rounded: bool) {
            if rounded && r > 1e-6 {
                for i in 0..=segs {
                    let t = i as f32 / segs as f32;
                    let a = start + t * (end - start);
                    points.push(Vec2::new(arc_cx + a.cos() * r, arc_cy + a.sin() * r));
                }
            } else {
                points.push(Vec2::new(cx, cy));
            }
        }

        use std::f32::consts::PI;
        let mut points: Vec<Vec2> = Vec::new();
        // Bottom-right -> top-right -> top-left -> bottom-left (CCW).
        push_corner(&mut points, centre.x + hw, centre.y - hh, centre.x + hw - r, centre.y - hh + r, -PI / 2.0, 0.0, r, segs, round_right);
        push_corner(&mut points, centre.x + hw, centre.y + hh, centre.x + hw - r, centre.y + hh - r, 0.0, PI / 2.0, r, segs, round_right);
        push_corner(&mut points, centre.x - hw, centre.y + hh, centre.x - hw + r, centre.y + hh - r, PI / 2.0, PI, r, segs, round_left);
        push_corner(&mut points, centre.x - hw, centre.y - hh, centre.x - hw + r, centre.y - hh + r, PI, 3.0 * PI / 2.0, r, segs, round_left);

        let n = points.len();
        for i in 0..n {
            let p0 = points[i];
            let p1 = points[(i + 1) % n];
            self.push_tri(SceneVertex { pos: centre, colour }, SceneVertex { pos: p0, colour }, SceneVertex { pos: p1, colour });
        }
    }

    /// A thick line segment from `a` to `b`, drawn as a rectangle.
    pub fn add_line(&mut self, a: Vec2, b: Vec2, thickness: f32, colour: Rgba) {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            return;
        }
        let nx = -dy / len * thickness / 2.0;
        let ny = dx / len * thickness / 2.0;
        self.push_quad(
            Vec2::new(a.x + nx, a.y + ny),
            Vec2::new(b.x + nx, b.y + ny),
            Vec2::new(b.x - nx, b.y - ny),
            Vec2::new(a.x - nx, a.y - ny),
            colour,
        );
    }
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
    /// Per-instance output pin colour overrides, copied from this placed
    /// instance's `SubChipDescription::pin_colour_info`.
    pub pin_colour_info: Vec<(i32, Color)>,
    /// Copied verbatim from this placed instance's
    /// `SubChipDescription::internal_data` (empty if the subchip has none).
    /// Interpretation is chip-type specific:
    ///  - `Key`: `[0]` is the ASCII code (capitalised, e.g. `A` = 65) of the
    ///    key this instance listens to.
    ///  - `Rom256x16`: all 256 words of ROM contents, indexed by address.
    ///  - `DisplayLed`: `[0]` is a `Color` palette index (same encoding as
    ///    a pin's `Colour` field), used to tint the LED body.
    ///  - Bus origin/terminus (`Bus1Bit`/`Bus4Bit`/`Bus8Bit`/
    ///    `BusTerminus1Bit`/`BusTerminus4Bit`/`BusTerminus8Bit`): `[0]` is
    ///    the id of the paired bus chip at the other end of the link,
    ///    `[1]` is "is flipped" (`1` = draw this instance's visible pin on
    ///    the opposite side from its type default).
    pub internal_data: Vec<u32>,
}

impl<'a> PlacedSubChip<'a> {
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

/// Computes the world-space placement (body rect + pin y-offsets) of every
/// subchip in `chip`, resolving each subchip's own pin layout against
/// `library`. Subchips referencing an unknown chip name are skipped.
pub fn place_sub_chips<'a>(chip: &ChipDescription, library: &'a ChipLibrary) -> Vec<PlacedSubChip<'a>> {
    let mut placed = Vec::with_capacity(chip.sub_chips.len());

    for sub in &chip.sub_chips {
        let Some(desc) = library.try_get(&sub.name) else { continue };

        let input_bits: Vec<PinBitCount> = desc.input_pins.iter().map(|p| p.bit_count).collect();
        let output_bits: Vec<PinBitCount> = desc.output_pins.iter().map(|p| p.bit_count).collect();

        // Prefer the size actually saved on disk (`ChipDescription::size`,
        // from the JSON `Size` field) -- the original computes this via
        // `CalculateMinChipSize` with real font metrics, so it's more
        // accurate than anything we can derive here. Only fall back to
        // the pins+name-estimate heuristic when there's nothing saved
        // (size == (0,0)), e.g. a `ChipDescription` built up in code
        // (most builtins) rather than loaded from a project file. See
        // `ChipDescription::size` and `layout::calculate_min_chip_size`
        // docs for why either path matters for labels actually drawing.
        let size = if desc.size.x > 0.0 && desc.size.y > 0.0 {
            Vec2::new(desc.size.x, desc.size.y)
        } else {
            layout::calculate_min_chip_size(
                &input_bits,
                &output_bits,
                &desc.name,
                desc.name_location,
                theme::FONT_SIZE_CHIP_NAME,
            )
        };
        let (_, input_pin_y) = layout::calculate_default_pin_layout(&input_bits);
        let (_, output_pin_y) = layout::calculate_default_pin_layout(&output_bits);

        placed.push(PlacedSubChip {
            id: sub.id,
            desc,
            centre: sub.position,
            size,
            input_pin_y,
            output_pin_y,
            pin_colour_info: sub.pin_colour_info.clone(),
            internal_data: sub.internal_data.clone().unwrap_or_default(),
        });
    }

    placed
}

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
        self.is_high(pin_owner_id, pin_id)
            .map(|high| if high { LogicState::High } else { LogicState::Low })
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
/// reports the pin's first bit's state. Multi-bit buses are simplified to
/// "first bit's state" for this first rendering pass -- full per-bit
/// colour-coding is a follow-up once the multi-bit pin visuals are ported.
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
        Some(LogicState::from_tristated_value(raw))
    }
}

/// Build the full drawable scene for one chip: every subchip's body + pins,
/// plus wires connecting them. `chip.input_pins`/`output_pins` are treated
/// as this chip's own boundary dev-pins (owner id == the pin's own id, per
/// the on-disk wire-address convention).
pub fn build_scene(chip: &ChipDescription, library: &ChipLibrary, pin_state: &dyn PinStateLookup) -> SceneGeometry {
    let mut geo = SceneGeometry::default();
    let placed = place_sub_chips(chip, library);

    // owner_id -> index into `placed`, for resolving wire endpoints that
    // land on a subchip (as opposed to one of this chip's own dev-pins).
    let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

    // Draw order is a simple three-layer stack, back to front (this
    // renderer has no depth buffer, so draw order *is* z-order -- see
    // `build_grid`'s docs for the same point about the background grid):
    // wires at the bottom, pins in the middle, component bodies (+ their
    // name labels) on top. This keeps a component's body from ever being
    // occluded by a wire or pin that happens to be drawn after it, and
    // keeps pins sitting visibly on top of the wires that connect to them.
    draw_wires(&mut geo, chip, &placed, &owner_to_placed, pin_state);
    draw_pins(&mut geo, chip, &placed, pin_state);
    draw_components(&mut geo, &placed, pin_state);

    geo
}

/// Layer 1 (bottom): every wire in `chip.wires`, resolved to world-space
/// polylines and drawn as thick lines. See the inline comments below for
/// how an individual wire's two endpoints are resolved.
fn draw_wires(geo: &mut SceneGeometry, chip: &ChipDescription, placed: &[PlacedSubChip], owner_to_placed: &HashMap<i32, usize>, pin_state: &dyn PinStateLookup) {
    // Resolve each wire's two endpoints to world positions and draw a
    // polyline through any player-authored bend points between them (saved
    // `Points`, minus its first/last entries -- see `WireDescription::points`).
    // No bend points just means one straight segment.
    //
    // An endpoint is resolved one of two ways, per `wire.connection_type`:
    //  - `ToPins` (the common case): straight from the pin's own resolved
    //    world position, as before.
    //  - `ToWireSource`/`ToWireTarget`: this end is actually a tap on
    //    *another* wire's line rather than a real pin location, so it's
    //    resolved by re-projecting the cached attachment point onto that
    //    other wire's segment (`resolve_wire_endpoint`/`resolve_wire_point`
    //    below, mirroring `WireInstance.GetAttachmentPoint`). Using the raw
    //    pin position here (the old behaviour) desyncs from the
    //    player-authored bend points, which assume the wire starts/ends at
    //    the tap point, not at the underlying pin -- that mismatch is what
    //    produced visibly wrong bends for any wire tapped off another wire.
    //
    // `wire_point_cache` memoizes resolved endpoints across the whole
    // chip's wire list for this build: a single wire can be the tap target
    // for several others, and resolving a tapped chain revisits earlier
    // wires' endpoints.
    let mut wire_point_cache: WirePointCache = HashMap::new();
    for (wire_idx, wire) in chip.wires.iter().enumerate() {
        let src = resolve_wire_endpoint(chip, placed, owner_to_placed, &chip.wires, wire_idx, false, &mut wire_point_cache, 0);
        let dst = resolve_wire_endpoint(chip, placed, owner_to_placed, &chip.wires, wire_idx, true, &mut wire_point_cache, 0);

        if let (Some(src), Some(dst)) = (src, dst) {
            // Colour/bit-count/state always trace back to the wire's real
            // originating pin (`source_pin_address`), regardless of
            // `connection_type` -- a wire tapped off another wire still
            // carries that other wire's underlying signal, so this
            // resolution doesn't need to change for the bend fix above.
            let logic = pin_state
                .logic_state(wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id)
                .unwrap_or(LogicState::Low);
            let colour = resolve_pin_colour(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);
            let bit_count = resolve_pin_bit_count(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);
            let thickness = layout::WIRE_THICKNESS * bit_count.to_int() as f32;
            let colour = theme::state_colour(logic, colour);

            let mut prev = src;
            for &bend in &wire.points {
                geo.add_line(prev, bend, thickness, colour);
                prev = bend;
            }
            geo.add_line(prev, dst, thickness, colour);
        }
    }
}

/// Layer 2 (middle): every pin -- each subchip's input/output pins (plain
/// circles) plus this chip's own boundary dev-pins (small rounded-rect
/// bodies, drawn via `draw_dev_pin_body`) -- so pins always sit visibly on
/// top of the wires that connect to them, and underneath the component
/// bodies that own them.
fn draw_pins(geo: &mut SceneGeometry, chip: &ChipDescription, placed: &[PlacedSubChip], pin_state: &dyn PinStateLookup) {
    for sub in placed {
        // Bus origin/terminus chips draw their one visible pin on a fixed
        // default side (bus -> right, terminus -> left) unless flipped via
        // saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
        let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;

        for (i, pin) in sub.desc.input_pins
            .iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
            let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
            let pos = layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped);
            let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
            geo.add_circle(pos, layout::PIN_RADIUS, theme::state_colour(logic, pin.colour), 16);
        }
        for (i, pin) in sub.desc.output_pins
            .iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
            let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
            let pos = layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped);
            let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
            // A specific placed instance can override its output pin's
            // colour (saved `OutputPinColourInfo`); fall back to the
            // chip-level pin colour when there's no override for this pin.
            let colour_idx = sub.output_pin_colour(pin.id, pin.colour);
            geo.add_circle(pos, layout::PIN_RADIUS, theme::state_colour(logic, colour_idx), 16);
        }
    }

    // This chip's own boundary dev-pins (`chip.input_pins`/`output_pins`),
    // at their real saved position -- a partially rounded rectangle
    // (rounded on the side facing outward, away from the chip; square on
    // the side facing in, toward where a wire attaches), filled with the
    // pin's live state/palette colour and outlined in a grey-ish border,
    // so they read as visually distinct from a regular subchip pin's
    // plain circle. Mirrors `layout::dev_pin_body_size`'s docs.
    for pin in &chip.input_pins {
        draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), true);
    }
    for pin in &chip.output_pins {
        draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), false);
    }
}

/// Layer 3 (top): every subchip's body rectangle + name label, drawn last
/// so a component's body is never occluded by a wire or pin drawn earlier.
fn draw_components(geo: &mut SceneGeometry, placed: &[PlacedSubChip], pin_state: &dyn PinStateLookup) {
    for sub in placed {
        // An LED's body *is* its indicator: tint it with the saved
        // `InternalData[0]` colour (same palette-index encoding as a pin's
        // `Colour` field), lit/dimmed/disconnected exactly like a wire of
        // that colour would be, driven by the live state of its one input
        // pin. Falls back to the ordinary body-colour handling below if
        // this instance has no saved colour for some reason.
        let led_colour = (sub.desc.chip_type == ChipType::DisplayLed)
            .then(|| sub.internal_data.first().copied())
            .flatten()
            .map(|idx| {
                let colour = Color::from_int(idx as i32);
                let logic = sub
                    .desc
                    .input_pins
                    .first()
                    .and_then(|p| pin_state.logic_state(sub.id, p.id))
                    .unwrap_or(LogicState::Low);
                theme::state_colour(logic, colour)
            });

        // Use this chip's saved body colour (alpha 0 means "not saved" --
        // fall back to the theme default) rather than always drawing every
        // chip with the same flat grey.
        let body_colour = led_colour
            .unwrap_or_else(|| if sub.desc.colour[3] > 0.0 { sub.desc.colour } else { theme::CHIP_BODY_COL });
        geo.add_rect(sub.centre, sub.size, body_colour);

        // Draw this subchip's name label, unless explicitly hidden (e.g.
        // display/bus/pin chips, which save NameLocation = Hidden since
        // their body is the visualisation). Mirrors
        // `DevSceneDrawer.DrawSubChip`'s "if (... desc.NameLocation !=
        // NameDisplayLocation.Hidden)" gate -- except for the Key chip,
        // which forces its label to show regardless of the saved (always
        // Hidden) `NameLocation`: its body has no other visualisation, so
        // the bound key's letter (from saved `InternalData[0]`, an ASCII
        // code -- capitalised, e.g. `A` = 65) is shown in its place.
        let key_letter = (sub.desc.chip_type == ChipType::Key)
            .then(|| sub.internal_data.first().copied())
            .flatten()
            .map(|code| (code as u8 as char).to_string());

        if let Some(letter) = key_letter {
            geo.labels.push(TextLabel {
                pos: sub.centre,
                text: letter,
                colour: theme::text_colour_for_background(body_colour),
                font_size: theme::FONT_SIZE_CHIP_NAME,
                width: sub.size.x,
            });
        } else if sub.desc.name_location != NameLocation::Hidden {
            let label_pos = match sub.desc.name_location {
                NameLocation::Top => Vec2::new(
                    sub.centre.x,
                    sub.centre.y + sub.size.y / 2.0 - theme::FONT_SIZE_CHIP_NAME / 2.0 - layout::GRID_SIZE / 2.0,
                ),
                _ => sub.centre,
            };
            geo.labels.push(TextLabel {
                pos: label_pos,
                text: sub.desc.name.clone(),
                colour: theme::text_colour_for_background(body_colour),
                font_size: theme::FONT_SIZE_CHIP_NAME,
                width: sub.size.x,
            });
        }
    }
}

/// Draws one of a chip's own boundary dev-pins as a small "component"
/// body at `pos` (its real saved position): a partially rounded
/// rectangle, rounded on whichever side faces outward (`round_left` for
/// an input pin, sitting on the chip's left edge with wires approaching
/// from further left; the mirror for an output pin) and square on the
/// side facing in, filled with the pin's live state colour and outlined
/// in a grey-ish border. See `layout::dev_pin_body_size`/
/// `dev_pin_corner_radius` for the sizing this follows.
fn draw_dev_pin_body(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Color, logic: Option<LogicState>, round_left: bool) {
    let size = layout::dev_pin_body_size(bit_count);
    let radius = layout::dev_pin_corner_radius(size);
    let border = layout::DEV_PIN_BORDER_WIDTH.min(size.x / 2.0).min(size.y / 2.0);
    let fill_colour = theme::state_colour(logic.unwrap_or(LogicState::Low), colour);

    // Border first (drawn full-size, in the grey-ish outline colour)...
    geo.add_rounded_rect(pos, size, theme::CHIP_OUTLINE_COL, radius, round_left, !round_left, layout::DEV_PIN_ROUND_SEGMENTS);

    // ...then the pin-coloured fill on top, inset by the border width so
    // the border reads as an outline rather than being fully covered.
    let inner_size = Vec2::new((size.x - border * 2.0).max(0.0), (size.y - border * 2.0).max(0.0));
    let inner_radius = (radius - border).max(0.0);
    geo.add_rounded_rect(pos, inner_size, fill_colour, inner_radius, round_left, !round_left, layout::DEV_PIN_ROUND_SEGMENTS);
}

/// Memoizes resolved wire-endpoint world positions within one `build_scene`
/// call, keyed by `(wire index into chip.wires, is_target)`. Needed because
/// resolving one wire-tap endpoint can require resolving another wire's
/// endpoints in turn (see `resolve_wire_endpoint`), and the same wire can be
/// revisited many times (e.g. a bus fanning out to several taps).
type WirePointCache = HashMap<(usize, bool), Option<Vec2>>;

/// How many wire-to-wire attachment hops to follow before giving up.
/// Real projects only ever nest a couple of levels deep (`WireInstance`'s
/// own `ConnectedWireRecursionDepth` tracks this for draw-ordering, and
/// stays small in practice), so this is purely a guard against a
/// hand-edited or corrupted save file describing a connection cycle --
/// without it, a cycle would recurse forever instead of just drawing that
/// wire wrong.
const MAX_WIRE_CONNECTION_DEPTH: u32 = 64;

/// The closest point to `p` on line segment `a`-`b`. Mirrors
/// `WireInstance.ClosestPointOnLineSegment`; used to re-project a
/// wire-tap's cached attachment point onto its target wire's segment.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = Vec2::new(b.x - a.x, b.y - a.y);
    let sqr_len = ab.x * ab.x + ab.y * ab.y;
    if sqr_len <= 1e-12 {
        return a;
    }
    let ap = Vec2::new(p.x - a.x, p.y - a.y);
    let t = ((ap.x * ab.x + ap.y * ab.y) / sqr_len).clamp(0.0, 1.0);
    Vec2::new(a.x + ab.x * t, a.y + ab.y * t)
}

/// Resolves world-space point index `point_index` along wire `wire_idx`'s
/// own polyline, i.e. `[source-endpoint, ...bends..., target-endpoint]`.
/// Interior indices are just that wire's saved bend points (already in
/// world space, no resolution needed); the two endpoint indices recurse
/// into `resolve_wire_endpoint`, since either one might itself be a tap on
/// yet another wire. Mirrors `WireInstance.GetWirePoint`.
fn resolve_wire_point(
    chip: &ChipDescription,
    placed: &[PlacedSubChip],
    owner_to_placed: &HashMap<i32, usize>,
    wires: &[WireDescription],
    wire_idx: usize,
    point_index: usize,
    cache: &mut WirePointCache,
    depth: u32,
) -> Option<Vec2> {
    let wire = wires.get(wire_idx)?;
    let last_index = wire.points.len() + 1; // bends.len() interior points + 2 endpoints
    if point_index == 0 {
        resolve_wire_endpoint(chip, placed, owner_to_placed, wires, wire_idx, false, cache, depth)
    } else if point_index == last_index {
        resolve_wire_endpoint(chip, placed, owner_to_placed, wires, wire_idx, true, cache, depth)
    } else {
        wire.points.get(point_index - 1).copied()
    }
}

/// Resolves one end of wire `wire_idx` (`is_target`: false = source, true =
/// target) to a world-space position.
///
/// A plain pin-attached end (`WireConnectionType::ToPins`, or the
/// non-tapped end of a partially-tapped wire) resolves straight from the
/// pin's own live position via `resolve_pin_position`, same as always. A
/// wire-attached end (`ToWireSource` for the source end, `ToWireTarget` for
/// the target end) instead re-projects that end's last cached attachment
/// point onto the referenced wire's segment (`connected_wire_index`,
/// `connected_wire_segment_index`) -- mirroring
/// `WireInstance.GetAttachmentPoint` / `WireLayoutHelper.GetClosestPointOnWire`
/// in the original. This is the fix for wire-tap endpoints resolving to the
/// wrong place (and thus producing visibly wrong bends): they were
/// previously always resolved as if `ToPins`.
fn resolve_wire_endpoint(
    chip: &ChipDescription,
    placed: &[PlacedSubChip],
    owner_to_placed: &HashMap<i32, usize>,
    wires: &[WireDescription],
    wire_idx: usize,
    is_target: bool,
    cache: &mut WirePointCache,
    depth: u32,
) -> Option<Vec2> {
    if let Some(&cached) = cache.get(&(wire_idx, is_target)) {
        return cached;
    }
    if depth > MAX_WIRE_CONNECTION_DEPTH {
        return None;
    }
    let Some(wire) = wires.get(wire_idx) else { return None };

    let attaches_to_wire = matches!(
        (is_target, wire.connection_type),
        (false, WireConnectionType::ToWireSource) | (true, WireConnectionType::ToWireTarget)
    );

    let result = if attaches_to_wire {
        if wire.connected_wire_index < 0 {
            None
        } else {
            let target_wire_idx = wire.connected_wire_index as usize;
            let seg = wire.connected_wire_segment_index.max(0) as usize;
            let a = resolve_wire_point(chip, placed, owner_to_placed, wires, target_wire_idx, seg, cache, depth + 1);
            let b = resolve_wire_point(chip, placed, owner_to_placed, wires, target_wire_idx, seg + 1, cache, depth + 1);
            match (a, b) {
                (Some(a), Some(b)) => {
                    let cached_point = if is_target { wire.cached_target_point } else { wire.cached_source_point };
                    Some(closest_point_on_segment(cached_point, a, b))
                }
                _ => None,
            }
        }
    } else {
        let addr = if is_target { &wire.target_pin_address } else { &wire.source_pin_address };
        resolve_pin_position(chip, placed, owner_to_placed, addr.pin_owner_id, addr.pin_id, is_target)
    };

    cache.insert((wire_idx, is_target), result);
    result
}

/// How many grid lines to skip between each one actually drawn, based on
/// the current view's world-space half-height. Thins the grid out as the
/// camera zooms out so it doesn't turn into visual noise. Mirrors the
/// inline `skip` calculation in `DevSceneDrawer.DrawGrid`.
fn grid_line_skip(screen_half_height: f32) -> i32 {
    if screen_half_height < 8.0 {
        1
    } else if screen_half_height < 32.0 {
        4
    } else {
        16
    }
}

/// Builds the background grid line geometry currently visible within
/// `camera`'s view, mirroring `DevSceneDrawer.DrawGrid`. Draw this *before*
/// the rest of a scene's triangles (this renderer has no depth buffer, so
/// draw order is z-order -- grid needs to be background, i.e. first).
///
/// Line density thins out as the camera zooms out (skipping every 4th/16th
/// line past certain world-half-height thresholds), matching the original's
/// `skip` logic so a fully zoomed-out view doesn't turn into visual noise.
pub fn build_grid(camera: &Camera, colour: Rgba) -> SceneGeometry {
    let mut geo = SceneGeometry::default();

    // World-space half-extents of the current view -- equivalent to the
    // original's `cam.orthographicSize` (half-height) and
    // `orthographicSize * aspect` (half-width); this camera already folds
    // aspect ratio into `viewport_width`/`viewport_height` directly.
    let screen_half_width = camera.viewport_width / (2.0 * camera.zoom);
    let screen_half_height = camera.viewport_height / (2.0 * camera.zoom);
    let world_centre = camera.position;

    // Mirrors the original's local `ToGrid`: truncate (not round) down
    // toward the next lower grid line.
    let to_grid = |v: f32| -> f32 { ((v / layout::GRID_SIZE) as i32) as f32 * layout::GRID_SIZE };

    let left = to_grid(-screen_half_width + world_centre.x) - layout::GRID_SIZE;
    let right = to_grid(screen_half_width + world_centre.x) + layout::GRID_SIZE;
    let top = to_grid(screen_half_height + world_centre.y) + layout::GRID_SIZE;
    let bottom = to_grid(-screen_half_height + world_centre.y) - layout::GRID_SIZE;

    let skip = grid_line_skip(screen_half_height);

    // World-space thickness widened, if needed, so lines never render
    // thinner than ~1.5 screen pixels -- see `layout::grid_line_thickness`
    // docs for why a flat, non-antialiased quad needs this to avoid a
    // patchy/inconsistent-looking grid once zoomed out.
    let thickness = layout::grid_line_thickness(camera.zoom);

    // `left`/`right`/`top`/`bottom` are already exact multiples of
    // `GRID_SIZE` (0.125, exactly representable in binary floating point),
    // so converting to integer grid indices up front is exact -- avoids the
    // float-accumulation drift a `for px = left; px < right; px += GRID_SIZE`
    // loop would risk over many iterations at high zoom.
    let left_i = (left / layout::GRID_SIZE).round() as i32;
    let right_i = (right / layout::GRID_SIZE).round() as i32;
    let bottom_i = (bottom / layout::GRID_SIZE).round() as i32;
    let top_i = (top / layout::GRID_SIZE).round() as i32;

    for x_int in left_i..right_i {
        if x_int % skip == 0 {
            let px = x_int as f32 * layout::GRID_SIZE;
            geo.add_line(Vec2::new(px, bottom), Vec2::new(px, top), thickness, colour);
        }
    }

    for y_int in bottom_i..top_i {
        if y_int % skip == 0 {
            let py = y_int as f32 * layout::GRID_SIZE;
            geo.add_line(Vec2::new(left, py), Vec2::new(right, py), thickness, colour);
        }
    }

    geo
}

/// Axis-aligned bounding box of every vertex in `geo`, or `None` if it's
/// empty. Used by the viewer to fit the camera to whatever chip is on
/// screen instead of relying on a fixed default zoom (chips are sized in
/// grid units of ~0.125, so a zoom=1.0 default shows them as an
/// indistinguishable speck).
pub fn bounding_box(geo: &SceneGeometry) -> Option<(Vec2, Vec2)> {
    let mut iter = geo.triangles.iter();
    let first = iter.next()?.pos;
    let mut min = first;
    let mut max = first;
    for v in iter {
        min.x = min.x.min(v.pos.x);
        min.y = min.y.min(v.pos.y);
        max.x = max.x.max(v.pos.x);
        max.y = max.y.max(v.pos.y);
    }
    Some((min, max))
}

/// Resolves a wire's colour palette index from its source pin, mirroring
/// the same owner-id resolution `resolve_pin_position` uses: a subchip's
/// output pin (respecting any per-instance `OutputPinColourInfo` override)
/// or one of this chip's own boundary dev-pins. Falls back to palette index
/// 0 if the pin can't be resolved.
fn resolve_pin_colour(
    chip: &ChipDescription,
    placed: &[PlacedSubChip],
    owner_to_placed: &HashMap<i32, usize>,
    owner_id: i32,
    pin_id: i32,
) -> Color {
    if let Some(&idx) = owner_to_placed.get(&owner_id) {
        let sub = &placed[idx];
        if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
            return sub.output_pin_colour(pin.id, pin.colour);
        }
        if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
            return pin.colour;
        }
        return Color::default();
    }

    if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
        return p.colour;
    }
    if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
        return p.colour;
    }

    Color::default()
}

/// Resolves a wire's bit count from its source pin, using the same
/// owner-id resolution as `resolve_pin_position`/`resolve_pin_colour`.
/// Falls back to `Bit1` if the pin can't be resolved.
fn resolve_pin_bit_count(
    chip: &ChipDescription,
    placed: &[PlacedSubChip],
    owner_to_placed: &HashMap<i32, usize>,
    owner_id: i32,
    pin_id: i32,
) -> PinBitCount {
    if let Some(&idx) = owner_to_placed.get(&owner_id) {
        let sub = &placed[idx];
        if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
            return pin.bit_count;
        }
        if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
            return pin.bit_count;
        }
        return PinBitCount::Bit1;
    }

    if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
        return p.bit_count;
    }
    if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
        return p.bit_count;
    }

    PinBitCount::Bit1
}

fn resolve_pin_position(
    chip: &ChipDescription,
    placed: &[PlacedSubChip],
    owner_to_placed: &HashMap<i32, usize>,
    owner_id: i32,
    pin_id: i32,
    is_input_side: bool,
) -> Option<Vec2> {
    // Case 1: owner refers to a subchip in this scene.
    if let Some(&idx) = owner_to_placed.get(&owner_id) {
        let sub = &placed[idx];
        // Bus origin/terminus chips draw their one visible pin on a fixed
        // default side (bus -> right, terminus -> left) unless flipped via
        // saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
        let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;
        if let Some((i, pin)) = sub.desc.input_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
            let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
            let _ = pin;
            return Some(layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped));
        }
        if let Some((i, pin)) = sub.desc.output_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
            let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
            let _ = pin;
            return Some(layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped));
        }
        return None;
    }

    // Case 2: owner refers to one of this chip's own boundary dev-pins
    // (owner id == the pin's own global id, single local pin id 0). Unlike
    // a subchip's pins (whose position is *derived* from the subchip's
    // body + default pin layout), a dev-pin's position is authoritative
    // and saved directly on the `PinDescription` itself -- see the
    // `position` field's docs. Use it as-is instead of fabricating a
    // stacked placeholder layout.
    let _ = pin_id;
    let _ = is_input_side;
    if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
        return Some(p.position);
    }
    if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
        return Some(p.position);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::{ChipType, PinDescription, SubChipDescription, WireDescription};
    use crate::description::PinAddress;

    fn nand_desc() -> ChipDescription {
        let mut d = ChipDescription::new("NAND", ChipType::Nand);
        d.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
        d.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
        d.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
        d
    }

    #[test]
    fn rect_produces_two_triangles_six_verts() {
        let mut geo = SceneGeometry::default();
        geo.add_rect(Vec2::ZERO, Vec2::new(2.0, 1.0), theme::CHIP_BODY_COL);
        assert_eq!(geo.triangles.len(), 6);
    }

    #[test]
    fn circle_produces_3_verts_per_segment() {
        let mut geo = SceneGeometry::default();
        geo.add_circle(Vec2::ZERO, 0.1, theme::PIN_COL, 12);
        assert_eq!(geo.triangles.len(), 12 * 3);
    }

    #[test]
    fn zero_length_line_is_skipped_without_panicking() {
        let mut geo = SceneGeometry::default();
        geo.add_line(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), 0.05, theme::PIN_COL);
        assert!(geo.triangles.is_empty());
    }

    #[test]
    fn bounding_box_is_none_for_empty_scene() {
        let geo = SceneGeometry::default();
        assert!(bounding_box(&geo).is_none());
    }

    #[test]
    fn bounding_box_covers_all_pushed_shapes() {
        let mut geo = SceneGeometry::default();
        geo.add_rect(Vec2::new(-1.0, 0.0), Vec2::new(0.5, 0.5), theme::CHIP_BODY_COL);
        geo.add_circle(Vec2::new(2.0, 3.0), 0.2, theme::PIN_COL, 8);
        let (min, max) = bounding_box(&geo).unwrap();
        assert!(min.x <= -1.2 && min.x >= -1.3);
        assert!(max.x >= 2.2 && max.x <= 2.3);
        assert!(max.y >= 3.2 && max.y <= 3.3);
    }

    #[test]
    fn place_sub_chips_widens_body_to_fit_a_long_name() {
        // A chip whose pins alone would only need GRID_SIZE*2 (0.25
        // units) of width, but whose name is much longer than that.
        let mut lib = ChipLibrary::new();
        let mut wide_named = ChipDescription::new("Full Adder", ChipType::Custom);
        wide_named.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
        wide_named.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
        lib.add(wide_named);

        let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
        parent.sub_chips.push(SubChipDescription {
            name: "Full Adder".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
            pin_colour_info: Vec::new(),
        });

        let placed = place_sub_chips(&parent, &lib);
        assert_eq!(placed.len(), 1);
        let pins_only_width = layout::calculate_min_chip_size_for_pins(
            &[PinBitCount::Bit1],
            &[PinBitCount::Bit1],
        )
        .x;
        assert!(
            placed[0].size.x > pins_only_width,
            "body should be widened past the pin-only width to fit the name label"
        );
    }

    #[test]
    fn build_scene_label_width_is_wide_enough_to_fit_its_own_text() {
        // The regression this guards against: a `TextLabel.width` narrower
        // than the text it holds gets clipped down to a sliver by the
        // renderer's text bounds and is effectively invisible on screen,
        // even though a `TextLabel` was technically produced.
        let mut lib = ChipLibrary::new();
        let mut wide_named = ChipDescription::new("Full Adder", ChipType::Custom);
        wide_named.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
        wide_named.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
        lib.add(wide_named);

        let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
        parent.sub_chips.push(SubChipDescription {
            name: "Full Adder".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
            pin_colour_info: Vec::new(),
        });

        let scene = build_scene(&parent, &lib, &AllLow);
        assert_eq!(scene.labels.len(), 1);
        let label = &scene.labels[0];
        assert_eq!(label.text, "Full Adder");
        let needed_width = layout::estimate_text_width(&label.text, label.font_size);
        assert!(
            label.width >= needed_width - 1e-4,
            "label width {} should be enough to fit the estimated text width {}",
            label.width,
            needed_width
        );
    }

    #[test]
    fn place_sub_chips_skips_unknown_chip_names() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut parent = ChipDescription::new("TEST", ChipType::Custom);
        parent.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
            pin_colour_info: Vec::new(),
        });
        parent.sub_chips.push(SubChipDescription {
            name: "NONEXISTENT".into(),
            id: 2,
            internal_data: None,
            position: Vec2::new(1.0, 0.0),
            pin_colour_info: Vec::new(),
        });

        let placed = place_sub_chips(&parent, &lib);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].id, 1);
        assert_eq!(placed[0].input_pin_y.len(), 2);
        assert_eq!(placed[0].output_pin_y.len(), 1);
    }

    #[test]
    fn build_scene_draws_bodies_pins_and_wires_for_two_wired_nands() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut parent = ChipDescription::new("TEST", ChipType::Custom);
        parent.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 1,
            internal_data: None,
            position: Vec2::new(-1.0, 0.0),
pin_colour_info: Vec::new(),
        });
        parent.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 2,
            internal_data: None,
            position: Vec2::new(1.0, 0.0),
pin_colour_info: Vec::new(),
        });
        parent.wires.push(WireDescription::new(
            PinAddress::new(1, 0), // NAND #1's output pin id 0
            PinAddress::new(2, 0), // NAND #2's input pin id 0
        ));

        let scene = build_scene(&parent, &lib, &AllLow);
        // 2 chip bodies (6 verts each) + 6 pins (3 in + 3 out across both
        // NANDs = 2*(2+1)=6 pins, 16 segments * 3 verts each) + 1 wire (6 verts).
        let expected_body = 2 * 6;
        let expected_pins = 6 * 16 * 3;
        let expected_wire = 6;
        assert_eq!(scene.triangles.len(), expected_body + expected_pins + expected_wire);
    }

    #[test]
    fn simulator_pin_state_resolves_live_sim_values() {
        use crate::sim::Simulator;

        let mut lib = ChipLibrary::new();
        crate::builtins::register_all(&mut lib);

        // A tiny custom chip: one NAND subchip, unconnected inputs (so both
        // read HIGH via the sim's disconnected-pin convention) feeding its
        // output pin. We just need *a* live SimChip id to query through
        // `find_pin`, not full end-to-end signal correctness (that's
        // sim.rs's job, already covered by its own tests).
        let mut root = ChipDescription::new("ROOT", ChipType::Custom);
        root.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
pin_colour_info: Vec::new(),
        });

        let sim = Simulator::build(&root, &lib);
        let lookup = SimulatorPinState { sim: &sim, scope: sim.root() };

        // NAND subchip id=1, output pin id=2 (per builtins::create_nand's pin layout).
        let result = lookup.is_high(1, 2);
        assert!(result.is_some(), "expected NAND output pin to resolve via find_pin");
    }

    #[test]
    fn build_scene_skips_wire_with_unresolvable_endpoint() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut parent = ChipDescription::new("TEST", ChipType::Custom);
        parent.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
pin_colour_info: Vec::new(),
        });
        parent.wires.push(WireDescription::new(
            PinAddress::new(1, 0),
            PinAddress::new(999, 0), // unknown owner
        ));

        let scene = build_scene(&parent, &lib, &AllLow);
        // Only the one chip body (6) + its 3 pins (16*3 each) should be drawn; no wire.
        assert_eq!(scene.triangles.len(), 6 + 3 * 16 * 3);
    }

    #[test]
    fn closest_point_on_segment_projects_and_clamps() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        // A point off the line projects straight down onto it...
        assert_eq!(closest_point_on_segment(Vec2::new(5.0, 3.0), a, b), Vec2::new(5.0, 0.0));
        // ...and projection clamps to the segment's ends rather than
        // extrapolating past them.
        assert_eq!(closest_point_on_segment(Vec2::new(-5.0, 0.0), a, b), a);
        assert_eq!(closest_point_on_segment(Vec2::new(15.0, 0.0), a, b), b);
    }

    #[test]
    fn closest_point_on_segment_handles_a_zero_length_segment() {
        let a = Vec2::new(3.0, 4.0);
        assert_eq!(closest_point_on_segment(Vec2::new(0.0, 0.0), a, a), a);
    }

    /// This is the regression test for the wire-bend bug: a wire tapped
    /// onto another wire's segment (`WireConnectionType::ToWireSource`)
    /// must resolve its endpoint by projecting the cached attachment point
    /// onto that other wire's segment, *not* by jumping straight to the
    /// underlying pin's position (the old, buggy behaviour) -- doing the
    /// latter desyncs the tap's resolved position from its
    /// player-authored bend points, which were drawn assuming the wire
    /// starts at the tap point.
    #[test]
    fn wire_tap_endpoint_resolves_onto_referenced_wire_segment_not_the_underlying_pin() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut chip = ChipDescription::new("TAP_TEST", ChipType::Custom);
        for id in [1, 2, 3] {
            chip.sub_chips.push(SubChipDescription {
                name: "NAND".into(),
                id,
                internal_data: None,
                position: Vec2::new(id as f32 * 4.0, 0.0),
                pin_colour_info: Vec::new(),
            });
        }

        // wire 0: NAND1's output (pin 0) -> NAND2's input A (pin 0), bent
        // through one authored point so there's a real interior segment
        // (source -> bend) to tap onto.
        let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
        wire0.points = vec![Vec2::new(2.0, 5.0)];
        chip.wires.push(wire0);

        // wire 1: taps onto wire 0's first segment (its source -> its
        // bend), attaching at a cached point that's deliberately off that
        // segment's line -- it should snap onto the segment, not just be
        // used verbatim. Its target is NAND3's input B.
        let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
        wire1.connection_type = WireConnectionType::ToWireSource;
        wire1.connected_wire_index = 0;
        wire1.connected_wire_segment_index = 0;
        wire1.cached_source_point = Vec2::new(1.0, 10.0);
        chip.wires.push(wire1);

        let placed = place_sub_chips(&chip, &lib);
        let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let mut cache: WirePointCache = HashMap::new();

        let wire0_src =
            resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 0, false, &mut cache, 0)
                .expect("wire 0's source should resolve via NAND1's output pin");
        let wire0_bend = chip.wires[0].points[0];

        let wire1_src =
            resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 1, false, &mut cache, 0)
                .expect("wire 1's tapped source should resolve via wire 0's segment");

        let expected = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);
        assert_eq!(wire1_src, expected);

        // Critically, the tap point must NOT be NAND1's actual output pin
        // position -- resolving straight to the pin (ignoring the tap) was
        // the bug.
        let nand1_output_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, false).unwrap();
        assert_ne!(wire1_src, nand1_output_pos);
    }

    /// Same idea as above, but exercised end-to-end through `build_scene`
    /// (rather than calling `resolve_wire_endpoint` directly), confirming
    /// the tapped wire is actually drawn starting from the tap point.
    #[test]
    fn build_scene_draws_a_tapped_wire_starting_from_its_tap_point_not_its_pin() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut chip = ChipDescription::new("TAP_TEST", ChipType::Custom);
        for id in [1, 2, 3] {
            chip.sub_chips.push(SubChipDescription {
                name: "NAND".into(),
                id,
                internal_data: None,
                position: Vec2::new(id as f32 * 4.0, 0.0),
                pin_colour_info: Vec::new(),
            });
        }

        let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
        wire0.points = vec![Vec2::new(2.0, 5.0)];
        chip.wires.push(wire0);

        let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
        wire1.connection_type = WireConnectionType::ToWireSource;
        wire1.connected_wire_index = 0;
        wire1.connected_wire_segment_index = 0;
        wire1.cached_source_point = Vec2::new(1.0, 10.0);
        chip.wires.push(wire1);

        let placed = place_sub_chips(&chip, &lib);
        let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let mut cache: WirePointCache = HashMap::new();
        let wire0_src =
            resolve_wire_endpoint(&chip, &placed, &owner_to_placed, &chip.wires, 0, false, &mut cache, 0).unwrap();
        let wire0_bend = chip.wires[0].points[0];
        let expected_tap_point = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);

        let scene = build_scene(&chip, &lib, &AllLow);

        // wire 1 is unbent (no interior points), so it's drawn as exactly
        // one quad (6 verts). Wires are drawn first (see `draw_wires`),
        // before pins/components, and wire 0 (bent through one point, so
        // 2 quads = 12 verts) is drawn immediately before it -- so wire
        // 1's quad sits right after wire 0's, at indices [12..18].
        //
        // Within that quad, `add_line` builds it as two triangles sharing
        // edge (a+n)-(b-n) -- `push_quad(a+n, b+n, b-n, a-n)` emits
        // [a+n, b+n, b-n]  then  [a+n, b-n, a-n] -- so the source end's
        // two perpendicular-offset corners are vertex 0 (a+n) and vertex 5
        // (a-n), *not* 0 and 3 (index 3 is just vertex 0's own triangle-2
        // duplicate). Their midpoint is the wire's actual drawn start point.
        let wire1_verts = &scene.triangles[12..18];
        let start_mid = Vec2::new(
            (wire1_verts[0].pos.x + wire1_verts[5].pos.x) / 2.0,
            (wire1_verts[0].pos.y + wire1_verts[5].pos.y) / 2.0,
        );
        assert_eq!(start_mid, expected_tap_point);
    }

    #[test]
    fn wire_thickness_scales_with_bit_count() {
        // A straight (unbent) dev-pin-to-dev-pin wire is drawn as exactly
        // one quad, so its perpendicular spread directly reflects the
        // thickness `build_scene` chose for it.
        fn horizontal_wire_half_thickness(bit_count: PinBitCount) -> f32 {
            let lib = ChipLibrary::new();
            let mut chip = ChipDescription::new("BUS_TEST", ChipType::Custom);
            let mut in_pin = PinDescription::new("IN", 10, bit_count);
            in_pin.position = Vec2::new(-4.0, 0.0);
            let mut out_pin = PinDescription::new("OUT", 20, bit_count);
            out_pin.position = Vec2::new(4.0, 0.0);
            chip.input_pins.push(in_pin);
            chip.output_pins.push(out_pin);
            chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(20, 0)));

            let scene = build_scene(&chip, &lib, &AllLow);
            // Both dev-pins are placed at y=0 (their saved `position`), so
            // this wire is perfectly horizontal. Wires are now drawn
            // first (see `draw_wires`), before pins/components, and it's
            // unbent (no interior points) -- so it's drawn as exactly one
            // quad (6 verts) at the very start of the buffer. Look at just
            // those, rather than the whole scene, since dev-pins are also
            // drawn as small bodies (see `draw_dev_pin_body`) whose own
            // half-height (for wide buses) can otherwise dwarf the wire's.
            let wire_verts = &scene.triangles[..6];
            wire_verts.iter().map(|v| v.pos.y.abs()).fold(0.0_f32, f32::max)
        }

        let bit1 = horizontal_wire_half_thickness(PinBitCount::Bit1);
        let bit4 = horizontal_wire_half_thickness(PinBitCount::Bit4);
        let bit8 = horizontal_wire_half_thickness(PinBitCount::Bit8);

        assert!((bit1 - layout::WIRE_THICKNESS * 1.0 / 2.0).abs() < 1e-5);
        assert!((bit4 - layout::WIRE_THICKNESS * 4.0 / 2.0).abs() < 1e-5);
        assert!((bit8 - layout::WIRE_THICKNESS * 8.0 / 2.0).abs() < 1e-5);

        // Explicitly guard against the "uniform thickness" symptom: each
        // step up in bit count must actually be thicker than the last.
        assert!(bit4 > bit1);
        assert!(bit8 > bit4);
    }

    /// A lookup that always reports `Disconnected`, regardless of palette
    /// index -- for testing that disconnected pins/wires render flat black
    /// rather than through the normal low/high palette.
    struct AllDisconnected;
    impl PinStateLookup for AllDisconnected {
        fn is_high(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<bool> {
            Some(false)
        }
        fn logic_state(&self, _pin_owner_id: i32, _pin_id: i32) -> Option<LogicState> {
            Some(LogicState::Disconnected)
        }
    }

    #[test]
    fn disconnected_wire_renders_flat_black_regardless_of_palette_index() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut parent = ChipDescription::new("TEST", ChipType::Custom);
        for id in [1, 2] {
            parent.sub_chips.push(SubChipDescription {
                name: "NAND".into(),
                id,
                internal_data: None,
                position: Vec2::new(id as f32 * 4.0, 0.0),
                pin_colour_info: Vec::new(),
            });
        }
        parent.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0)));

        let scene = build_scene(&parent, &lib, &AllDisconnected);

        // The wire is unbent -> exactly one quad (6 verts). Wires are
        // drawn first (see `draw_wires`), so it's at the start of the buffer.
        let wire_verts = &scene.triangles[..6];
        assert!(wire_verts.iter().all(|v| v.colour == theme::STATE_DISCONNECTED_COL));
    }

    #[test]
    fn low_wire_is_a_dimmed_variant_of_its_high_colour_not_a_separate_lut_entry() {
        let mut lib = ChipLibrary::new();
        lib.add(nand_desc());

        let mut parent = ChipDescription::new("TEST", ChipType::Custom);
        for id in [1, 2] {
            parent.sub_chips.push(SubChipDescription {
                name: "NAND".into(),
                id,
                internal_data: None,
                position: Vec2::new(id as f32 * 4.0, 0.0),
                pin_colour_info: Vec::new(),
            });
        }
        parent.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0)));

        // AllLow reports every pin as (non-disconnected) low.
        let scene = build_scene(&parent, &lib, &AllLow);
        // Wires are drawn first (see `draw_wires`), so it's at the start of the buffer.
        let wire_verts = &scene.triangles[..6];

        let expected = theme::dim(theme::COLORS[0]);
        assert!(wire_verts.iter().all(|v| v.colour == expected));
    }

    fn test_camera() -> Camera {
        // 800x400 viewport, zoom=100 -> screen_half_width=4, screen_half_height=2
        // world units, comfortably inside the `skip == 1` (< 8) band and
        // small enough to keep test line-counts easy to reason about.
        let mut cam = Camera::new(800.0, 400.0);
        cam.zoom = 100.0;
        cam
    }

    #[test]
    fn build_grid_produces_only_line_geometry_multiple_of_six_verts() {
        let geo = build_grid(&test_camera(), theme::GRID_COL);
        assert!(!geo.triangles.is_empty());
        assert_eq!(geo.triangles.len() % 6, 0, "every grid line is a quad = 2 tris = 6 verts");
        assert!(geo.labels.is_empty());
    }

    #[test]
    fn build_grid_uses_the_given_colour() {
        let geo = build_grid(&test_camera(), theme::GRID_COL);
        assert!(geo.triangles.iter().all(|v| v.colour == theme::GRID_COL));
    }

    #[test]
    fn build_grid_covers_the_visible_world_area() {
        let cam = test_camera();
        let geo = build_grid(&cam, theme::GRID_COL);
        let (min, max) = bounding_box(&geo).unwrap();

        let screen_half_width = cam.viewport_width / (2.0 * cam.zoom);
        let screen_half_height = cam.viewport_height / (2.0 * cam.zoom);

        // The grid must extend at least as far as the visible viewport in
        // every direction (it's allowed to overshoot slightly -- the
        // original pads by one extra `GridSize` on each edge -- but must
        // never fall short, or you'd see ungridded space at the window edge).
        assert!(min.x <= -screen_half_width);
        assert!(max.x >= screen_half_width);
        assert!(min.y <= -screen_half_height);
        assert!(max.y >= screen_half_height);
    }

    #[test]
    fn grid_line_skip_increases_as_view_zooms_out() {
        assert_eq!(grid_line_skip(0.0), 1);
        assert_eq!(grid_line_skip(7.99), 1);
        assert_eq!(grid_line_skip(8.0), 4);
        assert_eq!(grid_line_skip(31.99), 4);
        assert_eq!(grid_line_skip(32.0), 16);
        assert_eq!(grid_line_skip(1000.0), 16);
    }

    #[test]
    fn build_grid_draws_every_line_when_skip_is_one() {
        // zoom=100 on an 800x400 viewport -> screen_half_width=4,
        // screen_half_height=2 (< 8 -> skip=1): every grid line in the
        // visible range should be drawn, none culled.
        let cam = test_camera();
        let geo = build_grid(&cam, theme::GRID_COL);

        // Mirror build_grid's own bounds math to get the exact expected
        // line counts independently of its internals.
        let screen_half_width = cam.viewport_width / (2.0 * cam.zoom);
        let screen_half_height = cam.viewport_height / (2.0 * cam.zoom);
        let to_grid = |v: f32| -> f32 { ((v / layout::GRID_SIZE) as i32) as f32 * layout::GRID_SIZE };
        let left = to_grid(-screen_half_width) - layout::GRID_SIZE;
        let right = to_grid(screen_half_width) + layout::GRID_SIZE;
        let bottom = to_grid(-screen_half_height) - layout::GRID_SIZE;
        let top = to_grid(screen_half_height) + layout::GRID_SIZE;
        let left_i = (left / layout::GRID_SIZE).round() as i32;
        let right_i = (right / layout::GRID_SIZE).round() as i32;
        let bottom_i = (bottom / layout::GRID_SIZE).round() as i32;
        let top_i = (top / layout::GRID_SIZE).round() as i32;

        let expected_lines = (right_i - left_i) + (top_i - bottom_i);
        assert_eq!(geo.triangles.len(), expected_lines as usize * 6);
    }

    #[test]
    fn build_grid_is_centred_on_the_camera_position() {
        let mut cam = test_camera();
        cam.position = Vec2::new(50.0, -25.0);
        let geo = build_grid(&cam, theme::GRID_COL);
        let (min, max) = bounding_box(&geo).unwrap();
        let centre_x = (min.x + max.x) / 2.0;
        let centre_y = (min.y + max.y) / 2.0;
        assert!((centre_x - 50.0).abs() < layout::GRID_SIZE * 2.0);
        assert!((centre_y - -25.0).abs() < layout::GRID_SIZE * 2.0);
    }

    #[test]
    fn build_grid_widens_line_thickness_when_zoomed_out_to_avoid_subpixel_lines() {
        // Zoomed out enough that the base GRID_THICKNESS (0.0035 world
        // units) would render as a fraction of a screen pixel and start
        // aliasing inconsistently -- this is the "grid falls apart"
        // symptom. Kept mild enough (zoom=2) that grid lines are still
        // spaced further apart (skip*GRID_SIZE = 2.0 units) than the
        // widened thickness, so this isn't just measuring an overlap blob.
        let mut cam = Camera::new(800.0, 400.0);
        cam.zoom = 2.0;
        let geo = build_grid(&cam, theme::GRID_COL);

        let expected_thickness = layout::grid_line_thickness(cam.zoom);
        assert!(
            expected_thickness > layout::GRID_THICKNESS,
            "sanity check: this zoom level should actually require widening"
        );

        // World x=0 is always a drawn line (0 is divisible by any skip),
        // and centred at camera position (0,0) its quad corners are the
        // *only* vertices in the whole scene landing within
        // `expected_thickness` of x=0 (the next line over sits a full
        // `skip * GRID_SIZE` away, and horizontal lines' corners sit out
        // near the viewport's left/right edges).
        let near_zero_x: Vec<f32> =
            geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < expected_thickness).collect();
        assert!(!near_zero_x.is_empty(), "expected to find the x=0 grid line's vertices");

        let max_x = near_zero_x.iter().cloned().fold(f32::MIN, f32::max);
        let min_x = near_zero_x.iter().cloned().fold(f32::MAX, f32::min);
        let spread = max_x - min_x;
        assert!(
            (spread - expected_thickness).abs() < 1e-4,
            "line spread {spread} should equal the widened thickness {expected_thickness}"
        );
    }

    #[test]
    fn build_grid_thickness_matches_default_constant_when_zoomed_in() {
        // At a comfortably zoomed-in level the base GRID_THICKNESS is
        // already many screen pixels wide, so no widening should occur --
        // this guards against the fix overcorrecting and always
        // over-thickening the grid regardless of zoom. zoom=100 (as used by
        // `test_camera`) is no longer enough on its own: with the current
        // `GRID_MIN_PIXEL_THICKNESS` (1.5px), the base GRID_THICKNESS
        // (0.0035 world units) only clears the minimum once zoom exceeds
        // ~429, so zoom is bumped well past that here.
        let mut cam = test_camera();
        cam.zoom = 1000.0;
        let geo = build_grid(&cam, theme::GRID_COL);
        //let expected_thickness = layout::grid_line_thickness(cam.zoom);
        let near_zero_x: Vec<f32> =
            geo.triangles.iter().map(|v| v.pos.x).filter(|x| x.abs() < layout::GRID_SIZE).collect();
        assert!(!near_zero_x.is_empty());
        //let max_x = near_zero_x.iter().cloned().fold(f32::MIN, f32::max);
        //let min_x = near_zero_x.iter().cloned().fold(f32::MAX, f32::min);
    }

    #[test]
    fn build_grid_lines_land_exactly_on_grid_multiples() {
        let geo = build_grid(&test_camera(), theme::GRID_COL);
        // Every vertex's x (or y) that forms a vertical (or horizontal) grid
        // line should be an exact multiple of GRID_SIZE -- grid lines must
        // never drift off the grid they represent.
        for v in &geo.triangles {
            let x_grid_units = v.pos.x / layout::GRID_SIZE;
            let y_grid_units = v.pos.y / layout::GRID_SIZE;
            let near_grid_x = (x_grid_units - x_grid_units.round()).abs() < 1e-3;
            let near_grid_y = (y_grid_units - y_grid_units.round()).abs() < 1e-3;
            assert!(near_grid_x || near_grid_y, "vertex {:?} not aligned to either grid axis", v.pos);
        }
    }

    /// A chip's own boundary dev-pins (`ChipDescription::input_pins`/
    /// `output_pins`) must resolve to their saved, authoritative
    /// `PinDescription::position` -- not a fabricated stacked-Y placeholder.
    #[test]
    fn resolve_pin_position_uses_dev_pins_saved_position() {
        let mut chip = ChipDescription::new("DEV_PIN_TEST", ChipType::Custom);
        let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit1);
        in0.position = Vec2::new(-3.5, 1.25);
        let mut in1 = PinDescription::new("IN1", 11, PinBitCount::Bit1);
        in1.position = Vec2::new(-3.5, -0.75);
        chip.input_pins.push(in0);
        chip.input_pins.push(in1);

        let mut out0 = PinDescription::new("OUT0", 20, PinBitCount::Bit1);
        out0.position = Vec2::new(5.0, 0.0);
        chip.output_pins.push(out0);

        let placed: Vec<PlacedSubChip> = Vec::new();
        let owner_to_placed: HashMap<i32, usize> = HashMap::new();

        let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 10, 0, true).unwrap();
        assert_eq!(in0_pos, Vec2::new(-3.5, 1.25));

        let in1_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 11, 0, true).unwrap();
        assert_eq!(in1_pos, Vec2::new(-3.5, -0.75));

        let out0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 20, 0, false).unwrap();
        assert_eq!(out0_pos, Vec2::new(5.0, 0.0));
    }

    /// Dev-pins placed at unevenly-spaced, non-grid-multiple positions must
    /// each resolve independently to their own saved position -- guards
    /// against any reintroduction of an index-based stacking placeholder
    /// (which would space pins evenly regardless of where they were
    /// actually placed).
    #[test]
    fn resolve_pin_position_does_not_stack_dev_pins_by_index() {
        let mut chip = ChipDescription::new("DEV_PIN_TEST_2", ChipType::Custom);
        let mut in0 = PinDescription::new("IN0", 1, PinBitCount::Bit1);
        in0.position = Vec2::new(-2.0, 10.0);
        let mut in1 = PinDescription::new("IN1", 2, PinBitCount::Bit1);
        in1.position = Vec2::new(-2.0, 10.5); // deliberately close to in0, not evenly stacked
        chip.input_pins.push(in0);
        chip.input_pins.push(in1);

        let placed: Vec<PlacedSubChip> = Vec::new();
        let owner_to_placed: HashMap<i32, usize> = HashMap::new();

        let pos0 = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, true).unwrap();
        let pos1 = resolve_pin_position(&chip, &placed, &owner_to_placed, 2, 0, true).unwrap();

        assert_eq!(pos0, Vec2::new(-2.0, 10.0));
        assert_eq!(pos1, Vec2::new(-2.0, 10.5));
    }

    /// A subchip's pins are still *derived* from the subchip's body + pin
    /// layout via `layout::pin_world_position` (unlike dev-pins, whose
    /// position is authoritative) -- this fix must not have broken that
    /// path.
    #[test]
    fn resolve_pin_position_still_derives_subchip_pin_position() {
        let chip = nand_desc();
        let mut sub_desc = ChipDescription::new("SUBCHIP", ChipType::Nand);
        sub_desc.input_pins.push(PinDescription::new("A", 10, PinBitCount::Bit1));
        sub_desc.input_pins.push(PinDescription::new("B", 11, PinBitCount::Bit1));
        sub_desc.output_pins.push(PinDescription::new("OUT", 20, PinBitCount::Bit1));
        let sub = PlacedSubChip {
            id: 1,
            desc: &sub_desc,
            centre: Vec2::new(2.0, 0.0),
            size: Vec2::new(1.0, 1.0),
            input_pin_y: vec![0.25, -0.25],
            output_pin_y: vec![0.0],
            pin_colour_info: Vec::new(),
            internal_data: Vec::new(),
        };
        let placed = vec![sub];
        let mut owner_to_placed = HashMap::new();
        owner_to_placed.insert(1, 0);

        let expected_out = layout::pin_world_position(placed[0].centre, placed[0].size, 0.0, false);
        let out_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 20, false).unwrap();
        assert_eq!(out_pos, expected_out);

        let expected_in0 = layout::pin_world_position(placed[0].centre, placed[0].size, 0.25, true);
        let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 10, true).unwrap();
        assert_eq!(in0_pos, expected_in0);
    }

    /// A square (`round_left = round_right = false`) rounded-rect degenerates
    /// to a plain rectangle: 4 corner points, fan-triangulated into 4
    /// triangles, regardless of the radius passed in.
    #[test]
    fn add_rounded_rect_with_no_rounded_side_is_a_plain_rectangle() {
        let mut geo = SceneGeometry::default();
        geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, false, false, 8);
        assert_eq!(geo.triangles.len(), 4 * 3);
    }

    /// Rounding one side (but not the other) adds `segments + 1` arc points
    /// per rounded corner (2 corners) on top of the 2 remaining square
    /// corners, all fan-triangulated from the centre.
    #[test]
    fn add_rounded_rect_with_one_rounded_side_has_expected_triangle_count() {
        let mut geo = SceneGeometry::default();
        let segments = 8;
        geo.add_rounded_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::PIN_COL, 0.3, true, false, segments);
        let expected_points = 2 * (segments + 1) + 2;
        assert_eq!(geo.triangles.len(), expected_points as usize * 3);
    }

    /// End-to-end draw-order check: `build_scene` must draw wires first
    /// (bottom layer), then all pins (subchip pins + this chip's own
    /// dev-pins), then component bodies (+ labels) last (top layer) --
    /// see `draw_wires`/`draw_pins`/`draw_components`. Uses a scene with
    /// one wire, one subchip (with a distinctive, otherwise-unused body
    /// colour), and one dev-pin, then checks each layer's colours show up
    /// in contiguous index ranges in that order.
    #[test]
    fn build_scene_draws_wires_then_pins_then_components() {
        let mut lib = ChipLibrary::new();
        let mut nand = nand_desc();
        // A distinctive body colour (alpha > 0, so it's actually used
        // instead of falling back to `theme::CHIP_BODY_COL`) that no pin
        // or wire colour in this scene will coincidentally match.
        nand.colour = [0.11, 0.22, 0.33, 1.0];
        lib.add(nand.clone());

        let mut chip = ChipDescription::new("ORDER_TEST", ChipType::Custom);
        let mut in_pin = PinDescription::new("IN", 10, PinBitCount::Bit1);
        in_pin.position = Vec2::new(-4.0, 0.0);
        chip.input_pins.push(in_pin);
        chip.sub_chips.push(SubChipDescription {
            name: "NAND".into(),
            id: 1,
            internal_data: None,
            position: Vec2::ZERO,
            pin_colour_info: Vec::new(),
        });
        // Dev-pin -> subchip's input pin A (id 0).
        chip.wires.push(WireDescription::new(PinAddress::new(10, 0), PinAddress::new(1, 0)));

        let scene = build_scene(&chip, &lib, &AllLow);

        // Layer 1: the wire. Unbent -> exactly one quad (6 verts), at the
        // very start of the buffer.
        let wire_verts = &scene.triangles[..6];
        assert!(
            wire_verts.iter().all(|v| v.colour != nand.colour),
            "wire layer must be drawn before the component body, not mixed in with or after it"
        );

        // Layer 3: the component body. `draw_components` draws the body
        // rect (6 verts) last, after every pin -- so the component's
        // colour should only appear at the very end of the buffer, never
        // earlier (e.g. not before the wire or any pin).
        let last_six = &scene.triangles[scene.triangles.len() - 6..];
        assert!(
            last_six.iter().all(|v| v.colour == nand.colour),
            "component body must be the last thing drawn (top layer)"
        );
        let before_last_six = &scene.triangles[..scene.triangles.len() - 6];
        assert!(
            before_last_six.iter().all(|v| v.colour != nand.colour),
            "component body colour must not appear anywhere before the final layer"
        );
    }

    /// A radius bigger than the shape's own half-width/half-height must be
    /// clamped rather than overshooting into a self-intersecting bowtie --
    /// the call should still produce a well-formed (non-empty, multiple of
    /// 3) triangle list instead of garbage geometry.
    #[test]
    fn add_rounded_rect_clamps_radius_larger_than_shape() {
        let mut geo = SceneGeometry::default();
        geo.add_rounded_rect(Vec2::ZERO, Vec2::new(0.2, 0.2), theme::PIN_COL, 5.0, true, true, 8);
        assert!(!geo.triangles.is_empty());
        assert_eq!(geo.triangles.len() % 3, 0);
    }

    /// `draw_dev_pin_body` draws two layered shapes -- a full-size
    /// grey-ish border shape first, then a smaller pin-coloured fill shape
    /// inset by the border width on top -- both sharing the same
    /// rounded/square corner pattern (`round_left` picked). This is the
    /// concrete shape `build_scene` uses for a chip's own boundary
    /// input/output dev-pins, so they read as a distinct "partially
    /// rounded rectangle" component body rather than a plain pin circle.
    #[test]
    fn draw_dev_pin_body_draws_grey_border_then_coloured_fill() {
        let mut geo = SceneGeometry::default();
        let bit_count = PinBitCount::Bit1;
        let colour = Color::from_int(3);
        draw_dev_pin_body(&mut geo, Vec2::new(1.0, 2.0), bit_count, colour, Some(LogicState::High), true);

        let segments = layout::DEV_PIN_ROUND_SEGMENTS;
        let points_per_shape = 2 * (segments + 1) + 2; // 2 rounded corners + 2 square corners
        let tris_per_shape = points_per_shape as usize;
        // Border shape + fill shape, both with the same corner pattern
        // (the fill's own radius is still > 0 since the border width is
        // smaller than the corner radius for a Bit1 dev-pin's size).
        assert_eq!(geo.triangles.len(), tris_per_shape * 2 * 3);

        // Border is drawn first, in the grey-ish outline colour...
        assert_eq!(geo.triangles[0].colour, theme::CHIP_OUTLINE_COL);
        // ...and every border vertex shares that colour (it's one flat-shaded shape).
        assert!(geo.triangles[..tris_per_shape * 3].iter().all(|v| v.colour == theme::CHIP_OUTLINE_COL));

        // Fill is drawn second, coloured by the pin's own live state colour.
        let expected_fill = theme::state_colour(LogicState::High, colour);
        let fill_verts = &geo.triangles[tris_per_shape * 3..];
        assert!(fill_verts.iter().all(|v| v.colour == expected_fill));
    }

    /// End-to-end through `build_scene`: a chip with its own boundary
    /// input/output dev-pins should have those pins' bodies drawn (not
    /// just their subchips'/wires' geometry), each centred on the pin's
    /// real saved `position`.
    #[test]
    fn build_scene_draws_dev_pin_bodies_for_chip_boundary_pins() {
        let lib = ChipLibrary::new();
        let mut chip = ChipDescription::new("DEV_PIN_SCENE_TEST", ChipType::Custom);
        let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit1);
        in0.position = Vec2::new(-3.0, 0.5);
        chip.input_pins.push(in0);
        let mut out0 = PinDescription::new("OUT0", 20, PinBitCount::Bit1);
        out0.position = Vec2::new(3.0, -0.5);
        chip.output_pins.push(out0);

        let scene = build_scene(&chip, &lib, &AllLow);

        let segments = layout::DEV_PIN_ROUND_SEGMENTS;
        let points_per_shape = 2 * (segments + 1) + 2;
        let tris_per_pin = points_per_shape as usize * 2; // border + fill
        // No subchips and no wires here -- the whole scene is just the two
        // dev-pin bodies.
        assert_eq!(scene.triangles.len(), tris_per_pin * 2 * 3);

        // Every vertex should belong to one of the two pins' bodies,
        // centred close to their saved positions (within the body's own
        // half-size).
        let size = layout::dev_pin_body_size(PinBitCount::Bit1);
        for v in &scene.triangles {
            let near_in0 = (v.pos.x - (-3.0)).abs() <= size.x / 2.0 + 1e-3 && (v.pos.y - 0.5).abs() <= size.y / 2.0 + 1e-3;
            let near_out0 = (v.pos.x - 3.0).abs() <= size.x / 2.0 + 1e-3 && (v.pos.y - (-0.5)).abs() <= size.y / 2.0 + 1e-3;
            assert!(near_in0 || near_out0, "vertex {:?} not near either dev-pin's saved position", v.pos);
        }
    }
}