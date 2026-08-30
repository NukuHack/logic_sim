//! The integrated frontend: everything the windowed app needs beyond the pure renderer --
//! per-project editor state, canvas interaction (wires, placement, input-pin toggling),
//! overlay/popup flows, chip save/open flows, and the winit event loop driving it all.

pub mod actions;
pub mod app;
pub mod bus_wiring;
pub mod canvas;
pub mod chip_interaction;
pub mod context_menu;
pub mod customize;
pub mod events;
pub mod frame;
pub mod input;
pub mod library;
pub mod popups;
pub mod save_flow;
pub mod sim_thread;
pub mod sim_timing;
pub mod state;
pub mod undo;
pub mod wire_draft;
pub mod wire_edit;

pub use app::run;
