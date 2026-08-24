//! Viewer-side state: the working data for one open project's chip editor
//! (simulation + camera + every overlay's draft state), plus the overlay
//! bookkeeping that opens/closes panels on the live UI stack.

use crate::render::camera::Camera;
use crate::render::context_menu::{ContextMenuAction, ContextMenuState};
use crate::render::editor_ui::{self, LibrarySelection, PrefValueField};
use crate::render::ui_stack::{LayerId, UiStack};
use crate::sim::key_mods_bits;
use crate::sim::Simulator;
use crate::viewer::chip_interaction;
use crate::viewer::customize::CustomizeState;
use crate::viewer::sim_timing::PerfWindow;
use crate::viewer::wire_draft::PendingWire;
use crate::{ChipLibrary, ProjectDescription};

use crate::structs::Vec2;
use std::time::Instant;

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
	CustomizeChip,
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
			Overlay::CustomizeChip => LayerId::CustomizePanel,
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

/// Bookkeeping for stepping the simulation from the render loop at the
/// project's target rate (see [`crate::viewer::sim_timing`] for the math):
/// when the last frame stepped, how many ticks are still "owed", the
/// rolling throughput window behind the preferences panel's measured-speed
/// display, and whether the previous frame saw the sim paused (so leaving
/// pause starts measurement and debt from scratch instead of bursting).
#[derive(Default)]
pub(crate) struct SimPacing {
	pub(crate) last_tick: Option<Instant>,
	pub(crate) debt_ticks: f64,
	pub(crate) window: PerfWindow,
	pub(crate) was_paused: bool,
	/// Latest measured average ticks/second (`0` while paused or before
	/// anything has been measured).
	pub(crate) avg_ticks_per_sec: f64,
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

	/// The app's shared buzzer-audio state: simulation steps register
	/// notes into it, the output stream samples from it. Shared (rather
	/// than owned here) so the stream keeps running across project
	/// switches, like the original's ever-present `AudioUnity`.
	pub(crate) audio: crate::audio::SharedAudioState,

	/// The project's saved prefs/collections, edited live by the
	/// preferences/library overlays and written back to disk on Apply.
	pub(crate) prefs: ProjectDescription,

	/// Pacing/throughput state for stepping the simulation (see [`SimPacing`]).
	pub(crate) sim_pacing: SimPacing,
	/// Set when the player requests one single step while the sim is
	/// paused (`SimNextStepShortcutTriggered`); consumed by the next
	/// frame's simulation update.
	pub(crate) advance_single_step: bool,
	/// How many single steps have advanced since the sim was paused --
	/// shown on the paused banner, mirroring
	/// `Project.simPausedSingleStepCounter`.
	pub(crate) paused_step_counter: u32,

	/// Which of the preferences panel's numeric fields currently owns
	/// typed input, if any.
	pub(crate) prefs_field_focus: Option<PrefValueField>,
	/// Draft text of the preferences panel's "steps per clock tick" field.
	pub(crate) prefs_clock_text: String,
	/// Draft text of the preferences panel's "steps per second (target)"
	/// field.
	pub(crate) prefs_rate_text: String,

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
	/// Draft state for `Overlay::CustomizeChip`, when open -- the cloned
	/// chip description being customized (name location / colour / size /
	/// embedded displays) plus whatever grab/resize interaction is in
	/// flight. Written back onto the library entry only on Confirm; see
	/// `viewer::customize`.
	pub(crate) customize: Option<CustomizeState>,

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

	/// The components currently picked up for placement (the library's
	/// "USE" button, `EditorAction::PlaceChip`), if any -- each entry is
	/// `(offset from the cursor, what to place)`, drawn as translucent
	/// previews following the cursor (`build_pending_place_scene`) and
	/// dropped as real descriptions on the next canvas click that lands on
	/// free space (`try_place_pending_components`). A pickup normally
	/// carries one entry; picking up a bus origin additionally carries its
	/// linked terminus partner as a second one -- see
	/// `chip_interaction::start_placing`. Mutually exclusive with
	/// `pending_wire` in practice (starting one clears the other), and
	/// cleared on the same triggers `pending_wire` is: Escape, a
	/// right-click, or the root chip changing.
	pub(crate) pending_place: Vec<(Vec2, chip_interaction::PendingComponent)>,

	/// Ids of the currently selected placed components (subchips of the
	/// current root chip; dev-pins deliberately don't take part -- see
	/// `chip_interaction::begin_drag_on_component`). Populated by clicking
	/// a component body or rubber-band box selection, consumed by Delete,
	/// and cleared by Escape/right-click/root-chip switches.
	pub(crate) selected_ids: Vec<i32>,

	/// What the current left-press drag over the canvas is doing (carrying
	/// the selection around, or drawing a rubber band) -- see
	/// [`chip_interaction::CanvasInteraction`].
	pub(crate) canvas_interaction: chip_interaction::CanvasInteraction,
}

impl ViewerState {
	/// Builds a fresh viewer for `root_chip_name` (which must already be
	/// present in `library`), with the simulation built and prefs-driven
	/// simulation settings applied. Callers tweak post-construction bits
	/// (key modifiers, camera fitting) directly afterwards.
	pub(crate) fn new(
		project_name: &str,
		library: ChipLibrary,
		root_chip_name: String,
		viewport: Vec2,
		audio: crate::audio::SharedAudioState,
	) -> Self {
		let root_desc = library.get(&root_chip_name).clone();
		let sim = Simulator::build(&root_desc, &library);
		let mut v = Self {
			project_name: project_name.to_string(),
			library,
			sim,
			root_chip_name,
			camera: Camera::new(viewport),
			dragging: false,
			last_cursor: Vec2::ZERO,
			camera_fitted: false,
			audio,
			prefs: ProjectDescription::default(),
			sim_pacing: SimPacing::default(),
			advance_single_step: false,
			paused_step_counter: 0,
			prefs_field_focus: None,
			prefs_clock_text: String::new(),
			prefs_rate_text: String::new(),
			overlays: Vec::new(),
			search_query: String::new(),
			overlay_text_input: String::new(),
			overlay_key_choice: None,
			naming_purpose: Default::default(),
			key_select_purpose: Default::default(),
			rom_editor: None,
			customize: None,
			stack: UiStack::new(),
			bottom_bar_scroll_x: 0.0,
			bottom_bar_scroll_max: 0.0,
			library_selection: LibrarySelection::None,
			library_creating_collection: false,
			library_renaming_collection: false,
			library_confirming_chip_delete: false,
			library_confirming_collection_delete: false,
			library_delete_message: String::new(),
			bottom_bar_open_collection: None,
			context_menu: None,
			pending_wire: None,
			pending_place: Vec::new(),
			selected_ids: Vec::new(),
			canvas_interaction: Default::default(),
		};
		v.sync_sim_clock_pref();
		v
	}
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
		self.sync_sim_clock_pref();
	}

	// ---- Prefs-derived queries (`Project.ShowGrid` / `.ShouldSnapToGrid` /
	// `.ForceStraightWires` / `.targetTicksPerSecond`) ----

	pub(crate) fn show_grid(&self) -> bool {
		self.prefs.prefs_grid_display_mode == 1
	}

	/// Mirrors `Project.targetTicksPerSecond`'s `Max(1, ..)` clamp.
	pub(crate) fn target_ticks_per_second(&self) -> i32 {
		self.prefs.prefs_sim_target_steps_per_second.max(1)
	}

	/// Ctrl-hold forces snapping regardless of prefs; "If Grid Shown"
	/// snaps only while the grid is displayed; "Always" snaps always.
	pub(crate) fn should_snap_to_grid(&self) -> bool {
		self.sim.key_modifiers & key_mods_bits::CONTROL != 0 || (self.prefs.prefs_snapping == 1 && self.show_grid()) || self.prefs.prefs_snapping == 2
	}

	/// Same three-way structure as [`Self::should_snap_to_grid`], for shift.
	pub(crate) fn force_straight_wires(&self) -> bool {
		self.sim.key_modifiers & key_mods_bits::SHIFT != 0
			|| (self.prefs.prefs_straight_wires == 1 && self.show_grid())
			|| self.prefs.prefs_straight_wires == 2
	}

	/// Pushes the clock-speed pref into the live simulator (`SimThread.Run`
	/// assigning `Simulator.stepsPerClockTransition` every tick).
	pub(crate) fn sync_sim_clock_pref(&mut self) {
		self.sim.steps_per_clock_transition = self.prefs.prefs_sim_steps_per_clock_tick.max(0) as u32;
	}

	// ---- Shortcut-driven pref mutations (`PreferencesMenu.HandleKeyboardShortcuts`) ----

	/// Mirrors `Project.ToggleGridDisplay`.
	pub(crate) fn toggle_grid_display(&mut self) {
		self.prefs.prefs_grid_display_mode = 1 - self.prefs.prefs_grid_display_mode;
	}

	/// Mirrors the sim-pause toggle shortcut's description mutation.
	pub(crate) fn toggle_sim_paused(&mut self) {
		self.prefs.prefs_sim_paused = !self.prefs.prefs_sim_paused;
	}

	/// Mirrors the single-step shortcut: only does anything while paused.
	pub(crate) fn request_single_sim_step(&mut self) {
		if self.prefs.prefs_sim_paused {
			self.advance_single_step = true;
		}
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
	if v.customize.is_none() {
		v.stack.pop_if_top(|id| id == LayerId::CustomizePanel);
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

/// Ctrl+P / 'p': opens (or re-focuses) the preferences panel, seeding its
/// numeric fields from the live prefs -- mirrors
/// `PreferencesMenu.OnMenuOpened` -> `UpdateUIFromDescription`.
pub(crate) fn open_preferences(v: &mut ViewerState) {
	open_overlay(v, Overlay::Preferences);
	v.prefs_clock_text = v.prefs.prefs_sim_steps_per_clock_tick.to_string();
	v.prefs_rate_text = v.prefs.prefs_sim_target_steps_per_second.to_string();
	v.prefs_field_focus = None;
}

/// Clears the preferences panel's draft state (field focus + typed text).
pub(crate) fn reset_preferences_draft(v: &mut ViewerState) {
	v.prefs_field_focus = None;
	v.prefs_clock_text.clear();
	v.prefs_rate_text.clear();
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
		// The customizer borrowed the shared buffer for its hex colour
		// field; give the save popup's name back (see `open_customize`).
		Overlay::CustomizeChip => {
			if let Some(customize) = v.customize.take() {
				v.overlay_text_input = customize.saved_save_text;
			}
		}
		// The preferences panel keeps its numeric drafts in their own
		// buffers (not the shared one), so they're dropped here.
		Overlay::Preferences => reset_preferences_draft(v),
	}
	// The shared buffer belongs to whichever text-field overlay owned it
	// while open -- except CustomizeChip, whose arm above just handed it
	// back to the save popup underneath and mustn't be wiped here.
	if !matches!(top, Overlay::Library | Overlay::Search | Overlay::CustomizeChip) {
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
