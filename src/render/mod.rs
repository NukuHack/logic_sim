//! Rendering layer: the first slice of the ported `Graphics/` + `Game/Interaction` code from
//! the original C# project.

pub mod camera;
pub mod context_menu;
pub mod customize_ui;
pub mod editor_ui;
pub mod foundation;
pub mod gpu;
pub mod layout;
pub mod menu_ui;
pub mod scene;
pub mod theme;
pub mod ui_kit;
pub mod ui_stack;

pub use camera::Camera;
pub use context_menu::{build_context_menu, ContextMenuButton, ContextMenuFrame, ContextMenuItem, ContextMenuState};
pub use editor_ui::{EditorAction, EditorButton, EditorFrame};
pub use menu_ui::{MenuFrame, UiAction, UiButton, UiRect};
pub use scene::{
	bounding_box, build_grid, build_scene, closest_wire_hit, delete_wire, hit_test_any_pin, hit_test_dev_pin, hit_test_input_dev_pin_bit,
	hit_test_sub_chip, hit_test_sub_chip_pin, hit_test_wire, place_sub_chips, AllLow, PinHit, PinStateLookup, PlacedSubChip, SceneGeometry,
	SceneVertex, TextLabel, WireTapHit,
};
pub use ui_stack::{Capture, Dispatch, InputResult, LayerId, StackLayer, UiStack};
