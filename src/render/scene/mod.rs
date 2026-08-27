//! Builds drawable geometry for one "view" of a chip (i.e. what the editor shows when you open a
//! custom chip: its subchips, each subchip's pins, and the wires between them). This is the
//! scene-graph half of the renderer -- pure data in, triangles out, no wgpu types -- so it can be
//! unit tested without a GPU. Mirrors (a first-pass subset of) `DLS.Graphics.World.DevSceneDrawer`.
//!
//! Split by concern: [`lookup`] (pin-state queries), [`placed`] (subchip placement), [`wires`]
//! (wire drawing/hit-testing/deletion), [`pins`] (pin drawing/hit-testing), [`components`]
//! (component bodies + displays), and [`grid`] (the canvas background), all composed here by
//! [`build_scene`] on top of [`crate::render::foundation`]'s primitives.

pub mod components;
pub mod displays;
pub mod grid;
pub mod lookup;
pub mod pin_hits;
pub mod pin_resolve;
pub mod pins;
pub mod placed;
pub mod wire_endpoints;
pub mod wires;

use crate::description::{ChipDescription, ChipLibrary};
use crate::render::layout;
use crate::render::theme;
use crate::structs::Vec2;
use std::collections::HashMap;

pub use crate::render::foundation::{
	apply_alpha, bounding_box, point_in_circle, point_in_rect, point_in_rounded_rect, RoundCorners, SceneGeometry, SceneVertex, TextLabel,
};
pub use displays::{display_base_size, is_display_type};
pub use grid::build_grid;
pub use lookup::{AllLow, PinStateLookup, SimulatorPinState};
pub use pin_hits::{hit_test_any_pin, hit_test_dev_pin, hit_test_input_dev_pin_bit, hit_test_sub_chip_pin, PinHit};
pub use placed::{place_sub_chips, PlacedSubChip};
pub use wire_endpoints::{closest_wire_hit, hit_test_wire, WireTapHit};
pub use wires::{delete_wire, delete_wire_old, delete_wire_segment};

/// Finds whichever placed subchip's body (as laid out by
/// [`place_sub_chips`]) contains `world_pos`, if any -- used to resolve a
/// right-click on the canvas to "which component did the player click".
/// Iterates back-to-front (last-placed first) so, on the rare case two
/// bodies overlap, the one actually drawn on top (and thus visible to the
/// player) is the one that gets hit, matching `components::draw_component`'s
/// draw order.
pub fn hit_test_sub_chip<'a, 'b>(placed: &'b [PlacedSubChip<'a>], world_pos: Vec2) -> Option<&'b PlacedSubChip<'a>> {
	placed.iter().rev().find(|p| point_in_rect(world_pos, p.centre, p.size))
}

/// Build the full drawable scene for one chip: every subchip's body + pins,
/// plus wires connecting them. `chip.input_pins`/`output_pins` are treated
/// as this chip's own boundary dev-pins (owner id == the pin's own id, per
/// the on-disk wire-address convention).
pub fn build_scene(chip: &ChipDescription, library: &ChipLibrary, pin_state: &dyn PinStateLookup, hover_world_pos: Option<Vec2>) -> SceneGeometry {
	build_scene_with_spans(chip, library, pin_state, hover_world_pos, true).0
}

/// Vertex-index span of one placed subchip's own geometry (its body, name
/// label, and any embedded displays) inside the [`SceneGeometry`] returned
/// by [`build_scene_with_spans`] -- what lets a caller fade exactly that
/// component (dragging draws carried components translucently) without
/// touching its pins or wires.
#[derive(Debug, Clone)]
pub struct ComponentSpan {
	pub triangles: std::ops::Range<usize>,
	pub labels: std::ops::Range<usize>,
}

/// Per-subchip-id map of [`ComponentSpan`]s.
#[derive(Debug, Default, Clone)]
pub struct ComponentSpans {
	spans: HashMap<i32, ComponentSpan>,
}

impl ComponentSpans {
	pub fn get(&self, subchip_id: i32) -> Option<&ComponentSpan> {
		self.spans.get(&subchip_id)
	}

	fn insert(&mut self, subchip_id: i32, span: ComponentSpan) {
		self.spans.insert(subchip_id, span);
	}
}

/// Multiplies every vertex/label alpha of `span`'s slice of `geo` by
/// `alpha` -- the per-component counterpart of
/// [`crate::render::foundation::apply_alpha`].
pub fn fade_component(geo: &mut SceneGeometry, span: &ComponentSpan, alpha: f32) {
	for v in &mut geo.triangles[span.triangles.clone()] {
		v.colour[3] *= alpha;
	}
	for l in &mut geo.labels[span.labels.clone()] {
		l.colour[3] *= alpha;
	}
}

/// Vertex-index span of one wire's own strand geometry inside the
/// [`SceneGeometry`] returned by [`build_scene_with_spans`], plus the
/// `pin_owner_id`s of both its endpoints -- lets a caller fade exactly the
/// wires that run *between* a set of carried components (a drag or a
/// duplicate-then-drag), matching [`ComponentSpan`]'s ghost fade instead of
/// stretching along at full strength.
#[derive(Debug, Clone)]
pub struct WireEndpoints {
	pub triangles: std::ops::Range<usize>,
	pub owners: (i32, i32),
}

/// Per-wire-index map of [`WireEndpoints`].
#[derive(Debug, Default, Clone)]
pub struct WireSpans {
	spans: HashMap<usize, WireEndpoints>,
}

impl WireSpans {
	fn insert(&mut self, wire_idx: usize, span: WireEndpoints) {
		self.spans.insert(wire_idx, span);
	}

	/// Every wire whose *both* endpoints belong to `owner_ids` -- i.e. a
	/// wire fully internal to a set of carried components, which should
	/// fade with them rather than stay opaque while it stretches.
	pub fn fully_within<'a>(&'a self, owner_ids: &'a std::collections::HashSet<i32>) -> impl Iterator<Item = &'a WireEndpoints> {
		self.spans.values().filter(move |w| owner_ids.contains(&w.owners.0) && owner_ids.contains(&w.owners.1))
	}
}

/// Multiplies every vertex alpha of `span`'s triangle slice of `geo` by
/// `alpha` -- the wire counterpart of [`fade_component`] (wires have no
/// labels of their own).
pub fn fade_wire(geo: &mut SceneGeometry, span: &WireEndpoints, alpha: f32) {
	for v in &mut geo.triangles[span.triangles.clone()] {
		v.colour[3] *= alpha;
	}
}

/// [`build_scene`] plus a per-component index of where each placed
/// subchip's own geometry landed in the returned buffers (see
/// [`ComponentSpan`]). Wires/pins/dev-pins stay untracked -- they're never
/// faded with a dragged component.
pub fn build_scene_with_spans(
	chip: &ChipDescription,
	library: &ChipLibrary,
	pin_state: &dyn PinStateLookup,
	hover_world_pos: Option<Vec2>,
	labels_visible: bool,
) -> (SceneGeometry, ComponentSpans, WireSpans) {
	let mut geo = SceneGeometry::default();
	let placed = place_sub_chips(chip, library);

	// owner_id -> index into `placed`, for resolving wire endpoints that
	// land on a subchip (as opposed to one of this chip's own dev-pins).
	let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

	// Draw order is a simple four-layer stack, back to front (no depth buffer, so draw order is z-order):
	// wires, then pins, then component bodies + name labels on top, then any
	// display surfaces those components embed inside their own bodies (the
	// "customize" feature -- a display must cover its host's body, never the
	// other way around). Name labels are hover-gated to whichever thing
	// `hover_world_pos` lands on; pins are checked first so an edge-hover
	// shows the pin. Components and their displays draw interleaved (rather
	// than as two whole layers) purely so each component's triangles land in
	// one contiguous span.
	let wire_spans = wires::draw_wires(&mut geo, chip, &placed, &owner_to_placed, pin_state);
	let effective_hover = if labels_visible { hover_world_pos } else { None };
	let hovered_pin_name = pins::draw_pins(&mut geo, chip, &placed, pin_state, effective_hover);
	let mut spans = ComponentSpans::default();
	for sub in &placed {
		let triangle_start = geo.triangles.len();
		let label_start = geo.labels.len();
		components::draw_component(&mut geo, sub, pin_state, effective_hover, hovered_pin_name.is_some());
		displays::draw_placed_displays_for(&mut geo, sub, library, pin_state);
		spans.insert(sub.id, ComponentSpan { triangles: triangle_start..geo.triangles.len(), labels: label_start..geo.labels.len() });
	}
	if let Some((pos, name)) = hovered_pin_name {
		push_hover_label(&mut geo, pos, name);
	}

	(geo, spans, wire_spans)
}

/// Pushes a small hover-triggered name label just above `pos`. Shared by
/// both the pin and component hover paths in `build_scene` so their
/// labels look consistent.
fn push_hover_label(geo: &mut SceneGeometry, pos: Vec2, name: String) {
	let width = layout::estimate_text_width(&name, theme::FONT_SIZE_CHIP_NAME);
	geo.labels.push(TextLabel {
		pos: Vec2::new(pos.x, pos.y + layout::GRID_SIZE * 2.0),
		text: name,
		colour: theme::HOVER_LABEL_COL,
		font_size: theme::FONT_SIZE_CHIP_NAME,
		width,
	});
}

#[cfg(test)]
pub(crate) mod test_support {
	//! Tiny chip fixtures shared by the scene submodules' unit tests.

	use crate::description::{ChipDescription, ChipType, PinBitCount, PinDescription};

	pub fn nand_desc() -> ChipDescription {
		let mut d = ChipDescription::new("NAND", ChipType::Nand);
		d.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
		d.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
		d.output_pins.push(PinDescription::new("OUT", 0, PinBitCount::Bit1));
		d
	}
}

#[cfg(test)]
mod span_tests {
	//! White-box: the per-component spans `build_scene_with_spans` records
	//! are what the viewer fades a dragged component by -- they must cover
	//! exactly that component's own vertices and nothing else.

	use super::*;
	use crate::description::{PinAddress, SubChipDescription, WireDescription};
	use crate::render::scene::test_support::nand_desc;

	fn two_nands_and_a_wire() -> (ChipLibrary, ChipDescription) {
		let mut library = ChipLibrary::new();
		library.add(nand_desc());
		let mut chip = ChipDescription::new("SPANS", crate::description::ChipType::Custom);
		for id in [1, 2] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				label: None,
				pin_colour_info: Vec::new(),
			});
		}
		chip.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 1)));
		(library, chip)
	}

	#[test]
	fn build_scene_with_spans_tracks_each_components_own_vertices() {
		let (library, chip) = two_nands_and_a_wire();

		let (scene, spans, _wire_spans) = build_scene_with_spans(&chip, &library, &AllLow, None, true);
		let span1 = spans.get(1).expect("component 1 is indexed");
		let span2 = spans.get(2).expect("component 2 is indexed");

		assert!(!span1.triangles.is_empty() && !span1.labels.is_empty(), "a body rect + name label were drawn");
		assert!(span1.triangles.end <= span2.triangles.start, "spans are disjoint and in draw order");

		// Fading one component's span touches exactly its slice: the wire
		// layer drawn before every span, and the other component's span,
		// both stay at full alpha.
		let full_alpha = scene.triangles[0].colour[3];
		assert_eq!(full_alpha, 1.0);

		let mut faded = scene.clone();
		fade_component(&mut faded, span1, 0.5);
		assert!((faded.triangles[span1.triangles.start].colour[3] - 0.5).abs() < 1e-6, "the span itself fades");
		assert_eq!(faded.triangles[span2.triangles.start].colour[3], full_alpha, "other components don't");
		assert_eq!(faded.triangles[0].colour[3], full_alpha, "wires drawn beneath every span don't");
		assert!((faded.labels[span1.labels.start].colour[3] - 0.5).abs() < 1e-6, "labels fade with their component");
	}
}
