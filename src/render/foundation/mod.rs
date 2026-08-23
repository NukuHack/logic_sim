//! Shared low-level rendering foundation: the geometry primitives and
//! point-in-shape hit tests every higher render module builds on. Split
//! out of the old monolithic `scene` module so chip-scene drawing, UI
//! builders, and interaction code all compose the same single
//! implementation instead of re-deriving their own.

pub mod geometry;
pub mod hit_test;
pub mod polyline;

pub use geometry::{apply_alpha, bounding_box, RoundCorners, SceneGeometry, SceneVertex, TextLabel};
pub use hit_test::{point_in_circle, point_in_rect, point_in_rounded_rect};
pub use polyline::offset_polyline;
