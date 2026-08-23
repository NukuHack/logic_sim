//! The UI layer stack: an ordered list of immediate-mode layers that everything on screen is
//! built from, replacing the old fixed `[world, ui_overlay, context_menu]` triple plus the
//! hand-coded priority chains over six separate `last_*_buttons` lists. Layers are pushed
//! bottom-to-top each frame (canvas first, popups last); rendering walks them front-to-back so
//! every layer's triangles *and* text composite whole over the layer beneath it (a layer can never
//! have its text painted over by an earlier layer's shapes -- the bug that motivated this). Input
//! is offered to the stack top-first via [`UiStack::dispatch_click`] / [`UiStack::dispatch_wheel`]
//! and resolves to [`InputResult::Handled`] (a layer consumed it), [`InputResult::Propagate`]
//! (nobody wanted it -- it falls through to whatever is under the whole stack, in practice the
//! canvas) or [`InputResult::Stop`] (consumed *and* swallowed from app-level listeners too).
//! Keyboard focus is resolved by [`UiStack::keyboard_target`] (with [`UiStack::keyboard_stop`]
//! saying whether typed characters are UI data rather than simulation input), right-click row
//! lookups by [`UiStack::topmost_button`]; the canvas sits at the bottom of the stack and is
//! simply what an event falls through to when every layer propagates it.

use crate::render::scene::SceneGeometry;
use crate::render::ui_kit::{Button, Frame, UiRect};
use crate::structs::Vec2;

/// What happened when an input event was offered to the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResult {
	/// A layer consumed the event. Layers underneath are skipped, but app-level listeners that
	/// run regardless of the UI (e.g. feeding held keys to the simulation) still see it.
	Handled,
	/// No layer wanted it; it keeps travelling down and belongs to whatever is under the whole
	/// stack -- in practice the canvas/world.
	Propagate,
	/// Consumed *and* fully swallowed: even app-level always-on listeners must not react to it
	/// (e.g. a character typed into a text field must not trigger shortcuts or Key chips).
	Stop,
}

/// Identifies which layer of the stack did (or would) receive an event, so the host can apply
/// layer-specific side effects (e.g. "a click that landed on the flyout's padding closes the
/// flyout") without the stack itself knowing anything about chips or projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerId {
	MenuScreen,
	MenuPopup,
	/// The chip canvas / world. Always the bottom-most layer; never captures input itself.
	Canvas,
	BottomBar,
	BottomBarFlyout,
	Library,
	Search,
	Preferences,
	Naming,
	KeySelect,
	RomEditor,
	SaveChip,
	/// Transient status/error message. Never captures anything.
	StatusToast,
	ContextMenu,
}

impl LayerId {
	/// Whether this layer is one of the full-screen editor panels opened "on" the viewer
	/// (`ViewerState::overlay`, plus the search popup which stacks independently above any of
	/// them). Used to keep the stack in sync with live state between redraws.
	pub fn is_overlay_panel(self) -> bool {
		matches!(
			self,
			LayerId::Library | LayerId::Search | LayerId::Preferences | LayerId::Naming | LayerId::KeySelect | LayerId::RomEditor | LayerId::SaveChip
		)
	}

	/// Whether events should route here for keyboard purposes: full-screen modals, any layer
	/// owning a text field, and the context menu. Pointer-hover surfaces (bottom bar, flyout,
	/// toast) are deliberately excluded -- hovering the bar must not steal the editor's
	/// shortcuts.
	pub fn captures_keyboard(self) -> bool {
		if self.is_overlay_panel() {
			return true;
		}
		matches!(self, LayerId::MenuScreen | LayerId::MenuPopup | LayerId::ContextMenu)
	}
}

/// Which part of a layer claims mouse events that miss its buttons.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Capture {
	/// Only button hits belong to this layer; everything else passes through.
	#[default]
	None,
	/// Events inside this rect belong to the layer even when they hit no button.
	Rect(UiRect),
	/// Every event on screen belongs to this layer (full-screen modal panels).
	FullScreen,
}

impl Capture {
	fn contains(&self, pos: Vec2) -> bool {
		match self {
			Capture::None => false,
			Capture::Rect(r) => r.contains(pos),
			Capture::FullScreen => true,
		}
	}
}

/// One layer of the stack: what to draw, what can be clicked, and which regions it claims.
pub struct StackLayer<A> {
	pub id: LayerId,
	pub geometry: SceneGeometry,
	pub buttons: Vec<Button<A>>,
	/// Hit-box of this layer's text-entry field, if any -- presence marks the layer as a
	/// keyboard focus target (see [`UiStack::keyboard_target`]).
	pub text_field: Option<UiRect>,
	pub capture: Capture,
	/// Vertical/horizontal viewports whose wheel events this layer scrolls. Empty = the layer
	/// never consumes scrolling (but may still block it from reaching lower layers, via `capture`).
	pub scroll_regions: Vec<UiRect>,
}

impl<A> StackLayer<A> {
	pub fn new(id: LayerId, capture: Capture) -> Self {
		Self { id, geometry: SceneGeometry::default(), buttons: Vec::new(), text_field: None, capture, scroll_regions: Vec::new() }
	}

	/// Adopts a [`Frame`] built by one of the `menu_ui`/`editor_ui`/context-menu builders:
	/// geometry, buttons and text field move in as-is. Accepts a `Frame<B>` of any action type
	/// convertible into this stack's `A` -- for `Frame<A>` itself the reflexive
	/// `impl From<T> for T` makes the plain call just work.
	pub fn from_frame<B>(id: LayerId, frame: Frame<B>, capture: Capture) -> Self
	where
		A: From<B>,
	{
		Self::convert_frame(id, frame, capture, A::from)
	}

	/// Adopts a [`Frame<B>`] whose buttons carry a *different* action type than this stack's,
	/// mapping each into `A` (e.g. an `EditorFrame`'s `EditorAction`s wrapped in the host app's
	/// unified viewer-action enum). Geometry, text field and panel rect move over unchanged.
	pub fn convert_frame<B>(id: LayerId, frame: Frame<B>, capture: Capture, map: impl Fn(B) -> A) -> Self {
		Self {
			id,
			geometry: frame.geometry,
			buttons: frame.buttons.into_iter().map(|b| Button { rect: b.rect, action: map(b.action), enabled: b.enabled }).collect(),
			text_field: frame.text_field,
			capture,
			scroll_regions: Vec::new(),
		}
	}

	pub fn with_geometry(mut self, geometry: SceneGeometry) -> Self {
		self.geometry = geometry;
		self
	}

	pub fn with_scroll_region(mut self, rect: UiRect) -> Self {
		self.scroll_regions.push(rect);
		self
	}
}

/// What a dispatched event resolved to: how far it got, where it stopped, and (for clicks) the
/// button under the cursor. `layer`/`button`/`scroll_regions` are all `None`/empty exactly when
/// `result` is [`InputResult::Propagate`].
pub struct Dispatch<'a, A> {
	pub result: InputResult,
	pub layer: Option<LayerId>,
	pub button: Option<&'a Button<A>>,
	pub scroll_regions: &'a [UiRect],
}

impl<A> Dispatch<'_, A> {
	fn propagated() -> Self {
		Self { result: InputResult::Propagate, layer: None, button: None, scroll_regions: &[] }
	}
}

/// The stack itself. Index 0 is drawn first (furthest back); input dispatch walks in reverse.
pub struct UiStack<A> {
	layers: Vec<StackLayer<A>>,
}

impl<A> Default for UiStack<A> {
	fn default() -> Self {
		Self { layers: Vec::new() }
	}
}

impl<A> UiStack<A> {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn push(&mut self, layer: StackLayer<A>) {
		self.layers.push(layer);
	}

	pub fn layers(&self) -> &[StackLayer<A>] {
		&self.layers
	}

	pub fn top_id(&self) -> Option<LayerId> {
		self.layers.last().map(|l| l.id)
	}

	/// Bottom-to-top geometry references, ready to hand to `gpu::Renderer::render`.
	pub fn geometries(&self) -> Vec<&SceneGeometry> {
		self.layers.iter().map(|l| &l.geometry).collect()
	}

	/// Removes the top layer when `predicate` matches it -- used to keep the stack in sync with
	/// live app state between redraws (e.g. a popup closed since the last frame must not eat one
	/// more click). Returns whether anything was removed.
	pub fn pop_if_top(&mut self, predicate: impl Fn(LayerId) -> bool) -> bool {
		match self.layers.last() {
			Some(top) if predicate(top.id) => {
				self.layers.pop();
				true
			}
			_ => false,
		}
	}

	/// Repeatedly drops top layers matching `predicate` (same purpose as [`pop_if_top`], for
	/// when several stacked things closed at once).
	pub fn pop_while_top(&mut self, predicate: impl Fn(LayerId) -> bool) {
		while self.pop_if_top(&predicate) {}
	}

	/// The layer keyboard events belong to: the topmost layer that either captures the keyboard
	/// by nature (full-screen modal, context menu) or owns a text field. `None` means "no UI has
	/// focus" -- keys fall through to the canvas's own handling (shortcuts).
	pub fn keyboard_target(&self) -> Option<LayerId> {
		self.layers.iter().rev().find(|l| l.id.captures_keyboard()).map(|l| l.id)
	}

	/// Whether the current keyboard target claims typed characters *outright* (the
	/// [`InputResult::Stop`] half of key routing): it owns a text field, or it's the key-select
	/// popup whose whole point is capturing the next keystroke. While this is true, characters
	/// must not also reach app-level always-on listeners (e.g. the simulation's Key chips).
	pub fn keyboard_stop(&self) -> bool {
		self.layers.iter().rev().find(|l| l.id.captures_keyboard()).is_some_and(|l| l.text_field.is_some() || l.id == LayerId::KeySelect)
	}

	/// The topmost *enabled button* under `pos` across every layer, captures ignored -- used for
	/// right-click "open a menu on whatever row I'm over" routing, where a click on a row of a
	/// panel should find that row even though the panel's padding around it would normally
	/// swallow clicks first.
	pub fn topmost_button(&self, pos: Vec2) -> Option<(LayerId, &Button<A>)> {
		self.layers.iter().rev().find_map(|l| l.buttons.iter().find(|b| b.enabled && b.rect.contains(pos)).map(|b| (l.id, b)))
	}

	/// Offers a left-click at `pos` to the stack, top layer first. A layer takes the click when
	/// an enabled button of its own contains `pos`, or when its capture region does; otherwise it
	/// propagates down. Falling off the bottom returns [`InputResult::Propagate`] -- the click
	/// belongs to the canvas.
	pub fn dispatch_click(&self, pos: Vec2) -> Dispatch<'_, A> {
		for layer in self.layers.iter().rev() {
			if let Some(button) = layer.buttons.iter().find(|b| b.enabled && b.rect.contains(pos)) {
				return Dispatch { result: InputResult::Handled, layer: Some(layer.id), button: Some(button), scroll_regions: &[] };
			}
			if layer.capture.contains(pos) {
				return Dispatch { result: InputResult::Handled, layer: Some(layer.id), button: None, scroll_regions: &[] };
			}
		}
		Dispatch::propagated()
	}

	/// Offers a mouse-wheel event at `pos` to the stack, top layer first: a layer consumes it if
	/// one of its scroll regions contains `pos` (the host then scrolls that surface) or, failing
	/// that, its capture region does (the wheel is swallowed so e.g. zoom can't act "through" a
	/// modal). Otherwise the event propagates down, ultimately to the canvas's zoom.
	pub fn dispatch_wheel(&self, pos: Vec2) -> Dispatch<'_, A> {
		for layer in self.layers.iter().rev() {
			if layer.scroll_regions.iter().any(|r| r.contains(pos)) {
				return Dispatch { result: InputResult::Handled, layer: Some(layer.id), button: None, scroll_regions: &layer.scroll_regions };
			}
			if layer.capture.contains(pos) {
				return Dispatch { result: InputResult::Handled, layer: Some(layer.id), button: None, scroll_regions: &[] };
			}
		}
		Dispatch::propagated()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn button_layer(id: LayerId, rect: UiRect, capture: Capture) -> StackLayer<&'static str> {
		let mut frame: Frame<&'static str> = Frame::default();
		frame.geometry.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), [1.0, 1.0, 1.0, 1.0]);
		frame.buttons.push(Button { rect, action: "btn", enabled: true });
		StackLayer::from_frame(id, frame, capture)
	}

	fn point(x: f32, y: f32) -> Vec2 {
		Vec2::new(x, y)
	}

	#[test]
	fn geometries_render_bottom_to_top() {
		let mut stack = UiStack::new();
		stack.push(button_layer(LayerId::Canvas, UiRect::new(0.0, 0.0, 10.0, 10.0), Capture::None));
		stack.push(button_layer(LayerId::ContextMenu, UiRect::new(0.0, 0.0, 10.0, 10.0), Capture::Rect(UiRect::new(0.0, 0.0, 5.0, 5.0))));
		assert_eq!(stack.top_id(), Some(LayerId::ContextMenu));
		assert_eq!(stack.geometries().len(), 2);
	}

	#[test]
	fn click_goes_to_the_topmost_button_first() {
		let shared = UiRect::new(0.0, 0.0, 50.0, 50.0);
		let mut stack = UiStack::new();
		stack.push(button_layer(LayerId::BottomBar, shared, Capture::Rect(shared)));
		stack.push(button_layer(LayerId::ContextMenu, shared, Capture::Rect(shared)));

		let d = stack.dispatch_click(point(25.0, 25.0));
		assert_eq!(d.result, InputResult::Handled);
		assert_eq!(d.layer, Some(LayerId::ContextMenu));
		assert_eq!(d.button.map(|b| b.action), Some("btn"));
	}

	#[test]
	fn click_falls_through_uncaptured_empty_space_to_the_bottom() {
		let mut stack = UiStack::new();
		stack.push(StackLayer::<&'static str>::new(LayerId::Canvas, Capture::None));
		stack.push(button_layer(LayerId::StatusToast, UiRect::new(100.0, 100.0, 40.0, 20.0), Capture::None));

		let d = stack.dispatch_click(point(5.0, 5.0));
		assert_eq!(d.result, InputResult::Propagate);
		assert_eq!(d.layer, None);
	}

	#[test]
	fn captured_padding_swallows_clicks_without_a_button_hit() {
		let panel = UiRect::new(0.0, 0.0, 100.0, 100.0);
		let mut stack = UiStack::new();
		stack.push(StackLayer::<&'static str>::new(LayerId::Canvas, Capture::None));
		stack.push(button_layer(LayerId::Naming, UiRect::new(200.0, 200.0, 10.0, 10.0), Capture::Rect(panel)));

		let d = stack.dispatch_click(point(90.0, 90.0)); // inside the panel, off its (distant) button
		assert_eq!(d.result, InputResult::Handled);
		assert_eq!(d.layer, Some(LayerId::Naming));
		assert_eq!(d.button, None);
	}

	#[test]
	fn disabled_buttons_do_not_capture_but_capture_region_still_does() {
		let rect = UiRect::new(0.0, 0.0, 50.0, 50.0);
		let mut layer = button_layer(LayerId::BottomBar, rect, Capture::None);
		layer.buttons[0].enabled = false;

		let mut stack = UiStack::new();
		stack.push(StackLayer::<&'static str>::new(LayerId::Canvas, Capture::None));
		stack.push(layer);

		assert_eq!(stack.dispatch_click(point(10.0, 10.0)).result, InputResult::Propagate);
	}

	#[test]
	fn full_screen_modal_blocks_everything_underneath() {
		let mut stack = UiStack::new();
		stack.push(button_layer(LayerId::BottomBar, UiRect::new(0.0, 760.0, 1280.0, 40.0), Capture::Rect(UiRect::new(0.0, 760.0, 1280.0, 40.0))));
		stack.push(StackLayer::<&'static str>::new(LayerId::Library, Capture::FullScreen));

		let d = stack.dispatch_click(point(640.0, 400.0));
		assert_eq!(d.result, InputResult::Handled);
		assert_eq!(d.layer, Some(LayerId::Library));
		assert_eq!(d.button, None);
	}

	#[test]
	fn wheel_routes_to_scrollable_regions_and_is_blocked_by_captures() {
		let bar_rect = UiRect::new(0.0, 756.0, 1280.0, 44.0);
		let mut stack = UiStack::new();
		stack.push(button_layer(LayerId::BottomBar, bar_rect, Capture::Rect(bar_rect)).with_scroll_region(bar_rect));
		stack.push(StackLayer::<&'static str>::new(LayerId::Preferences, Capture::FullScreen));

		// Over the modal: swallowed (captured), not scrolled, and never reaches the bar.
		let d = stack.dispatch_wheel(point(300.0, 300.0));
		assert_eq!(d.result, InputResult::Handled);
		assert_eq!(d.layer, Some(LayerId::Preferences));
		assert!(d.scroll_regions.is_empty());

		// Without the modal the same point falls through to canvas zoom.
		stack.pop_if_top(|id| id == LayerId::Preferences);
		assert_eq!(stack.dispatch_wheel(point(300.0, 300.0)).result, InputResult::Propagate);

		// Over the bar: routed to its scroll region.
		let d = stack.dispatch_wheel(point(300.0, 770.0));
		assert_eq!(d.layer, Some(LayerId::BottomBar));
		assert_eq!(d.scroll_regions.len(), 1);
	}

	#[test]
	fn keyboard_target_skips_pointer_hover_surfaces_and_picks_the_topmost_focusable() {
		let mut stack = UiStack::new();
		stack.push(StackLayer::<&'static str>::new(LayerId::Canvas, Capture::None));
		stack.push(button_layer(LayerId::BottomBar, UiRect::new(0.0, 756.0, 1280.0, 44.0), Capture::Rect(UiRect::new(0.0, 756.0, 1280.0, 44.0))));
		assert_eq!(stack.keyboard_target(), None, "bar hover must not take keyboard focus");

		stack.push(StackLayer::<&'static str>::new(LayerId::Search, Capture::FullScreen));
		assert_eq!(stack.keyboard_target(), Some(LayerId::Search));

		stack.pop_while_top(|id| id.is_overlay_panel());
		assert_eq!(stack.top_id(), Some(LayerId::BottomBar));
	}

	#[test]
	fn text_field_marks_a_layer_as_keyboard_target_even_without_full_screen_capture() {
		let frame: Frame<&'static str> = Frame { text_field: Some(UiRect::new(10.0, 10.0, 100.0, 30.0)), ..Default::default() };
		let mut stack: UiStack<&'static str> = UiStack::new();
		stack.push(StackLayer::from_frame(LayerId::Library, frame, Capture::FullScreen));
		assert_eq!(stack.keyboard_target(), Some(LayerId::Library));
	}

	#[test]
	fn convert_frame_maps_a_foreign_action_type_into_the_stacks_own() {
		// The host app wraps `EditorAction`s in its own unified viewer-action enum; conversion must
		// carry buttons (mapped), geometry and the text-field hit-box across unchanged.
		let mut frame: Frame<&'static str> = Frame::default();
		frame.geometry.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), [1.0, 1.0, 1.0, 1.0]);
		frame.buttons.push(Button { rect: UiRect::new(0.0, 0.0, 10.0, 10.0), action: "btn", enabled: true });
		frame.text_field = Some(UiRect::new(1.0, 2.0, 3.0, 4.0));

		let layer = StackLayer::<String>::convert_frame(LayerId::Search, frame, Capture::FullScreen, |s| format!("mapped-{s}"));
		assert_eq!(layer.id, LayerId::Search);
		assert_eq!(layer.buttons.len(), 1);
		assert_eq!(layer.buttons[0].action, "mapped-btn");
		assert!(layer.buttons[0].enabled);
		assert_eq!(layer.text_field, Some(UiRect::new(1.0, 2.0, 3.0, 4.0)));
		assert_eq!(layer.geometry.triangles.len(), 6); // one quad = two triangles = six vertices
	}

	#[test]
	fn from_frame_still_works_for_same_type_frames() {
		let mut frame: Frame<&'static str> = Frame::default();
		frame.buttons.push(Button { rect: UiRect::new(0.0, 0.0, 10.0, 10.0), action: "x", enabled: true });
		let layer: StackLayer<&'static str> = StackLayer::from_frame(LayerId::BottomBar, frame, Capture::None);
		assert_eq!(layer.buttons[0].action, "x");
	}

	#[test]
	fn topmost_button_walks_front_to_back_and_skips_disabled_rows() {
		let rect = UiRect::new(0.0, 0.0, 50.0, 50.0);
		let mut bottom = button_layer(LayerId::BottomBar, rect, Capture::Rect(rect));
		bottom.buttons[0].action = "bottom";
		let mut top = button_layer(LayerId::Library, rect, Capture::FullScreen);
		top.buttons[0].action = "top";

		let mut stack = UiStack::new();
		stack.push(bottom);
		stack.push(top);

		let (layer, button) = stack.topmost_button(point(10.0, 10.0)).expect("a row is under the point");
		assert_eq!(layer, LayerId::Library);
		assert_eq!(button.action, "top");

		// Disable the top row: the search now sees through to the bar's own row underneath.
		stack.layers[1].buttons[0].enabled = false;
		let (layer, button) = stack.topmost_button(point(10.0, 10.0)).expect("the bar's row is still under the point");
		assert_eq!(layer, LayerId::BottomBar);
		assert_eq!(button.action, "bottom");

		assert_eq!(stack.topmost_button(point(500.0, 500.0)), None);
	}

	#[test]
	fn keyboard_stop_is_true_only_for_text_or_key_capture_focus() {
		// Pointer-hover surfaces never take keyboard focus, so characters stay app-level.
		let mut stack = UiStack::new();
		stack.push(StackLayer::<&'static str>::new(LayerId::Canvas, Capture::None));
		stack.push(button_layer(LayerId::BottomBar, UiRect::new(0.0, 756.0, 1280.0, 44.0), Capture::FullScreen));
		assert!(!stack.keyboard_stop());

		// A text field anywhere up the stack claims typed characters outright.
		let field: Frame<&'static str> = Frame { text_field: Some(UiRect::new(0.0, 0.0, 10.0, 10.0)), ..Frame::default() };
		stack.push(StackLayer::from_frame(LayerId::Naming, field, Capture::FullScreen));
		assert!(stack.keyboard_stop());

		// So does the key-select popup, which has no text field but captures keystrokes as data.
		stack.pop_if_top(|id| id == LayerId::Naming);
		stack.push(StackLayer::<&'static str>::new(LayerId::KeySelect, Capture::FullScreen));
		assert!(stack.keyboard_stop());

		// A modal without either (e.g. plain preferences) leaves characters to the app.
		stack.pop_if_top(|id| id == LayerId::KeySelect);
		stack.push(StackLayer::<&'static str>::new(LayerId::Preferences, Capture::FullScreen));
		assert!(!stack.keyboard_stop());
	}
}
