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

use crate::render::foundation::SceneGeometry;
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
