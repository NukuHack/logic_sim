//! Plain-data "immediate mode" UI primitives shared by [`crate::render::menu_ui`] (the startup
//! screen) and [`crate::render::editor_ui`] (the in-editor overlays). Both build a [`Frame`] per
//! redraw -- draw rects/labels into `geometry`, push [`Button`] hit-boxes, let the host hit-test
//! the next click against them -- so this module owns the bits that are identical either way
//! (button/label/text-field drawing, hover colouring), leaving each caller only its own layout.

use crate::render::camera::Camera;
use crate::render::foundation::{SceneGeometry, TextLabel};
use crate::render::theme;
use crate::structs::Vec2;

pub const FONT_SIZE: f32 = 18.0;

/// Converts a screen-space point (origin top-left, +y down) into the
/// world-space point that lands there when drawn through a camera
/// positioned at `(vw / 2, vh / 2)` with `zoom = 1.0` -- the inverse of
/// what `Camera::world_to_screen` computes for that same camera.
pub fn to_world(screen: Vec2, vw: f32, vh: f32) -> Vec2 {
	let _ = vw; // kept for symmetry / clarity at call sites, x maps 1:1
	Vec2::new(screen.x, vh - screen.y)
}

/// Re-maps geometry laid out in `to_world`'s fixed "pixel" space (the
/// convention every `ui_kit`-based overlay builder draws in) into the
/// world points that land on those *same pixels* when drawn through
/// `camera`, which pans and zooms freely -- keeping overlays pinned to
/// the screen (constant position and size in pixels) no matter how far
/// the canvas underneath has been panned/zoomed, using one real render
/// pass instead of needing a second camera/pipeline in `render::gpu`.
///
/// Text labels additionally divide their size by the zoom, since a label
/// drawn at constant pixel size covers a proportionally smaller world rect.
pub fn pin_geometry_to_screen(mut geometry: SceneGeometry, camera: &Camera, vh: f32) -> SceneGeometry {
	let to_screen_px = |world: Vec2| Vec2::new(world.x, vh - world.y); // inverse of `to_world`, which is its own inverse
	for v in &mut geometry.triangles {
		v.pos = camera.screen_to_world(to_screen_px(v.pos));
	}
	for l in &mut geometry.labels {
		l.pos = camera.screen_to_world(to_screen_px(l.pos));
		l.font_size /= camera.zoom;
		l.width /= camera.zoom;
	}
	geometry
}

/// Per-frame ambient context shared by every draw helper: viewport size
/// for the px->world mapping plus the current mouse position for hover
/// styling. Bundled into one [`Copy`] value (rather than threaded through
/// as three separate trailing parameters) so the drawing primitives keep
/// short, readable signatures -- callers build it once per frame and
/// pass it down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiCtx {
	pub vw: f32,
	pub vh: f32,
	pub mouse: Vec2,
}

impl UiCtx {
	pub fn new(vw: f32, vh: f32, mouse: Vec2) -> Self {
		Self { vw, vh, mouse }
	}
}

/// An axis-aligned rectangle in screen pixel space.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct UiRect {
	pub x: f32,
	pub y: f32,
	pub w: f32,
	pub h: f32,
}

impl UiRect {
	pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
		Self { x, y, w, h }
	}

	pub fn contains(&self, p: Vec2) -> bool {
		p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
	}

	pub fn centre(&self) -> Vec2 {
		Vec2::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
	}
}

/// One clickable region of a [`Frame`] -- a hit-box plus the action a click on it means, in the
/// caller's own action enum (`menu_ui::UiAction` / `editor_ui::EditorAction`).
#[derive(Debug, Clone, PartialEq)]
pub struct Button<A> {
	pub rect: UiRect,
	pub action: A,
	pub enabled: bool,
}

/// Everything needed to draw one frame of a screen/overlay and hit-test the next mouse event
/// against it. Generic over the caller's own action enum so `menu_ui` and `editor_ui` can each
/// keep their own `MenuFrame`/`EditorFrame` alias without duplicating this shape.
#[derive(Debug, Clone)]
pub struct Frame<A> {
	pub geometry: SceneGeometry,
	pub buttons: Vec<Button<A>>,
	/// Hit-box of this frame's text-entry field, if it has one.
	pub text_field: Option<UiRect>,
	/// Bounding box of the panel/popup this frame draws (its background rect), if any -- lets a
	/// host claim clicks on the panel's padding as a [`crate::render::ui_stack::Capture::Rect`]
	/// without re-deriving the geometry from the individual button rects.
	pub panel: Option<UiRect>,
	/// The button currently under the mouse, if any -- see [`hovered_button`] for whether
	/// disabled buttons count (callers differ, so this isn't filled in automatically).
	pub hovered: Option<A>,
}

// Written by hand rather than `#[derive(Default)]`: a derive would also require `A: Default`,
// but action enums have no meaningful default variant -- an empty frame just has `hovered: None`.
impl<A> Default for Frame<A> {
	fn default() -> Self {
		Self { geometry: SceneGeometry::default(), buttons: Vec::new(), text_field: None, panel: None, hovered: None }
	}
}

/// Solid-fill a rectangle (a popup/panel background, a row highlight, a text field's backing, ...).
pub fn fill_rect<A>(frame: &mut Frame<A>, ui: UiCtx, rect: UiRect, colour: theme::Rgba) {
	frame.geometry.add_rect(to_world(rect.centre(), ui.vw, ui.vh), Vec2::new(rect.w, rect.h), colour);
}

pub fn add_label<A>(frame: &mut Frame<A>, ui: UiCtx, centre: Vec2, width: f32, text: &str, colour: theme::Rgba, font_size: f32) {
	frame.geometry.labels.push(TextLabel { pos: to_world(centre, ui.vw, ui.vh), text: text.to_string(), colour, font_size, width });
}

/// Draws one button at [`FONT_SIZE`] and appends its hit-box to `frame.buttons`. `base_colour`
/// overrides the ordinary grey-when-enabled/brighten-on-hover palette -- pass `None` for that
/// default look, or `Some(colour)` for a differently-tinted button (e.g. editor_ui's destructive
/// "Replace" button, drawn red so it reads as backing up and overwriting a *different* chip).
pub fn add_button<A: Clone>(frame: &mut Frame<A>, ui: UiCtx, rect: UiRect, label: &str, action: A, enabled: bool, base_colour: Option<theme::Rgba>) {
	let hovered = enabled && rect.contains(ui.mouse);
	let bg = if !enabled {
		theme::PIN_INVALID_COL
	} else {
		match base_colour {
			Some(base) if hovered => [(base[0] + 0.12).min(1.0), (base[1] + 0.12).min(1.0), (base[2] + 0.12).min(1.0), base[3]],
			Some(base) => base,
			None if hovered => [0.45, 0.45, 0.5, 1.0],
			None => theme::CHIP_BODY_COL,
		}
	};
	fill_rect(frame, ui, rect, bg);
	add_label(frame, ui, rect.centre(), rect.w - 12.0, label, theme::text_colour_for_background(bg), FONT_SIZE);
	frame.buttons.push(Button { rect, action, enabled });
}

/// Draws the recurring "dark box + typed text + trailing cursor" text-entry field and records its
/// hit-box as `frame.text_field`. `placeholder` is shown (with the same trailing `|` cursor) when
/// `text` is empty -- pass `""` for fields with no placeholder copy.
pub fn text_field_row<A>(frame: &mut Frame<A>, ui: UiCtx, rect: UiRect, text: &str, placeholder: &str, font_size: f32, label_inset: f32) {
	fill_rect(frame, ui, rect, [0.08, 0.08, 0.09, 1.0]);
	let shown = if text.is_empty() { format!("{placeholder}|") } else { format!("{text}|") };
	add_label(frame, ui, rect.centre(), rect.w - label_inset, &shown, [1.0, 1.0, 1.0, 1.0], font_size);
	frame.text_field = Some(rect);
}

/// The action of whichever button `mouse` is currently over. `require_enabled` controls whether a
/// disabled button still counts as "hovered" -- `menu_ui` says yes (hover styling doesn't gate on
/// it there), `editor_ui`'s [`finish`] says no, so both behaviours are kept rather than picked one.
pub fn hovered_button<A: Clone>(buttons: &[Button<A>], mouse: Vec2, require_enabled: bool) -> Option<A> {
	buttons.iter().filter(|b| !require_enabled || b.enabled).find(|b| b.rect.contains(mouse)).map(|b| b.action.clone())
}

/// Resolves `frame.hovered` from its own buttons (requiring `enabled`) and returns it -- the
/// tail call every `editor_ui` builder function ends with.
pub fn finish<A: Clone>(mut frame: Frame<A>, mouse: Vec2) -> Frame<A> {
	frame.hovered = hovered_button(&frame.buttons, mouse, true);
	frame
}
