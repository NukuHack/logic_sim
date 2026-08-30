//! 2D pan/zoom camera for the chip canvas. This is the "base" piece of `Game/Interaction/CameraController.cs`
//! that the renderer needs (world <-> screen mapping) without any of the Unity input plumbing. Kept
//! GPU-free and unit-testable; `render::gpu` consumes `Camera::view_proj_matrix` as a uniform.

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

impl Default for Camera {
	fn default() -> Self {
		Self { position: Vec2::default(), zoom: Self::DEF_ZOOM, viewport: Vec2::default() }
	}
}

// Chips are sized in grid units of ~0.125, so getting a single small chip to fill a comfortable
// fraction of a ~1000px window legitimately needs zoom in the hundreds-to-low-thousands. The old
// cap of 40 meant `fit_to_bounds` was silently clamped and could never make small chips readable.
impl Camera {
	pub const DEF_ZOOM: f32 = 96.0;
	pub const MIN_ZOOM: f32 = 0.05;
	pub const MAX_ZOOM: f32 = 4096.0;

	pub fn new(viewport: Vec2) -> Self {
		Self { position: Vec2::ZERO, zoom: 1.0, viewport: Self::sanitize_viewport(viewport) }
	}

	/// Clamps a viewport size to a sane minimum (1x1) so `screen_to_world`/
	/// `world_to_screen`/`view_proj_matrix` never divide by zero. A `0` (or negative/NaN)
	/// viewport can genuinely happen for a frame or two on some platforms -- e.g. the window's
	/// real size can arrive via a `Resized` event *after* the very first frame is drawn -- and
	/// dividing by it produces `Infinity`/`NaN` world-space geometry that gets sent straight to
	/// the GPU, which some drivers handle very badly (up to and including a hard crash) rather
	/// than just misrendering the one bad frame.
	fn sanitize_viewport(viewport: Vec2) -> Vec2 {
		let x = if viewport.x.is_finite() { viewport.x.max(1.0) } else { 1.0 };
		let y = if viewport.y.is_finite() { viewport.y.max(1.0) } else { 1.0 };
		Vec2::new(x, y)
	}

	pub fn pan(&mut self, delta_world: Vec2) {
		self.position += delta_world;
	}

	/// Zoom in/out around a fixed screen-space anchor (e.g. the mouse
	/// cursor), keeping the world point under that anchor stationary --
	/// same UX as scroll-to-zoom in the original editor.
	pub fn zoom_at(&mut self, screen_anchor: Vec2, zoom_factor: f32) {
		let world_before = self.screen_to_world(screen_anchor);
		self.zoom = (self.zoom * zoom_factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
		let world_after = self.screen_to_world(screen_anchor);
		let correction = Vec2::new(world_before.x - world_after.x, world_before.y - world_after.y);
		self.position += correction;
	}

	pub fn resize_viewport(&mut self, width: f32, height: f32) {
		self.viewport = Self::sanitize_viewport(Vec2::new(width, height));
	}

	/// Fit the camera so the world-space box `[min, max]` is fully visible, with
	/// `padding_fraction` extra room on each side (e.g. 0.1 = 10% margin).
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
		[[sx, 0.0, 0.0, 0.0], [0.0, sy, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [-self.position.x * sx, -self.position.y * sy, 0.0, 1.0]]
	}
}
