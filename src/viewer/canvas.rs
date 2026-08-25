//! Canvas interaction: what a click on the chip-editing surface means --
//! starting/continuing wire placements, dropping a pending multi-component
//! placement, selecting/dragging placed components, rubber-band box
//! selection, toggling an input dev-pin's bits -- plus the scene-space
//! previews (in-progress wire, placement ghost) those interactions draw.
//! The selection/drag/box-select state machine itself lives in
//! `viewer::chip_interaction`; this module routes clicks between it and
//! the wire-placement flow.

use crate::description::ChipDescription;
use crate::render::camera::Camera;
use crate::render::layout;
use crate::render::scene::{self, SceneGeometry};
use crate::render::theme;
use crate::structs::Vec2;
use crate::viewer::bus_wiring;
use crate::viewer::chip_interaction;
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
		pin.driven_state.toggle_bit(bit_index);
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
///    any bend points collected so far -- and so does landing on *any*
///    other bus-family chip's pin: the completing click converts that
///    second chip to the complementary origin/terminus type (keeping its
///    visible pin side) and links the pair instantly (see
///    `viewer::bus_wiring::resolve_bus_pair_completion`);
///  - landing on a pin of the *same* role (e.g. input-to-input,
///    output-to-output) is rejected with a status message, leaving the
///    placement active so the player can just try a different pin --
///    unless both ends are bus chips, where the conversion above absorbs
///    exactly those cases;
///  - landing on an existing wire *completes into it* ("wiring into the
///    wire"): inputs may tap into any wire, outputs only into bus wires,
///    and the electrical endpoints resolve from the tapped wire
///    (bus-corrected on the output-start side) -- see `viewer::bus_wiring`.
///    A placement that itself started on a wire ignores wire clicks instead
///    (wire-to-wire is ambiguous);
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
		let pending_ref = v.pending_wire.as_ref().expect("caller only calls this with a pending wire");
		/*		// optional : if you want to only connect same bitcount wires
			   if pending.bit_count != hit.bit_count {
				   *status = Some("Can't connect different bitcounts".to_string());
				   return;
			   }
		*/
		// Any bus-family chip may wire to any other bus-family chip (see
		// `resolve_bus_pair_completion`): the completing click converts the
		// second half to the complementary origin/terminus type -- keeping
		// its visible pin side -- and links the pair instantly. That makes
		// the usual same-role rejections inapplicable here: two origins'
		// visible output pins read as "output to output" but are exactly
		// how two plain buses join.
		let bus_start_owner = match pending_ref.start {
			PendingWireEnd::Pin { owner_id: start_owner, .. } => {
				let start_is_bus = bus_wiring::owner_chip_type(root_desc, &v.library, start_owner).is_some_and(|t| t.is_bus_type());
				let end_is_bus = bus_wiring::owner_chip_type(root_desc, &v.library, hit.owner_id).is_some_and(|t| t.is_bus_type());
				(start_is_bus && end_is_bus).then_some(start_owner)
			}
			PendingWireEnd::WireTap { .. } => None,
		};

		if bus_start_owner.is_none() && pending_ref.start.is_source() == hit.is_wire_source() {
			*status = Some(if hit.is_wire_source() {
				"Can't connect an output to an output".to_string()
			} else {
				"Can't connect an input to an input".to_string()
			});
			return;
		}

		let pending = v.pending_wire.take().expect("checked above");

		let mut wire = match bus_start_owner {
			Some(start_owner) => {
				// Resolve on a scratch copy -- the resolver reads sibling
				// descriptions out of the same library the edited chip
				// lives in -- then write the converted/linked chip back.
				let mut edited = v.library.get(&root_chip_name).clone();
				match bus_wiring::resolve_bus_pair_completion(&mut edited, &v.library, start_owner, hit.owner_id) {
					Ok((source, target)) => {
						*v.library.get_mut(&root_chip_name) = edited;
						WireDescription::new(source, target)
					}
					Err(reason) => {
						v.pending_wire = Some(pending);
						*status = Some(reason.to_string());
						return;
					}
				}
			}
			None => {
				let end_pin_address = PinAddress::new(hit.owner_id, hit.pin_id);
				if pending.start.is_source() {
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
				}
			}
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
	let on_wire = scene::closest_wire_hit(root_desc, &v.library, world_pos, max_dist);

	// Completing ONTO an existing wire ("wiring into the wire"). Only
	// placements started from a real pin complete here; a wire-tap start
	// landing on another wire falls through to the ignore below (the
	// wire-to-wire case `resolve_completion_on_wire` rejects as ambiguous).
	if !on_component && v.pending_wire.as_ref().is_some_and(|p| matches!(p.start, PendingWireEnd::Pin { .. })) {
		let pending = v.pending_wire.as_ref().expect("checked above");
		let PendingWireEnd::Pin { owner_id, pin_id, .. } = pending.start else { unreachable!("branch guarantees a pin start") };
		if let Some(tap) = on_wire {
			match bus_wiring::resolve_completion_on_wire(root_desc, &v.library, tap.wire_index, false, pending.start.is_source(), owner_id, pin_id) {
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

	if on_component || on_wire.is_some() {
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

/// Attempts to drop `v.pending_place`'s carried components (assumed
/// non-empty) at `world_pos`. Only actually places anything -- and clears
/// `v.pending_place` -- when the click lands on genuinely free canvas
/// space: not a subchip's pin, one of the current chip's own boundary
/// dev-pins, an existing placed component's body, or a wire. Landing on
/// any of those just leaves the pending carry untouched, so the player can
/// simply try again elsewhere (mirrors `try_continue_pending_wire`'s
/// "component/wire clicks are ignored outright" behaviour). Each entry is
/// dropped at the cursor plus its carried offset (grid-snapped when the
/// snapping pref -- or held Ctrl -- says so), with consecutive fresh ids
/// (`next_component_id`). An "IN/OUT" palette entry is the one exception:
/// it adds a boundary dev-pin to the current chip instead of a subchip
/// instance. Bus-pair entries (see `chip_interaction::start_placing`)
/// write their mutual link into both halves' `internal_data[0]` on drop,
/// exactly what the original's auto-place + `SetLinkedBusPair` produced --
/// everything downstream of that link (pair-wiring rules, tap merging,
/// paired deletion) keys off it unchanged.
///
/// Also defensively re-checks `would_create_cycle` per distinct name --
/// unlike a free-space miss, this can never be resolved by clicking
/// somewhere else, so (unlike the free-space case) it cancels the whole
/// pending carry outright and reports why via `status`, rather than
/// leaving it dangling for a retry that could never succeed.
pub(crate) fn try_place_pending_components(v: &mut ViewerState, world_pos: Vec2, status: &mut Option<String>) {
	// Everything the drop decision needs, resolved against the *current*
	// chip before any mutation below.
	let blocked = {
		let root_desc = v.library.get(&v.root_chip_name);
		let placed = scene::place_sub_chips(root_desc, &v.library);
		let max_dist = wire_click_tolerance(&v.camera);
		scene::hit_test_any_pin(root_desc, &placed, world_pos).is_some()
			|| scene::hit_test_dev_pin(root_desc, world_pos).is_some()
			|| scene::hit_test_sub_chip(&placed, world_pos).is_some()
			|| scene::hit_test_wire(root_desc, &v.library, world_pos, max_dist).is_some()
	};
	if blocked {
		return;
	}

	// Defensive re-check: the "USE"/bottom-bar buttons that fill
	// `pending_place` in the first place are already greyed out for a chip
	// that would cycle (see `would_create_cycle`'s docs), but a click
	// always gets the final say rather than trusting that alone.
	let carry = std::mem::take(&mut v.pending_place);
	for (_, component) in &carry {
		if crate::viewer::library::would_create_cycle(&v.library, &v.root_chip_name, &component.name) {
			*status = Some(format!("Can't place '{}' inside '{}' -- it would contain itself", component.name, v.root_chip_name));
			return;
		}
	}

	// "IN/OUT" palette entries (`In1Bit` .. `Out8Bit`) aren't real placeable components -- they're
	// the boundary I/O pins a custom chip is parsed with from its saved JSON. Placing one adds a
	// fresh dev-pin straight to the current chip's boundary instead of a subchip instance.
	let chip_types: Vec<Option<ChipType>> = carry.iter().map(|(_, c)| v.library.try_get(&c.name).map(|d| d.chip_type)).collect();

	// Every entry drops at the cursor plus its carried offset, each snapped
	// individually when snapping says so (a pair's spacing is an exact grid
	// multiple, so snapping preserves it).
	let snap = v.should_snap_to_grid();

	let first_id = next_component_id(v.library.get(&v.root_chip_name));
	let chip = v.library.get_mut(&v.root_chip_name);
	for (index, (offset, component)) in carry.iter().enumerate() {
		let id = first_id + index as i32;
		let place_pos = if snap { crate::render::layout::snap_to_grid_centred(world_pos + *offset) } else { world_pos + *offset };

		if let Some((is_input, template)) = chip_types[index].and_then(builtins::io_pin_template) {
			let mut new_pin = PinDescription::new(template.name, id, template.bit_count);
			new_pin.position = place_pos;
			if is_input {
				chip.input_pins.push(new_pin);
			} else {
				chip.output_pins.push(new_pin);
			}
			continue;
		}

		let internal_data = match component.linked_bus_partner {
			Some(partner_index) => Some(vec![(first_id + partner_index as i32) as u32]),
			None => default_internal_data(chip_types[index]),
		};
		chip.sub_chips.push(SubChipDescription {
			name: component.name.clone(),
			id,
			internal_data,
			position: place_pos,
			label: None,
			pin_colour_info: Vec::new(),
		});
	}
	v.rebuild_sim();
}

/// Builds the translucent "ghost" preview of everything currently carried
/// by a pending placement (`pending`), floating at the cursor's live world
/// position -- each entry at the cursor plus its carried offset (snapped
/// when `snap_to_grid`). Reuses the exact same `build_scene` pipeline real
/// placed components draw through -- body, pins, name label, and any
/// type-specific rendering (a Key's bound letter, an LED's tint, ...) --
/// by wrapping them in a throwaway single-level `ChipDescription`, so the
/// preview can never drift out of sync with what actually gets placed.
/// Faded to [`PENDING_PLACEMENT_ALPHA`] via `scene::apply_alpha`. Entries
/// whose chip no longer resolves in `library` are skipped defensively
/// (shouldn't normally happen, but avoids a panic in `place_sub_chips` if
/// it somehow does).
///
/// An "IN/OUT" palette entry previews as a boundary dev-pin instead (see
/// `try_place_pending_components`), so its ghost is built the same way -- a
/// throwaway pin added straight to `input_pins`/`output_pins` -- rather
/// than wrapping it as a subchip, so the preview never shows the wrong
/// body shape for what's actually about to be placed.
pub(crate) fn build_pending_place_scene(
	library: &ChipLibrary,
	pending: &[(Vec2, crate::viewer::chip_interaction::PendingComponent)],
	cursor_world_pos: Vec2,
	snap_to_grid: bool,
) -> SceneGeometry {
	let mut ghost = ChipDescription::new("__pending_placement_ghost__", ChipType::Custom);

	for (index, (offset, component)) in pending.iter().enumerate() {
		let position =
			if snap_to_grid { crate::render::layout::snap_to_grid_centred(cursor_world_pos + *offset) } else { cursor_world_pos + *offset };
		let Some(chip_type) = library.try_get(&component.name).map(|d| d.chip_type) else { continue };

		if let Some((is_input, template)) = builtins::io_pin_template(chip_type) {
			let mut new_pin = PinDescription::new(template.name, index as i32, template.bit_count);
			new_pin.position = position;
			if is_input {
				ghost.input_pins.push(new_pin);
			} else {
				ghost.output_pins.push(new_pin);
			}
		} else {
			ghost.sub_chips.push(SubChipDescription {
				name: component.name.clone(),
				id: index as i32,
				internal_data: None,
				position,
				label: None,
				pin_colour_info: Vec::new(),
			});
		}
	}

	let mut geo = scene::build_scene(&ghost, library, &scene::AllLow, None);
	scene::apply_alpha(&mut geo, PENDING_PLACEMENT_ALPHA);
	geo
}

/// Alpha applied to a chip's translucent placement preview (see
/// [`build_pending_place_scene`]) -- 75%, so the ghost reads clearly as
/// "not yet placed" without being hard to make out against the canvas.
/// Dragging fades carried components by the same amount, so both kinds of
/// "in the hand" state read identically.
pub(crate) const PENDING_PLACEMENT_ALPHA: f32 = 0.75;

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
	// `try_place_pending_components`'s doc comment for what actually happens with the click.
	if !v.pending_place.is_empty() {
		try_place_pending_components(v, world_pos, status);
		return;
	}

	// A wire already being placed claims every click ahead of anything else below --
	// including the input-pin toggle and component selection, so clicking a switch's
	// pin finishes/bends the wire instead of flipping it (see
	// `try_continue_pending_wire`'s doc comment).
	if v.pending_wire.is_some() {
		try_continue_pending_wire(v, world_pos, status);
		return;
	}
	if try_start_pending_wire(v, world_pos) {
		return;
	}

	{
		let root_desc = v.library.get(&v.root_chip_name);
		let placed = scene::place_sub_chips(root_desc, &v.library);

		// A press on a placed component's *body* selects it and starts carrying
		// the selection around -- pins stick out past the body (and wires sit
		// beneath everything), so they've each already had their say above.
		if let Some(sub) = scene::hit_test_sub_chip(&placed, world_pos) {
			chip_interaction::begin_drag_on_component(v, sub.id, world_pos);
			return;
		}
	}

	let root_desc = v.library.get(&v.root_chip_name);
	if let Some((pin_id, bit_index)) = hit_test_root_input_pin_click(root_desc, world_pos) {
		let root_chip_name = v.root_chip_name.clone();
		toggle_driven_input_bit(&mut v.library, &root_chip_name, pin_id, bit_index);
		return;
	}

	// Empty canvas (the click reached the grid): rubber-band select from here.
	chip_interaction::begin_selection_box(v, world_pos);
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

	// ---- Bus pairing: placement + paired deletion (white-box: drives the
	// pub(crate) placement flow on a real ViewerState) ----

	fn viewer_with_builtins() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	#[test]
	fn placing_a_bus_places_its_linked_terminus_pair() {
		let mut v = viewer_with_builtins();
		chip_interaction::start_placing(&mut v, "BUS-4");
		let mut status = None;

		try_place_pending_components(&mut v, Vec2::new(10.0, 10.0), &mut status);

		assert_eq!(status, None);
		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 2, "origin + terminus");
		assert_eq!(chip.sub_chips[0].name, "BUS-4");
		assert_eq!(chip.sub_chips[1].name, "BUS-TERMINUS-4");

		let (origin_id, terminus_id) = (chip.sub_chips[0].id, chip.sub_chips[1].id);
		assert_eq!(terminus_id, origin_id + 1, "pair ids are consecutive");
		assert_eq!(chip.sub_chips[0].internal_data, Some(vec![terminus_id as u32]), "origin links to terminus");
		assert_eq!(chip.sub_chips[1].internal_data, Some(vec![origin_id as u32]), "terminus links back to origin");
		assert!(bus_wiring::bus_pair_linked(chip, &v.library, origin_id, terminus_id), "the placed pair satisfies the linked-pair check");
	}

	#[test]
	fn deleting_one_bus_half_takes_the_other_and_their_wires() {
		let mut v = viewer_with_builtins();
		chip_interaction::start_placing(&mut v, "BUS-4");
		try_place_pending_components(&mut v, Vec2::ZERO, &mut None);

		// Wire something across the pair so deletion must cascade it away.
		let (origin_id, _terminus_id) = {
			let chip = v.library.get_mut("ROOT");
			let (origin_id, terminus_id) = (chip.sub_chips[0].id, chip.sub_chips[1].id);
			chip.wires.push(WireDescription::new(PinAddress::new(origin_id, 1), PinAddress::new(terminus_id, 0)));
			(origin_id, terminus_id)
		};

		delete_component(&mut v, origin_id);

		let chip = v.library.get("ROOT");
		assert!(chip.sub_chips.is_empty(), "both halves go together");
		assert!(chip.wires.is_empty(), "wires attached to either half go too");
	}

	/// World-space position of a placed subchip's pin, resolved the same way
	/// the renderer lays pins out (`place_sub_chips` + `pin_world_position`,
	/// including the bus-chip flip flag).
	fn pin_pos(v: &ViewerState, owner_id: i32, pin_id: i32) -> Vec2 {
		let root_desc = v.library.get(&v.root_chip_name);
		let placed = scene::place_sub_chips(root_desc, &v.library);
		let sub = placed.iter().find(|p| p.id == owner_id).expect("owner placed");
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;
		if let Some((i, _)) = sub.desc.input_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
			layout::pin_world_position(sub.centre, sub.size, sub.input_pin_y[i], true ^ is_flipped)
		} else {
			let (i, _) = sub.desc.output_pins.iter().enumerate().find(|(_, p)| p.id == pin_id).expect("pin exists");
			layout::pin_world_position(sub.centre, sub.size, sub.output_pin_y[i], false ^ is_flipped)
		}
	}

	/// The full placement -> linking -> wiring story through the real click
	/// flow: dropping the carried pair must leave it wireable exactly like
	/// the original's auto-placed pairs -- origin output to terminus input
	/// completes as a bus wire.
	#[test]
	fn dropped_bus_pair_wires_together_as_a_bus_wire_through_the_click_flow() {
		let mut v = viewer_with_builtins();
		chip_interaction::start_placing(&mut v, "BUS-4");
		try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		let chip = v.library.get("ROOT").clone();
		let (origin_id, terminus_id) = (chip.sub_chips[0].id, chip.sub_chips[1].id);

		let mut status = None;
		let origin_out = pin_pos(&v, origin_id, 1);
		let terminus_in = pin_pos(&v, terminus_id, 0);
		handle_canvas_click(&mut v, origin_out, &mut status); // origin OUT starts the wire
		assert!(status.is_none() && v.pending_wire.is_some(), "clicking the origin's output pin starts a wire");
		handle_canvas_click(&mut v, terminus_in, &mut status); // terminus IN completes it

		assert_eq!(status, None);
		assert!(v.pending_wire.is_none(), "the placement completed");
		let chip = v.library.get("ROOT");
		assert_eq!(chip.wires.len(), 1);
		assert!(bus_wiring::is_bus_wire(chip, &v.library, &chip.wires[0]), "the linked pair wires together as a bus wire");
		assert_eq!(chip.wires[0].source_pin_address, PinAddress::new(origin_id, 1));
		assert_eq!(chip.wires[0].target_pin_address, PinAddress::new(terminus_id, 0));
	}

	/// Two independently-dropped pairs are NOT linked, but crossing them
	/// (origin A -> terminus B) is now exactly the "any bus to any bus"
	/// rule: the wire completes and the two halves link instantly, while
	/// the orphaned previous partners' pointers are cleared so a later
	/// delete of either doesn't cascade across the old pairs.
	#[test]
	fn cross_pair_origin_to_terminus_wiring_links_instantly() {
		let mut v = viewer_with_builtins();
		for pos in [Vec2::ZERO, Vec2::new(20.0, 0.0)] {
			chip_interaction::start_placing(&mut v, "BUS-4");
			try_place_pending_components(&mut v, pos, &mut None);
		}
		let chip = v.library.get("ROOT").clone();
		let ids: Vec<i32> = chip.sub_chips.iter().map(|s| s.id).collect();
		let (origin_a, terminus_a, origin_b, terminus_b) = (ids[0], ids[1], ids[2], ids[3]);

		let mut status = None;
		let origin_a_out = pin_pos(&v, origin_a, 1);
		let terminus_b_in = pin_pos(&v, terminus_b, 0);
		handle_canvas_click(&mut v, origin_a_out, &mut status);
		handle_canvas_click(&mut v, terminus_b_in, &mut status);

		assert_eq!(status, None);
		assert!(v.pending_wire.is_none(), "the placement completed");
		let chip = v.library.get("ROOT");
		assert_eq!(chip.wires.len(), 1);
		assert!(bus_wiring::is_bus_wire(chip, &v.library, &chip.wires[0]));
		assert_eq!(chip.wires[0].source_pin_address, PinAddress::new(origin_a, 1));
		assert_eq!(chip.wires[0].target_pin_address, PinAddress::new(terminus_b, 0));

		assert!(bus_wiring::bus_pair_linked(chip, &v.library, origin_a, terminus_b), "the crossed halves linked instantly");
		let data = |id: i32| chip.sub_chips.iter().find(|s| s.id == id).expect("exists").internal_data.clone().unwrap();
		assert_eq!(data(terminus_a), vec![0, 0], "A's old terminus is unlinked (no dangling pointer)");
		assert_eq!(data(origin_b), vec![0, 0], "B's old origin is unlinked (no dangling pointer)");
	}

	/// Any bus to any other: completing a wire from one bus ORIGIN onto
	/// another bus origin converts the second into a terminus -- flipped
	/// relative to its old state so its visible pin stays on the same
	/// physical side -- and links the pair instantly. The finished wire
	/// runs origin-output -> converted-terminus-input, i.e. a proper bus
	/// wire.
	#[test]
	fn origin_to_origin_click_converts_the_second_bus_into_a_linked_terminus() {
		let mut v = viewer_with_builtins();
		for pos in [Vec2::ZERO, Vec2::new(20.0, 0.0)] {
			chip_interaction::start_placing(&mut v, "BUS-4");
			try_place_pending_components(&mut v, pos, &mut None);
		}
		let ids: Vec<i32> = v.library.get("ROOT").sub_chips.iter().map(|s| s.id).collect();
		let (origin_a, _terminus_a, origin_b) = (ids[0], ids[1], ids[2]);

		// The click lands on B's visible output pin (unflipped => right side).
		let clicked = pin_pos(&v, origin_b, 1);
		assert!(clicked.x > sub_position(&v, origin_b).x, "sanity: B's visible pin starts on its right");

		let mut status = None;
		let origin_a_out = pin_pos(&v, origin_a, 1);
		handle_canvas_click(&mut v, origin_a_out, &mut status);
		handle_canvas_click(&mut v, clicked, &mut status);

		assert_eq!(status, None);
		assert!(v.pending_wire.is_none(), "output-to-output between buses completes via conversion");
		let chip = v.library.get("ROOT");
		let b = chip.sub_chips.iter().find(|s| s.id == origin_b).expect("B exists");
		assert_eq!(b.name, "BUS-TERMINUS-4", "the second bus became a terminus");
		assert_eq!(b.internal_data, Some(vec![origin_a as u32, 1]), "linked back to A, flip inverted relative to its old state");

		assert_eq!(chip.wires.len(), 1);
		assert!(bus_wiring::is_bus_wire(chip, &v.library, &chip.wires[0]), "the result is a proper bus wire");
		assert_eq!(chip.wires[0].source_pin_address, PinAddress::new(origin_a, 1));
		assert_eq!(chip.wires[0].target_pin_address, PinAddress::new(origin_b, 0), "the wire targets B's input");

		// The conversion kept the pin physically where it was clicked: same
		// side of the body (exact x differs because the renamed body is a
		// different width).
		let now_input = pin_pos(&v, origin_b, 0);
		assert!(now_input.x > sub_position(&v, origin_b).x, "the converted terminus' pin stays on the right");
		assert_eq!(now_input.y, clicked.y, "same row as before");
	}

	/// The mirror case: completing from one terminus onto another converts
	/// the second into a bus origin (flip inverted again) and the finished
	/// wire sources from the NEW origin's output into the first terminus'
	/// input.
	#[test]
	fn terminus_to_terminus_click_converts_the_second_terminus_into_a_linked_origin() {
		let mut v = viewer_with_builtins();
		for pos in [Vec2::ZERO, Vec2::new(20.0, 0.0)] {
			chip_interaction::start_placing(&mut v, "BUS-TERMINUS-4");
			try_place_pending_components(&mut v, pos, &mut None);
		}
		let ids: Vec<i32> = v.library.get("ROOT").sub_chips.iter().map(|s| s.id).collect();
		let (terminus_a, terminus_b) = (ids[0], ids[1]);

		let clicked = pin_pos(&v, terminus_b, 0);

		let mut status = None;
		let terminus_a_in = pin_pos(&v, terminus_a, 0);
		handle_canvas_click(&mut v, terminus_a_in, &mut status); // target-role start
		handle_canvas_click(&mut v, clicked, &mut status); // same role -- absorbed by conversion

		assert_eq!(status, None);
		assert!(v.pending_wire.is_none());
		let chip = v.library.get("ROOT");
		let b = chip.sub_chips.iter().find(|s| s.id == terminus_b).expect("B exists");
		assert_eq!(b.name, "BUS-4", "the second terminus became an origin");
		assert_eq!(b.internal_data, Some(vec![terminus_a as u32, 1]), "linked to A, flip inverted relative to its old state");

		assert_eq!(chip.wires.len(), 1);
		assert!(bus_wiring::is_bus_wire(chip, &v.library, &chip.wires[0]));
		assert_eq!(chip.wires[0].source_pin_address, PinAddress::new(terminus_b, 1), "the new origin drives the wire");
		assert_eq!(chip.wires[0].target_pin_address, PinAddress::new(terminus_a, 0));

		// Same-role rejections still apply when no bus conversion absorbs them.
		let mut v2 = viewer_with_builtins();
		let nand_a = place_nand_for_test(&mut v2, Vec2::ZERO);
		let nand_b = place_nand_for_test(&mut v2, Vec2::new(6.0, 0.0));
		let nand_out_a = pin_pos(&v2, nand_a, 2);
		let nand_out_b = pin_pos(&v2, nand_b, 2);
		handle_canvas_click(&mut v2, nand_out_a, &mut status); // NAND OUT (pin 2), source role
		handle_canvas_click(&mut v2, nand_out_b, &mut status);
		assert!(status.as_deref().is_some_and(|m| m.contains("output to an output")), "gate-to-gate output clicks are still rejected");
		assert!(v2.pending_wire.is_some(), "the placement stays active for a retry");
		assert!(v2.library.get("ROOT").wires.is_empty());
	}

	fn place_nand_for_test(v: &mut ViewerState, pos: Vec2) -> i32 {
		chip_interaction::start_placing(v, "NAND");
		try_place_pending_components(v, pos, &mut None);
		v.library.get("ROOT").sub_chips.last().expect("placed").id
	}

	fn sub_position(v: &ViewerState, id: i32) -> Vec2 {
		v.library.get("ROOT").sub_chips.iter().find(|s| s.id == id).expect("component exists").position
	}

	/// The ghost preview draws every entry of the carry, not just one --
	/// doubling an identical carry doubles its geometry.
	#[test]
	fn ghost_preview_renders_every_carried_entry() {
		use crate::viewer::chip_interaction::PendingComponent;
		let library = {
			let mut lib = ChipLibrary::new();
			crate::register_all_builtins(&mut lib);
			lib
		};
		let single = vec![(Vec2::ZERO, PendingComponent { name: "NAND".into(), linked_bus_partner: None })];
		let doubled = vec![single[0].clone(), (Vec2::new(4.0, 0.0), PendingComponent { name: "NAND".into(), linked_bus_partner: None })];

		let one = build_pending_place_scene(&library, &single, Vec2::ZERO, false);
		let two = build_pending_place_scene(&library, &doubled, Vec2::ZERO, false);

		assert!(!one.triangles.is_empty());
		assert_eq!(two.triangles.len(), one.triangles.len() * 2, "each carried entry contributes its own ghost geometry");
		assert!(one.triangles.iter().all(|v| v.pos.x < 0.4), "the single ghost hugs the cursor");
		assert!(two.triangles.iter().any(|v| v.pos.x > 3.7), "the second entry's ghost sits at cursor + its own offset");
	}
}
