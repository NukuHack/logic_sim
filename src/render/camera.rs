//! 2D pan/zoom camera for the chip canvas.
//!
//! This is the "base" piece of `Game/Interaction/CameraController.cs` that
//! the renderer needs (world <-> screen mapping) without any of the Unity
//! input plumbing. Kept GPU-free and unit-testable; `render::gpu` consumes
//! `Camera::view_proj_matrix` as a uniform.

use crate::structs::Vec2;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// World-space point the camera is centred on.
    pub position: Vec2,
    /// World units visible per screen pixel is `1 / zoom`; larger zoom =
    /// more zoomed in.
    pub zoom: f32,
    /// x = width
    /// y = heigth
    pub viewport: Vec2,
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.05;
    // Chips are sized in grid units of ~0.125, so getting a single small
    // chip to fill a comfortable fraction of a ~1000px window legitimately
    // needs zoom in the hundreds-to-low-thousands. The old cap of 40 meant
    // `fit_to_bounds` was silently clamped and could never actually zoom in
    // enough to make small chips readable.
    pub const MAX_ZOOM: f32 = 4096.0;

    pub fn new(viewport: Vec2) -> Self {
        Self { position: Vec2::ZERO, zoom: 1.0, viewport }
    }

    pub fn pan(&mut self, delta_world: Vec2) {
        self.position = self.position.add(delta_world);
    }

    /// Zoom in/out around a fixed screen-space anchor (e.g. the mouse
    /// cursor), keeping the world point under that anchor stationary --
    /// same UX as scroll-to-zoom in the original editor.
    pub fn zoom_at(&mut self, screen_anchor: Vec2, zoom_factor: f32) {
        let world_before = self.screen_to_world(screen_anchor);
        self.zoom = (self.zoom * zoom_factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let world_after = self.screen_to_world(screen_anchor);
        let correction = Vec2::new(world_before.x - world_after.x, world_before.y - world_after.y);
        self.position = self.position.add(correction);
    }

    pub fn resize_viewport(&mut self, width: f32, height: f32) {
        self.viewport = Vec2::new(width, height);
    }

    /// Fit the camera so the world-space box `[min, max]` is fully visible,
    /// with `padding_fraction` extra room on each side (e.g. 0.1 = 10%
    /// margin). No-ops (keeps current zoom/position) if the box is
    /// degenerate. Call this after building the first scene, and again
    /// whenever the viewed chip changes, so content isn't lost off in the
    /// weeds at the default zoom=1.0 (which shows ~viewport-pixels world
    /// units across -- far too zoomed out for chips sized in grid units of
    /// ~0.125).
    pub fn fit_to_bounds(&mut self, min: Vec2, max: Vec2, padding_fraction: f32) {
        let width = (max.x - min.x).max(1e-4);
        let height = (max.y - min.y).max(1e-4);
        self.position = Vec2::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);

        let pad = 1.0 + padding_fraction.max(0.0) * 2.0;
        let zoom_x = self.viewport.x / (width * pad);
        let zoom_y = self.viewport.y / (height * pad);
        self.zoom = zoom_x.min(zoom_y).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }

    /// Convert a screen-space pixel coordinate (origin top-left, +y down)
    /// into world space.
    pub fn screen_to_world(&self, screen: Vec2) -> Vec2 {
        let ndc_x = (screen.x / self.viewport.x) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen.y / self.viewport.y) * 2.0;
        let half_w = self.viewport.x / (2.0 * self.zoom);
        let half_h = self.viewport.y / (2.0 * self.zoom);
        Vec2::new(self.position.x + ndc_x * half_w, self.position.y + ndc_y * half_h)
    }

    pub fn world_to_screen(&self, world: Vec2) -> Vec2 {
        let half_w = self.viewport.x / (2.0 * self.zoom);
        let half_h = self.viewport.y / (2.0 * self.zoom);
        let ndc_x = (world.x - self.position.x) / half_w;
        let ndc_y = (world.y - self.position.y) / half_h;
        let screen_x = (ndc_x + 1.0) * 0.5 * self.viewport.x;
        let screen_y = (1.0 - ndc_y) * 0.5 * self.viewport.y;
        Vec2::new(screen_x, screen_y)
    }

    /// Column-major orthographic view-projection matrix mapping world space
    /// to wgpu clip space (x,y in [-1, 1], origin at `self.position`).
    /// Matches the layout expected by a `mat4x4<f32>` uniform in WGSL.
    pub fn view_proj_matrix(&self) -> [[f32; 4]; 4] {
        let half_w = self.viewport.x / (2.0 * self.zoom);
        let half_h = self.viewport.y / (2.0 * self.zoom);
        let sx = 1.0 / half_w;
        let sy = 1.0 / half_h;
        [
            [sx, 0.0, 0.0, 0.0],
            [0.0, sy, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-self.position.x * sx, -self.position.y * sy, 0.0, 1.0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_to_world_round_trips_through_world_to_screen() {
        let cam = Camera::new(1920.0, 1080.0);
        let world = Vec2::new(3.5, -2.25);
        let screen = cam.world_to_screen(world);
        let back = cam.screen_to_world(screen);
        assert!((back.x - world.x).abs() < 1e-4);
        assert!((back.y - world.y).abs() < 1e-4);
    }

    #[test]
    fn viewport_centre_maps_to_camera_position() {
        let mut cam = Camera::new(800.0, 600.0);
        cam.position = Vec2::new(10.0, -5.0);
        let centre_world = cam.screen_to_world(Vec2::new(400.0, 300.0));
        assert!((centre_world.x - 10.0).abs() < 1e-4);
        assert!((centre_world.y + 5.0).abs() < 1e-4);
    }

    #[test]
    fn zoom_at_keeps_world_point_under_cursor_fixed() {
        let mut cam = Camera::new(800.0, 600.0);
        let anchor = Vec2::new(200.0, 150.0);
        let world_before = cam.screen_to_world(anchor);
        cam.zoom_at(anchor, 2.0);
        let world_after = cam.screen_to_world(anchor);
        assert!((world_before.x - world_after.x).abs() < 1e-3);
        assert!((world_before.y - world_after.y).abs() < 1e-3);
        assert_eq!(cam.zoom, 2.0);
    }

    #[test]
    fn zoom_is_clamped_to_valid_range() {
        let mut cam = Camera::new(800.0, 600.0);
        cam.zoom_at(Vec2::new(400.0, 300.0), 0.0001);
        assert_eq!(cam.zoom, Camera::MIN_ZOOM);
        cam.zoom_at(Vec2::new(400.0, 300.0), 1_000_000.0);
        assert_eq!(cam.zoom, Camera::MAX_ZOOM);
    }

    #[test]
    fn pan_moves_position_by_world_delta() {
        let mut cam = Camera::new(800.0, 600.0);
        cam.pan(Vec2::new(1.0, 2.0));
        assert_eq!(cam.position, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn fit_to_bounds_centres_and_zooms_to_show_whole_box() {
        let mut cam = Camera::new(800.0, 400.0);
        cam.fit_to_bounds(Vec2::new(-2.0, -1.0), Vec2::new(2.0, 1.0), 0.0);
        assert_eq!(cam.position, Vec2::ZERO);
        // Both corners of the box should now land inside the viewport.
        let top_left = cam.world_to_screen(Vec2::new(-2.0, 1.0));
        let bottom_right = cam.world_to_screen(Vec2::new(2.0, -1.0));
        assert!(top_left.x >= -0.5 && top_left.x <= cam.viewport.x + 0.5);
        assert!(bottom_right.x >= -0.5 && bottom_right.x <= cam.viewport.y + 0.5);
    }

    #[test]
    fn fit_to_bounds_with_padding_zooms_out_further() {
        let mut cam_tight = Camera::new(800.0, 400.0);
        cam_tight.fit_to_bounds(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), 0.0);

        let mut cam_padded = Camera::new(800.0, 400.0);
        cam_padded.fit_to_bounds(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), 0.2);

        assert!(cam_padded.zoom < cam_tight.zoom);
    }

    #[test]
    fn fit_to_bounds_does_not_produce_nan_for_degenerate_box() {
        let mut cam = Camera::new(800.0, 400.0);
        cam.fit_to_bounds(Vec2::ZERO, Vec2::ZERO, 0.1);
        assert!(cam.zoom.is_finite());
        assert!(cam.zoom > 0.0);
    }
}
