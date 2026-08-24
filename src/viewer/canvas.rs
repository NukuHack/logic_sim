//! Canvas interaction: what a click on the chip-editing surface means --
//! starting/continuing wire placements, dropping a pending chip
//! placement, toggling an input dev-pin's bits -- plus the scene-space
//! previews (in-progress wire, placement ghost) those interactions draw.

use crate::description::ChipDescription;
use crate::pin_state;
use crate::render::camera::Camera;
use crate::render::layout;
use crate::render::scene::{self, SceneGeometry};
use crate::render::theme;
use crate::structs::Vec2;
use crate::viewer::bus_wiring;
use crate::viewer::state::ViewerState;
use crate::viewer::wire_draft::{PendingWire, PendingWireEnd};
use crate::{builtins, ChipLibrary, ChipType, PinAddress, PinDescription, SubChipDescription, WireDescription};

/// Finds whichever bit of one of `root_desc`'s own boundary *input*
/// dev-pins (if any) `world_pos` landed on -- the same per-bit grid
/// `scene::pins::draw_input_dev_pin_body` draws for each input pin
/// (one clickable circle for a 1-bit input, a 2x2/2x4 grid of cells for
/// 4/8-bit) -- returning that pin's own id and the clicked bit's index.
/// Output pins are never hit -- only inputs are meant to be toggled by a
/// click.
pub(crate) fn hit_test_root_input_pin_click(root_desc: &ChipDescription, world_pos: Vec2) -> Option<(i32, u32)> {
	for pin in &root_desc.input_pins {
		if let Some(bit_index) = scene::hit_test_input_dev_pin_bit(world_pos, pin.position, pin.bit_count) {
			return Some((pin.id, bit_index));
		}
	}
	None
}

/// Flips one bit (`bit_index`) of input dev-pin `pin_id`'s own
/// `PinDescription::driven_state`, directly on its entry in `library` --
/// see that field's docs for why it lives there rather than in a
/// separate lookup on [`ViewerState`]. The tristate flags half of the
/// packed state is left untouched (stays "driven", i.e. `0`) -- a
/// clicked input is always actively driven, never floating.
fn toggle_driven_input_bit(library: &mut ChipLibrary, root_chip_name: &str, pin_id: i32, bit_index: u32) {
	let chip = library.get_mut(root_chip_name);
	if let Some(pin) = chip.input_pins.iter_mut().find(|p| p.id == pin_id) {
		let last_state = pin.driven_state;
		let mut bits = pin_state::bit_states(last_state);
		bits ^= 1 << bit_index;
		pin_state::set(&mut pin.driven_state, bits, pin_state::tristate_flags(last_state));
	}
}

/// Fixed screen-pixel tolerance for landing a click on a wire, converted
/// to world units -- same value/reasoning as the one right-click wire
/// deletion uses, so a tap-to-place click feels exactly as forgiving as
/// a click-to-delete one at any zoom level.
pub(crate) fn wire_click_tolerance(camera: &Camera) -> f32 {
	6.0 / camera.zoom.max(0.0001)
}

/// Attempts to start a new wire placement from whatever's under
/// `world_pos`: a subchip's own pin, one of the current chip's own
/// boundary *output* dev-pins, or a tap point along an existing wire's
/// line. Returns whether a placement was actually started (i.e. whether
/// the click should be treated as consumed).
fn try_start_pending_wire(v: &mut ViewerState, world_pos: Vec2) -> bool {
	let root_desc = v.library.get(&v.root_chip_name);
	let placed = scene::place_sub_chips(root_desc, &v.library);

	if let Some(hit) = scene::hit_test_any_pin(root_desc, &placed, world_pos) {
		v.pending_wire = Some(PendingWire {
			start: PendingWireEnd::Pin { owner_id: hit.owner_id, pin_id: hit.pin_id, is_source: hit.is_wire_source(), position: hit.position },
			bend_points: Vec::new(),
			bit_count: hit.bit_count,
		});
		return true;
	}

	let max_dist = wire_click_tolerance(&v.camera);
	if let Some(tap) = scene::closest_wire_hit(root_desc, &v.library, world_pos, max_dist) {
		let source_pin_address = root_desc.wires[tap.wire_index].source_pin_address;
		v.pending_wire = Some(PendingWire {
			start: PendingWireEnd::WireTap { wire_index: tap.wire_index, segment_index: tap.segment_index, point: tap.point, source_pin_address },
			bend_points: Vec::new(),
			bit_count: tap.bit_count,
		});
		return true;
	}

	false
}

/// Advances an in-progress wire placement (`v.pending_wire`, assumed
/// `Some`) with a click at `world_pos`:
///  - landing on a pin of the *opposite* role (see `PinHit::is_wire_source`/
///    `PendingWireEnd::is_source`) completes the wire, connecting through
///    any bend points collected so far -- except that two bus chips may only
///    join when they're a linked pair;
///  - landing on a pin of the *same* role (e.g. input-to-input,
///    output-to-output) is rejected with a status message, leaving the
///    placement active so the player can just try a different pin;
///  - landing on an existing wire *completes into it* ("wiring into the
///    wire"): inputs may tap into any wire, outputs only into bus wires,
///    and the electrical endpoints resolve from the tapped wire
///    (bus-corrected on the target side) -- see `viewer::bus_wiring`;
///  - landing on a component body is ignored outright (deliberately *not*
///    a "turn" -- see this method's caller's doc comment on the
///    empty-space branch below);
///  - anywhere else (empty canvas) adds a bend ("turn") point there and
///    leaves the placement active.
fn try_continue_pending_wire(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	let root_chip_name = v.root_chip_name.clone();
	let root_desc = v.library.get(&root_chip_name);
	let placed = scene::place_sub_chips(root_desc, &v.library);

	if let Some(hit) = scene::hit_test_any_pin(root_desc, &placed, world_pos) {
		let pending = v.pending_wire.as_ref().expect("caller only calls this with a pending wire");
		/*		// optional : if you want to only connect same bitcount wires
			   if pending.bit_count != hit.bit_count {
				   *status = Some("Can't connect different bitcounts".to_string());
				   return;
			   }
		*/
		if pending.start.is_source() == hit.is_wire_source() {
			*status = Some(if hit.is_wire_source() {
				"Can't connect an output to an output".to_string()
			} else {
				"Can't connect an input to an input".to_string()
			});
			return;
		}

		// A wire between two bus chips (origin output -> terminus input) is
		// only allowed between a *linked* pair -- `CanCompleteWireConnection`'s
		// `LinkedBusPairID` check.
		if let PendingWireEnd::Pin { owner_id, .. } = pending.start {
			let start_type = bus_wiring::owner_chip_type(root_desc, &v.library, owner_id);
			let end_type = bus_wiring::owner_chip_type(root_desc, &v.library, hit.owner_id);
			let both_bus = start_type.is_some_and(|t| t.is_bus_type()) && end_type.is_some_and(|t| t.is_bus_type());
			if both_bus && !bus_wiring::bus_pair_linked(root_desc, &v.library, owner_id, hit.owner_id) {
				*status = Some("Bus chips can only be wired to their linked partner".to_string());
				return;
			}
		}

		let pending = v.pending_wire.take().expect("checked above");
		let end_pin_address = PinAddress::new(hit.owner_id, hit.pin_id);

		let mut wire = if pending.start.is_source() {
			match pending.start {
				PendingWireEnd::Pin { owner_id, pin_id, .. } => WireDescription::new(PinAddress::new(owner_id, pin_id), end_pin_address),
				PendingWireEnd::WireTap { wire_index, segment_index, point, source_pin_address } => {
					WireDescription::new_tapped_source(source_pin_address, end_pin_address, wire_index as i32, segment_index, point)
				}
			}
		} else {
			// The clicked pin is the real source; the wire always started from a plain pin in this
			// branch (a wire tap is always treated as the source -- see `PendingWireEnd::is_source`).
			let PendingWireEnd::Pin { owner_id, pin_id, .. } = pending.start else {
				unreachable!("a wire tap is always the source end, so this branch never sees one")
			};
			WireDescription::new(end_pin_address, PinAddress::new(owner_id, pin_id))
		};

		wire.points = pending.bend_points;
		if !pending.start.is_source() {
			wire.points.reverse();
		}

		v.library.get_mut(&root_chip_name).wires.push(wire);
		v.rebuild_sim();
		*status = None;
		return;
	}

	let max_dist = wire_click_tolerance(&v.camera);
	let on_component = scene::hit_test_sub_chip(&placed, world_pos).is_some();

	// Completing ONTO an existing wire ("wiring into the wire"). Only wires
	// started from a real pin get here: a wire-tap start completing onto
	// another wire is rejected inside `resolve_completion_on_wire`.
	if !on_component && v.pending_wire.as_ref().is_some_and(|p| matches!(p.start, PendingWireEnd::Pin { .. })) {
		let pending = v.pending_wire.as_ref().expect("checked above");
		let PendingWireEnd::Pin { owner_id, pin_id, .. } = pending.start else { unreachable!("branch guarantees a pin start") };
		if let Some(tap) = scene::closest_wire_hit(root_desc, &v.library, world_pos, max_dist) {
			match bus_wiring::resolve_completion_on_wire(
				root_desc,
				&v.library,
				tap.wire_index,
				matches!(pending.start, PendingWireEnd::WireTap { .. }),
				pending.start.is_source(),
				owner_id,
				pin_id,
			) {
				Ok((source, target)) => {
					let pending = v.pending_wire.take().expect("checked above");
					let mut wire = WireDescription::new_tapped_target(source, target, tap.wire_index as i32, tap.segment_index, tap.point);
					wire.points = pending.bend_points;
					if !pending.start.is_source() {
						wire.points.reverse();
					}
					v.library.get_mut(&root_chip_name).wires.push(wire);
					v.rebuild_sim();
					*status = None;
					return;
				}
				Err(reason) => {
					*status = Some(reason.to_string());
					return;
				}
			}
		}
	}

	if on_component {
		// Neither a pin nor empty space -- ignored outright (not a "turn"), so the placement just
		// stays exactly as it was and the player can click somewhere more useful instead.
		return;
	}

	// Same transform the live preview applies (see `frame::build_viewer_stack`):
	// grid-snap first, then flatten onto the previous point's row/column when
	// straight wires are forced -- mirroring `WireInstance.SetWirePointWithSnapping`.
	let snap = v.should_snap_to_grid();
	let straighten = v.force_straight_wires();
	let pending = v.pending_wire.as_mut().expect("caller only calls this with a pending wire");
	let mut turn = world_pos;
	if snap {
		turn = crate::render::layout::snap_to_grid_centred(turn);
	}
	if straighten {
		let prev = pending.bend_points.last().copied().unwrap_or_else(|| pending.start.position());
		turn = crate::render::layout::force_straight_line(prev, turn);
	}
	pending.bend_points.push(turn);
}

/// Next free id for a newly placed subchip or boundary dev-pin on `chip`:
/// one past whatever the highest existing id is, or `1` if it has none yet
/// (`SubChipDescription::id` docs say IDs are `> 0`). Dev-pins share this
/// same id space with subchips -- a wire's `PinOwnerID` is looked up
/// against both `chip.sub_chips` and `chip.input_pins`/`output_pins`
/// interchangeably (see `sim::Simulator::find_pin`) -- so every id handed
/// out here must stay unique across all three lists, not just within
/// whichever one the caller is about to push into.
fn next_component_id(chip: &ChipDescription) -> i32 {
	chip.sub_chips.iter().map(|s| s.id).chain(chip.input_pins.iter().map(|p| p.id)).chain(chip.output_pins.iter().map(|p| p.id)).max().unwrap_or(0)
		+ 1
}

/// Sensible starting `SubChipDescription::internal_data` for a freshly-placed subchip of
/// `chip_type`, so the chip is immediately simulate-able (and shows a sane value in its own
/// configuration popup) without the player having to open and confirm that popup first. Chip
/// types that don't need any persistent internal data -- or that already tolerate a missing one
/// (e.g. a boundary `KEY`-less lookup defaulting to "unbound") -- get `None`, same as before.
fn default_internal_data(chip_type: Option<ChipType>) -> Option<Vec<u32>> {
	match chip_type {
		// `Rom256x16`'s internal_state is indexed directly by the live 0..256 address bus with no
		// bounds check (see `sim::process_builtin_chip`'s `Rom256x16` arm), so it needs the full
		// `ROM_WORD_COUNT`-length contents up front, not just whatever's been configured so far --
		// an absent/short one is exactly what caused the placement-time panic this fixes.
		Some(ChipType::Rom256x16) => Some(vec![0u32; crate::render::editor_ui::ROM_WORD_COUNT]),
		// Bound to 'A' by default, so a freshly-placed KEY chip responds to a keypress right away
		// instead of silently sitting bound to the (unpressable) null character until configured.
		Some(ChipType::Key) => Some(vec![b'A' as u32]),
		// [duration, ticks_remaining, input_old] -- see `sim::process_builtin_chip`'s `Pulse` arm,
		// which indexes all three unconditionally. 200 simulation ticks is a short but visible
		// default pulse length; the other two are just-started runtime state, always zero.
		Some(ChipType::Pulse) => Some(vec![200, 0, 0]),
		_ => None,
	}
}

/// Attempts to drop `v.pending_place`'s chip (assumed `Some`) at
/// `world_pos`. Only actually places it -- and clears `v.pending_place`
/// -- when the click lands on genuinely free canvas space: not a
/// subchip's pin, one of the current chip's own boundary dev-pins, an
/// existing placed component's body, or a wire. Landing on any of those
/// just leaves the pending placement active untouched, so the player can
/// simply try again elsewhere (mirrors `try_continue_pending_wire`'s
/// "component/wire clicks are ignored outright" behaviour). The new
/// instance gets a fresh id (`next_component_id`), no label, no saved
/// internal data beyond `default_internal_data`, and no output-pin colour
/// overrides. An "IN/OUT" palette entry is the one exception: it adds a
/// boundary dev-pin to the current chip instead of a subchip instance --
/// see the branch below.
///
/// Also defensively re-checks `would_create_cycle` -- unlike a free-space
/// miss, this can never be resolved by clicking somewhere else, so
/// (unlike the free-space case) it cancels the pending placement outright
/// and reports why via `status`, rather than leaving it dangling for a
/// retry that could never succeed.
fn try_place_pending_chip(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	let root_chip_name = v.root_chip_name.clone();
	let root_desc = v.library.get(&root_chip_name);
	let placed = scene::place_sub_chips(root_desc, &v.library);

	let max_dist = wire_click_tolerance(&v.camera);
	let blocked = scene::hit_test_any_pin(root_desc, &placed, world_pos).is_some()
		|| scene::hit_test_dev_pin(root_desc, world_pos).is_some()
		|| scene::hit_test_sub_chip(&placed, world_pos).is_some()
		|| scene::hit_test_wire(root_desc, &v.library, world_pos, max_dist).is_some();
	if blocked {
		return;
	}

	// Defensive re-check: the "USE"/bottom-bar buttons that set `pending_place` in the first
	// place are already greyed out for a chip that would cycle (see `would_create_cycle`'s
	// docs), but a click always gets the final say rather than trusting that alone.
	let name = v.pending_place.take().expect("caller only calls this with a pending placement");
	if crate::viewer::library::would_create_cycle(&v.library, &root_chip_name, &name) {
		*status = Some(format!("Can't place '{name}' inside '{root_chip_name}' -- it would contain itself"));
		return;
	}

	// "IN/OUT" palette entries (`In1Bit` .. `Out8Bit`) aren't real placeable components -- they're
	// the boundary I/O pins a custom chip is parsed with from its saved JSON. Placing one adds a
	// fresh dev-pin straight to the current chip's boundary instead of a subchip instance.
	let chip_type = v.library.try_get(&name).map(|d| d.chip_type);
	let io_template = chip_type.and_then(builtins::io_pin_template);
	let terminus_type = chip_type.and_then(|t| t.corresponding_bus_terminus());

	// Snap the drop position when the snapping pref (or held Ctrl) says so --
	// `ChipInteractionController`'s `ShouldSnapToGrid` branch.
	let place_pos = if v.should_snap_to_grid() { crate::render::layout::snap_to_grid_centred(world_pos) } else { world_pos };

	// Resolved before the mutable borrow below: the terminus partner's
	// library name, for the bus-pair placement branch.
	let terminus_name = terminus_type.and_then(|t| v.library.iter().find(|d| d.chip_type == t).map(|d| d.name.clone()));

	let chip = v.library.get_mut(&root_chip_name);
	let id = next_component_id(chip);

	if let Some((is_input, template)) = io_template {
		let mut new_pin = PinDescription::new(template.name, id, template.bit_count);
		new_pin.position = place_pos;
		if is_input {
			chip.input_pins.push(new_pin);
		} else {
			chip.output_pins.push(new_pin);
		}
	} else if let Some(terminus_name) = terminus_name.as_deref() {
		debug_assert!(chip_type.is_some_and(|t| t.corresponding_bus_terminus().is_some()));
		// Placing a bus origin auto-places its terminus partner, linked to
		// it in both directions via `internal_data[0]`
		// (`ChipInteractionController`'s auto-place + `SetLinkedBusPair`).
		// The origin sits left of the drop point so the two are both
		// visible (the original spreads the pair by `GridSize * 8`).
		let terminus_id = id + 1;
		let pair_spacing = crate::render::layout::GRID_SIZE * 4.0;
		chip.sub_chips.push(SubChipDescription {
			name,
			id,
			internal_data: Some(vec![terminus_id as u32]),
			position: place_pos - Vec2::new(pair_spacing, 0.0),
			label: None,
			pin_colour_info: Vec::new(),
		});
		chip.sub_chips.push(SubChipDescription {
			name: terminus_name.to_string(),
			id: terminus_id,
			internal_data: Some(vec![id as u32]),
			position: place_pos + Vec2::new(pair_spacing, 0.0),
			label: None,
			pin_colour_info: Vec::new(),
		});
	} else {
		let internal_data = default_internal_data(chip_type);
		chip.sub_chips.push(SubChipDescription { name, id, internal_data, position: place_pos, label: None, pin_colour_info: Vec::new() });
	}
	v.rebuild_sim();
}

/// Builds the translucent "ghost" preview of the chip currently pending
/// placement, floating at the cursor's live world position. Reuses the
/// exact same `build_scene` pipeline a real placed component draws
/// through -- body, pins, name label, and any type-specific rendering
/// (a Key's bound letter, an LED's tint, a display's live pixels, ...)
/// -- by wrapping the chip in a throwaway single-subchip `ChipDescription`,
/// so the preview can never drift out of sync with what actually gets
/// placed. Faded to [`PENDING_PLACEMENT_ALPHA`] via `scene::apply_alpha`.
/// Returns `None` if `chip_name` no longer resolves in `library` (e.g. it
/// was deleted while pending -- shouldn't normally happen, but avoids a
/// panic in `place_sub_chips` if it somehow does).
///
/// An "IN/OUT" palette entry previews as a boundary dev-pin instead (see
/// `try_place_pending_chip`), so its ghost is built the same way -- a
/// throwaway chip with the pin added straight to `input_pins`/`output_pins`
/// -- rather than wrapping it as a subchip, so the preview never shows the
/// wrong body shape for what's actually about to be placed.
pub(crate) fn build_pending_place_scene(library: &ChipLibrary, chip_name: &str, cursor_world_pos: Vec2) -> Option<SceneGeometry> {
	let chip_type = library.try_get(chip_name)?.chip_type;

	let mut ghost = ChipDescription::new("__pending_placement_ghost__", ChipType::Custom);
	if let Some((is_input, template)) = builtins::io_pin_template(chip_type) {
		let mut new_pin = PinDescription::new(template.name, 0, template.bit_count);
		new_pin.position = cursor_world_pos;
		if is_input {
			ghost.input_pins.push(new_pin);
		} else {
			ghost.output_pins.push(new_pin);
		}
	} else {
		ghost.sub_chips.push(SubChipDescription {
			name: chip_name.to_string(),
			id: 0,
			internal_data: None,
			position: cursor_world_pos,
			label: None,
			pin_colour_info: Vec::new(),
		});
	}

	let mut geo = scene::build_scene(&ghost, library, &scene::AllLow, None);
	scene::apply_alpha(&mut geo, PENDING_PLACEMENT_ALPHA);
	Some(geo)
}

/// Alpha applied to a chip's translucent placement preview (see
/// [`build_pending_place_scene`]) -- 75%, so the ghost reads clearly as
/// "not yet placed" without being hard to make out against the canvas.
const PENDING_PLACEMENT_ALPHA: f32 = 0.75;

/// Draws the in-progress wire preview: a line from its start endpoint,
/// through any turn points placed so far, to the cursor's current world
/// position -- so the player can see what they're about to connect
/// before actually clicking the second endpoint. Purely cosmetic (never
/// touches `chip.wires`), drawn in `theme::PIN_HIGHLIGHT_COL` so it
/// reads clearly against both wires and pins.
pub(crate) fn draw_pending_wire_preview(geo: &mut SceneGeometry, pending: &PendingWire, cursor_world_pos: Vec2) {
	let mut path = Vec::with_capacity(pending.bend_points.len() + 2);
	path.push(pending.start.position());
	path.extend_from_slice(&pending.bend_points);
	path.push(cursor_world_pos);
	geo.add_polyline(&path, layout::WIRE_THICKNESS, theme::PIN_HIGHLIGHT_COL);

	// Small markers at each already-placed turn point, so a bend the player just
	// clicked in stays visible even where the preview line passes straight through it.
	for &turn in &pending.bend_points {
		geo.add_circle(turn, layout::WIRE_THICKNESS * 1.5, theme::PIN_HIGHLIGHT_COL, 12);
	}
}

/// Deletes subchip/dev-pin `id` from the current root chip, plus every
/// wire directly attached to it -- but, per the brief, only the "shortest
/// possible section" of wiring: just the wire(s) whose source or target
/// pin actually belongs to this subchip (via `scene::delete_wire`, which
/// itself only cascades to wires *tapping onto* one of those, never
/// anything further away). A wire fanning out from one of this
/// component's *output* pins to some other, unrelated component is left
/// completely alone at the far end -- only the segment that touched the
/// deleted component goes.
///
/// `id` may equally be a placed subchip's `SubChipDescription::id` or one of
/// the current chip's own boundary dev-pins' `PinDescription::id` -- the two
/// share one id space (see `next_component_id`), and a wire's cascade-delete
/// above already keys off `pin_owner_id` without caring which kind of thing
/// it belonged to, so removing the component itself just means checking all
/// three lists it could actually be sitting in.
pub(crate) fn delete_component(v: &mut ViewerState, id: i32) {
	let root_chip_name = v.root_chip_name.clone();

	// Bus chips go together with their linked partner (`GetNonIncludedLinkedBusElements`
	// keeps the pair together on delete in the original).
	let mut ids = vec![id];
	loop {
		let mut added = false;
		for s in &v.library.get(&root_chip_name).sub_chips {
			let pairs_into_ids =
				bus_wiring::bus_partner_id(v.library.get(&root_chip_name), &v.library, s.id).is_some_and(|partner| ids.contains(&partner));
			if !ids.contains(&s.id) && pairs_into_ids {
				ids.push(s.id);
				added = true;
			}
		}
		if !added {
			break;
		}
	}

	let chip = v.library.get_mut(&root_chip_name);
	for &id in &ids {
		loop {
			let next = chip.wires.iter().position(|w| w.source_pin_address.pin_owner_id == id || w.target_pin_address.pin_owner_id == id);
			match next {
				Some(idx) => {
					scene::delete_wire(chip, idx);
				}
				None => break,
			}
		}
	}

	chip.sub_chips.retain(|s| !ids.contains(&s.id));
	chip.input_pins.retain(|p| !ids.contains(&p.id));
	chip.output_pins.retain(|p| !ids.contains(&p.id));
	v.rebuild_sim();
}

/// Applies a canvas click that the UI stack let fall all the way through
/// (`UiStack::dispatch_click` returned [`crate::render::ui_stack::InputResult::Propagate`] --
/// every visible UI layer was either missed or transparent at that point).
pub(crate) fn handle_canvas_click(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	// A chip picked up for placement claims every click ahead of anything else below,
	// same "claims the click" priority a wire in progress gets just below -- see
	// `try_place_pending_chip`'s doc comment for what actually happens with the click.
	if v.pending_place.is_some() {
		try_place_pending_chip(v, world_pos, status);
		return;
	}

	// A wire already being placed claims every click ahead of anything else below --
	// including the input-pin toggle, so clicking a switch's pin finishes/bends the
	// wire instead of flipping it (see `try_continue_pending_wire`'s doc comment).
	if v.pending_wire.is_some() {
		try_continue_pending_wire(v, world_pos, status);
		return;
	}
	if try_start_pending_wire(v, world_pos) {
		return;
	}

	let root_desc = v.library.get(&v.root_chip_name);
	if let Some((pin_id, bit_index)) = hit_test_root_input_pin_click(root_desc, world_pos) {
		let root_chip_name = v.root_chip_name.clone();
		toggle_driven_input_bit(&mut v.library, &root_chip_name, pin_id, bit_index);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::PinBitCount;

	fn empty_viewer_library() -> ChipLibrary {
		ChipLibrary::new()
	}

	/// A wire-click tolerance must grow as the camera zooms out (so the
	/// same apparent screen distance keeps hitting), and never divide by zero.
	#[test]
	fn wire_click_tolerance_scales_with_zoom_and_survives_zero() {
		let mut cam = crate::render::camera::Camera::new(Vec2::new(800.0, 600.0));
		cam.zoom = 10.0;
		assert_eq!(wire_click_tolerance(&cam), 0.6);
		cam.zoom = 100.0;
		assert_eq!(wire_click_tolerance(&cam), 0.06);
		cam.zoom = 0.0;
		assert!(wire_click_tolerance(&cam).is_finite());
	}

	#[test]
	fn default_internal_data_covers_every_configurable_builtin() {
		assert_eq!(default_internal_data(Some(ChipType::Key)), Some(vec![b'A' as u32]));
		assert_eq!(default_internal_data(Some(ChipType::Pulse)), Some(vec![200, 0, 0]));
		let rom = default_internal_data(Some(ChipType::Rom256x16)).expect("ROM needs its full contents buffer");
		assert_eq!(rom.len(), crate::render::editor_ui::ROM_WORD_COUNT);
		assert!(rom.iter().all(|&w| w == 0));
		assert_eq!(default_internal_data(None), None);
		assert_eq!(default_internal_data(empty_viewer_library().try_get("nope").map(|d| d.chip_type)), None);
	}

	/// `next_component_id` hands out ids unique across *all three*
	/// lists (subchips + both dev-pin lists), since they share one id space.
	#[test]
	fn next_component_id_is_unique_across_all_id_spaces() {
		let mut chip = ChipDescription::new("TEST", ChipType::Custom);
		chip.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 3,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		let mut pin = PinDescription::new("IN", 7, PinBitCount::Bit1);
		pin.position = Vec2::ZERO;
		chip.input_pins.push(pin);

		assert_eq!(next_component_id(&chip), 8);
	}
}
