//! Minimal windowed viewer for the rendering port.
//!
//! Usage: `cargo run --bin viewer -- <path-to-project-dir> [chip-name]`
//!
//! Loads the project (same loader used by the headless sim, `json::load_project`),
//! picks either the named chip or the last custom chip in the project as the
//! "root" being viewed, builds a `Simulator` for it, and renders its
//! subchips/pins/wires each frame with the wgpu renderer in
//! `logic_sim::render::gpu`.
//!
//! This file could not be run or tested in the sandbox that produced it (no
//! GPU adapter / display server available there) -- it's provided so you can
//! build+run it locally to check the renderer end to end. The parts it
//! depends on (`render::layout`, `render::theme`, `render::camera`,
//! `render::scene`) all have unit tests that *do* run headlessly; if
//! something's wrong here it's most likely in this glue file rather than in
//! the tested pieces.

use logic_sim::render::camera::Camera;
use logic_sim::render::gpu::Renderer;
use logic_sim::structs::Vec2;
use logic_sim::render::scene::{bounding_box, build_grid, build_scene, AllLow, SceneGeometry, SimulatorPinState};
use logic_sim::render::theme;
use logic_sim::sim::Simulator;
use logic_sim::{load_project, register_all_builtins, ChipLibrary};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use logic_sim::sim::key_mods_bits;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

/// See the identical helper in `bin/app.rs` for why this uses winit's
/// boolean modifier accessors rather than its raw `bits()` value.
fn encode_modifiers(mods: ModifiersState) -> u32 {
    let mut bits = 0u32;
    if mods.shift_key() {
        bits |= key_mods_bits::SHIFT;
    }
    if mods.control_key() {
        bits |= key_mods_bits::CONTROL;
    }
    if mods.alt_key() {
        bits |= key_mods_bits::ALT;
    }
    if mods.super_key() {
        bits |= key_mods_bits::SUPER;
    }
    bits
}

#[allow(dead_code)] // kept for reference / future hot-reload support
struct App {
    project_dir: std::path::PathBuf,
    chip_name: Option<String>,
    library: ChipLibrary,
    root_chip_name: String,
    sim: Simulator,
    camera: Camera,
    state: Option<RenderState>,
    dragging: bool,
    last_cursor: Vec2,
    camera_fitted: bool,
    show_grid: bool,
}

struct RenderState {
    window: Arc<Window>,
    renderer: Renderer,
}

impl App {
    fn new(project_dir: std::path::PathBuf, chip_name: Option<String>) -> Self {
        let (project, mut library, errors) =
            load_project(&project_dir).expect("failed to read project directory");
        for e in &errors {
            eprintln!("warning: {e}");
        }
        register_all_builtins(&mut library);

        let root_chip_name = chip_name.clone().unwrap_or_else(|| {
            // Prefer the project's own custom chips (the "ROOT"/last-saved
            // one is usually what the user was last working on) over a
            // builtin fallback -- a builtin like NAND is a leaf with no
            // subchips of its own, so `build_scene` would have nothing to
            // draw and the window would just show an empty background.
            project
                .all_custom_chip_names
                .last()
                .cloned()
                .or_else(|| {
                    // No project-declared chips (or the field wasn't
                    // populated) -- fall back to any *custom* chip actually
                    // present in the library, preferring one with subchips
                    // so there's something visible.
                    library
                        .iter()
                        .filter(|d| d.chip_type == logic_sim::ChipType::Custom)
                        .max_by_key(|d| d.sub_chips.len())
                        .map(|d| d.name.clone())
                })
                .unwrap_or_else(|| {
                    eprintln!(
                        "warning: no custom chip found to display (and none named on the command line); \
                         falling back to a builtin, which will render as an empty canvas"
                    );
                    "NAND".to_string()
                })
        });

        eprintln!("viewing chip: {root_chip_name}");

        let root_desc = library.get(&root_chip_name).clone();
        let sim = Simulator::build(&root_desc, &library);

        // Mirrors `Project.ShowGrid` (`Prefs_GridDisplayMode == 1`), the
        // project's saved "show grid" preference. Toggle with `G` at runtime.
        let show_grid = project.prefs_grid_display_mode == 1;

        App {
            project_dir,
            chip_name,
            library,
            root_chip_name,
            sim,
            camera: Camera::new(1280.0, 800.0),
            state: None,
            dragging: false,
            last_cursor: Vec2::ZERO,
            camera_fitted: false,
            show_grid,
        }
    }

    fn rebuild_sim(&mut self) {
        let root_desc = self.library.get(&self.root_chip_name).clone();
        let held_keys = std::mem::take(&mut self.sim.held_keys);
        let key_modifiers = self.sim.key_modifiers;
        self.sim = Simulator::build(&root_desc, &self.library);
        self.sim.held_keys = held_keys;
        self.sim.key_modifiers = key_modifiers;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window_attrs = Window::default_attributes()
            .with_title(format!("Digital Logic Sim -- {}", self.root_chip_name))
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(window_attrs).expect("failed to create window"));

        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).expect("failed to create surface");
        let renderer = pollster::block_on(Renderer::new(&instance, surface, size.width, size.height));

        self.camera.resize_viewport(size.width as f32, size.height as f32);
        self.state = Some(RenderState { window, renderer });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
                self.camera.resize_viewport(size.width as f32, size.height as f32);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Key chip: feed held_keys on press *and* release. Stored/compared
                // in capitals, so lowercase 'a' must also register as 'A'.
                if let Key::Character(s) = &event.logical_key {
                    if let Some(c) = s.chars().next() {
                        let c = c.to_ascii_uppercase();
                        match event.state {
                            ElementState::Pressed => {
                                self.sim.held_keys.insert(c);
                            }
                            ElementState::Released => {
                                self.sim.held_keys.remove(&c);
                            }
                        }
                    }
                }

                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::KeyR) => self.rebuild_sim(),
                        PhysicalKey::Code(KeyCode::KeyF) => self.camera_fitted = false, // re-fit next frame
                        PhysicalKey::Code(KeyCode::KeyG) => self.show_grid = !self.show_grid,
                        _ => {}
                    }
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.sim.key_modifiers = encode_modifiers(mods.state());
            }

            // See the identical handling in `bin/app.rs` for why.
            WindowEvent::Focused(false) => {
                self.sim.held_keys.clear();
                self.sim.key_modifiers = 0;
            }

            WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Left, .. } => {
                self.dragging = btn_state == ElementState::Pressed;
            }

            WindowEvent::CursorMoved { position, .. } => {
                let cursor = Vec2::new(position.x as f32, position.y as f32);
                if self.dragging {
                    let before = self.camera.screen_to_world(self.last_cursor);
                    let after = self.camera.screen_to_world(cursor);
                    self.camera.pan(Vec2::new(before.x - after.x, before.y - after.y));
                }
                self.last_cursor = cursor;
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 100.0) as f32,
                };
                let zoom_factor = 1.0 + scroll * 0.1;
                self.camera.zoom_at(self.last_cursor, zoom_factor);
            }

            WindowEvent::RedrawRequested => {
                // Step the simulation and re-run signal propagation before
                // drawing, so wire/pin colours reflect live state.
                self.sim.run_simulation_step(&[]);

                let root_desc = self.library.get(&self.root_chip_name);
                let lookup = SimulatorPinState { sim: &self.sim, scope: self.sim.root() };
                let chip_scene = build_scene(root_desc, &self.library, &lookup);

                if !self.camera_fitted {
                    // Default zoom=1.0 shows ~viewport-pixel-count world
                    // units across, but chips are sized in grid units of
                    // ~0.125 -- so without this the whole scene would be an
                    // invisible speck in the middle of the window.
                    let bounds = bounding_box(&chip_scene)
                        .or_else(|| bounding_box(&build_scene(root_desc, &self.library, &AllLow)));
                    if let Some((min, max)) = bounds {
                        self.camera.fit_to_bounds(min, max, 0.15);
                    }
                    self.camera_fitted = true;
                }

                // Grid is background, so its triangles must be drawn first
                // (this renderer has no depth buffer -- draw order is
                // z-order). Rebuilt every frame since it depends on the
                // camera's current pan/zoom, same as the original's
                // per-frame `DrawGridIfActive`.
                let mut scene = if self.show_grid { build_grid(&self.camera, theme::GRID_COL) } else { SceneGeometry::default() };
                scene.triangles.extend(chip_scene.triangles);
                scene.labels.extend(chip_scene.labels);

                match state.renderer.render(&scene, &self.camera, theme::BACKGROUND_COL) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let size = state.window.inner_size();
                        state.renderer.resize(size.width, size.height);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(e) => eprintln!("render error: {e:?}"),
                }

                state.window.request_redraw();
            }

            _ => {}
        }
    }
}

fn main() {
    env_logger::init();

    let mut args = std::env::args().skip(1);
    let project_dir = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("usage: viewer <path-to-project-dir> [chip-name]");
            std::process::exit(1);
        });
    let chip_name = args.next();

    let mut app = App::new(project_dir, chip_name);
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.run_app(&mut app).expect("event loop error");
}