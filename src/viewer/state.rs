//! Viewer-side state: the working data for one open project's chip editor
//! (simulation + camera + every overlay's draft state), plus the overlay
//! bookkeeping that opens/closes panels on the live UI stack.

use crate::render::camera::Camera;
use crate::render::context_menu::{ContextMenuAction, ContextMenuState};
use crate::render::editor_ui::LibrarySelection;
use crate::render::ui_stack::{LayerId, UiStack};
use crate::sim::Simulator;
use crate::{ChipLibrary, PinAddress, PinBitCount, ProjectDescription};

use crate::render::editor_ui;
use crate::structs::Vec2;

/// One editor panel from `render::editor_ui` that can sit in
/// [`ViewerState::overlays`]. Overlays are entries of the live UI stack:
/// several may be open at once, stacked bottom-to-top in open order
/// (e.g. Ctrl+F pushes Search *on top of* an already-open Library, and
/// Escape pops just the Search back off). The top-most overlay is the
/// stack's keyboard target; only the Library leaves its bar buttons
/// usable beneath it (see `viewer::frame`'s `bar_enabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
	Library,
	Search,
	Preferences,
	Naming,
	KeySelect,
	RomEditor,
	SaveChip,
}

impl Overlay {
	pub(crate) fn layer_id(self) -> LayerId {
		match self {
			Overlay::Library => LayerId::Library,
			Overlay::Search => LayerId::Search,
			Overlay::Preferences => LayerId::Preferences,
			Overlay::Naming => LayerId::Naming,
			Overlay::KeySelect => LayerId::KeySelect,
			Overlay::RomEditor => LayerId::RomEditor,
			Overlay::SaveChip => LayerId::SaveChip,
		}
	}
}

/// Unified action type carried by every button of the viewer's UI stack.
/// The stack mixes surfaces built by different modules (editor panels and
/// bars produce `EditorAction`s; right-click popups produce
/// `ContextMenuAction`s), so each layer's buttons are mapped into this
/// one enum when pushed on -- see `StackLayer::convert_frame`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewerAction {
	Editor(editor_ui::EditorAction),
	Context(ContextMenuAction),
}

/// Mapping function handed to `StackLayer::convert_frame` for every layer built from an
/// `EditorFrame`.
pub(crate) fn editor_action(action: editor_ui::EditorAction) -> ViewerAction {
	ViewerAction::Editor(action)
}

/// What `Overlay::Naming`'s Confirm/Enter should actually *do* with the
/// typed text once confirmed -- the popup itself (a title + one text
/// field) is generic, reused for the project-rename prompt as well as
/// every "Label"/"Configure" popup opened from a right-click context menu
/// (see `apply_context_menu_action`). Defaults to `RenameProject` so the
/// existing 'n' shortcut keeps working unchanged; every other variant is
/// set right before opening the overlay and consumed (reset back to
/// `RenameProject`) by `confirm_naming_popup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NamingPurpose {
	#[default]
	RenameProject,
	/// Set a placed subchip's display label (`SubChipDescription::label`).
	/// `i32` is that subchip's id within the current root chip.
	LabelComponent(i32),
	/// Set the name of one of the *current root chip's own* boundary
	/// dev-pins (`ChipDescription::input_pins`/`output_pins`) -- never a
	/// subchip's pin, per the brief.
	LabelDevPin { is_input: bool, id: i32 },
	/// Pulse length, in simulation ticks (`SubChipDescription::internal_data[0]`,
	/// mirrored into `Simulator`'s per-chip `internal_state[DURATION]` --
	/// see `sim::process_builtin_chip`'s `Pulse` arm).
	ConfigurePulseDuration(i32),
}

/// What `Overlay::KeySelect`'s Confirm/Enter should do with the chosen
/// key -- same idea as `NamingPurpose`, for the one overlay that isn't a
/// plain text field. Defaults to `Rebind` (today's placeholder "not yet
/// wired to an action" behaviour); `ConfigureKeyChar` is used when a
/// `Key` component's own "Configure" popup reuses this same overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum KeySelectPurpose {
	#[default]
	Rebind,
	/// `SubChipDescription::internal_data[0]`, the ASCII code this `Key`
	/// instance listens for -- `i32` is that subchip's id.
	ConfigureKeyChar(i32),
}

/// Working state for the ROM contents grid editor (`Overlay::RomEditor` /
/// `editor_ui::build_rom_editor_popup`) -- a draft copy of all 256 words
/// plus which one's currently selected, kept separate from
/// `SubChipDescription::internal_data` until "Apply" is clicked (same
/// "edit a draft, commit on confirm" shape every other overlay here
/// uses).
pub(crate) struct RomEditorState {
	/// Id of the subchip (within the current root chip) being configured.
	pub(crate) component_id: i32,
	pub(crate) data: Vec<u32>,
	pub(crate) selected: usize,
}

/// One endpoint of an in-progress wire placement (`ViewerState::pending_wire`),
/// fixed at the moment the wire is started -- either a real pin (a
/// subchip's own, or one of the current chip's own boundary dev-pins) or
/// a tap point along an existing wire's line.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingWireEnd {
	Pin {
		owner_id: i32,
		pin_id: i32,
		is_source: bool,
		position: Vec2,
	},
	/// Tapping onto a wire always plays the *source* role for the new
	/// branch wire (see `try_start_pending_wire`'s doc comment) -- the
	/// tapped wire's own real source pin, needed to build the eventual
	/// `WireDescription::new_tapped_source`, travels along here rather
	/// than being re-looked-up at completion time.
	WireTap {
		wire_index: usize,
		segment_index: i32,
		point: Vec2,
		source_pin_address: PinAddress,
	},
}

impl PendingWireEnd {
	pub(crate) fn is_source(&self) -> bool {
		match self {
			PendingWireEnd::Pin { is_source, .. } => *is_source,
			PendingWireEnd::WireTap { .. } => true,
		}
	}

	pub(crate) fn position(&self) -> Vec2 {
		match self {
			PendingWireEnd::Pin { position, .. } => *position,
			PendingWireEnd::WireTap { point, .. } => *point,
		}
	}
}

/// State for an in-progress wire placement: the endpoint it started
/// from, plus any bend ("turn") points the player has since clicked on
/// empty canvas space, in click order -- becomes the finished
/// `WireDescription::points` once the wire is completed at a second,
/// opposite-role endpoint (reversed first if that second endpoint turns
/// out to be the wire's real *source*, since `points` always runs
/// source-to-target). `None` on [`ViewerState`] whenever no wire is being
/// placed.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct PendingWire {
	pub(crate) start: PendingWireEnd,
	pub(crate) bend_points: Vec<Vec2>,
	pub(crate) bit_count: PinBitCount,
}

/// State specific to viewing/simulating one open project's chip, split out
/// so [`crate::viewer::app::App`] can hold either this or the menu depending on `Screen`.
pub(crate) struct ViewerState {
	pub(crate) project_name: String,
	pub(crate) library: ChipLibrary,
	pub(crate) root_chip_name: String,
	pub(crate) sim: Simulator,
	pub(crate) camera: Camera,
	pub(crate) dragging: bool,
	pub(crate) last_cursor: Vec2,
	pub(crate) camera_fitted: bool,
	pub(crate) show_grid: bool,

	/// The project's saved prefs/collections, edited live by the
	/// preferences/library overlays and written back to disk on Apply.
	pub(crate) prefs: ProjectDescription,
	/// Editor panels currently open, bottom-to-top in open order -- see
	/// [`Overlay`]'s docs for the stacking rules. Empty = plain viewer.
	pub(crate) overlays: Vec<Overlay>,
	/// The Search overlay's own query buffer (kept separate from the
	/// shared `overlay_text_input`, which a Library collection-name field
	/// underneath a popped-open Search must keep owning).
	pub(crate) search_query: String,
	/// Shared text buffer for whichever *top-most* text-field overlay is
	/// currently open (the naming popup, ROM cell editor, save-chip name,
	/// or the library's inline new/rename-collection field).
	pub(crate) overlay_text_input: String,
	/// Pending key choice for the key-select popup.
	pub(crate) overlay_key_choice: Option<char>,
	/// What `Overlay::Naming`'s confirm should do -- see [`NamingPurpose`]'s
	/// docs.
	pub(crate) naming_purpose: NamingPurpose,
	/// What `Overlay::KeySelect`'s confirm should do -- see
	/// [`KeySelectPurpose`]'s docs.
	pub(crate) key_select_purpose: KeySelectPurpose,
	/// Draft state for `Overlay::RomEditor`, when open.
	pub(crate) rom_editor: Option<RomEditorState>,

	/// The viewer's UI stack as of the *last drawn* frame -- every
	/// visible surface is a layer in here (canvas at the bottom, popups
	/// on top), rebuilt from live state each frame by `viewer::frame`. All
	/// input routing (click/wheel/keyboard focus) dispatches against it,
	/// same immediate-mode "hit-test what I just drew" contract the
	/// per-frame `last_*_buttons` lists this replaces used to have.
	pub(crate) stack: UiStack<ViewerAction>,

	/// Horizontal scroll offset (px) of the starred bottom bar's
	/// overflow, driven by wheel events the stack routes to the bar's
	/// scroll region; clamped against `bottom_bar_scroll_max`.
	pub(crate) bottom_bar_scroll_x: f32,
	/// How far the bar can scroll: its content width minus the viewport,
	/// recomputed each redraw. Zero while everything fits.
	pub(crate) bottom_bar_scroll_max: f32,

	/// Which row of the real `Overlay::Library` panel is currently
	/// selected -- see `editor_ui::LibrarySelection`'s docs.
	pub(crate) library_selection: LibrarySelection,
	/// Whether the library's inline "new collection" name field is open.
	pub(crate) library_creating_collection: bool,
	/// Whether the library's inline "rename collection" name field is
	/// open (for whichever collection `library_selection` points at).
	pub(crate) library_renaming_collection: bool,
	/// Whether the library's inline chip-delete confirmation is open.
	pub(crate) library_confirming_chip_delete: bool,
	/// Whether the library's inline collection-delete confirmation is
	/// open.
	pub(crate) library_confirming_collection_delete: bool,
	/// Precomputed message shown by whichever of the above two
	/// confirmations is open -- see `chip_delete_confirm_message`.
	pub(crate) library_delete_message: String,
	/// Name of the starred collection whose flyout is currently open in
	/// the bottom bar (`editor_ui::build_starred_collection_popup`), if
	/// any.
	pub(crate) bottom_bar_open_collection: Option<String>,

	/// The right-click popup currently open (over a canvas component or
	/// a library row), if any -- see `render::context_menu`. Lives in the
	/// UI stack as its own top-most layer (`LayerId::ContextMenu`), above
	/// even a modal overlay, so it's never hidden behind (or accidentally
	/// swallowed by) whatever else is open.
	pub(crate) context_menu: Option<ContextMenuState>,

	/// The wire currently being placed by clicking one endpoint then
	/// another, if any -- see [`PendingWire`]'s docs. Cleared (`None`)
	/// whenever the root chip changes, since a pending endpoint's
	/// `wire_index`/subchip id would otherwise silently refer to
	/// whatever now happens to sit at that index in the new chip.
	pub(crate) pending_wire: Option<PendingWire>,

	/// The name of a chip currently picked up for placement (the
	/// library's "USE" button, `EditorAction::PlaceChip`), if any --
	/// drawn as a translucent preview following the cursor
	/// (`build_pending_place_scene`) and dropped as a real
	/// `SubChipDescription` on the next canvas click that lands on free
	/// space (`try_place_pending_chip`). Mutually exclusive with
	/// `pending_wire` in practice (starting one clears the other), and
	/// cleared on the same triggers `pending_wire` is: Escape, a
	/// right-click, or the root chip changing.
	pub(crate) pending_place: Option<String>,
}

impl ViewerState {
	/// Rebuilds `self.sim` from `self.library`'s current copy of
	/// `self.root_chip_name` -- called after any edit that changes the
	/// simulated structure (deleting a component/wire, re-configuring a
	/// Pulse/Key/ROM, etc). Deliberately leaves the camera exactly where
	/// it is: an in-place edit to the chip you're already looking at
	/// shouldn't yank the view back to a fresh auto-fit every time (that
	/// was `camera_fitted`'s old, wrong role here). Actually switching to
	/// a *different* chip is a separate, explicit action --
	/// `open_chip_by_name` resets `camera_fitted` itself, only when the
	/// root chip is actually changing.
	pub(crate) fn rebuild_sim(&mut self) {
		let root_desc = self.library.get(&self.root_chip_name).clone();
		let held_keys = std::mem::take(&mut self.sim.held_keys);
		let key_modifiers = self.sim.key_modifiers;
		self.sim = Simulator::build(&root_desc, &self.library);
		self.sim.held_keys = held_keys;
		self.sim.key_modifiers = key_modifiers;
	}
}

/// Drops UI-stack layers whose backing live state has closed since the stack was last rebuilt --
/// events arrive between frames, so an action (or Escape) that closed a popup must not let the
/// *next* event be eaten by that popup's still-cached layer. Called before every dispatch.
/// Layers are only ever popped off the top; anything still live stays exactly where it is.
pub(crate) fn sync_stack_with_state(v: &mut ViewerState) {
	if v.context_menu.is_none() {
		v.stack.pop_if_top(|id| id == LayerId::ContextMenu);
	}
	if v.bottom_bar_open_collection.is_none() {
		v.stack.pop_if_top(|id| id == LayerId::BottomBarFlyout);
	}
	if v.overlays.is_empty() {
		v.stack.pop_while_top(|id| id.is_overlay_panel());
	}
}

/// Pushes `overlay` as the new top of [`ViewerState::overlays`], or -- if an instance is already
/// open somewhere down the stack -- re-focuses it (moves it to the top) instead of stacking a
/// duplicate, so e.g. mashing Ctrl+F can't pile up identical Search layers.
/// Reopening clears whichever draft text the previous instance had left behind.
pub(crate) fn open_overlay(v: &mut ViewerState, overlay: Overlay) {
	v.overlays.retain(|o| *o != overlay);
	v.overlays.push(overlay);
}

/// Ctrl+F: opens (or re-focuses) the search popup with a fresh query.
pub(crate) fn open_search(v: &mut ViewerState) {
	open_overlay(v, Overlay::Search);
	v.search_query.clear();
}

/// Ctrl+S: opens (or re-focuses) the save-chip popup pre-filled with the chip's current name.
pub(crate) fn open_save_chip(v: &mut ViewerState) {
	open_overlay(v, Overlay::SaveChip);
	v.overlay_text_input = v.root_chip_name.clone();
}

/// Pops the top-most overlay off [`ViewerState::overlays`], releasing whichever purpose/draft state
/// belonged to it (each overlay owns its own transient state, so closing it half-way through just
/// resets that one). A no-op when nothing is open.
pub(crate) fn close_top_overlay(v: &mut ViewerState) {
	let Some(top) = v.overlays.pop() else { return };
	match top {
		Overlay::Library => crate::viewer::library::reset_library_popup_state(v),
		Overlay::Naming => v.naming_purpose = NamingPurpose::default(),
		Overlay::KeySelect => v.key_select_purpose = KeySelectPurpose::default(),
		Overlay::RomEditor => v.rom_editor = None,
		Overlay::Search => v.search_query.clear(),
		// The shared buffer belongs to whichever text-field overlay owned it while open.
		Overlay::SaveChip => v.overlay_text_input.clear(),
		// Preferences carries no transient draft of its own.
		Overlay::Preferences => {}
	}
	if !matches!(top, Overlay::Library | Overlay::Search) {
		v.overlay_text_input.clear();
	}
}

/// Closes every open overlay at once -- the "leave whatever panels were open" gesture shared by
/// flows that hand control back to the plain viewer (Use-chip, Exit-library, Apply-preferences).
pub(crate) fn close_all_overlays(v: &mut ViewerState) {
	while !v.overlays.is_empty() {
		close_top_overlay(v);
	}
}
