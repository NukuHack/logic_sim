//! Frame construction: rebuilds each screen's whole UI stack from live
//! state, bottom-to-top -- the menu screen and its popup, or the viewer's
//! canvas/bottom-bar/flyout/overlay-panel/context-menu layers. Input
//! dispatch in the event handlers consults exactly what's built here.

use crate::render::camera::Camera;
use crate::render::context_menu;
use crate::render::editor_ui::{self, LibrarySelection};
use crate::render::menu_ui::{self};
use crate::render::scene::{bounding_box, build_grid, build_scene, AllLow, SceneGeometry, SimulatorPinState};
use crate::render::theme;
use crate::render::ui_kit::{pin_geometry_to_screen, Button, UiCtx, UiRect};
use crate::render::ui_stack::{Capture, LayerId, StackLayer, UiStack};
use crate::structs::Vec2;
use crate::ui_menu::{MainMenu, PopupKind};

use crate::viewer::canvas::{build_pending_place_scene, draw_pending_wire_preview};
use crate::viewer::library::{is_custom_chip, would_create_cycle};
use crate::viewer::save_flow::save_chip_mode;
use crate::viewer::state::{editor_action, NamingPurpose, Overlay, ViewerAction, ViewerState};

/// Builds the transient status/error toast (`LayerId::StatusToast`): a small dark strip with the
/// message centred in it, floating just above the bottom bar in the viewer (`above_y`) or near
/// the very bottom on the menu screen. Returned in raw screen-pixel space -- the caller pins it
/// through the active camera with `pin_geometry_to_screen`. Its layer never captures input, so it
/// can sit at the very top of the stack without getting in anyone's way.
fn status_toast_geometry(message: &str, vw: f32, vh: f32, above_y: Option<f32>) -> SceneGeometry {
	let width = (vw - 80.0).min(700.0);
	let centre_y = above_y.unwrap_or(vh - 26.0);
	let bg = UiRect::new((vw - width) / 2.0, centre_y - 14.0, width, 28.0);

	let mut geo = SceneGeometry::default();
	geo.add_rect(bg.centre(), Vec2::new(bg.w, bg.h), [0.08, 0.08, 0.1, 0.92]);
	geo.labels.push(crate::render::foundation::TextLabel {
		pos: bg.centre(),
		text: message.to_string(),
		colour: [0.95, 0.78, 0.35, 1.0],
		font_size: 15.0,
		width: width - 20.0,
	});
	geo
}

/// Rebuilds the menu screen's UI stack: the screen itself at the bottom,
/// the modal dialog on top of it.
pub(crate) fn build_menu_stack(
	menu: &MainMenu,
	text_input: &str,
	status: Option<&String>,
	vw: f32,
	vh: f32,
	mouse: Vec2,
) -> UiStack<menu_ui::UiAction> {
	let mut menu_stack = UiStack::new();

	let mut frame = menu_ui::build_screen(menu, vw, vh, mouse);
	if let Some(msg) = status {
		frame.geometry.labels.push(menu_ui::status_label(vw, vh, msg));
	}
	menu_stack.push(StackLayer::from_frame(LayerId::MenuScreen, frame, Capture::FullScreen));

	// Popup (rename/new-project/delete-confirm), if open, is its own layer stacked on
	// top: guarantees its background and text both composite over the screen underneath,
	// and its full-screen capture keeps clicks from reaching the screen beneath it.
	if menu.popup() != PopupKind::None {
		let popup_frame = menu_ui::build_popup_frame(menu, vw, vh, text_input, mouse);
		menu_stack.push(StackLayer::from_frame(LayerId::MenuPopup, popup_frame, Capture::FullScreen));
	}

	menu_stack
}

/// Advances one simulation step for the open chip (fed by every input
/// dev-pin's live driven state).
fn run_viewer_sim_step(v: &mut ViewerState) {
	let external_inputs: Vec<crate::sim::ExternalInput> = v
		.library
		.get(&v.root_chip_name)
		.input_pins
		.iter()
		.map(|pin| crate::sim::ExternalInput { address: crate::description::PinAddress::new(pin.id, 0), state: pin.driven_state })
		.collect();
	v.sim.run_simulation_step(&external_inputs);
}

/// Rebuilds the viewer's UI stack from live state, bottom-to-top: canvas
/// (chip scene + grid + pending wire/placement previews), the starred
/// bottom bar and any open collection flyout, every open overlay panel
/// (modal), the right-click context menu above even those, and finally
/// the non-capturing status toast.
pub(crate) fn build_viewer_stack(v: &mut ViewerState, status: Option<&str>, vw: f32, vh: f32, mouse: Vec2) -> UiStack<ViewerAction> {
	run_viewer_sim_step(v);

	let root_desc = v.library.get(&v.root_chip_name).clone();
	let hover_world_pos = v.camera.screen_to_world(mouse);
	let chip_scene = {
		let root_ref = v.library.get(&v.root_chip_name);
		let lookup = SimulatorPinState { sim: &v.sim, scope: v.sim.root() };
		build_scene(root_ref, &v.library, &lookup, Some(hover_world_pos))
	};

	if !v.camera_fitted {
		let bounds = bounding_box(&chip_scene).or_else(|| bounding_box(&build_scene(&root_desc, &v.library, &AllLow, None)));
		if let Some((min, max)) = bounds {
			v.camera.fit_to_bounds(min, max, 0.15);
		}
		v.camera_fitted = true;
	}

	let mut scene_geo = if v.show_grid { build_grid(&v.camera, theme::GRID_COL) } else { SceneGeometry::default() };
	scene_geo.triangles.extend(chip_scene.triangles);
	scene_geo.labels.extend(chip_scene.labels);
	if let Some(pending) = &v.pending_wire {
		draw_pending_wire_preview(&mut scene_geo, pending, hover_world_pos);
	}
	if let Some(chip_name) = &v.pending_place {
		if let Some(ghost) = build_pending_place_scene(&v.library, chip_name, hover_world_pos) {
			scene_geo.triangles.extend(ghost.triangles);
			scene_geo.labels.extend(ghost.labels);
		}
	}
	let mut viewer_stack: UiStack<ViewerAction> = UiStack::new();
	viewer_stack.push(StackLayer::<ViewerAction>::new(LayerId::Canvas, Capture::None).with_geometry(scene_geo));

	// Bottom bar of starred chips/collections is always drawn (mirrors `BottomBarUI`
	// always being visible), its buttons just disabled while an overlay panel is open --
	// see `EditorAction::ToggleStarredCollectionPopup`'s docs for what its "MENU" button
	// equivalent deliberately doesn't do here.
	let bar_enabled = v.overlays.is_empty();
	let bar_cycle_blocked: std::collections::HashSet<String> = v
		.prefs
		.starred_list
		.iter()
		.filter(|it| !it.is_collection && would_create_cycle(&v.library, &v.root_chip_name, &it.name))
		.map(|it| it.name.to_ascii_lowercase())
		.collect();
	let mut bar_frame = editor_ui::build_starred_bottom_bar(
		&v.prefs.starred_list,
		v.bottom_bar_open_collection.as_deref(),
		bar_enabled,
		&bar_cycle_blocked,
		v.bottom_bar_scroll_x,
		UiCtx::new(vw, vh, mouse),
	);
	// Measure how far the strip actually overflows (buttons are laid out at their
	// scrolled positions, so add the current offset back), clamp the stored offset
	// against it and rebuild if that moved anything -- e.g. right after un-starring
	// the last overflowing item.
	{
		let content_right = bar_frame.buttons.iter().map(|b| b.rect.x + b.rect.w).fold(0.0f32, f32::max);
		v.bottom_bar_scroll_max = (content_right + editor_ui::BOTTOM_BAR_BTN_PAD + v.bottom_bar_scroll_x - vw).max(0.0);
		if v.bottom_bar_scroll_x > v.bottom_bar_scroll_max {
			v.bottom_bar_scroll_x = v.bottom_bar_scroll_max;
			bar_frame = editor_ui::build_starred_bottom_bar(
				&v.prefs.starred_list,
				v.bottom_bar_open_collection.as_deref(),
				bar_enabled,
				&bar_cycle_blocked,
				v.bottom_bar_scroll_x,
				UiCtx::new(vw, vh, mouse),
			);
		}
	}
	let bar_rect = UiRect::new(0.0, vh - editor_ui::BOTTOM_BAR_HEIGHT, vw, editor_ui::BOTTOM_BAR_HEIGHT);
	let mut bar_layer = StackLayer::convert_frame(LayerId::BottomBar, bar_frame, Capture::Rect(bar_rect), editor_action).with_scroll_region(bar_rect);
	bar_layer.geometry = pin_geometry_to_screen(std::mem::take(&mut bar_layer.geometry), &v.camera, vh);
	viewer_stack.push(bar_layer);

	if bar_enabled {
		push_bottom_bar_flyout(v, &mut viewer_stack, vw, vh, mouse);
	}

	// Overlay panels, bottom-to-top in open order -- several can be stacked at once
	// (Ctrl+F pushes Search on top of an open Library). Each captures the full screen:
	// they're modal, so nothing underneath (bar included) gets clicked through them.
	for overlay in v.overlays.clone() {
		let overlay_frame = build_overlay_frame(v, overlay, vw, vh, mouse);
		let layer_id = overlay.layer_id();
		let mut overlay_layer = StackLayer::convert_frame(layer_id, overlay_frame, Capture::FullScreen, editor_action);
		overlay_layer.geometry = pin_geometry_to_screen(std::mem::take(&mut overlay_layer.geometry), &v.camera, vh);
		viewer_stack.push(overlay_layer);
	}

	// Right-click popup: above even the modal overlays.
	if let Some(state) = &v.context_menu {
		let menu_frame = context_menu::build_context_menu(state, vw, vh, mouse);
		// `ContextMenuFrame` isn't a `ui_kit::Frame` (it tracks hover for its own
		// highlight pass), so build its stack layer by hand: same capture/button shape.
		let mut ctx_layer = StackLayer::<ViewerAction>::new(LayerId::ContextMenu, Capture::Rect(menu_frame.panel_rect));
		ctx_layer.geometry = pin_geometry_to_screen(menu_frame.geometry, &v.camera, vh);
		ctx_layer.buttons =
			menu_frame.buttons.into_iter().map(|b| Button { rect: b.rect, action: ViewerAction::Context(b.id), enabled: true }).collect();
		viewer_stack.push(ctx_layer);
	}

	// Transient status/error toast floats above everything else; its layer never captures
	// input, so it never blocks what's underneath.
	if let Some(msg) = status {
		let geo = pin_geometry_to_screen(status_toast_geometry(msg, vw, vh, Some(vh - editor_ui::BOTTOM_BAR_HEIGHT - 34.0)), &v.camera, vh);
		viewer_stack.push(StackLayer::<ViewerAction>::new(LayerId::StatusToast, Capture::None).with_geometry(geo));
	}

	viewer_stack
}

/// The starred-collection flyout anchored to whichever bar button opened
/// it (falling back to the left edge if that button somehow isn't drawn).
fn push_bottom_bar_flyout(v: &mut ViewerState, viewer_stack: &mut UiStack<ViewerAction>, vw: f32, vh: f32, mouse: Vec2) {
	let Some(open_name) = v.bottom_bar_open_collection.clone() else { return };
	let Some(collection) = v.prefs.chip_collections.iter().find(|c| c.name.eq_ignore_ascii_case(&open_name)) else { return };
	let anchor_x = viewer_stack
		.layers()
		.iter()
		.flat_map(|l| l.buttons.iter())
		.find(
			|b| matches!(b.action, ViewerAction::Editor(editor_ui::EditorAction::ToggleStarredCollectionPopup(ref n)) if n.eq_ignore_ascii_case(&open_name)),
		)
		.map(|b| b.rect.x)
		.unwrap_or(8.0);
	let flyout_cycle_blocked: std::collections::HashSet<String> =
		collection.chips.iter().filter(|n| would_create_cycle(&v.library, &v.root_chip_name, n)).map(|n| n.to_ascii_lowercase()).collect();
	let flyout_frame = editor_ui::build_starred_collection_popup(collection, anchor_x, true, &flyout_cycle_blocked, vw, vh, mouse);
	// The flyout captures its whole panel rect (exposed by `frame.panel`),
	// so clicks on the padding between/around its rows belong to it rather
	// than falling through to the canvas or the bar underneath.
	let capture = flyout_frame.panel.map_or(Capture::FullScreen, Capture::Rect);
	let mut flyout_layer = StackLayer::convert_frame(LayerId::BottomBarFlyout, flyout_frame, capture, editor_action);
	flyout_layer.geometry = pin_geometry_to_screen(std::mem::take(&mut flyout_layer.geometry), &v.camera, vh);
	viewer_stack.push(flyout_layer);
}

/// Builds whichever overlay panel `overlay` names from live state.
fn build_overlay_frame(v: &ViewerState, overlay: Overlay, vw: f32, vh: f32, mouse: Vec2) -> editor_ui::EditorFrame {
	match overlay {
		Overlay::Library => {
			let selected_chip_name = match v.library_selection {
				LibrarySelection::Chip(ci, chi) => v.prefs.chip_collections.get(ci).and_then(|c| c.chips.get(chi)).cloned(),
				LibrarySelection::Starred(i) => v.prefs.starred_list.get(i).filter(|it| !it.is_collection).map(|it| it.name.clone()),
				_ => None,
			};
			let selected_chip_is_custom = selected_chip_name.as_deref().is_some_and(|n| is_custom_chip(&v.library, n));
			let selected_chip_would_cycle = selected_chip_name.as_deref().is_some_and(|n| would_create_cycle(&v.library, &v.root_chip_name, n));
			let state = editor_ui::ChipLibraryState {
				collections: &v.prefs.chip_collections,
				starred_list: &v.prefs.starred_list,
				selection: v.library_selection,
				selected_chip_is_custom,
				selected_chip_would_cycle,
				creating_collection: v.library_creating_collection,
				renaming_collection: v.library_renaming_collection,
				name_field_text: &v.overlay_text_input,
				confirming_chip_delete: v.library_confirming_chip_delete,
				confirming_collection_delete: v.library_confirming_collection_delete,
				delete_confirm_message: &v.library_delete_message,
			};
			editor_ui::build_chip_library_panel(&state, vw, vh, mouse)
		}
		Overlay::Search => {
			let mut names: Vec<String> = v.library.iter().map(|d| d.name.clone()).collect();
			names.sort();
			editor_ui::build_search_popup(&names, &v.search_query, vw, vh, mouse)
		}
		Overlay::Preferences => editor_ui::build_preferences_panel(&v.prefs, vw, vh, mouse),
		Overlay::Naming => {
			let confirm_enabled = !v.overlay_text_input.trim().is_empty();
			let title = match v.naming_purpose {
				NamingPurpose::RenameProject => "Rename project",
				NamingPurpose::LabelComponent(_) => "Label component",
				NamingPurpose::LabelDevPin { .. } => "Label pin",
				NamingPurpose::ConfigurePulseDuration(_) => "Pulse length (ticks)",
			};
			editor_ui::build_simple_naming_popup(title, &v.overlay_text_input, confirm_enabled, vw, vh, mouse)
		}
		Overlay::KeySelect => editor_ui::build_key_select_popup(v.overlay_key_choice, vw, vh, mouse),
		Overlay::RomEditor => {
			let (data, selected) =
				v.rom_editor.as_ref().map(|e| (e.data.clone(), e.selected)).unwrap_or_else(|| (vec![0; editor_ui::ROM_WORD_COUNT], 0));
			editor_ui::build_rom_editor_popup(&data, selected, &v.overlay_text_input, vw, vh, mouse)
		}
		Overlay::SaveChip => {
			let mode = save_chip_mode(v, &v.overlay_text_input);
			editor_ui::build_save_chip_popup(&v.root_chip_name, &v.overlay_text_input, mode, vw, vh, mouse)
		}
	}
}

/// The fixed "camera" the menu screen draws through: identity zoom,
/// centred on the viewport (the convention `ui_kit::to_world` builds for).
pub(crate) fn menu_camera(vw: f32, vh: f32) -> Camera {
	Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport: Vec2::new(vw, vh) }
}
