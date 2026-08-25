//! Frame construction: rebuilds each screen's whole UI stack from live
//! state, bottom-to-top -- the menu screen and its popup, or the viewer's
//! canvas/bottom-bar/flyout/overlay-panel/context-menu layers. Input
//! dispatch in the event handlers consults exactly what's built here.
//!
//! This is also where the frame syncs with the background simulation
//! thread (see [`crate::viewer::sim_thread`]): prefs-derived pacing
//! inputs (pause, target rate, clock speed) are pushed out, the input
//! dev-pins' driven states are published for the worker to apply, and the
//! thread's measured speed / single-step counter are read back for the
//! preferences panel and paused banner.

use crate::render::camera::Camera;
use crate::render::context_menu;
use crate::render::editor_ui::{self, LibrarySelection, PrefsPanelState};
use crate::render::layout::{force_straight_line, snap_to_grid_centred};
use crate::render::menu_ui::{self};
use crate::render::scene::{bounding_box, build_grid, build_scene, build_scene_with_spans, fade_component, AllLow, SceneGeometry, SimulatorPinState};
use crate::render::theme;
use crate::render::ui_kit::{pin_geometry_to_screen, Button, UiCtx, UiRect};
use crate::render::ui_stack::{Capture, LayerId, StackLayer, UiStack};
use crate::structs::Vec2;
use crate::ui_menu::{MainMenu, PopupKind};
use crate::viewer::chip_interaction::{self, CanvasInteraction};

use crate::viewer::canvas::{build_pending_place_scene, draw_pending_wire_preview, PENDING_PLACEMENT_ALPHA};
use crate::viewer::library::{is_custom_chip, is_listed_in_current_build, would_create_cycle};
use crate::viewer::save_flow::save_chip_mode;
use crate::viewer::state::{editor_action, NamingPurpose, Overlay, SceneTarget, ViewerAction, ViewerState};

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

/// The top-of-screen "Simulation Paused" strip (`SimPausedUI`): a
/// full-width info bar with the message centred and the single-step count
/// at the right.
fn paused_banner_geometry(paused_step_counter: u32, vw: f32) -> SceneGeometry {
	const BANNER_H: f32 = 34.0;
	let bg = UiRect::new(0.0, 0.0, vw, BANNER_H);
	let mut geo = SceneGeometry::default();
	geo.add_rect(bg.centre(), Vec2::new(bg.w, bg.h), [0.1, 0.1, 0.12, 0.95]);
	geo.labels.push(crate::render::foundation::TextLabel {
		pos: bg.centre(),
		text: "Simulation Paused (press space to advance one step)".to_string(),
		colour: [1.0, 0.85, 0.25, 1.0],
		font_size: 15.0,
		width: vw - 120.0,
	});
	geo.labels.push(crate::render::foundation::TextLabel {
		pos: Vec2::new(vw - 60.0, BANNER_H / 2.0),
		text: paused_step_counter.to_string(),
		colour: [1.0, 1.0, 1.0, 0.8],
		font_size: 15.0,
		width: 50.0,
	});
	geo
}

/// The viewed-chips bar (`ViewedChipsBar.DrawViewedChipsBanner`): a
/// full-width info strip pinned to the very top of the screen, showing the
/// "Viewing: a > b" chain, with a Back button at its right edge for
/// popping back to the parent chip. Returns the bar's geometry plus the
/// Back button's hit rect (both plain screen-pixel space, like every
/// other UI surface pre-pinning).
fn viewed_chips_bar_geometry(v: &ViewerState, vw: f32, top_y: f32) -> (SceneGeometry, UiRect) {
	const BAR_H: f32 = 34.0;
	const PAD: f32 = 12.0;
	const BACK_W: f32 = 70.0;
	const FONT_SIZE: f32 = 15.0;

	let bg = UiRect::new(0.0, top_y, vw, BAR_H);
	let back = UiRect::new(vw - PAD - BACK_W, top_y + (BAR_H - 26.0) / 2.0, BACK_W, 26.0);
	// The label lives in the strip between the left pad and the Back
	// button -- and since `TextLabel::pos` is the *centre* of the text
	// box, anchoring it anywhere else would let long chains run off the
	// screen edge.
	let text_left = PAD;
	let text_right = vw - BACK_W - PAD * 2.0;
	let max_text_width = (text_right - text_left).max(40.0);
	let mut geo = SceneGeometry::default();
	geo.add_rect(bg.centre(), Vec2::new(bg.w, bg.h), [0.1, 0.1, 0.12, 0.95]);
	geo.add_rect(back.centre(), Vec2::new(back.w, back.h), [0.28, 0.28, 0.34, 1.0]);
	geo.labels.push(crate::render::foundation::TextLabel {
		pos: Vec2::new((text_left + text_right) / 2.0, top_y + BAR_H / 2.0),
		text: fit_banner_label(&v.viewed_chips_string(), max_text_width),
		colour: [0.95, 0.95, 0.95, 1.0],
		font_size: FONT_SIZE,
		width: max_text_width,
	});
	geo.labels.push(crate::render::foundation::TextLabel {
		pos: back.centre(),
		text: "Back".to_string(),
		colour: [1.0, 1.0, 1.0, 1.0],
		font_size: 14.0,
		width: BACK_W - 8.0,
	});
	(geo, back)
}

const BANNER_PREFIX: &str = "Viewing: ";
const BANNER_FONT_SIZE: f32 = 15.0;

/// Trims a "Viewing: a > b > c" chain to `max_width`, dropping the
/// oldest ancestors (the root-ward end of the list) first and marking the
/// cut with an ellipsis, so even absurdly deep view stacks stay inside
/// the bar instead of spilling off-screen.
fn fit_banner_label(chain: &str, max_width: f32) -> String {
	if crate::render::layout::estimate_text_width(chain, BANNER_FONT_SIZE) <= max_width {
		return chain.to_string();
	}
	let names: Vec<&str> = chain.strip_prefix(BANNER_PREFIX).unwrap_or(chain).split(" > ").collect();
	for keep in (0..names.len()).rev() {
		let candidate = format!("{}{}…", BANNER_PREFIX, names[..keep].join(" > "));
		if keep == 0 || crate::render::layout::estimate_text_width(&candidate, BANNER_FONT_SIZE) <= max_width {
			return candidate;
		}
	}
	format!("{}…", BANNER_PREFIX)
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

/// Syncs the open chip's simulation thread with this frame's live state
/// (the main-thread half of the original's `Project.Update` /
/// `SimThread` split -- stepping itself happens on the worker):
///
/// - prefs-derived pacing inputs are pushed out (pause flag, target rate,
///   clock speed), so the panel/shortcut paths only ever touch `prefs`;
/// - every input dev-pin's current driven state is published, mirroring
///   the original capturing `inputPins` once per Update for the thread;
/// - the thread's measured ticks/second and paused single-step counter
///   are read back for the preferences readout and paused banner.
fn update_viewer_sim(v: &mut ViewerState) {
	v.sync_sim_clock_pref();
	v.sim.set_paused(v.prefs.prefs_sim_paused);
	v.sim.set_target_ticks_per_second(v.target_ticks_per_second() as u32);

	// Every input dev-pin's current driven state
	// (`Simulator.RunSimulationStep(simChip, project.inputPins, ..)`).
	let external_inputs: Vec<crate::sim::ExternalInput> = v
		.library
		.get(&v.root_chip_name)
		.input_pins
		.iter()
		.map(|pin| crate::sim::ExternalInput { address: crate::description::PinAddress::new(pin.id, 0), state: pin.driven_state })
		.collect();
	v.sim.publish_external_inputs(external_inputs);

	v.paused_step_counter = v.sim.paused_step_counter();
	v.sim_pacing.avg_ticks_per_sec = v.sim.avg_ticks_per_sec();
}

/// Rebuilds the viewer's UI stack from live state, bottom-to-top: canvas
/// (chip scene + grid + pending wire/placement previews), the starred
/// bottom bar and any open collection flyout, every open overlay panel
/// (modal), the right-click context menu above even those, the paused
/// banner, and finally the non-capturing status toast.
pub(crate) fn build_viewer_stack(v: &mut ViewerState, status: Option<&str>, vw: f32, vh: f32, mouse: Vec2) -> UiStack<ViewerAction> {
	update_viewer_sim(v);
	// The customizer's in-flight resize/move/scale applies against the live
	// cursor before the frame is built, so this frame already shows its
	// effect (same immediate-mode beat as the camera pan above).
	crate::viewer::customize::update_live_interaction(v);

	let hover_world_pos = v.camera.screen_to_world(mouse);
	// The scene shows the top of the view stack when one is open (a chip
	// being watched in view-only mode), else the edited root -- with pin
	// states resolved against that chip's own live sim scope, which is
	// exactly how a viewed subchip's subtree stays "live".
	let scene_chip_name = match &v.resolve_scene_target() {
		SceneTarget::EditRoot => v.root_chip_name.clone(),
		SceneTarget::Viewed { name, .. } => name.clone(),
	};
	let root_desc = v.library.get(&scene_chip_name).clone();
	// The scene reads pin states straight out of the shared arena under a
	// short-lived lock -- the read half of the original's per-frame
	// `ViewedChip.UpdateStateFromSim` sync.
	let (mut chip_scene, component_spans) = {
		let scene_target = v.resolve_scene_target();
		let sim_guard = v.sim.lock();
		let scope = match scene_target {
			SceneTarget::EditRoot => sim_guard.root(),
			SceneTarget::Viewed { scope, .. } => scope,
		};
		let root_ref = v.library.get(&scene_chip_name);
		let lookup = SimulatorPinState { sim: &sim_guard, scope };
		build_scene_with_spans(root_ref, &v.library, &lookup, Some(hover_world_pos))
	};

	// A selection being dragged renders translucently -- deliberately the
	// same read as a placement ghost -- by fading exactly the carried
	// components' own geometry (bodies, labels, embedded displays). Their
	// wires stretch along at full strength. Fading happens here, before the
	// scene merges with the grid layer, because the spans index into the
	// chip scene's buffers alone.
	if let CanvasInteraction::MovingSelection { originals, .. } = &v.canvas_interaction {
		for &(id, _) in originals {
			if let Some(span) = component_spans.get(id) {
				fade_component(&mut chip_scene, span, PENDING_PLACEMENT_ALPHA);
			}
		}
	}

	if !v.camera_fitted {
		let bounds = bounding_box(&chip_scene).or_else(|| bounding_box(&build_scene(&root_desc, &v.library, &AllLow, None)));
		if let Some((min, max)) = bounds {
			v.camera.fit_to_bounds(min, max, 0.15);
		}
		v.camera_fitted = true;
	}

	let mut scene_geo = if v.show_grid() { build_grid(&v.camera, theme::GRID_COL) } else { SceneGeometry::default() };
	scene_geo.triangles.extend(chip_scene.triangles);
	scene_geo.labels.extend(chip_scene.labels);
	// Previews apply exactly the same snap/straighten transform a click
	// would (see `canvas::handle_canvas_click`), so what follows the cursor
	// is precisely what landing there would place.
	if let Some(pending) = &v.pending_wire {
		let prev_point = pending.bend_points.last().copied().unwrap_or_else(|| pending.start.position());
		let mut end = hover_world_pos;
		if v.should_snap_to_grid() {
			end = snap_to_grid_centred(end);
		}
		if v.force_straight_wires() {
			end = force_straight_line(prev_point, end);
		}
		draw_pending_wire_preview(&mut scene_geo, pending, end);
	}
	if !v.pending_place.is_empty() {
		let ghost = build_pending_place_scene(&v.library, &v.pending_place, hover_world_pos, v.should_snap_to_grid());
		scene_geo.triangles.extend(ghost.triangles);
		scene_geo.labels.extend(ghost.labels);
	}
	// Selection highlights and any rubber band draw last of all, on top of
	// everything else in the canvas layer.
	chip_interaction::append_selection_geometry(&mut scene_geo, v, hover_world_pos);
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

	// Viewed-chips bar (`ViewedChipsBar`): pinned to the very top of the
	// screen while a chip is being viewed in view-only mode (stacking just
	// below the paused banner when both apply, like the original's
	// `topLeft += Vector2.down * InfoBarHeight`). Only its strip captures
	// input, and only the Back button on it acts.
	if !v.view_stack.is_empty() && v.overlays.is_empty() {
		let paused_banner_active = v.prefs.prefs_sim_paused && v.context_menu.is_none();
		let top_y = if paused_banner_active { 34.0 } else { 0.0 };
		let (geo, back_rect) = viewed_chips_bar_geometry(v, vw, top_y);
		let mut bar_layer = StackLayer::<ViewerAction>::new(LayerId::ViewedChipsBar, Capture::Rect(UiRect::new(0.0, top_y, vw, 34.0)));
		bar_layer.geometry = pin_geometry_to_screen(geo, &v.camera, vh);
		bar_layer.buttons.push(Button { rect: back_rect, action: ViewerAction::Editor(editor_ui::EditorAction::ExitViewedChip), enabled: true });
		viewer_stack.push(bar_layer);
	}

	// Overlay panels, bottom-to-top in open order -- several can be stacked at once
	// (Ctrl+F pushes Search on top of an open Library). Each captures the full screen:
	// they're modal, so nothing underneath (bar included) gets clicked through them.
	for overlay in v.overlays.clone() {
		let mut customize_out = (overlay == Overlay::CustomizeChip).then(|| crate::viewer::customize::build_layer(v, vw, vh, mouse));
		if let Some(out) = customize_out.as_mut() {
			let frame = std::mem::take(&mut out.frame);
			let (layout, scroll_max) = (out.layout, out.list_scroll_max);
			crate::viewer::customize::cache_layout(v, layout, scroll_max);
			let layer = StackLayer::convert_frame(overlay.layer_id(), frame, Capture::FullScreen, editor_action)
				.with_scroll_region(layout.preview)
				.with_scroll_region(layout.list);
			let mut layer_owned = layer;
			layer_owned.geometry = pin_geometry_to_screen(std::mem::take(&mut layer_owned.geometry), &v.camera, vh);
			viewer_stack.push(layer_owned);
			continue;
		}
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

	// "Simulation Paused" banner (`SimPausedUI`): shown whenever the sim is
	// paused and no modal panel covers the editor (the original draws it
	// only for `MenuType.None`). Never captures input.
	if v.prefs.prefs_sim_paused && v.overlays.is_empty() && v.context_menu.is_none() {
		let geo = pin_geometry_to_screen(paused_banner_geometry(v.paused_step_counter, vw), &v.camera, vh);
		viewer_stack.push(StackLayer::<ViewerAction>::new(LayerId::StatusToast, Capture::None).with_geometry(geo));
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
			// Release builds keep the dev-only builtins (dev.RAM-8, the bus
			// termini) out of the results -- see `is_listed_in_current_build`.
			let mut names: Vec<String> = v.library.iter().map(|d| d.name.clone()).filter(|n| is_listed_in_current_build(&v.library, n)).collect();
			names.sort();
			editor_ui::build_search_popup(&names, &v.search_query, vw, vh, mouse)
		}
		Overlay::Preferences => {
			let state = PrefsPanelState {
				desc: &v.prefs,
				clock_text: &v.prefs_clock_text,
				rate_text: &v.prefs_rate_text,
				focused_field: v.prefs_field_focus,
				measured_speed_label: if v.prefs.prefs_sim_paused { "0".to_string() } else { format!("{:.0}", v.sim_pacing.avg_ticks_per_sec) },
			};
			editor_ui::build_preferences_panel(&state, vw, vh, mouse)
		}
		Overlay::Naming => {
			let confirm_enabled = !v.overlay_text_input.trim().is_empty();
			let title = match v.naming_purpose {
				NamingPurpose::RenameProject => "Rename project",
				NamingPurpose::LabelComponent(_) => "Label component",
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
		Overlay::PinEdit => {
			let Some(edit) = v.pin_edit.as_ref() else { return editor_ui::EditorFrame::default() };
			let chip = v.library.get(&v.root_chip_name);
			let pins = if edit.is_input { &chip.input_pins } else { &chip.output_pins };
			let pin = pins.iter().find(|p| p.id == edit.pin_id);
			editor_ui::build_pin_edit_popup(
				pin.map(|p| p.name.as_str()).unwrap_or_default(),
				pin.is_some_and(|p| p.bit_count != crate::description::PinBitCount::Bit1),
				edit.display_mode_index,
				vw,
				vh,
				mouse,
			)
		}
		Overlay::UnsavedChanges => editor_ui::build_unsaved_changes_popup(vw, vh, mouse),
		// Handled separately by `build_viewer_stack` (its frame carries the
		// preview layout back onto `ViewerState::customize`), so reaching
		// this arm at all would double-build it.
		Overlay::CustomizeChip => editor_ui::EditorFrame::default(),
	}
}

/// The fixed "camera" the menu screen draws through: identity zoom,
/// centred on the viewport (the convention `ui_kit::to_world` builds for).
pub(crate) fn menu_camera(vw: f32, vh: f32) -> Camera {
	Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport: Vec2::new(vw, vh) }
}

#[cfg(test)]
mod viewed_chips_bar_tests {
	//! White-box: the banner is hand-built geometry (no `editor_ui`
	//! builder behind it), so its screen-fitting contract -- pinned to the
	//! top strip, text centred inside the space left of the Back button,
	//! long chains truncated rather than overflowing -- is pinned here
	//! directly against the pure builder.

	use super::*;
	use crate::viewer::state::ViewedChip;

	fn viewer_with_chain(names: &[&str]) -> ViewerState {
		let mut library = crate::ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(crate::ChipDescription::new("ROOT", crate::ChipType::Custom));
		let mut v = ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		// Id paths are irrelevant to the bar's geometry; fabricate the stack.
		v.view_stack = names.iter().map(|name| ViewedChip { name: name.to_string(), path: vec![] }).collect();
		v
	}

	fn label_bounds(geo: &SceneGeometry) -> (f32, f32) {
		let label = geo.labels.first().expect("the chain label exists");
		(label.pos.x - label.width / 2.0, label.pos.x + label.width / 2.0)
	}

	#[test]
	fn bar_spans_the_top_strip_with_the_text_left_of_back() {
		let v = viewer_with_chain(&["INNER"]);
		let vw = 800.0;
		let (geo, back) = viewed_chips_bar_geometry(&v, vw, 0.0);

		for v in &geo.triangles {
			assert!((0.0..=34.0).contains(&v.pos.y), "every vertex sits in the top strip, got {}", v.pos.y);
			assert!((0.0..=vw).contains(&v.pos.x));
		}
		let (left, right) = label_bounds(&geo);
		assert!(left >= 12.0 - f32::EPSILON, "text box starts at/after the left pad");
		assert!(right <= back.x + f32::EPSILON, "text box ends before the Back button");
	}

	#[test]
	fn long_chains_truncate_instead_of_overflowing() {
		let chain = format!("Viewing: {}", (0..40).map(|i| format!("CHIP{i}")).collect::<Vec<_>>().join(" > "));
		let max_width = 300.0;
		let fitted = fit_banner_label(&chain, max_width);

		assert!(crate::render::layout::estimate_text_width(&fitted, BANNER_FONT_SIZE) <= max_width, "fitted label measures within the budget");
		assert!(fitted.starts_with("Viewing: CHIP0"), "nearest ancestors survive the cut: {fitted}");
		assert!(fitted.ends_with('…'), "the cut is marked");

		// A short chain passes through untouched.
		assert_eq!(fit_banner_label("Viewing: ROOT", 300.0), "Viewing: ROOT");
	}

	#[test]
	fn even_a_narrow_window_keeps_the_label_on_screen() {
		let v = viewer_with_chain(&["A", "B", "C"]);
		let (geo, _) = viewed_chips_bar_geometry(&v, 320.0, 34.0);
		let (left, right) = label_bounds(&geo);
		assert!(left >= 0.0 && right <= 320.0, "label box stays inside a narrow viewport");
	}
}
