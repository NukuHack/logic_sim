//! A small, generic right-click "popup" menu, meant to be easily attachable to anything the host app
//! can identify by a string id -- a placed subchip on the canvas, a row in the chip library, etc. --
//! without needing a bespoke menu type per call site. Same immediate-mode philosophy as
//! [`crate::render::editor_ui`] and [`crate::render::menu_ui`]: builds drawable geometry plus
//! clickable hit-boxes for one frame; the host keeps the open/closed state and re-calls it each frame.

use crate::render::foundation::{SceneGeometry, TextLabel};
use crate::render::menu_ui::UiRect;
use crate::structs::Vec2;

pub use crate::render::menu_ui::to_world;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuAction {
	Configure,
	/// Enter wire edit mode on the target wire (`EnterWireEditMode`).
	Edit,
	Delete,
	/// Remove just the wire segment without cascading to dependents
	/// ("Delete Part" in the wire context menu).
	DeletePart,
	Label,
	Flip,
	Open,
	Unstar,
	/// Enter the target placed component's own definition in view-only
	/// mode (`ViewedChipsBar`/`Project.EnterViewMode`): watch its live
	/// simulation without being able to edit it.
	View,
	Other,
}

/// One selectable row of a context menu: the label shown to the player,
/// and an opaque id the host matches on in its own action-handling code
/// (kept as a plain string, rather than an enum, so this one menu type
/// can be reused for different kinds of targets without growing a new
/// variant per call site).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuItem {
	pub label: String,
	pub id: ContextMenuAction,
	/// Disabled rows still draw (dimmed, no hover highlight) but never
	/// appear in `ContextMenuFrame::hovered`/`buttons`' hit-testable
	/// sense -- see `build_context_menu`. Used e.g. to grey out "Open"
	/// for a built-in chip that has no definition to navigate into.
	pub enabled: bool,
}

impl ContextMenuItem {
	pub fn new(label: impl Into<String>, id: ContextMenuAction) -> Self {
		Self { label: label.into(), id, enabled: true }
	}

	pub fn new_enabled(label: impl Into<String>, id: ContextMenuAction, enabled: bool) -> Self {
		Self { label: label.into(), id, enabled }
	}
}

/// Everything the host needs to remember while a context menu is open:
/// which thing it was opened on (`target`, e.g. a chip name -- again
/// kept generic as a string rather than an enum), where it was anchored
/// on screen, and what rows it offers. The host owns an
/// `Option<ContextMenuState>`; `Some` means "draw + hit-test this popup
/// on top of everything else this frame", `None` means "closed".
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenuState {
	pub target: String,
	pub screen_pos: Vec2,
	pub items: Vec<ContextMenuItem>,
}

impl ContextMenuState {
	pub fn new(target: impl Into<String>, screen_pos: Vec2, items: Vec<ContextMenuItem>) -> Self {
		Self { target: target.into(), screen_pos, items }
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenuButton {
	pub rect: UiRect,
	pub id: ContextMenuAction,
}

/// One drawn/hit-testable frame of an open context menu. Analogous to
/// `editor_ui::EditorFrame` / `menu_ui::MenuFrame`, but deliberately
/// slimmer (no text field, no per-row "enabled" flag) since a context
/// menu's rows are always simple, always-enabled actions.
#[derive(Debug, Default, Clone)]
pub struct ContextMenuFrame {
	pub geometry: SceneGeometry,
	pub buttons: Vec<ContextMenuButton>,
	/// The whole popup's own bounding box (background panel), so the
	/// host can tell "click landed somewhere inside the popup" (even on
	/// its padding/gaps between rows) apart from "click landed
	/// elsewhere -> close the popup" without re-deriving it from the
	/// individual row rects.
	pub panel_rect: UiRect,
	pub hovered: Option<ContextMenuAction>,
}

const ROW_H: f32 = 30.0;
const ROW_W: f32 = 150.0;
const FONT_SIZE: f32 = 15.0;
const BORDER: f32 = 1.0;

fn centre(r: &UiRect) -> Vec2 {
	Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Builds one frame of a right-click popup anchored at `state.screen_pos`
/// -- clamped so the whole panel stays fully on-screen, in case the
/// click landed near a viewport edge -- with one clickable row per
/// `state.items`, in order. Purely presentational: the host applies
/// whichever `ContextMenuButton::id` got clicked back onto its own
/// understanding of `state.target` (see that field's docs).
pub fn build_context_menu(state: &ContextMenuState, vw: f32, vh: f32, mouse: Vec2) -> ContextMenuFrame {
	let mut frame = ContextMenuFrame::default();
	if state.items.is_empty() {
		return frame;
	}

	let panel_h = ROW_H * state.items.len() as f32;
	let x = state.screen_pos.x.clamp(4.0, (vw - ROW_W - 4.0).max(4.0));
	let y = state.screen_pos.y.clamp(4.0, (vh - panel_h - 4.0).max(4.0));
	let panel_rect = UiRect::new(x, y, ROW_W, panel_h);
	frame.panel_rect = panel_rect;

	// Border, then background, then rows -- the shared
	// "outline behind slightly-inset fill" pattern, with the outline's own rect sized one border
	// wider than the panel on every side so the fill lands exactly on `panel_rect`.
	frame.geometry.add_outlined_rect(
		to_world(centre(&panel_rect), vw, vh),
		Vec2::new(panel_rect.w + BORDER * 2.0, panel_rect.h + BORDER * 2.0),
		BORDER,
		[0.17, 0.17, 0.19, 1.0],
		[0.05, 0.05, 0.06, 1.0],
	);

	for (i, item) in state.items.iter().enumerate() {
		let row_rect = UiRect::new(panel_rect.x, panel_rect.y + i as f32 * ROW_H, panel_rect.w, ROW_H);
		let hovered = item.enabled && row_rect.contains(mouse);
		if hovered {
			frame.geometry.add_rect(to_world(centre(&row_rect), vw, vh), Vec2::new(row_rect.w, row_rect.h), [0.32, 0.32, 0.4, 1.0]);
		}
		let text_colour = if item.enabled { [0.95, 0.95, 0.95, 1.0] } else { [0.5, 0.5, 0.5, 1.0] };
		frame.geometry.labels.push(TextLabel {
			pos: to_world(centre(&row_rect), vw, vh),
			text: item.label.clone(),
			colour: text_colour,
			font_size: FONT_SIZE,
			width: row_rect.w - 20.0,
		});
		// Disabled rows still draw (dimmed) but are never hit-testable --
		// omitted from `buttons` entirely, so neither a click nor
		// `hovered` can ever resolve to one.
		if item.enabled {
			frame.buttons.push(ContextMenuButton { rect: row_rect, id: item.id.clone() });
		}
	}

	frame.hovered = frame.buttons.iter().find(|b| b.rect.contains(mouse)).map(|b| b.id.clone());
	frame
}
