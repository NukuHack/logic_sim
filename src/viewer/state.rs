//! Viewer-side state: the working data for one open project's chip editor
//! (simulation + camera + every overlay's draft state), plus the overlay
//! bookkeeping that opens/closes panels on the live UI stack.

use crate::render::camera::Camera;
use crate::render::context_menu::{ContextMenuAction, ContextMenuState};
use crate::render::editor_ui::{self, LibrarySelection, PrefValueField};
use crate::render::scene::{PlacedBuf, SceneGeometry};
use crate::render::ui_stack::{LayerId, UiStack};
use crate::sim::key_mods_bits;
use crate::sim::{ChipIdx, Simulator};
use crate::viewer::chip_interaction;
use crate::viewer::customize::CustomizeState;
use crate::viewer::sim_thread::SimHandle;
use crate::viewer::wire_draft::PendingWire;
use crate::{ChipLibrary, ProjectDescription};

use crate::structs::Vec2;

/// Which inline sub-popup the library overlay's collection/chip-delete UI is showing, if any --
/// replaces four bools (`creating_collection`/`renaming_collection`/`confirming_chip_delete`/
/// `confirming_collection_delete`) plus their shared delete-confirmation message, which
/// together could represent combinations (e.g. "confirming both deletes at once") that never
/// actually occur. Carrying each confirmation's message as that variant's own field means it
/// can't be stale or left over from the other confirmation kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum LibraryMode {
	/// No inline popup open; the plain browse/select view.
	#[default]
	Normal,
	/// The inline "new collection" name field is open.
	CreatingCollection,
	/// The inline "rename collection" name field is open, for whichever
	/// collection `ViewerState::library_selection` points at.
	RenamingCollection,
	/// The inline chip-delete confirmation is open, with its precomputed
	/// warning message -- see `chip_delete_confirm_message`.
	ConfirmingChipDelete { message: String },
	/// The inline collection-delete confirmation is open, with its
	/// precomputed warning message.
	ConfirmingCollectionDelete { message: String },
}

impl LibraryMode {
	/// Whether a name-entry field is open (new/rename collection) -- these two share every bit
	/// of input-routing behaviour (typing, Backspace, Enter-to-confirm) and only differ in what
	/// `EditorAction::ConfirmCollectionName` does with the typed text.
	pub(crate) fn is_naming(&self) -> bool {
		matches!(self, LibraryMode::CreatingCollection | LibraryMode::RenamingCollection)
	}

	/// Whether either delete confirmation is open.
	pub(crate) fn is_confirming_delete(&self) -> bool {
		matches!(self, LibraryMode::ConfirmingChipDelete { .. } | LibraryMode::ConfirmingCollectionDelete { .. })
	}
}

/// One editor panel from `render::editor_ui` that can sit in [`ViewerState::overlays`].
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
	/// The boundary-dev-pin edit popup (`PinEditMenu`): rename +, for
	/// multi-bit pins, the "Decimal Display" wheel.
	PinEdit,
	/// The LED colour picker popup: pick a palette colour for an LED
	/// component's tint.
	LedColour,
	/// The unsaved-changes confirmation popup (`UnsavedChangesPopup`).
	UnsavedChanges,
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
			Overlay::PinEdit => LayerId::PinEdit,
			Overlay::LedColour => LayerId::LedColour,
			Overlay::UnsavedChanges => LayerId::UnsavedChanges,
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

/// What `Overlay::Naming`'s Confirm/Enter should actually *do* with the typed text once
/// confirmed -- the popup itself (a title + one text field) is generic, reused for the
/// project-rename prompt as well as every "Label"/"Configure" popup opened from a right-click
/// context menu (see `apply_context_menu_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NamingPurpose {
	#[default]
	RenameProject,
	/// Set a placed subchip's display label (`SubChipDescription::label`).
	/// `i32` is that subchip's id within the current root chip.
	LabelComponent(i32),
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

/// One chip entered in view-only mode ("View" row of a placed component's right-click menu,
/// mirroring `Project.EnterViewMode`): its definition's name, plus the chain of subchip ids
/// leading to *its live instance* from the edited root chip (so the view keeps resolving
/// across sim rebuilds -- ids are stable, arena indices are not).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewedChip {
	pub(crate) name: String,
	pub(crate) path: Vec<i32>,
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

/// Working state for the pin-edit popup (`Overlay::PinEdit`, mirroring `PinEditMenu`): which
/// of the current root chip's own boundary dev-pins it's editing, plus the drafts of the two
/// option rows -- the "Decimal Display" wheel selection (`display_mode_index`, an index into
/// `ValueDisplayMode::ALL`, only meaningful for pins wider than one bit) and the colour-
/// palette swatch pick (`colour`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PinEditState {
	pub(crate) is_input: bool,
	pub(crate) pin_id: i32,
	pub(crate) display_mode_index: usize,
	pub(crate) colour: crate::description::Color,
}

/// Working state for the LED colour picker (`Overlay::LedColour`):
/// which LED subchip is being configured and the draft colour index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LedColourState {
	/// Id of the subchip within the current root chip.
	pub(crate) component_id: i32,
	/// Draft colour palette index (written to `internal_data[0]` on confirm).
	pub(crate) colour_index: usize,
}

/// Working state for wire edit mode (see [`viewer::wire_edit`]): which of
/// the current root chip's wires is being edited, plus which of its bend
/// points (an index into `WireDescription::points`) is selected for
/// dragging/deletion. `None` = not in edit mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WireEditState {
	pub(crate) wire_index: usize,
	pub(crate) selected_bend: Option<usize>,
}

/// What [`Overlay::UnsavedChanges`]'s Continue button should do once the
/// player confirms walking away from the current chip's unsaved edits --
/// this port's stand-in for `UnsavedChangesPopup`'s stored
/// `Action<bool> onClosedCallback` (Cancel simply drops it). Set by the
/// `viewer::save_flow` request/gate helpers right before they open the
/// popup; consumed by `confirm_unsaved_changes_popup`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingUnsavedAction {
	/// Switch to editing this chip's definition. `close_overlays` mirrors
	/// the extra leave-the-library cleanup the library-panel/search open
	/// paths do on an ordinary switch.
	OpenChip { name: String, close_overlays: bool },
	/// Ctrl+N: start a fresh blank chip (`start_new_chip`).
	StartNewChip,
	/// Escape: leave the editor for the startup menu. The viewer can't
	/// swap screens itself, so this arm only sets
	/// [`ViewerState::exit_requested`] for the app shell to act on.
	ReturnToMenu,
}

/// Bookkeeping fed back from the background simulation thread each frame
/// (the thread itself owns all the pacing state -- see
/// [`crate::viewer::sim_thread`]): the measured-speed readout behind the
/// preferences panel and the single-step counter shown on the paused
/// banner.
#[derive(Default)]
pub(crate) struct SimPacing {
	/// Latest measured average ticks/second (`0` while paused or before
	/// anything has been measured).
	pub(crate) avg_ticks_per_sec: f64,
}

/// Ids/indices swept over by an in-progress shift+right-drag delete (see
/// [`ViewerState::delete_drag`]). Deliberately dumb data -- the drag just
/// grows these two lists as the cursor sweeps across new elements; nothing
/// here performs a deletion.
#[derive(Default)]
pub(crate) struct DeleteDragSweep {
	pub(crate) components: Vec<i32>,
	pub(crate) wires: Vec<usize>,
}

/// State specific to viewing/simulating one open project's chip, split out
/// so [`crate::viewer::app::App`] can hold either this or the menu depending on `Screen`.
#[derive(Default)]
pub(crate) struct ViewerState {
	pub(crate) library: ChipLibrary,
	pub(crate) root_chip_name: String,
	/// The simulated world, stepped on its own background thread (see
	/// [`SimHandle`]); the main thread reads it through short locks and
	/// swaps whole new simulators in on structural edits.
	pub(crate) sim: SimHandle,
	pub(crate) camera: Camera,
	pub(crate) dragging: bool,
	pub(crate) last_cursor: Vec2,
	pub(crate) camera_fitted: bool,

	/// Shift+right-click-and-drag delete-drag in progress
	pub(crate) delete_drag: Option<DeleteDragSweep>,

	/// The project's saved prefs/collections, edited live by the
	/// preferences/library overlays and written back to disk on Apply.
	pub(crate) prefs: ProjectDescription,

	/// Names (lower-cased) of custom chips that exist only in [`Self::library`] so far --
	/// created with Ctrl+N (or the blank chip every project opens onto) but never saved with
	/// Ctrl+S.
	pub(crate) unsaved_drafts: std::collections::HashSet<String>,

	/// Names (lower-cased) of chips whose caching checkbox (`ChipDescription::should_be_cached`,
	/// toggled via `customize::toggle_force_cache`) has been manually flipped by the user this
	/// session.
	pub(crate) cache_toggle_touched: std::collections::HashSet<String>,

	/// Pacing/throughput readouts fed back from the background simulation
	/// thread each frame (see [`SimPacing`]).
	pub(crate) sim_pacing: SimPacing,
	/// How many single steps have advanced since the sim was paused --
	/// shown on the paused banner, mirroring
	/// `Project.simPausedSingleStepCounter`. Owned by the sim thread (see
	/// [`crate::viewer::sim_thread`]); mirrored here each frame for
	/// drawing.
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
	/// The Search overlay's currently-selected result, if any -- kept by
	/// name (rather than by list index) so it survives the list being
	/// re-filtered as `search_query` changes. Drives the detail panel's
	/// Open/Delete/Use/Star buttons, mirroring the chip-selected side of
	/// [`ChipLibraryState`](crate::render::editor_ui::ChipLibraryState).
	pub(crate) search_selected: Option<String>,
	/// Whether the Search overlay's inline DELETE confirmation is open
	/// for `search_selected`.
	pub(crate) search_confirming_delete: bool,
	/// Message shown above the Search overlay's DELETE confirmation
	/// buttons -- built the same way as the library panel's
	/// (`chip_delete_confirm_message`).
	pub(crate) search_delete_message: String,
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
	/// Draft state for `Overlay::PinEdit`, when open -- see [`PinEditState`].
	pub(crate) pin_edit: Option<PinEditState>,
	/// Draft state for `Overlay::LedColour`, when open -- see [`LedColourState`].
	pub(crate) led_colour: Option<LedColourState>,
	/// What `Overlay::UnsavedChanges`' Continue should resume -- see
	/// [`PendingUnsavedAction`]'s docs. `Some` exactly while the popup is
	/// open.
	pub(crate) pending_unsaved_action: Option<PendingUnsavedAction>,
	/// Set by the unsaved-changes flow's confirmed `ReturnToMenu` arm:
	/// leaving the editor lives at the app-shell level (`App::return_to_menu`),
	/// which the viewer's action funnel can't reach, so this asks
	/// `viewer::events` to run that transition after the current
	/// click/key finishes dispatching. Consumed (with the whole viewer)
	/// by that transition.
	pub(crate) exit_requested: bool,
	/// Draft state for `Overlay::CustomizeChip`, when open -- the cloned
	/// chip description being customized (name location / colour / size /
	/// embedded displays) plus whatever grab/resize interaction is in
	/// flight. Written back onto the library entry only on Confirm; see
	/// `viewer::customize`.
	pub(crate) customize: Option<CustomizeState>,

	/// Whether pin and component name labels are visible on the canvas.
	/// Toggled by Tab; defaults to `true` (labels shown on hover).
	pub(crate) labels_visible: bool,

	/// Persistent scratch buffer for the canvas's main chip scene, rebuilt every frame via
	/// `render::scene::build_scene_with_spans_into`. Kept here (rather than a fresh
	/// `SceneGeometry` per frame) so the triangle/label `Vec`s are `.clear()`ed and reused
	/// instead of reallocated -- a steady-state frame (roughly the same amount of on-screen
	/// geometry as last frame) then costs zero heap allocation for the scene itself.
	pub(crate) chip_scene_buf: SceneGeometry,

	/// Persistent scratch buffer for that same per-frame scene build's
	/// resolved subchip placements -- the other half of the allocation
	/// this exists to avoid; see `render::scene::PlacedBuf`.
	pub(crate) placed_buf: PlacedBuf,

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
	/// Which inline collection/delete popup the library overlay is showing, if any -- see
	/// [`LibraryMode`].
	pub(crate) library_mode: LibraryMode,
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
	/// whenever the root chip changes
	pub(crate) pending_wire: Option<PendingWire>,

	/// The components currently picked up for placement (the library's "USE" button,
	/// `EditorAction::PlaceChip`), if any -- each entry is `(offset from the cursor, what to
	/// place)`, drawn as translucent previews following the cursor (`build_pending_place_scene`)
	/// and dropped as real descriptions on the next canvas click that lands on free space
	/// (`try_place_pending_components`).
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

	/// Wire currently being edited in wire edit mode (bends draggable),
	/// if any -- see [`viewer::wire_edit`] and [`WireEditState`]. Cleared
	/// wherever the root chip changes, like every other index-bearing
	/// draft.
	pub(crate) wire_edit: Option<WireEditState>,

	/// Chips being viewed in view-only mode, stacked above the edited
	/// chip (`Project.chipViewStack`; empty = editing normally). See
	/// [`ViewedChip`] and [`ViewerState::can_edit_viewed_chip`].
	pub(crate) view_stack: Vec<ViewedChip>,

	/// Undo/redo history for the edited chip (`DevChipInstance`'s
	/// `UndoController`). Cleared wherever the edited root changes.
	pub(crate) undo: crate::viewer::undo::UndoController,
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
		let root_desc = library.get_arc(&root_chip_name);
		let sim = Simulator::build(&root_desc, &library);
		let mut v = Self {
			library,
			sim: SimHandle::new(sim, std::sync::Arc::clone(&audio)),
			root_chip_name,
			camera: Camera::new(viewport),
			dragging: false,
			last_cursor: Vec2::ZERO,
			delete_drag: None,
			camera_fitted: false,
			prefs: ProjectDescription { project_name: project_name.to_string(), ..ProjectDescription::default() },
			unsaved_drafts: std::collections::HashSet::new(),
			cache_toggle_touched: std::collections::HashSet::new(),
			sim_pacing: SimPacing::default(),
			paused_step_counter: 0,
			prefs_field_focus: None,
			prefs_clock_text: String::new(),
			prefs_rate_text: String::new(),
			overlays: Vec::new(),
			search_query: String::new(),
			search_selected: None,
			search_confirming_delete: false,
			search_delete_message: String::new(),
			overlay_text_input: String::new(),
			overlay_key_choice: None,
			naming_purpose: Default::default(),
			key_select_purpose: Default::default(),
			rom_editor: None,
			pin_edit: None,
			led_colour: None,
			pending_unsaved_action: None,
			exit_requested: false,
			customize: None,
			labels_visible: true,
			chip_scene_buf: SceneGeometry::default(),
			placed_buf: PlacedBuf::new(),
			stack: UiStack::new(),
			bottom_bar_scroll_x: 0.0,
			bottom_bar_scroll_max: 0.0,
			library_selection: LibrarySelection::None,
			library_mode: LibraryMode::default(),
			bottom_bar_open_collection: None,
			context_menu: None,
			pending_wire: None,
			pending_place: Vec::new(),
			selected_ids: Vec::new(),
			canvas_interaction: Default::default(),
			wire_edit: None,
			view_stack: Vec::new(),
			undo: Default::default(),
		};
		v.sync_sim_clock_pref();
		v
	}
	// ---- Unsaved-draft bookkeeping (see `unsaved_drafts`) ----

	/// Records `name` as a never-saved draft (case-insensitively).
	pub(crate) fn mark_unsaved_draft(&mut self, name: &str) {
		self.unsaved_drafts.insert(name.to_ascii_lowercase());
	}

	/// Records that `name` has actually been saved to disk (case-insensitively),
	/// letting it join -- and be persisted with -- the project's library.
	pub(crate) fn mark_saved(&mut self, name: &str) {
		self.unsaved_drafts.remove(&name.to_ascii_lowercase());
	}

	/// Rebuilds `self.sim` from `self.library`'s current copy of `self.root_chip_name` -- called
	/// after any edit that changes the simulated structure (deleting a component/wire, re-
	/// configuring a Pulse/Key/ROM, etc).
	pub(crate) fn rebuild_sim(&mut self) {
		let root_desc = self.library.get_arc(&self.root_chip_name);
		// Carry the player-driven transient input state across the swap so
		// an in-place edit doesn't drop held keys / modifiers / toggled
		// switches (see `SimHandle::take_transient_input_state`).
		let (held_keys, key_modifiers, driven_inputs) = self.sim.take_transient_input_state();
		// Also carry every chip's volatile memory (RAM/ROM contents, pulse
		// countdowns, display buffers) so an unrelated edit -- placing one
		// wire, deleting another component -- no longer resets the whole
		// circuit's runtime state. `restart_sim_fresh` is the explicit
		// opt-out for rebuilds that *mean* a fresh run.
		let internal_states = self.sim.capture_internal_states();
		// Carry live pin states (wire signal levels) so the renderer
		// doesn't see a frame of DISCONNECTED defaults between the rebuild
		// and the sim thread's first re-propagation tick.
		let pin_states = self.sim.capture_pin_states();
		let mut sim = Simulator::build(&root_desc, &self.library);
		sim.held_keys = held_keys;
		sim.key_modifiers = key_modifiers;
		sim.driven_inputs = driven_inputs;
		sim.restore_internal_states(&internal_states);
		sim.restore_pin_states(&pin_states);
		// Carry the LUT cache across too
		self.sim.capture_caching_state(sim);
		self.sync_sim_clock_pref();
	}

	/// Rebuilds the simulator from scratch -- no carried-over chip memory.
	/// The explicit "R restarts the simulation" path and the root-chip
	/// switching flows use this: a fresh run (or a different circuit)
	/// shouldn't inherit whatever the previous circuit's RAM happened to
	/// hold.
	pub(crate) fn restart_sim_fresh(&mut self) {
		let root_desc = self.library.get_arc(&self.root_chip_name);
		let (held_keys, key_modifiers, driven_inputs) = self.sim.take_transient_input_state();
		let mut sim = Simulator::build(&root_desc, &self.library);
		sim.held_keys = held_keys;
		sim.key_modifiers = key_modifiers;
		sim.driven_inputs = driven_inputs;
		self.sim.capture_caching_state(sim);
		self.sync_sim_clock_pref();
	}

	// ---- Prefs-derived queries (`Project.ShowGrid` / `.ShouldSnapToGrid` /
	// `.ForceStraightWires` / `.targetTicksPerSecond`) ----

	pub(crate) fn show_grid(&self) -> bool {
		self.prefs.prefs_grid_display_mode == 1
	}

	/// Mirrors `Project.targetTicksPerSecond`'s `Max(1, ..)` clamp,
	/// capped at 100 000 so absurdly high values can't starve the
	/// render loop.
	pub(crate) fn target_ticks_per_second(&self) -> i32 {
		self.prefs.prefs_sim_target_steps_per_second.clamp(1, 100_000)
	}

	/// Ctrl-hold forces snapping regardless of prefs; "If Grid Shown"
	/// snaps only while the grid is displayed; "Always" snaps always.
	pub(crate) fn should_snap_to_grid(&self) -> bool {
		self.sim.key_modifiers() & key_mods_bits::CONTROL != 0
			|| (self.prefs.prefs_snapping == 1 && self.show_grid())
			|| self.prefs.prefs_snapping == 2
	}

	/// Same three-way structure as [`Self::should_snap_to_grid`], for shift.
	pub(crate) fn force_straight_wires(&self) -> bool {
		self.sim.key_modifiers() & key_mods_bits::SHIFT != 0
			|| (self.prefs.prefs_straight_wires == 1 && self.show_grid())
			|| self.prefs.prefs_straight_wires == 2
	}

	/// Pushes the clock-speed pref into the live simulator (`SimThread.Run`
	/// assigning `Simulator.stepsPerClockTransition` every tick).
	pub(crate) fn sync_sim_clock_pref(&mut self) {
		self.sim.set_steps_per_clock_transition(self.prefs.prefs_sim_steps_per_clock_tick.max(0) as u32);
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
			self.sim.request_single_step();
		}
	}

	// ---- Viewed-chip stack (`Project.chipViewStack` / EnterViewMode) ----

	/// Whether the chip currently on screen may be edited: only the bottom
	/// of the view stack can (`Project.CanEditViewedChip`). While any
	/// view-only chip sits on top, canvas interaction is read-only.
	pub(crate) fn can_edit_viewed_chip(&self) -> bool {
		self.view_stack.is_empty()
	}

	/// Enters `subchip_id`'s own definition in view-only mode, if that component exists on the
	/// *currently displayed* chip (the edited root, or the chip being viewed when stacking
	/// deeper -- mirroring `EnterViewMode` looking the instance up on `ViewedChip`) and is a
	/// player-authored chip (builtins have no definition to enter -- their View row is greyed
	/// out in the popup too).
	pub(crate) fn enter_view_mode(&mut self, subchip_id: i32) {
		let displayed_name = match self.resolve_scene_target() {
			SceneTarget::EditRoot => self.root_chip_name.clone(),
			SceneTarget::Viewed { name, .. } => name,
		};
		let name = self.library.get(&displayed_name).sub_chips.iter().find(|s| s.id == subchip_id).map(|s| s.name.clone());
		let Some(name) = name.filter(|name| crate::viewer::library::is_custom_chip(&self.library, name)) else { return };

		let mut path = self.view_stack.last().map(|top| top.path.clone()).unwrap_or_default();
		path.push(subchip_id);

		self.pending_wire = None;
		self.pending_place.clear();
		chip_interaction::cancel_all(self);
		self.view_stack.push(ViewedChip { name, path });
		self.camera_fitted = false;
	}

	/// Pops back to the parent of the chip being viewed
	/// (`Project.ReturnToPreviousViewedChip`): a no-op when already at the
	/// edited root.
	pub(crate) fn return_to_previous_viewed_chip(&mut self) {
		if self.view_stack.pop().is_some() {
			chip_interaction::cancel_all(self);
			self.camera_fitted = false;
		}
	}

	/// Drops the whole view stack at once -- used wherever the *edited*
	/// root changes (open/new/save-as/rename), since every entry's id path
	/// hangs off the old root.
	pub(crate) fn exit_view_mode(&mut self) {
		if !self.view_stack.is_empty() {
			self.view_stack.clear();
			chip_interaction::cancel_all(self);
			self.camera_fitted = false;
		}
	}

	/// What should be drawn this frame: the edited root, or the top of the
	/// view stack together with its live instance's arena scope. The scope
	/// is resolved fresh every call by walking the stored id path through
	/// the sim, so views survive rebuilds; a path that no longer resolves
	/// (it can't be edited while viewed, so this is purely defensive)
	/// falls back to the root rather than drawing something stale.
	pub(crate) fn resolve_scene_target(&self) -> SceneTarget {
		let Some(top) = self.view_stack.last() else { return SceneTarget::EditRoot };
		let sim = self.sim.lock();
		let mut scope = sim.root();
		for id in &top.path {
			let Some(next) = sim.find_sub_chip(scope, *id) else { return SceneTarget::EditRoot };
			scope = next;
		}
		SceneTarget::Viewed { name: top.name.clone(), scope }
	}

	/// String form of the viewed-chips stack for the banner
	/// (`Project.UpdateViewedChipsString`): the ancestors of the chip on
	/// screen, nearest first, joined with " > ". Empty while nothing is
	/// being viewed, which keeps the banner hidden.
	pub(crate) fn viewed_chips_string(&self) -> String {
		if self.view_stack.is_empty() {
			return String::new();
		}
		let mut names: Vec<&str> = Vec::with_capacity(self.view_stack.len() + 1);
		names.push(self.root_chip_name.as_str());
		for viewed in &self.view_stack[..self.view_stack.len() - 1] {
			names.push(viewed.name.as_str());
		}
		names.reverse();
		format!("Viewing: {}", names.join(" > "))
	}
}

/// What the frame builder should draw this frame -- see
/// [`ViewerState::resolve_scene_target`].
pub(crate) enum SceneTarget {
	/// The chip being edited (the bottom of the view stack).
	EditRoot,
	/// A view-only chip: its definition's name plus its live instance's
	/// arena scope for pin-state lookups.
	Viewed { name: String, scope: ChipIdx },
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
	v.search_selected = None;
	v.search_confirming_delete = false;
	v.search_delete_message.clear();
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
		Overlay::Search => {
			v.search_query.clear();
			v.search_selected = None;
			v.search_confirming_delete = false;
			v.search_delete_message.clear();
		}
		// The pin-edit draft dies with the popup, success or not (the
		// confirm path writes its values onto the pin *before* closing).
		Overlay::PinEdit => v.pin_edit = None,
		// Same pattern for the LED colour picker.
		Overlay::LedColour => v.led_colour = None,
		Overlay::UnsavedChanges => {
			// Cancel: the pending action is dropped with the prompt --
			// mirroring `UnsavedChangesPopup` never firing its callback
			// with anything on a cancel.
			v.pending_unsaved_action = None;
		}
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

#[cfg(test)]
mod view_stack_tests {
	//! White-box: the viewed-chip stack only exists to steer the live
	//! frame builder, so driving it against a real `ViewerState` (with the
	//! same placement helpers every other viewer test uses) is what pins
	//! its enter/pop/fallback and banner-string contracts.

	use super::*;
	use crate::description::{ChipDescription, SubChipDescription};
	use crate::ChipType;

	fn viewer_with_viewable_component() -> (ViewerState, i32) {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		// Viewing is custom-only, so the viewable thing on the canvas is a
		// player-authored chip instance, not a builtin gate.
		library.add(ChipDescription::new("SUB", ChipType::Custom));
		let mut v = ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		chip_interaction::start_placing(&mut v, "SUB");
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		let id = v.library.get("ROOT").sub_chips[0].id;
		(v, id)
	}

	#[test]
	fn enter_and_pop_round_trip_through_edit_mode() {
		let (mut v, id) = viewer_with_viewable_component();

		assert!(v.can_edit_viewed_chip());
		assert_eq!(v.viewed_chips_string(), "", "nothing viewed: banner hidden");
		assert!(matches!(v.resolve_scene_target(), SceneTarget::EditRoot));

		v.enter_view_mode(id);
		assert!(!v.can_edit_viewed_chip(), "a viewed chip is read-only");
		assert_eq!(v.viewed_chips_string(), "Viewing: ROOT");
		match v.resolve_scene_target() {
			SceneTarget::Viewed { name, .. } => assert_eq!(name, "SUB"),
			SceneTarget::EditRoot => panic!("viewing must resolve to the SUB scope"),
		}

		v.return_to_previous_viewed_chip();
		assert!(v.can_edit_viewed_chip());
		assert_eq!(v.viewed_chips_string(), "");
	}

	/// Builtins have no definition to enter: their View row is greyed out
	/// in the popup, and calling straight through is refused too.
	#[test]
	fn builtins_cannot_be_entered_in_view_mode() {
		let (mut v, _custom_id) = viewer_with_viewable_component();
		chip_interaction::start_placing(&mut v, "NAND");
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::new(6.0, 0.0), &mut None);
		let nand_id = v.library.get("ROOT").sub_chips.iter().find(|s| s.name == "NAND").expect("placed").id;

		v.enter_view_mode(nand_id);
		assert!(v.can_edit_viewed_chip(), "a builtin never opens the view stack");

		// ...and the popup offers no usable View row for one either.
		let items = crate::viewer::context_menu::context_menu_items_for_component(&v.library, "NAND");
		let view_row = items.iter().find(|i| matches!(i.id, crate::render::context_menu::ContextMenuAction::View)).expect("row exists");
		assert!(!view_row.enabled, "the View row is greyed out for builtins");
	}

	#[test]
	fn nested_views_stack_and_the_banner_lists_ancestors_nearest_first() {
		let (mut v, _sub_id) = viewer_with_viewable_component();

		// Real two-level nesting: MID (placed in ROOT) contains an
		// instance of LEAF in its own definition, so entering MID and then
		// its LEAF builds a two-hop id path.
		let leaf = ChipDescription::new("LEAF", ChipType::Custom);
		v.library.add(leaf);
		let mut mid = ChipDescription::new("MID", ChipType::Custom);
		mid.sub_chips.push(SubChipDescription {
			name: "LEAF".into(),
			id: 88,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		v.library.add(mid);
		v.library.get_mut("ROOT").sub_chips.push(SubChipDescription {
			name: "MID".into(),
			id: 77,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		v.rebuild_sim();

		v.enter_view_mode(77);
		assert_eq!(v.viewed_chips_string(), "Viewing: ROOT");
		match v.resolve_scene_target() {
			SceneTarget::Viewed { name, .. } => assert_eq!(name, "MID"),
			SceneTarget::EditRoot => panic!("viewing must resolve to the MID scope"),
		}

		// Entering again resolves against the chip currently on screen,
		// not the edited root (`EnterViewMode` reads `ViewedChip`).
		v.enter_view_mode(88);
		assert_eq!(
			v.viewed_chips_string(),
			"Viewing: MID > ROOT",
			"ancestors of what's on screen, nearest first -- the original's SkipLast(1).Reverse() over [root, MID, LEAF] reads \"MID > ROOT\""
		);
		match v.resolve_scene_target() {
			SceneTarget::Viewed { name, .. } => assert_eq!(name, "LEAF"),
			SceneTarget::EditRoot => panic!("nested view lost"),
		}

		v.return_to_previous_viewed_chip();
		assert_eq!(v.viewed_chips_string(), "Viewing: ROOT");
		v.return_to_previous_viewed_chip();
		assert!(v.can_edit_viewed_chip());
	}

	#[test]
	fn entering_a_view_cancels_canvas_state_and_unknown_ids_are_ignored() {
		let (mut v, id) = viewer_with_viewable_component();
		chip_interaction::start_placing(&mut v, "NAND");
		assert!(!v.pending_place.is_empty(), "precondition: a carry is in flight");

		v.enter_view_mode(id);
		assert!(v.pending_place.is_empty() && v.selected_ids.is_empty(), "entering a view cancels in-flight state");

		v.exit_view_mode();
		v.enter_view_mode(9999);
		assert!(v.can_edit_viewed_chip(), "an id that resolves to nothing never opens a view");
	}

	/// A stale id path (only possible if the viewed subtree was somehow
	/// deleted out from under the view) falls back to drawing the edited
	/// root instead of something wrong.
	#[test]
	fn unresolvable_paths_fall_back_to_the_edited_root() {
		let (mut v, _id) = viewer_with_viewable_component();
		v.view_stack.push(ViewedChip { name: "GHOST".into(), path: vec![4242] });

		assert!(matches!(v.resolve_scene_target(), SceneTarget::EditRoot));
	}
}
