//! Rendering layer: the first slice of the ported `Graphics/` +
//! `Game/Interaction` code from the original C# project.
//!
//! Split into:
//!   - `layout`  -- grid/pin/chip-size math (`DrawSettings`, `SubChipHelper`, `GridHelper`)
//!   - `theme`   -- colour palette (`DrawSettings.CreateTheme`)
//!   - `camera`  -- pan/zoom world<->screen transform (`CameraController`, transform-only slice)
//!   - `scene`   -- ChipDescription + ChipLibrary -> coloured triangles (`DevSceneDrawer`, first pass)
//!   - `gpu`     -- wgpu device/pipeline/draw call, consumes the above
//!
//! `layout`, `theme`, `camera`, and `scene` have no GPU dependency and are
//! covered by unit tests that run in `cargo test` without a display or GPU
//! adapter. `gpu` needs a real wgpu adapter + window surface, so it isn't
//! (and can't be) exercised by tests here -- see `src/bin/viewer.rs` for
//! how it's meant to be driven, and the module doc-comment on `gpu` for
//! details.

pub mod camera;
pub mod gpu;
pub mod layout;
pub mod menu_ui;
pub mod scene;
pub mod theme;

pub use camera::Camera;
pub use menu_ui::{MenuFrame, UiAction, UiButton, UiRect};
pub use scene::{bounding_box, build_grid, build_scene, AllLow, PinStateLookup, SceneGeometry, SceneVertex, TextLabel};