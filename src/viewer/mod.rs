//! The integrated frontend: everything the windowed app needs beyond the
//! pure renderer -- per-project editor state, canvas interaction (wires,
//! placement, input-pin toggling), overlay/popup flows, chip save/open
//! flows, and the winit event loop driving it all. Split out of the old
//! monolithic `bin/app` so the whole frontend lives in the library,
//! testable headless like the rest of the crate; `src/bin/app.rs` is
//! now just a thin entry point calling [`run`].
//!
//! Modules by concern: [`state`] (viewer/project working state + overlay
//! bookkeeping), [`canvas`] (chip-canvas clicks), [`wire_draft`]
//! (in-progress wire placement state), [`chip_interaction`] (component
//! selection/dragging, multi-component placement carries, box selection),
//! [`library`] (chip-library
//! bookkeeping), [`save_flow`] (save/save-as/rename/new-chip plus the
//! unsaved-changes gate over leaving a chip), [`popups`]
//! (generic popup confirm handlers), [`context_menu`] (right-click popups),
//! [`actions`] (editor action funnel), [`input`] (keyboard routing),
//! [`undo`] (the editor's undo/redo history),
//! [`sim_timing`] (pacing math shared by the background thread),
//! [`sim_thread`] (the background simulation thread itself),
//! [`frame`] (per-frame UI-stack construction), [`app`] (app shell +
//! entry point) and [`events`] (window-event handlers).

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
