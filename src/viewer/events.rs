//! Window-event handling for the integrated app: the
//! `ApplicationHandler` impl dispatching mouse/keyboard/resize events to
//! whichever screen is active, and the per-screen input handlers (left /
//! right / middle mouse, keyboard) that route through each screen's UI
//! stack.

use crate::render::context_menu::{ContextMenuAction, ContextMenuItem, ContextMenuState};
use crate::render::editor_ui::EditorAction;
use crate::render::scene::{hit_test_dev_pin, hit_test_sub_chip, hit_test_wire, place_sub_chips};
use crate::render::ui_stack::{InputResult, LayerId};
use crate::structs::Vec2;
use crate::viewer::app::{App, Screen};
use crate::viewer::state::{sync_stack_with_state, DeleteDragSweep, ViewerAction, ViewerState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::viewer::actions::apply_editor_action;
use crate::viewer::canvas::{handle_canvas_click, try_finish_pending_wire, wire_click_tolerance};
use crate::viewer::chip_interaction::{self, CanvasInteraction};
use crate::viewer::context_menu::{apply_context_menu_action, context_menu_items_for_component};
use crate::viewer::frame::{build_menu_stack, build_viewer_stack};
use crate::viewer::input::{encode_modifiers, handle_viewer_key};
use crate::viewer::library::is_custom_chip;

impl ApplicationHandler for App {
	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		if self.state.is_some() {
			return;
		}

		let window_attrs =
			winit::window::Window::default_attributes().with_title(self.window_title()).with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
		let window = std::sync::Arc::new(event_loop.create_window(window_attrs).expect("failed to create window"));

		let size = window.inner_size();
		self.viewport = Vec2::new(size.width as f32, size.height as f32);
		self.state = Some(crate::viewer::app::create_render_state(window, size));
	}

	fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
		if self.state.is_none() {
			return;
		}

		// Snapshot the toast text so anything this event set/cleared can
		// restart its auto-dismiss clock afterwards (see
		// [`App::note_status_maybe_changed`]).
		let status_before = self.status.clone();

		match event {
			WindowEvent::CloseRequested => event_loop.exit(),

			WindowEvent::Resized(size) => {
				if let Some(state) = self.state.as_mut() {
					state.renderer.resize(size.width, size.height);
				}
				self.viewport = Vec2::new(size.width as f32, size.height as f32);
				if let Screen::Viewer(v) = &mut self.screen {
					v.camera.resize_viewport(size.width as f32, size.height as f32);
				}
			}

			WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(event),

			WindowEvent::ModifiersChanged(mods) => {
				self.modifiers = mods.state();
				if let Screen::Viewer(v) = &mut self.screen {
					v.sim.set_key_modifiers(encode_modifiers(self.modifiers));
				}
			}

			// Physically-held keys don't generate a release event if focus
			// is lost while they're down (e.g. alt-tabbing away) -- without
			// this, a Key/KeyMods chip could get stuck "on" indefinitely.
			WindowEvent::Focused(false) => {
				self.modifiers = winit::keyboard::ModifiersState::empty();
				if let Screen::Viewer(v) = &mut self.screen {
					v.sim.clear_held_keys();
					v.sim.set_key_modifiers(0);
				}
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Left, .. } => {
				self.handle_mouse_button(btn_state, event_loop);
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Middle, .. } => {
				self.handle_middle_mouse_button(btn_state);
			}

			WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Right, .. } => {
				self.handle_right_mouse_button(btn_state);
			}

			WindowEvent::CursorMoved { position, .. } => {
				let cursor = Vec2::new(position.x as f32, position.y as f32);
				self.mouse_pos = cursor;
				if let Screen::Viewer(v) = &mut self.screen {
					// A selection being carried or a wire bend being dragged
					// follows the cursor in world space.
					if matches!(v.canvas_interaction, CanvasInteraction::MovingSelection { .. } | CanvasInteraction::WireBendDrag { .. }) {
						let world_pos = v.camera.screen_to_world(cursor);
						chip_interaction::update_move_to_cursor(v, world_pos);
					}
					if v.dragging {
						let before = v.camera.screen_to_world(v.last_cursor);
						let after = v.camera.screen_to_world(cursor);
						v.camera.pan(Vec2::new(before.x - after.x, before.y - after.y));
					}
					v.last_cursor = cursor;
					// Shift+right-drag delete in progress: whatever's newly
					// under the cursor is added to the sweep (dimmed for
					// visual feedback elsewhere) so the drag visibly eats
					// through the circuit as the mouse moves, but nothing
					// is actually deleted until the button is released --
					// see `Self::delete_drag_hit` and
					// `handle_right_mouse_button`'s release branch, which
					// applies the whole sweep as a single undo step.
					if v.delete_drag.is_some() {
						Self::delete_drag_hit(v, cursor)
					}
				}
			}

			WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),

			WindowEvent::RedrawRequested => self.redraw(event_loop),

			_ => {}
		}

		self.note_status_maybe_changed(&status_before);
	}
}

impl App {
	/// Left-click handling, routed through the screen's UI stack
	/// (`ViewerState::stack` / `App::menu_stack`): the click is offered to
	/// layers front-to-back and lands on the first one whose capture
	/// region contains it -- exactly the priority chain the old hand-rolled
	/// sequence of `last_*_buttons` checks implemented, now expressed as
	/// stack order instead. A click that propagates past every UI layer is
	/// a canvas click (`handle_canvas_click`); its *release* ends whatever
	/// that press started over the canvas (a selection drag or a rubber
	/// band -- see `chip_interaction::handle_canvas_release`; releases are
	/// self-guarding, so a press a UI layer swallowed never reaches it).
	/// Camera panning is *not* handled here -- see
	/// `handle_middle_mouse_button`.
	fn handle_mouse_button(&mut self, btn_state: ElementState, event_loop: &ActiveEventLoop) {
		match &mut self.screen {
			Screen::Menu => {
				if btn_state == ElementState::Pressed {
					let dispatch = self.menu_stack.dispatch_click(self.mouse_pos);
					if let Some(action) = dispatch.button.map(|b| b.action.clone()) {
						self.handle_menu_action(action, event_loop);
					}
				}
			}
			Screen::Viewer(v) => match btn_state {
				ElementState::Pressed => {
					sync_stack_with_state(v);
					let world_pos = v.camera.screen_to_world(self.mouse_pos);
					let dispatch = v.stack.dispatch_click(self.mouse_pos);
					// A left press not aimed at an open right-click popup
					// dismisses it first -- the same click then acts on
					// whatever else it landed on, mirroring the original's
					// close-on-any-left-down (`ContextMenu.Update`).
					if v.context_menu.is_some() && !Self::left_press_keeps_context_menu(dispatch.layer) {
						v.context_menu = None;
					}
					match dispatch.result {
						// Nobody wanted it: it falls through the whole stack to the canvas.
						InputResult::Propagate => {
							// Same as ever, a click anywhere outside the bar closes its open flyout --
							// with the bar no longer swallowing clicks in its padding, "outside" now
							// includes the strip between/around its buttons.
							v.bottom_bar_open_collection = None;
							handle_canvas_click(v, world_pos, &mut self.status);
						}
						InputResult::Stop => {}
						InputResult::Handled => match dispatch.layer {
							// A row inside an open starred-collection flyout acts *and* closes the
							// flyout; the flyout's own capture region swallows everything else.
							Some(LayerId::BottomBarFlyout) => {
								let shift_add = v.sim.key_modifiers() & crate::sim::key_mods_bits::SHIFT != 0;
								if let Some(ViewerAction::Editor(action)) = dispatch.button.map(|b| b.action.clone()) {
									apply_editor_action(v, &self.paths, &mut self.status, action);
								}
								if !shift_add {
									v.bottom_bar_open_collection = None;
								}
							}
							// The context menu's layer either picks one of its rows or just dismisses
							// it -- either way the click is swallowed, never reaching what's underneath.
							Some(LayerId::ContextMenu) => {
								if let (Some(ViewerAction::Context(id)), Some(target)) =
									(dispatch.button.map(|b| b.action.clone()), v.context_menu.take().map(|s| s.target))
								{
									apply_context_menu_action(v, &self.paths, &mut self.status, &target, id);
								}
							}
							// The customize workspace: hotspot clicks go through the
							// action funnel; a click landing on the preview's padding
							// commits whatever's being carried (drop placement /
							// finish resize), mirroring the canvas's own
							// "background click" idiom.
							Some(LayerId::CustomizePanel) => match dispatch.button.map(|b| b.action.clone()) {
								Some(ViewerAction::Editor(action)) => apply_editor_action(v, &self.paths, &mut self.status, action),
								None => crate::viewer::customize::handle_preview_click(v),
								_ => {}
							},
							_ => {
								if let Some(ViewerAction::Editor(action)) = dispatch.button.map(|b| b.action.clone()) {
									apply_editor_action(v, &self.paths, &mut self.status, action);
								}
							}
						},
					}
				}
				ElementState::Released => {
					let world_pos = v.camera.screen_to_world(self.mouse_pos);
					chip_interaction::handle_canvas_release(v, world_pos);
					// Finish resizing on mouse release instead of requiring
					// a second click.
					if v.stack.top_id() == Some(LayerId::CustomizePanel) && v.customize.as_ref().is_some_and(|c| c.interaction.is_resizing()) {
						crate::viewer::customize::handle_preview_click(v);
					} else if v.pending_wire.is_some() && matches!(v.stack.top_id(), None | Some(LayerId::Canvas)) {
						// An in-progress wire also tries to complete on
						// release (`HandleLeftMouseUp`'s TryFinishPlacingWire):
						// press-empty then release-over-pin finishes the wire,
						// matching the original's two chances to land.
						let mut status_before = self.status.clone();
						try_finish_pending_wire(v, world_pos, &mut status_before);
						self.note_status_maybe_changed(&status_before);
					}
				}
			},
		}
		self.check_viewer_exit_request();
	}

	/// Right-click handling:
	/// Hit-tests run in the same order things are actually drawn on top
	/// of each other (component > wire), so a click that could plausibly land on more
	/// than one resolves to whichever one the player can actually see --
	/// with UI-surface lookups now delegated to the stack instead of
	/// hand-rolled per list.
	fn handle_right_mouse_button(&mut self, btn_state: ElementState) {
		let Screen::Viewer(v) = &mut self.screen else { return };

		if v.delete_drag.is_some() && btn_state == ElementState::Released {
			if let Some(sweep) = v.delete_drag.take() {
				// Everything the drag swept over -- components and wires
				// alike -- goes in as ONE undo entry, so a single undo
				// after releasing the mouse brings the whole sweep back.
				crate::viewer::undo::delete_batch_with_undo(v, sweep.components.into_iter(), sweep.wires.into_iter());
			}
			return;
		}
		if v.sim.key_modifiers() & crate::sim::key_mods_bits::SHIFT != 0 {
			let mouse_pos = self.mouse_pos;
			match btn_state {
				ElementState::Pressed => {
					v.context_menu = None;
					Self::delete_drag_hit(v, mouse_pos);
					return;
				}
				ElementState::Released => return, // handled above
			}
		}
		// Not a delete-drag: don't leave a stale one pending forever.
		v.delete_drag = None;

		if btn_state != ElementState::Pressed {
			return;
		}

		// Right-clicking always closes whatever popup was already open
		// (matches normal desktop-app behaviour: a fresh right-click
		// replaces the previous context menu rather than stacking).
		v.context_menu = None;
		// Also the standard "cancel" gesture for an in-progress wire
		// placement, a chip pending placement, a selection drag (reverting
		// it), and the selection itself -- same as Escape (see the keyboard
		// handler).
		let cancelled_something = v.pending_wire.is_some()
			|| !v.pending_place.is_empty()
			|| !matches!(v.canvas_interaction, crate::viewer::chip_interaction::CanvasInteraction::None)
			|| crate::viewer::customize::is_interacting(v);
		v.pending_wire = None;
		v.pending_place.clear();
		chip_interaction::cancel_all(v);
		// ...and for a customize-workspace grab/resize in flight.
		crate::viewer::customize::cancel_interaction(v);
		if cancelled_something {
			// The cancel *consumed* the click (`ConsumeMouseButtonDownEvent`
			// in the original): falling through would also delete a wire or
			// open a context menu under the cursor on this very click.
			return;
		}

		// In wire edit mode, right-clicking on a bend removes it directly
		// (same as Delete on a selected bend). This mirrors the original's
		// `DeleteWirePoint` triggered by a right-click during wire editing.
		if v.wire_edit.is_some() {
			let world_pos = v.camera.screen_to_world(self.mouse_pos);
			if let Some(bend) = crate::viewer::wire_edit::bend_hit(v, world_pos) {
				v.wire_edit.as_mut().unwrap().selected_bend = Some(bend);
				crate::viewer::wire_edit::delete_selected_bend(v);
				return;
			}
		}

		sync_stack_with_state(v);

		// A right click while a *modal* overlay is open (anything but
		// the library panel) has nothing sensible to attach to, so
		// just leave the popup closed.
		let top = v.stack.top_id();
		if top.is_some_and(LayerId::is_overlay_panel) && top != Some(LayerId::Library) {
			return;
		}

		// 1) A visible UI row under the cursor: a chip row in the library
		// panel, or a chip button in the bottom bar / its open flyout.
		// All screen-space, so they're looked up through the stack rather
		// than hit-tested in world space like everything below. The
		// lookup counts disabled buttons too: a greyed-out starred chip
		// (cycle-blocked for *placement*) still offers its Open/Un-star
		// popup -- the grey only guards left-click placement.
		if let Some((layer, action)) = v.stack.topmost_button_or_disabled(self.mouse_pos).map(|(l, b)| (l, b.action.clone())) {
			match (&layer, &action) {
				(LayerId::Library, ViewerAction::Editor(EditorAction::SelectChipRow { collection, chip })) => {
					if let Some(name) = v.prefs.chip_collections.get(*collection).and_then(|c| c.chips.get(*chip)).cloned() {
						let custom = is_custom_chip(&v.library, &name);
						let items = vec![
							ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, custom),
							ContextMenuItem::new_enabled("Delete", ContextMenuAction::Delete, custom),
						];
						v.context_menu = Some(ContextMenuState::new(format!("libchip:{name}"), self.mouse_pos, items));
					}
					return;
				}
				(LayerId::BottomBar | LayerId::BottomBarFlyout, ViewerAction::Editor(EditorAction::PlaceChip(name))) => {
					let items = if layer == LayerId::BottomBar {
						vec![
							ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(&v.library, name)),
							ContextMenuItem::new("Un-star", ContextMenuAction::Unstar),
						]
					} else {
						vec![ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(&v.library, name))]
					};
					let prefix = if layer == LayerId::BottomBar { "barchip" } else { "flyoutchip" };
					v.context_menu = Some(ContextMenuState::new(format!("{prefix}:{name}"), self.mouse_pos, items));
					return;
				}
				_ => {}
			}
		}

		let root_chip_name = v.root_chip_name.clone();
		let world_pos = v.camera.screen_to_world(self.mouse_pos);
		let can_edit = v.can_edit_viewed_chip();
		// The canvas shows whichever chip tops the view stack (the edited
		// root when none is open), so a component popup attaches to that
		// one -- "View" stays available even in view-only mode, everything
		// else below is edited-chip business.
		let displayed_chip_name = match v.resolve_scene_target() {
			crate::viewer::state::SceneTarget::EditRoot => root_chip_name.clone(),
			crate::viewer::state::SceneTarget::Viewed { name, .. } => name,
		};

		// 2) One of the current root chip's own boundary dev-pins. "Edit"
		// opens the pin-edit popup (`PinEditMenu`: rename +, for multi-bit
		// pins, the Decimal Display wheel).
		if can_edit {
			let root_desc = v.library.get(&root_chip_name);
			if let Some((is_input, pin_id)) = hit_test_dev_pin(root_desc, world_pos) {
				let target = format!("devpin:{}:{}", if is_input { "in" } else { "out" }, pin_id);
				let items =
					vec![ContextMenuItem::new("Edit", ContextMenuAction::Configure), ContextMenuItem::new("Delete", ContextMenuAction::Delete)];
				v.context_menu = Some(ContextMenuState::new(target, self.mouse_pos, items));
				return;
			}
		}

		// 3) A placed component on whatever chip is currently displayed.
		{
			let displayed_desc = v.library.get(&displayed_chip_name);
			let placed = place_sub_chips(displayed_desc, &v.library);
			if let Some(sub) = hit_test_sub_chip(&placed, world_pos) {
				let id = sub.id;
				let chip_name = sub.desc.name.clone();
				let mut items = context_menu_items_for_component(&v.library, &chip_name);
				if !can_edit {
					// View-only mode: watching deeper is allowed, editing/
					// configuring/deleting is not (`CanEditViewedChip`).
					// Only *enabled* View rows survive -- e.g. right-
					// clicking a builtin inside the viewed chip offers
					// nothing and opens no popup at all.
					items.retain(|item| matches!(item.id, ContextMenuAction::View) && item.enabled);
				}
				if items.is_empty() {
					return;
				}
				v.context_menu = Some(ContextMenuState::new(format!("component:{id}"), self.mouse_pos, items));
				return;
			}
		}

		// 4) A wire -- EDIT/DELETE popup (`ContextMenu`'s entries_wire);
		// "Edit" enters wire edit mode (draggable bends), "Delete" removes
		// the wire. Edited-chip territory only.
		if can_edit {
			// Fixed screen-pixel tolerance converted to world units, so the click target stays the
			// same apparent size regardless of current zoom.
			let hit = {
				let root_desc = v.library.get(&root_chip_name);
				let max_dist = wire_click_tolerance(&v.camera);
				hit_test_wire(root_desc, &v.library, world_pos, max_dist)
			};
			if let Some(wire_idx) = hit {
				let items = vec![
					ContextMenuItem::new("Edit", ContextMenuAction::Edit),
					ContextMenuItem::new("Delete Part", ContextMenuAction::DeletePart),
					ContextMenuItem::new("Delete", ContextMenuAction::Delete),
				];
				v.context_menu = Some(ContextMenuState::new(format!("wire:{wire_idx}"), self.mouse_pos, items));
			}
		}
	}

	/// What a shift+right-drag delete step's cursor position landed on.
	/// Only *records* the hit into `v.delete_drag` -- nothing is actually
	/// deleted here. The whole sweep is applied in one shot as a single
	/// undo entry once the button comes back up (see
	/// `handle_right_mouse_button`'s release branch, which hands the
	/// accumulated sweep to `undo::delete_batch_with_undo`). Dimming
	/// already-swept elements while the drag is in progress is handled
	/// elsewhere, reading `v.delete_drag` directly.
	fn delete_drag_hit(v: &mut ViewerState, mouse_pos: Vec2) {
		if v.delete_drag.is_none() {
			v.delete_drag = Some(DeleteDragSweep::default());
		}
		if !v.can_edit_viewed_chip() {
			return;
		}

		let component_hit = {
			let displayed_chip_name = match v.resolve_scene_target() {
				crate::viewer::state::SceneTarget::EditRoot => v.root_chip_name.clone(),
				crate::viewer::state::SceneTarget::Viewed { name, .. } => name,
			};
			let world_pos = v.camera.screen_to_world(mouse_pos);
			let displayed_desc = v.library.get(&displayed_chip_name);
			let placed = place_sub_chips(displayed_desc, &v.library);
			hit_test_sub_chip(&placed, world_pos).map(|sub| sub.id)
		};
		if let Some(id) = component_hit {
			let sweep = v.delete_drag.as_mut().expect("ensured Some above");
			if !sweep.components.contains(&id) {
				sweep.components.push(id);
			}
		}

		let root_chip_name = v.root_chip_name.clone();
		let world_pos = v.camera.screen_to_world(mouse_pos);
		let max_dist = wire_click_tolerance(&v.camera);
		let wire_hit = {
			let root_desc = v.library.get(&root_chip_name);
			hit_test_wire(root_desc, &v.library, world_pos, max_dist)
		};
		if let Some(wire_idx) = wire_hit {
			let sweep = v.delete_drag.as_mut().expect("ensured Some above");
			if !sweep.wires.contains(&wire_idx) {
				sweep.wires.push(wire_idx);
			}
		}
	}

	/// Middle-click handling: drags/pans the camera, exactly like left-click
	/// used to. Split out from `handle_mouse_button` so left-click is free
	/// to toggle input dev-pins instead, and right-click free for
	/// `handle_right_mouse_button`'s context-menu popup. Panning starts
	/// only where a press would propagate past every UI layer (i.e. land
	/// on the canvas) -- the same "swallow clicks while a modal popup is
	/// open" gate the old code applied by hand -- but releasing always
	/// stops an in-flight drag, so a drag started on the canvas can end
	/// anywhere.
	fn handle_middle_mouse_button(&mut self, btn_state: ElementState) {
		if let Screen::Viewer(v) = &mut self.screen {
			if btn_state == ElementState::Pressed {
				sync_stack_with_state(v);
				// Panning is never aimed at the popup.
				if v.context_menu.is_some() {
					v.context_menu = None;
				}
				if v.stack.dispatch_click(self.mouse_pos).result != InputResult::Propagate {
					return;
				}
			}
			v.dragging = btn_state == ElementState::Pressed;
		}
	}

	/// Whether a left press / wheel event resolved to the open right-click
	/// popup itself (and so should keep it open); anything else --
	/// including bare-canvas clicks -- dismisses it. Split out for the
	/// truth-table test below.
	fn left_press_keeps_context_menu(layer: Option<LayerId>) -> bool {
		layer == Some(LayerId::ContextMenu)
	}

	fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
		let scroll = match delta {
			MouseScrollDelta::LineDelta(_, y) => y,
			MouseScrollDelta::PixelDelta(p) => (p.y / 100.0) as f32,
		};
		if let Screen::Viewer(v) = &mut self.screen {
			sync_stack_with_state(v);
			let dispatch = v.stack.dispatch_wheel(self.mouse_pos);
			// Wheeling anywhere but over the popup itself dismisses it.
			if v.context_menu.is_some() && !Self::left_press_keeps_context_menu(dispatch.layer) {
				v.context_menu = None;
			}
			match dispatch.result {
				// Over the bottom bar's scrollable strip: scroll the bar horizontally
				// instead of zooming the canvas underneath it.
				InputResult::Handled if dispatch.layer == Some(LayerId::BottomBar) && !dispatch.scroll_regions.is_empty() => {
					v.bottom_bar_scroll_x = (v.bottom_bar_scroll_x - scroll * 40.0).clamp(0.0, v.bottom_bar_scroll_max.max(0.0));
				}
				// Customize workspace: wheel over the DISPLAYS viewport scrolls
				// the list; over the preview it zooms the chip preview.
				InputResult::Handled if dispatch.layer == Some(LayerId::CustomizePanel) => {
					let in_list = v.customize.as_ref().is_some_and(|c| c.layout.valid && c.layout.list.contains(self.mouse_pos));
					if in_list {
						crate::viewer::customize::scroll_list(v, scroll * 40.0);
					} else {
						crate::viewer::customize::zoom_preview(v, 1.0 + scroll * 0.1);
					}
				}
				// Swallowed by some other capturing layer (a modal panel, flyout, popup...).
				InputResult::Handled | InputResult::Stop => {}
				// Nobody wanted it: zoom the camera, as before.
				InputResult::Propagate => {
					let zoom_factor = 1.0 + scroll * 0.1;
					v.camera.zoom_at(v.last_cursor, zoom_factor);
				}
			}
		}
	}

	pub(crate) fn handle_key_event(&mut self, event: winit::event::KeyEvent) {
		let pressed = event.state == ElementState::Pressed;
		// Feed the Key chip's held-key set on both press and release (not just press, unlike the
		// shortcut handling below) since it needs to know when a key stops being held. The chip
		// stores/compares its target letter in capitals, so lowercase 'a' must register as 'A' here.
		//
		// Typed characters are only *simulation input* while no UI surface wants them, though:
		// `UiStack::keyboard_stop` says when the top of the stack owns typing outright (a text
		// field, or the key-select popup capturing its next key as data) -- configuring a Key chip
		// to 'A' must not itself hold 'A' down in the simulator. Only *presses* are gated;
		// releases always go through, so a key that was being held when a text overlay opened can
		// never get stuck "on".
		if let Some(c) = crate::viewer::input::char_for_keys(event.physical_key, &event.logical_key) {
			if let Screen::Viewer(v) = &mut self.screen {
				if v.stack.keyboard_stop() && c.is_ascii_alphanumeric() {
					if pressed {
						v.sim.held_key_press(c);
					} else {
						v.sim.held_key_release(c);
					}
				}
			}
		}

		if !pressed {
			return;
		}

		match &mut self.screen {
			Screen::Menu => {
				if self.is_text_popup_open() {
					match &event.logical_key {
						Key::Named(NamedKey::Backspace) => {
							self.text_input.pop();
						}
						Key::Named(NamedKey::Enter) => self.confirm_popup(),
						Key::Named(NamedKey::Escape) => {
							self.menu.cancel_popup();
							self.text_input.clear();
						}
						Key::Character(s) if self.text_input.chars().count() < crate::ui_menu::MAX_PROJECT_NAME_LENGTH => {
							self.text_input.push_str(s);
						}
						_ => {}
					}
				} else if self.menu.popup() == crate::ui_menu::PopupKind::DeleteConfirmation {
					match &event.logical_key {
						Key::Named(NamedKey::Enter) => self.confirm_popup(),
						Key::Named(NamedKey::Escape) => self.menu.cancel_popup(),
						_ => {}
					}
				} else if event.logical_key == Key::Named(NamedKey::Escape) {
					self.return_to_menu();
				}
			}
			Screen::Viewer(v) => {
				sync_stack_with_state(v);
				let old_name = v.root_chip_name.clone();
				handle_viewer_key(v, &self.paths, &mut self.status, &event, self.modifiers);
				if v.root_chip_name != old_name {
					self.set_window_title();
				}
			}
		}
		self.check_viewer_exit_request();
	}

	/// Runs the deferred leave-the-editor transition the viewer asked for
	/// via [`ViewerState::exit_requested`] (the unsaved-changes popup's
	/// confirmed "leave" action; the viewer can't swap screens itself).
	/// Called after every viewer input-dispatch site so both a clicked
	/// Continue button and a pressed Enter funnel through the same check.
	fn check_viewer_exit_request(&mut self) {
		if matches!(self.screen, Screen::Viewer(ref v) if v.exit_requested) {
			self.return_to_menu();
		}
	}

	fn redraw(&mut self, event_loop: &ActiveEventLoop) {
		// The toast dismisses itself after its linger window -- the render
		// loop keeps ticking even with no input, so no interaction is
		// ever needed for this.
		self.expire_status_toast();

		// Keep the window title in sync with the current chip name --
		// cheap to call and avoids tracking dirty flags across every
		// code path that mutates `root_chip_name`.
		self.set_window_title();

		let (vw, vh) = self.viewport.to_tuple();

		// Rebuild this screen's whole UI stack from live state -- layers bottom-to-top, each drawn
		// back-to-front as its own fully-submitted pass, so a later layer's triangles paint over an
		// earlier layer's *text* too (the reason the old fixed triple-pass layout couldn't stack
		// surfaces). Input dispatch in the handlers consults exactly what's built here.
		match &mut self.screen {
			Screen::Menu => {
				self.menu_stack = build_menu_stack(&self.menu, &self.text_input, self.status.as_ref(), vw, vh, self.mouse_pos);
			}
			Screen::Viewer(v) => {
				let status = self.status.clone();
				let viewer_stack = build_viewer_stack(v, status.as_deref(), vw, vh, self.mouse_pos);
				v.stack = viewer_stack;
			}
		}

		let camera = match &self.screen {
			Screen::Menu => crate::viewer::frame::menu_camera(vw, vh),
			Screen::Viewer(v) => v.camera,
		};

		if let Some(state) = self.state.as_mut() {
			let geoms = match &self.screen {
				Screen::Menu => self.menu_stack.geometries(),
				Screen::Viewer(v) => v.stack.geometries(),
			};
			match state.renderer.render(&geoms, &camera, crate::render::theme::BACKGROUND_COL) {
				Ok(()) => {}
				Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
					let size = state.window.inner_size();
					state.renderer.resize(size.width, size.height);
				}
				Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
				Err(e) => eprintln!("render error: {e:?}"),
			}
			state.window.request_redraw();
		}
	}
}

#[cfg(test)]
mod context_menu_dismiss_tests {
	use super::*;

	#[test]
	fn only_the_popup_layer_keeps_the_context_menu_open() {
		assert!(App::left_press_keeps_context_menu(Some(LayerId::ContextMenu)), "a press the popup consumed keeps it");
		assert!(!App::left_press_keeps_context_menu(None), "bare-canvas clicks dismiss it");
		for layer in [LayerId::BottomBar, LayerId::Library, LayerId::Search, LayerId::Naming] {
			assert!(!App::left_press_keeps_context_menu(Some(layer)), "clicking {layer:?} must dismiss the popup before acting");
		}
	}
}
