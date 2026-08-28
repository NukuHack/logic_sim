//! Render-UI integration tests: the shared `ui_kit` primitives, the
//! layered input-dispatch `UiStack`, the generic right-click context
//! menu, every editor overlay builder, and the startup-menu screen
//! builders -- all exercised through their public constructors.

use logic_sim::json::{ChipCollection, ProjectDescription, StarredItem};
use logic_sim::render::context_menu::{build_context_menu, ContextMenuAction, ContextMenuItem, ContextMenuState};
use logic_sim::render::editor_ui::{
	build_chip_library_panel, build_key_select_popup, build_pin_edit_popup, build_preferences_panel, build_rom_editor_popup, build_save_chip_popup,
	build_search_popup, build_simple_naming_popup, build_starred_bottom_bar, build_starred_collection_popup, build_unsaved_changes_popup,
	ChipLibraryState, EditorAction, LibrarySelection, PrefValueField, PrefsPanelState, SaveChipMode, SearchPopupState, BOTTOM_BAR_HEIGHT,
	KEY_SELECT_ALLOWED_CHARS, ROM_WORD_COUNT,
};
use logic_sim::render::menu_ui::{build, build_popup_frame, build_screen, status_label, UiAction};
use logic_sim::render::ui_kit::{hovered_button, text_field_row, Button, Frame, FONT_SIZE};
use logic_sim::render::ui_kit::{to_world, UiCtx, UiRect};
use logic_sim::render::ui_stack::{Capture, InputResult, LayerId, StackLayer, UiStack};
use logic_sim::save_system::{create_project, SavePaths};
use logic_sim::ui_menu::MainMenu;
use logic_sim::Vec2;
use std::collections::HashSet;

/// Scratch-directory helper (the crate's own `test_util::temp_dir` is
/// unit-test-only).
fn temp_dir(label: &str) -> std::path::PathBuf {
	let pid = std::process::id();
	let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
	std::env::temp_dir().join(format!("dls_rust_integration_{label}_{pid}_{nanos}"))
}

fn point(x: f32, y: f32) -> Vec2 {
	Vec2::new(x, y)
}

fn button_layer(id: LayerId, rect: UiRect, capture: Capture) -> StackLayer<&'static str> {
	let mut frame: Frame<&'static str> = Frame::default();
	frame.geometry.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), [1.0, 1.0, 1.0, 1.0]);
	frame.buttons.push(Button { rect, action: "btn", enabled: true });
	StackLayer::from_frame(id, frame, capture)
}

fn sample_desc() -> ProjectDescription {
	ProjectDescription {
		prefs_main_pin_names_display_mode: 1,
		prefs_chip_pin_names_display_mode: 0,
		prefs_grid_display_mode: 1,
		prefs_snapping: 2,
		prefs_straight_wires: 0,
		prefs_sim_paused: true,
		prefs_sim_target_steps_per_second: 1000,
		prefs_sim_steps_per_clock_tick: 250,
		..Default::default()
	}
}

fn prefs_panel_state<'a>(desc: &'a ProjectDescription) -> PrefsPanelState<'a> {
	PrefsPanelState { desc, clock_text: "250", rate_text: "1000", focused_field: None, measured_speed_label: "0".to_string() }
}

fn sample_library_state<'a>(collections: &'a [ChipCollection], starred: &'a [StarredItem], selection: LibrarySelection) -> ChipLibraryState<'a> {
	ChipLibraryState {
		collections,
		starred_list: starred,
		selection,
		selected_chip_is_custom: true,
		selected_chip_would_cycle: false,
		creating_collection: false,
		renaming_collection: false,
		name_field_text: "",
		confirming_chip_delete: false,
		confirming_collection_delete: false,
		delete_confirm_message: "",
	}
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

#[test]
fn topmost_button_walks_front_to_back_and_skips_disabled_rows() {
	let rect = UiRect::new(0.0, 0.0, 50.0, 50.0);

	fn layer_with(id: LayerId, capture: Capture, action: &'static str, enabled: bool) -> StackLayer<&'static str> {
		let mut frame: Frame<&'static str> = Frame::default();
		frame.geometry.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), [1.0, 1.0, 1.0, 1.0]);
		frame.buttons.push(Button { rect: UiRect::new(0.0, 0.0, 50.0, 50.0), action, enabled });
		StackLayer::from_frame(id, frame, capture)
	}

	let mut stack = UiStack::new();
	stack.push(layer_with(LayerId::BottomBar, Capture::Rect(rect), "bottom", true));
	stack.push(layer_with(LayerId::Library, Capture::FullScreen, "top", true));

	let (layer, button) = stack.topmost_button(Vec2::new(10.0, 10.0)).expect("a row is under the point");
	assert_eq!(layer, LayerId::Library);
	assert_eq!(button.action, "top");

	// Rebuild the same stack with the top row disabled -- built through
	// the public frame API rather than mutating the stack's private layer
	// list -- and the search now sees through to the bar's own row underneath.
	let mut stack = UiStack::new();
	stack.push(layer_with(LayerId::BottomBar, Capture::Rect(rect), "bottom", true));
	stack.push(layer_with(LayerId::Library, Capture::FullScreen, "top", false));

	let (layer, button) = stack.topmost_button(Vec2::new(10.0, 10.0)).expect("the bar's row is still under the point");
	assert_eq!(layer, LayerId::BottomBar);
	assert_eq!(button.action, "bottom");

	assert_eq!(stack.topmost_button(Vec2::new(500.0, 500.0)), None);
}

#[test]
fn ui_rect_contains_checks_bounds_inclusively() {
	let r = UiRect::new(10.0, 10.0, 100.0, 20.0);
	assert!(r.contains(Vec2::new(10.0, 10.0)));
	assert!(r.contains(Vec2::new(110.0, 30.0)));
	assert!(r.contains(Vec2::new(60.0, 20.0)));
	assert!(!r.contains(Vec2::new(9.0, 20.0)));
	assert!(!r.contains(Vec2::new(60.0, 31.0)));
}

#[test]
fn hovered_button_respects_require_enabled() {
	let rect = UiRect::new(0.0, 0.0, 10.0, 10.0);
	let buttons = vec![Button { rect, action: "a", enabled: false }];
	let inside = Vec2::new(5.0, 5.0);
	assert_eq!(hovered_button(&buttons, inside, true), None);
	assert_eq!(hovered_button(&buttons, inside, false), Some("a"));
}

#[test]
fn text_field_row_falls_back_to_placeholder_when_empty() {
	let mut frame: Frame<()> = Frame::default();
	let rect = UiRect::new(0.0, 0.0, 100.0, 20.0);
	text_field_row(&mut frame, UiCtx::new(200.0, 100.0, Vec2::ZERO), rect, "", "Search...", FONT_SIZE, 16.0);
	assert_eq!(frame.text_field, Some(rect));
	assert!(frame.geometry.labels.iter().any(|l| l.text == "Search...|"));
}

#[test]
fn builds_one_row_per_item() {
	let state = ContextMenuState::new("AND", Vec2::new(100.0, 100.0), vec![ContextMenuItem::new("Open", ContextMenuAction::Open)]);
	let frame = build_context_menu(&state, 1280.0, 800.0, Vec2::ZERO);
	assert_eq!(frame.buttons.len(), 1);
	assert_eq!(frame.buttons[0].id, ContextMenuAction::Open);
	assert!(frame.geometry.labels.iter().any(|l| l.text == "Open"));
}

#[test]
fn clamps_panel_to_stay_on_screen() {
	// Anchored near the bottom-right corner -- panel must not run
	// off either edge.
	let state = ContextMenuState::new(
		"AND",
		Vec2::new(1275.0, 795.0),
		vec![ContextMenuItem::new("Open", ContextMenuAction::Open), ContextMenuItem::new("Another", ContextMenuAction::Other)],
	);
	let frame = build_context_menu(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.panel_rect.x + frame.panel_rect.w <= 1280.0);
	assert!(frame.panel_rect.y + frame.panel_rect.h <= 800.0);
}

#[test]
fn hovered_row_is_reported() {
	let state = ContextMenuState::new("AND", Vec2::new(10.0, 10.0), vec![ContextMenuItem::new("Open", ContextMenuAction::Open)]);
	let mouse = Vec2::new(20.0, 20.0); // inside the single row
	let frame = build_context_menu(&state, 1280.0, 800.0, mouse);
	assert_eq!(frame.hovered, Some(ContextMenuAction::Open));
}

#[test]
fn empty_items_produces_no_panel() {
	let state = ContextMenuState::new("AND", Vec2::new(10.0, 10.0), vec![]);
	let frame = build_context_menu(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.is_empty());
	assert!(frame.geometry.triangles.is_empty());
}

#[test]
fn preferences_panel_has_one_cycle_button_per_row_plus_apply_and_close() {
	let frame = build_preferences_panel(&prefs_panel_state(&sample_desc()), 1280.0, 800.0, Vec2::ZERO);
	let cycle_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::CyclePref(_))).count();
	assert_eq!(cycle_count, 6);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ApplyPreferences));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
}

#[test]
fn preferences_panel_shows_currently_selected_option_text() {
	let frame = build_preferences_panel(&prefs_panel_state(&sample_desc()), 1280.0, 800.0, Vec2::ZERO);
	// Row 0 is "Show I/O pin names" with mode 1 => "On Hover".
	assert!(frame.geometry.labels.iter().any(|l| l.text == "On Hover"));
	// Row 5 is "Sim status" with prefs_sim_paused = true => "Paused".
	assert!(frame.geometry.labels.iter().any(|l| l.text == "Paused"));
}

#[test]
fn preferences_panel_offers_both_numeric_fields_and_shows_their_drafts() {
	let frame = build_preferences_panel(&prefs_panel_state(&sample_desc()), 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SelectPrefsField(PrefValueField::ClockSpeed)));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SelectPrefsField(PrefValueField::TargetRate)));

	// The draft texts (rendered with their trailing caret) and the
	// measured-speed readout are drawn as-is.
	assert!(frame.geometry.labels.iter().any(|l| l.text == "250|"));
	assert!(frame.geometry.labels.iter().any(|l| l.text == "1000|"));
	assert!(frame.geometry.labels.iter().any(|l| l.text == "Steps per second (current)"));
	assert!(frame.geometry.labels.iter().any(|l| l.text == "0"));
	assert!(!frame.geometry.labels.iter().any(|l| l.text == "999"));
}

#[test]
fn preferences_panel_registers_the_focused_field_as_its_text_field() {
	let desc = sample_desc();
	let mut state = prefs_panel_state(&desc);
	state.focused_field = Some(PrefValueField::ClockSpeed);
	let frame = build_preferences_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.text_field.is_some(), "a focused field makes the panel the keyboard target");

	let mut unfocused = prefs_panel_state(&desc);
	unfocused.focused_field = None;
	let frame = build_preferences_panel(&unfocused, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.text_field.is_none(), "no field focused => typing stays with the app");
}

#[test]
fn chip_library_panel_only_lists_chips_for_open_collections() {
	let collections = vec![
		ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into(), "OR".into()] },
		ChipCollection { name: "CLOSED".into(), is_toggled_open: false, chips: vec!["XOR".into()] },
	];
	let state = sample_library_state(&collections, &[], LibrarySelection::None);
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

	let select_actions: Vec<_> = frame
		.buttons
		.iter()
		.filter_map(|b| if let EditorAction::SelectChipRow { collection, chip } = &b.action { Some((*collection, *chip)) } else { None })
		.collect();
	assert_eq!(select_actions, vec![(0, 0), (0, 1)]);

	let toggle_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::SelectCollection(_))).count();
	assert_eq!(toggle_count, 2);
}

#[test]
fn chip_library_panel_shows_open_and_delete_for_the_selected_chip() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
	let state = sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0));
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	let open_btn = frame.buttons.iter().find(|b| b.action == EditorAction::OpenSelectedChip("AND".to_string())).unwrap();
	assert!(open_btn.enabled);
	let delete_btn = frame.buttons.iter().find(|b| b.action == EditorAction::RequestDeleteChip("AND".to_string())).unwrap();
	assert!(delete_btn.enabled);
}

fn sample_search_state<'a>(names: &'a [String], query: &'a str, selected: Option<&'a str>) -> SearchPopupState<'a> {
	SearchPopupState {
		all_names: names,
		query,
		selected,
		selected_is_starred: false,
		selected_is_custom: true,
		selected_would_cycle: false,
		confirming_delete: false,
		delete_confirm_message: "",
	}
}

#[test]
fn search_popup_filters_case_insensitively() {
	let names = vec!["AND".to_string(), "OR".to_string(), "NAND".to_string()];
	let state = sample_search_state(&names, "an", None);
	let frame = build_search_popup(&state, 1280.0, 800.0, Vec2::ZERO);
	let shown: Vec<_> =
		frame.buttons.iter().filter_map(|b| if let EditorAction::SelectSearchResult(n) = &b.action { Some(n.clone()) } else { None }).collect();
	assert_eq!(shown, vec!["AND".to_string(), "NAND".to_string()]);
}

#[test]
fn search_popup_with_empty_query_lists_everything() {
	let names = vec!["AND".to_string(), "OR".to_string()];
	let state = sample_search_state(&names, "", None);
	let frame = build_search_popup(&state, 1280.0, 800.0, Vec2::ZERO);
	let select_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::SelectSearchResult(_))).count();
	assert_eq!(select_count, 2);
	assert!(frame.text_field.is_some());
}

#[test]
fn search_popup_shows_a_message_when_nothing_matches() {
	let names = vec!["AND".to_string()];
	let state = sample_search_state(&names, "zzz", None);
	let frame = build_search_popup(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().all(|b| !matches!(b.action, EditorAction::SelectSearchResult(_))));
	assert!(frame.geometry.labels.iter().any(|l| l.text.contains("No matching")));
}

#[test]
fn search_popup_selection_shows_open_use_delete_and_star_buttons() {
	let names = vec!["AND".to_string(), "OR".to_string()];
	let state = sample_search_state(&names, "", Some("AND"));
	let frame = build_search_popup(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::OpenSelectedChip("AND".to_string())));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::RequestDeleteSearchChip("AND".to_string())));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::PlaceChip("AND".to_string())));
	assert!(frame
		.buttons
		.iter()
		.any(|b| b.action == EditorAction::ToggleStarred { name: "AND".to_string(), is_collection: false }));
}

#[test]
fn simple_naming_popup_exposes_a_text_field_and_respects_confirm_enabled() {
	let frame = build_simple_naming_popup("Rename", "My Label", false, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.text_field.is_some());
	let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::ConfirmName).unwrap();
	assert!(!confirm.enabled);

	let frame_ok = build_simple_naming_popup("Rename", "My Label", true, 1280.0, 800.0, Vec2::ZERO);
	let confirm_ok = frame_ok.buttons.iter().find(|b| b.action == EditorAction::ConfirmName).unwrap();
	assert!(confirm_ok.enabled);
}

#[test]
fn key_select_popup_disables_confirm_until_a_key_is_chosen() {
	let frame_none = build_key_select_popup(None, 1280.0, 800.0, Vec2::ZERO);
	let confirm_none = frame_none.buttons.iter().find(|b| b.action == EditorAction::ConfirmKey).unwrap();
	assert!(!confirm_none.enabled);
	assert!(frame_none.text_field.is_none());

	let frame_some = build_key_select_popup(Some('Q'), 1280.0, 800.0, Vec2::ZERO);
	let confirm_some = frame_some.buttons.iter().find(|b| b.action == EditorAction::ConfirmKey).unwrap();
	assert!(confirm_some.enabled);
	assert!(frame_some.geometry.labels.iter().any(|l| l.text == "Q"));
}

#[test]
fn pin_edit_popup_offers_the_display_wheel_only_when_asked() {
	// Multi-bit call: one option button per Decimal Display mode plus
	// Confirm/Cancel, with the chosen mode's tile highlighted.
	let multi = build_pin_edit_popup("BUS", true, 2, 0, 1280.0, 800.0, Vec2::ZERO);
	for i in 0..4 {
		assert!(multi.buttons.iter().any(|b| b.action == EditorAction::PinEditSetDisplayMode(i)), "Decimal Display option {i} must be clickable");
	}
	assert!(multi.buttons.iter().any(|b| b.action == EditorAction::ConfirmPinEdit));
	assert!(multi.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
	assert!(multi.text_field.is_some(), "the pin name field owns typing");
	let active_highlight = [0.3f32, 0.42, 0.58, 1.0].map(f32::to_bits);
	assert!(multi.geometry.triangles.iter().any(|v| v.colour.map(f32::to_bits) == active_highlight), "the selected wheel option is highlighted");

	// 1-bit call: no wheel at all, and no highlight tiles.
	let single = build_pin_edit_popup("CLK", false, 1, 0, 1280.0, 800.0, Vec2::ZERO);
	assert!(!single.buttons.iter().any(|b| matches!(b.action, EditorAction::PinEditSetDisplayMode(_))));
	assert!(!single.geometry.triangles.iter().any(|v| v.colour.map(f32::to_bits) == active_highlight));
}

#[test]
fn pin_edit_popup_offers_colour_swatch_rows() {
	// One clickable swatch per palette colour in both variants; the
	// picked swatch carries the translucent-white wash.
	let pick_wash = [1.0f32, 1.0, 1.0, 0.35].map(f32::to_bits);
	for (show_display_options, colour_index) in [(true, 4usize), (false, 7)] {
		let frame = build_pin_edit_popup("BUS", show_display_options, 0, colour_index, 1280.0, 800.0, Vec2::ZERO);
		for i in 0..8 {
			assert!(frame.buttons.iter().any(|b| b.action == EditorAction::PinEditSetColour(i)), "colour swatch {i} must be clickable");
		}
		assert!(frame.geometry.triangles.iter().any(|v| v.colour.map(f32::to_bits) == pick_wash), "the picked swatch is washed out");
	}
}

#[test]
fn pin_edit_popup_gates_confirm_on_name_presence_and_length() {
	let confirm_for = |name: &str| {
		build_pin_edit_popup(name, false, 0, 0, 1280.0, 800.0, Vec2::ZERO)
			.buttons
			.iter()
			.find(|b| b.action == EditorAction::ConfirmPinEdit)
			.unwrap()
			.enabled
	};

	assert!(!confirm_for(""), "empty draft keeps Confirm off");
	assert!(confirm_for("CLK"));
	assert!(confirm_for("MY LONG PIN NAME"), "exactly the max length is allowed");
	assert!(!confirm_for("MY LONG PIN NAME+"), "one past the max is rejected");
}

#[test]
fn unsaved_changes_popup_warns_and_offers_continue_or_cancel() {
	let frame = build_unsaved_changes_popup(1280.0, 800.0, Vec2::ZERO);
	let cont = frame.buttons.iter().find(|b| b.action == EditorAction::UnsavedChangesConfirm).unwrap();
	assert!(cont.enabled, "Continue is always available");
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup), "Cancel closes without acting");
	assert!(frame.geometry.labels.iter().any(|l| l.text.contains("unsaved changes")), "the warning copy is shown");
}

#[test]
fn rom_editor_has_one_selectable_cell_per_word() {
	let data = vec![0u32; ROM_WORD_COUNT];
	let frame = build_rom_editor_popup(&data, 0, "0", 1280.0, 800.0, Vec2::ZERO);
	let cell_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::RomSelectCell(_))).count();
	assert_eq!(cell_count, ROM_WORD_COUNT);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::RomApply));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
	assert!(frame.text_field.is_some());
}

#[test]
fn rom_editor_shows_selected_cells_value_in_its_text_field() {
	let mut data = vec![0u32; ROM_WORD_COUNT];
	data[5] = 1234;
	let frame = build_rom_editor_popup(&data, 5, "1234", 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.geometry.labels.iter().any(|l| l.text.contains("1234")));
	assert!(frame.geometry.labels.iter().any(|l| l.text.contains("Address 5")));
}

#[test]
fn save_chip_mode_save_shows_single_confirm_button() {
	let frame = build_save_chip_popup("Full Adder", "Full Adder", SaveChipMode::Save, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipConfirm));
	assert!(!frame.buttons.iter().any(|b| matches!(b.action, EditorAction::SaveChipSaveAs | EditorAction::SaveChipRename)));
}

#[test]
fn save_chip_mode_save_as_or_rename_shows_both_options() {
	let frame = build_save_chip_popup("Full Adder", "Full Adder 2", SaveChipMode::SaveAsOrRename, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipSaveAs));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipRename));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
}

#[test]
fn save_chip_confirm_disabled_for_empty_name() {
	let frame = build_save_chip_popup("Full Adder", "", SaveChipMode::Save, 1280.0, 800.0, Vec2::ZERO);
	let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::SaveChipConfirm).unwrap();
	assert!(!confirm.enabled);
}

#[test]
fn key_select_allowed_chars_are_alphanumeric_uppercase() {
	assert!(KEY_SELECT_ALLOWED_CHARS.chars().all(|c| c.is_ascii_alphanumeric() && (c.is_ascii_digit() || c.is_ascii_uppercase())));
}

#[test]
fn chip_library_star_button_reads_add_or_remove_depending_on_starred_state() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
	let starred = vec![StarredItem::new("AND", false)];
	let state = sample_library_state(&collections, &starred, LibrarySelection::Chip(0, 0));
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ToggleStarred { name: "AND".to_string(), is_collection: false }));
	assert!(frame.geometry.labels.iter().any(|l| l.text == "REMOVE FROM STARRED"));
}

#[test]
fn chip_library_move_buttons_disabled_at_the_ends_of_a_collection() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into(), "OR".into()] }];
	let state = sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0));
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	let up = frame.buttons.iter().find(|b| b.action == EditorAction::MoveSelectedStep(false)).unwrap();
	assert!(!up.enabled, "first chip in the only collection can't move up or jump");
	let down = frame.buttons.iter().find(|b| b.action == EditorAction::MoveSelectedStep(true)).unwrap();
	assert!(down.enabled, "second chip in the collection is still below it");
}

#[test]
fn chip_library_new_collection_footer_shows_a_text_field_when_creating() {
	let state =
		ChipLibraryState { creating_collection: true, name_field_text: "My Collection", ..sample_library_state(&[], &[], LibrarySelection::None) };
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.text_field.is_some());
	let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::ConfirmCollectionName).unwrap();
	assert!(confirm.enabled);
}

#[test]
fn chip_library_delete_confirmation_hides_the_normal_detail_buttons() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
	let state = ChipLibraryState {
		confirming_chip_delete: true,
		delete_confirm_message: "Are you sure?",
		..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0))
	};
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ConfirmDelete));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::CancelLibraryPopup));
	assert!(!frame.buttons.iter().any(|b| matches!(b.action, EditorAction::OpenSelectedChip(_))));
}

#[test]
fn chip_library_detail_panel_offers_use_for_both_custom_and_builtin_chips() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
	let state = ChipLibraryState { selected_chip_is_custom: false, ..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0)) };
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

	let use_button = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("AND".to_string())).unwrap();
	assert!(use_button.enabled, "USE should place builtins too, unlike OPEN/DELETE");
}

#[test]
fn chip_library_detail_panel_greys_out_use_for_a_chip_that_would_cycle() {
	let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["SubCircuit".into()] }];
	let state = ChipLibraryState { selected_chip_would_cycle: true, ..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0)) };
	let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

	let use_button = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("SubCircuit".to_string())).unwrap();
	assert!(!use_button.enabled);
}

#[test]
fn starred_bottom_bar_has_one_button_per_starred_item() {
	let starred = vec![StarredItem::new("AND", false), StarredItem::new("Basics", true)];
	let frame = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::PlaceChip("AND".to_string())));
	assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ToggleStarredCollectionPopup("Basics".to_string())));
	assert_eq!(frame.panel, Some(UiRect::new(0.0, 800.0 - BOTTOM_BAR_HEIGHT, 1280.0, BOTTOM_BAR_HEIGHT)));
}

#[test]
fn starred_bottom_bar_buttons_disabled_when_not_editable() {
	let starred = vec![StarredItem::new("AND", false)];
	let frame = build_starred_bottom_bar(&starred, None, false, &HashSet::new(), 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
	assert!(frame.buttons.iter().all(|b| !b.enabled));
}

#[test]
fn starred_bottom_bar_scroll_x_shifts_buttons_left() {
	let starred: Vec<_> = (1..=6).map(|i| StarredItem::new(format!("ChipName{i}"), false)).collect();
	let unscrolled = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 0.0, UiCtx::new(600.0, 800.0, Vec2::ZERO));
	let scrolled = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 120.0, UiCtx::new(600.0, 800.0, Vec2::ZERO));
	for (a, b) in unscrolled.buttons.iter().zip(&scrolled.buttons) {
		assert!((a.rect.x - 120.0 - b.rect.x).abs() < 1e-3, "every button shifts left by the scroll offset");
		assert!((a.rect.y - b.rect.y).abs() < 1e-3 && a.rect.w == b.rect.w);
	}
}

#[test]
fn starred_bottom_bar_greys_out_a_chip_that_would_cycle() {
	let starred = vec![StarredItem::new("AND", false), StarredItem::new("SubCircuit", false)];
	let blocked: HashSet<String> = ["subcircuit".to_string()].into_iter().collect();
	let frame = build_starred_bottom_bar(&starred, None, true, &blocked, 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
	let and_btn = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("AND".to_string())).unwrap();
	let sub_btn = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("SubCircuit".to_string())).unwrap();
	assert!(and_btn.enabled);
	assert!(!sub_btn.enabled);
}

#[test]
fn starred_collection_popup_has_one_button_per_chip_and_exposes_its_panel() {
	let collection = ChipCollection::new("Basics", ["AND", "OR", "NOT"]);
	let frame = build_starred_collection_popup(&collection, 20.0, true, &HashSet::new(), 1280.0, 800.0, Vec2::ZERO);
	let names: Vec<_> =
		frame.buttons.iter().filter_map(|b| if let EditorAction::PlaceChip(n) = &b.action { Some(n.clone()) } else { None }).collect();
	assert_eq!(names, vec!["AND".to_string(), "OR".to_string(), "NOT".to_string()]);

	// The panel rect is what the host turns into the flyout layer's capture region -- it must
	// cover every row so clicks on the padding between/around rows belong to the flyout.
	let panel = frame.panel.expect("the flyout exposes its background rect");
	for b in &frame.buttons {
		assert!(panel.x <= b.rect.x && b.rect.x + b.rect.w <= panel.x + panel.w);
		assert!(panel.y <= b.rect.y && b.rect.y + b.rect.h <= panel.y + panel.h);
	}
}

#[test]
fn to_world_round_trips_through_camera_world_to_screen() {
	// Mirrors the camera setup the app's menu screen draws through: a camera centred on the viewport
	// with zoom 1.0, so world space and screen space coincide (with a y-flip, since world is y-up and screen is y-down).
	let vw = 1280.0;
	let vh = 800.0;
	let cam = logic_sim::render::camera::Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport: Vec2::new(vw, vh) };

	let screen = Vec2::new(300.0, 150.0);
	let world = to_world(screen, vw, vh);
	let back = cam.world_to_screen(world);

	assert!((back.x - screen.x).abs() < 1e-3);
	assert!((back.y - screen.y).abs() < 1e-3);
}

#[test]
fn main_screen_has_five_buttons_with_expected_actions() {
	let root = temp_dir("menu_ui_main_screen");
	let menu = MainMenu::new(SavePaths::new(&root));
	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);

	let actions: Vec<_> = frame.buttons.iter().map(|b| b.action.clone()).collect();
	assert_eq!(actions, vec![UiAction::NewProject, UiAction::OpenProjectScreen, UiAction::SettingsScreen, UiAction::AboutScreen, UiAction::Quit]);
	assert!(frame.buttons.iter().all(|b| b.enabled));
	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_project_screen_lists_projects_as_clickable_rows() {
	let root = temp_dir("menu_ui_load_screen");
	let paths = SavePaths::new(&root);
	create_project(&paths, "Alpha").unwrap();
	create_project(&paths, "Beta").unwrap();

	let mut menu = MainMenu::new(paths);
	menu.choose_open_project();

	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	let select_actions: Vec<_> = frame.buttons.iter().filter(|b| matches!(b.action, UiAction::SelectProject(_))).collect();
	assert_eq!(select_actions.len(), 2);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_project_screen_toolbar_buttons_disabled_without_a_selection() {
	let root = temp_dir("menu_ui_toolbar_disabled");
	let paths = SavePaths::new(&root);
	create_project(&paths, "Alpha").unwrap();
	let mut menu = MainMenu::new(paths);
	menu.choose_open_project();

	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	let open_btn = frame.buttons.iter().find(|b| b.action == UiAction::OpenSelected).unwrap();
	let delete_btn = frame.buttons.iter().find(|b| b.action == UiAction::DeleteSelected).unwrap();
	assert!(!open_btn.enabled);
	assert!(!delete_btn.enabled);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_project_screen_toolbar_buttons_enabled_once_a_compatible_project_is_selected() {
	let root = temp_dir("menu_ui_toolbar_enabled");
	let paths = SavePaths::new(&root);
	create_project(&paths, "Alpha").unwrap();
	let mut menu = MainMenu::new(paths);
	menu.choose_open_project();
	menu.select_project(0);

	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	let open_btn = frame.buttons.iter().find(|b| b.action == UiAction::OpenSelected).unwrap();
	let rename_btn = frame.buttons.iter().find(|b| b.action == UiAction::RenameSelected).unwrap();
	let delete_btn = frame.buttons.iter().find(|b| b.action == UiAction::DeleteSelected).unwrap();
	assert!(open_btn.enabled);
	assert!(rename_btn.enabled);
	assert!(delete_btn.enabled);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn new_project_popup_shows_a_text_field_and_disables_confirm_for_invalid_names() {
	let root = temp_dir("menu_ui_new_project_popup");
	let paths = SavePaths::new(&root);
	let mut menu = MainMenu::new(paths);
	menu.choose_new_project();

	let frame_empty = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	assert!(frame_empty.text_field.is_some());
	let confirm = frame_empty.buttons.iter().find(|b| b.action == UiAction::PopupConfirm).unwrap();
	assert!(!confirm.enabled, "empty name should not be confirmable");

	let frame_valid = build(&menu, 1280.0, 800.0, "My New Project", Vec2::ZERO);
	let confirm = frame_valid.buttons.iter().find(|b| b.action == UiAction::PopupConfirm).unwrap();
	assert!(confirm.enabled);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn popup_confirm_and_cancel_buttons_stay_inside_the_panel_and_are_symmetric() {
	// Regression test for the popup buttons overflowing/misaligning
	// (todo.txt: "some do even flow out from the container"). Confirm
	// and Cancel must be equal width, share the panel's own side
	// margins, and never spill past its left/right edges.
	let root = temp_dir("menu_ui_popup_button_bounds");
	let paths = SavePaths::new(&root);
	let mut menu = MainMenu::new(paths);
	menu.choose_new_project();

	let frame = build_popup_frame(&menu, 1280.0, 800.0, "My New Project", Vec2::ZERO);
	let confirm = frame.buttons.iter().find(|b| b.action == UiAction::PopupConfirm).unwrap();
	let cancel = frame.buttons.iter().find(|b| b.action == UiAction::PopupCancel).unwrap();

	assert!((confirm.rect.w - cancel.rect.w).abs() < 1e-6, "both buttons should be the same width");
	assert!(confirm.rect.x < cancel.rect.x, "confirm sits to the left of cancel");
	assert!(confirm.rect.x + confirm.rect.w <= cancel.rect.x, "buttons must not overlap");

	// The panel itself is centred and 420 wide (see `build_popup`); every
	// button edge must land strictly inside it, with no reliance on
	// `clamp_to` to pull an overflowing rect back in.
	let panel_left = 1280.0 / 2.0 - 420.0 / 2.0;
	let panel_right = 1280.0 / 2.0 + 420.0 / 2.0;
	assert!(confirm.rect.x >= panel_left, "confirm must not flow out of the panel's left edge");
	assert!(cancel.rect.x + cancel.rect.w <= panel_right, "cancel must not flow out of the panel's right edge");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_screen_excludes_popup_buttons_and_build_popup_frame_is_popup_only() {
	// Regression test: a click on the popup must never fall through and hit a main-menu button
	// underneath it (e.g. the New Project popup's Confirm overlapping the Quit button) -- which
	// requires the popup's hit-boxes to live in their own frame, separate from the screen's.
	let root = temp_dir("menu_ui_build_screen_vs_popup");
	let paths = SavePaths::new(&root);
	let mut menu = MainMenu::new(paths);
	menu.choose_new_project();

	let screen_frame = build_screen(&menu, 1280.0, 800.0, Vec2::ZERO);
	assert!(
		screen_frame.buttons.iter().all(|b| b.action != UiAction::PopupConfirm && b.action != UiAction::PopupCancel),
		"build_screen must not include the popup's buttons"
	);
	assert!(screen_frame.buttons.iter().any(|b| b.action == UiAction::Quit), "build_screen should still draw the main screen underneath the popup");

	let popup_frame = build_popup_frame(&menu, 1280.0, 800.0, "My New Project", Vec2::ZERO);
	assert!(popup_frame.buttons.iter().any(|b| b.action == UiAction::PopupConfirm));
	assert!(popup_frame.buttons.iter().any(|b| b.action == UiAction::PopupCancel));
	assert!(popup_frame.buttons.iter().all(|b| b.action != UiAction::Quit), "build_popup_frame must not include the screen's own buttons");

	// Together they must cover exactly what the combined `build` call
	// produces, so existing (non-layered) callers see no change.
	let combined = build(&menu, 1280.0, 800.0, "My New Project", Vec2::ZERO);
	let mut split_actions: Vec<_> = screen_frame.buttons.iter().chain(popup_frame.buttons.iter()).map(|b| b.action.clone()).collect();
	let mut combined_actions: Vec<_> = combined.buttons.iter().map(|b| b.action.clone()).collect();
	split_actions.sort_by_key(|a| format!("{a:?}"));
	combined_actions.sort_by_key(|a| format!("{a:?}"));
	assert_eq!(split_actions, combined_actions);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_popup_frame_is_empty_when_no_popup_is_open() {
	let root = temp_dir("menu_ui_build_popup_frame_empty");
	let paths = SavePaths::new(&root);
	let menu = MainMenu::new(paths);

	let popup_frame = build_popup_frame(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	assert!(popup_frame.buttons.is_empty());
	assert!(popup_frame.geometry.triangles.is_empty());
	assert!(popup_frame.geometry.labels.is_empty());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_confirmation_popup_has_no_text_field() {
	let root = temp_dir("menu_ui_delete_popup");
	let paths = SavePaths::new(&root);
	create_project(&paths, "Doomed").unwrap();
	let mut menu = MainMenu::new(paths);
	menu.choose_open_project();
	menu.select_project(0);
	menu.request_delete_selected();

	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	assert!(frame.text_field.is_none());
	assert!(frame.buttons.iter().any(|b| b.action == UiAction::PopupConfirm));
	assert!(frame.buttons.iter().any(|b| b.action == UiAction::PopupCancel));

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn hovered_reports_the_button_under_the_mouse() {
	let root = temp_dir("menu_ui_hovered");
	let menu = MainMenu::new(SavePaths::new(&root));
	let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	let new_project_btn = frame.buttons.iter().find(|b| b.action == UiAction::NewProject).unwrap();
	let inside = Vec2::new(new_project_btn.rect.x + 5.0, new_project_btn.rect.y + 5.0);

	let frame_hovered = build(&menu, 1280.0, 800.0, "", inside);
	assert_eq!(frame_hovered.hovered, Some(UiAction::NewProject));

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn status_label_is_positioned_near_the_bottom_of_the_viewport() {
	let label = status_label(1280.0, 800.0, "oh no");
	assert_eq!(label.text, "oh no");
	// world.y = vh - screen.y, and the label is placed near screen
	// bottom (small vh - y), so its world y should be small too.
	assert!(label.pos.y < 30.0);
}

#[test]
fn about_and_settings_screens_render_a_back_button() {
	let root = temp_dir("menu_ui_about_settings");
	let paths = SavePaths::new(&root);
	let mut menu = MainMenu::new(paths);

	menu.choose_about();
	let about_frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	assert!(about_frame.buttons.iter().any(|b| b.action == UiAction::BackToMain));

	menu.choose_settings();
	let settings_frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
	assert!(settings_frame.buttons.iter().any(|b| b.action == UiAction::BackToMain));
	assert!(settings_frame.buttons.iter().any(|b| b.action == UiAction::ToggleVsync));

	std::fs::remove_dir_all(&root).ok();
}
