//! Rendering layer: the first slice of the ported `Graphics/` +
//! `Game/Interaction` code from the original C# project.
//!
//! Split into:
//!   - `layout`  -- grid/pin/chip-size math (`DrawSettings`, `SubChipHelper`, `GridHelper`)
//!   - `theme`   -- colour palette (`DrawSettings.CreateTheme`)
//!   - `camera`  -- pan/zoom world<->screen transform (`CameraController`, transform-only slice)
//!   - `scene`   -- ChipDescription + ChipLibrary -> coloured triangles (`DevSceneDrawer`, first pass)
//!   - `editor_ui` -- in-editor overlays: prefs, chip library, search, simple naming, key select
//!   - `context_menu` -- generic right-click popup, attachable to anything by a string id/target
//!   - `gpu`     -- wgpu device/pipeline/draw call, consumes the above, one submitted pass per layer
//!
//! `layout`, `theme`, `camera`, and `scene` have no GPU dependency and are
//! covered by unit tests that run in `cargo test` without a display or GPU
//! adapter. `gpu` needs a real wgpu adapter + window surface, so it isn't
//! (and can't be) exercised by tests here -- see `src/bin/viewer.rs` for
//! how it's meant to be driven, and the module doc-comment on `gpu` for
//! details.

pub mod camera;
pub mod context_menu;
pub mod editor_ui;
pub mod gpu;
pub mod layout;
pub mod menu_ui;
pub mod scene;
pub mod theme;

pub use camera::Camera;
pub use context_menu::{build_context_menu, ContextMenuButton, ContextMenuFrame, ContextMenuItem, ContextMenuState};
pub use editor_ui::{EditorAction, EditorButton, EditorFrame};
pub use menu_ui::{MenuFrame, UiAction, UiButton, UiRect};
pub use scene::{
	bounding_box, build_grid, build_scene, delete_wire, hit_test_dev_pin, hit_test_sub_chip, hit_test_wire, place_sub_chips, AllLow, PinStateLookup,
	PlacedSubChip, SceneGeometry, SceneVertex, TextLabel,
};
