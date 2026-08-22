//! Rendering layer: the first slice of the ported `Graphics/` + `Game/Interaction` code from the
//! original C# project. Split into `layout` (grid/pin/chip-size math), `theme` (colour palette),
//! `camera` (pan/zoom transform), `scene` (chip descriptions -> coloured triangles), `editor_ui` and
//! `menu_ui` (in-editor and startup overlays), `context_menu` (generic right-click popup), and `gpu`
//! (the wgpu device/pipeline/draw call). Only `gpu` needs a real adapter, so only it skips `cargo test`.

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
