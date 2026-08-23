//! Window-event handling for the integrated app: the
//! `ApplicationHandler` impl dispatching mouse/keyboard/resize events to
//! whichever screen is active, and the per-screen input handlers (left /
//! right / middle mouse, keyboard) that route through each screen's UI
//! stack.

use crate::render::context_menu::{ContextMenuAction, ContextMenuItem, ContextMenuState};
use crate::render::editor_ui::EditorAction;
use crate::render::scene::{delete_wire, hit_test_dev_pin, hit_test_sub_chip, hit_test_wire, place_sub_chips};
use crate::render::ui_stack::{InputResult, LayerId};
use crate::structs::Vec2;
use crate::viewer::app::{App, Screen};
use crate::viewer::state::{sync_stack_with_state, ViewerAction};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::viewer::actions::apply_editor_action;
use crate::viewer::canvas::{handle_canvas_click, wire_click_tolerance};
use crate::viewer::context_menu::{apply_context_menu_action, context_menu_items_for_component};
use crate::viewer::frame::{build_menu_stack, build_viewer_stack};
use crate::viewer::input::{encode_modifiers, handle_viewer_key, KeyOutcome};
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
					v.sim.key_modifiers = encode_modifiers(self.modifiers);
				}
			}

			// Physically-held keys don't generate a release event if focus
			// is lost while they're down (e.g. alt-tabbing away) -- without
			// this, a Key/KeyMods chip could get stuck "on" indefinitely.
			WindowEvent::Focused(false) => {
				self.modifiers = winit::keyboard::ModifiersState::empty();
				if let Screen::Viewer(v) = &mut self.screen {
					v.sim.held_keys.clear();
					v.sim.key_modifiers = 0;
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
					if v.dragging {
						let before = v.camera.screen_to_world(v.last_cursor);
						let after = v.camera.screen_to_world(cursor);
						v.camera.pan(Vec2::new(before.x - after.x, before.y - after.y));
					}
					v.last_cursor = cursor;
				}
			}

			WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),

			WindowEvent::RedrawRequested => self.redraw(event_loop),

			_ => {}
		}
	}
}

impl App {
	/// Left-click handling, routed through the screen's UI stack
	/// (`ViewerState::stack` / `App::menu_stack`): the click is offered to
	/// layers front-to-back and lands on the first one whose capture
	/// region contains it -- exactly the priority chain the old hand-rolled
	/// sequence of `last_*_buttons` checks implemented, now expressed as
	/// stack order instead. A click that propagates past every UI layer is
	/// a canvas click (`handle_canvas_click`). Camera panning is *not*
	/// handled here -- see `handle_middle_mouse_button`.
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
			Screen::Viewer(v) => {
				if btn_state != ElementState::Pressed {
					return;
				}
				sync_stack_with_state(v);
				let world_pos = v.camera.screen_to_world(self.mouse_pos);
				let dispatch = v.stack.dispatch_click(self.mouse_pos);
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
							if let Some(ViewerAction::Editor(action)) = dispatch.button.map(|b| b.action.clone()) {
								apply_editor_action(v, &self.paths, &mut self.status, action);
							}
							v.bottom_bar_open_collection = None;
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
						_ => {
							if let Some(ViewerAction::Editor(action)) = dispatch.button.map(|b| b.action.clone()) {
								apply_editor_action(v, &self.paths, &mut self.status, action);
							}
						}
					},
				}
			}
		}
	}

	/// Right-click handling:
	///  - always first cancels whatever popup / pending wire / pending
	///    chip was in progress (the standard "cancel" gesture, same as
	///    Escape);
	///  - over a modal overlay panel other than the library, nothing
	///    sensible to attach to -- leaves everything closed;
	///  - on a library chip row (the panel itself), a starred bottom-bar
	///    chip button, or a row of an open bar flyout -- all found via
	///    `UiStack::topmost_button`, i.e. "whatever row is visibly on
	///    top" -- opens the generic context-menu popup from
	///    `render::context_menu` with whichever rows apply;
	///  - on a placed component on the canvas, a dev-pin of the current
	///    root chip, opens that same popup (`context_menu_items_for_component`);
	///  - on a wire, deletes it immediately (no popup -- see
	///    `scene::hit_test_wire`/`delete_wire`'s docs for the "shortest
	///    possible section" semantics);
	///  - anywhere else, just closes whatever popup was already open.
	///
	/// Hit-tests run in the same order things are actually drawn on top
	/// of each other (library row > flyout row > bottom bar > dev-pin >
	/// component > wire), so a click that could plausibly land on more
	/// than one resolves to whichever one the player can actually see --
	/// with UI-surface lookups now delegated to the stack instead of
	/// hand-rolled per list.
	fn handle_right_mouse_button(&mut self, btn_state: ElementState) {
		if btn_state != ElementState::Pressed {
			return;
		}
		let Screen::Viewer(v) = &mut self.screen else { return };

		// Right-clicking always closes whatever popup was already open
		// (matches normal desktop-app behaviour: a fresh right-click
		// replaces the previous context menu rather than stacking).
		v.context_menu = None;
		// Also the standard "cancel" gesture for an in-progress wire
		// placement or a chip pending placement, same as Escape (see
		// the keyboard handler).
		v.pending_wire = None;
		v.pending_place = None;

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
		// than hit-tested in world space like everything below.
		if let Some((layer, action)) = v.stack.topmost_button(self.mouse_pos).map(|(l, b)| (l, b.action.clone())) {
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

		// 2) One of the current root chip's own boundary dev-pins.
		{
			let root_desc = v.library.get(&root_chip_name);
			if let Some((is_input, pin_id)) = hit_test_dev_pin(root_desc, world_pos) {
				let target = format!("devpin:{}:{}", if is_input { "in" } else { "out" }, pin_id);
				let items = vec![ContextMenuItem::new("Label", ContextMenuAction::Label), ContextMenuItem::new("Delete", ContextMenuAction::Delete)];
				v.context_menu = Some(ContextMenuState::new(target, self.mouse_pos, items));
				return;
			}
		}

		// 3) A placed component on the canvas.
		{
			let root_desc = v.library.get(&root_chip_name);
			let placed = place_sub_chips(root_desc, &v.library);
			if let Some(sub) = hit_test_sub_chip(&placed, world_pos) {
				let id = sub.id;
				let chip_name = sub.desc.name.clone();
				let items = context_menu_items_for_component(&v.library, &chip_name);
				v.context_menu = Some(ContextMenuState::new(format!("component:{id}"), self.mouse_pos, items));
				return;
			}
		}

		// 4) A wire -- deleted immediately, no popup (see this method's
		// doc comment).
		{
			let root_desc = v.library.get(&root_chip_name);
			// Fixed screen-pixel tolerance converted to world units, so the click target stays the
			// same apparent size regardless of current zoom.
			let max_dist = wire_click_tolerance(&v.camera);
			if let Some(wire_idx) = hit_test_wire(root_desc, &v.library, world_pos, max_dist) {
				let chip = v.library.get_mut(&root_chip_name);
				delete_wire(chip, wire_idx);
				v.rebuild_sim();
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
				if v.stack.dispatch_click(self.mouse_pos).result != InputResult::Propagate {
					return;
				}
			}
			v.dragging = btn_state == ElementState::Pressed;
		}
	}

	fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
		let scroll = match delta {
			MouseScrollDelta::LineDelta(_, y) => y,
			MouseScrollDelta::PixelDelta(p) => (p.y / 100.0) as f32,
		};
		if let Screen::Viewer(v) = &mut self.screen {
			sync_stack_with_state(v);
			let dispatch = v.stack.dispatch_wheel(self.mouse_pos);
			match dispatch.result {
				// Over the bottom bar's scrollable strip: scroll the bar horizontally
				// instead of zooming the canvas underneath it.
				InputResult::Handled if dispatch.layer == Some(LayerId::BottomBar) && !dispatch.scroll_regions.is_empty() => {
					v.bottom_bar_scroll_x = (v.bottom_bar_scroll_x - scroll * 40.0).clamp(0.0, v.bottom_bar_scroll_max.max(0.0));
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
		if let Key::Character(s) = &event.logical_key {
			if let Screen::Viewer(v) = &mut self.screen {
				let press_swallowed_by_ui = event.state == ElementState::Pressed && v.stack.keyboard_stop();
				if !press_swallowed_by_ui {
					if let Some(c) = s.chars().next() {
						let c = c.to_ascii_uppercase();
						match event.state {
							ElementState::Pressed => {
								v.sim.held_keys.insert(c);
							}
							ElementState::Released => {
								v.sim.held_keys.remove(&c);
							}
						}
					}
				}
			}
		}

		if event.state != ElementState::Pressed {
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
				if let KeyOutcome::ReturnToMenu = handle_viewer_key(v, &self.paths, &mut self.status, &event, self.modifiers) {
					self.return_to_menu();
				}
			}
		}
	}

	fn redraw(&mut self, event_loop: &ActiveEventLoop) {
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
