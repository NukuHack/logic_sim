//! wgpu/WGSL renderer for the chip canvas. This module owns the GPU device/surface and turns a
//! `SceneGeometry` (from `render::scene`) into a single triangle-list draw call each frame, with the
//! camera's view-projection matrix fed in as a uniform. This file can't be exercised by `cargo test`
//! in a headless/CI sandbox (it needs a real GPU adapter + window surface), so it is kept as thin
//! glue around the pure, fully-tested `layout` / `theme` / `scene` / `camera` modules.

use crate::render::camera::Camera;
use crate::render::scene::{SceneGeometry, SceneVertex, TextLabel};
use crate::structs::Vec2;
use bytemuck::{Pod, Zeroable};
use glyphon::{
	Attrs, Buffer as TextBuffer, Cache as TextCache, Color as GlyphColour, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
	TextAtlas, TextBounds, TextRenderer, Viewport as TextViewport,
};

pub const SHADER_SRC: &str = include_str!("shader.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
	pub position: [f32; 2],
	pub colour: [f32; 4],
}

impl Vertex {
	pub const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];

	pub fn layout() -> wgpu::VertexBufferLayout<'static> {
		wgpu::VertexBufferLayout {
			array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
			step_mode: wgpu::VertexStepMode::Vertex,
			attributes: &Self::ATTRS,
		}
	}
}

impl From<SceneVertex> for Vertex {
	fn from(v: SceneVertex) -> Self {
		Vertex { position: [v.pos.x, v.pos.y], colour: v.colour }
	}
}

pub fn scene_to_vertices(scene: &SceneGeometry) -> Vec<Vertex> {
	scene.triangles.iter().map(|v| Vertex::from(*v)).collect()
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
	pub view_proj: [[f32; 4]; 4],
}

/// Owns the GPU device/queue/surface and the pipeline used to draw the chip
/// canvas. Construction requires a live `wgpu::Surface`, so it can only be
/// built once a window exists (see `src/bin/viewer.rs`).
pub struct Renderer {
	pub device: wgpu::Device,
	pub queue: wgpu::Queue,
	pub surface: wgpu::Surface<'static>,
	pub surface_format: wgpu::TextureFormat,
	pub config: wgpu::SurfaceConfiguration,
	pipeline: wgpu::RenderPipeline,
	camera_buffer: wgpu::Buffer,
	camera_bind_group: wgpu::BindGroup,
	vertex_buffer: wgpu::Buffer,
	vertex_capacity: usize,
	vertex_count: u32,

	// Text (gate/chip name labels): owns its own small glyphon pipeline layered on top of the
	// flat-colour triangle pass above; see `render::scene::TextLabel` for the label data this
	// consumes, kept behind its own `Cache`/`Viewport`/`TextAtlas` per glyphon's prepare()-then-render() split.
	font_system: FontSystem,
	swash_cache: SwashCache,
	text_atlas: TextAtlas,
	text_viewport: TextViewport,
	text_renderer: TextRenderer,
}

impl Renderer {
	/// Create a renderer targeting `surface`, sized `width`x`height`.
	/// `background` is the clear colour (see `render::theme::BACKGROUND_COL`).
	pub async fn new(instance: &wgpu::Instance, surface: wgpu::Surface<'static>, width: u32, height: u32) -> Renderer {
		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions {
				power_preference: wgpu::PowerPreference::HighPerformance,
				compatible_surface: Some(&surface),
				force_fallback_adapter: false,
			})
			.await
			.expect("Failed to find a suitable wgpu adapter");

		let (device, queue) = adapter
			.request_device(
				&wgpu::DeviceDescriptor {
					label: Some("dls-device"),
					required_features: wgpu::Features::empty(),
					required_limits: wgpu::Limits::default(),
					memory_hints: wgpu::MemoryHints::default(),
				},
				None,
			)
			.await
			.expect("Failed to create wgpu device");

		let info = adapter.get_info();
		eprintln!("wgpu: using adapter '{}' ({:?} backend, {:?})", info.name, info.backend, info.device_type);

		let caps = surface.get_capabilities(&adapter);
		let surface_format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);

		let config = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format: surface_format,
			width,
			height,
			present_mode: wgpu::PresentMode::Fifo,
			alpha_mode: caps.alpha_modes[0],
			view_formats: vec![],
			desired_maximum_frame_latency: 2,
		};
		surface.configure(&device, &config);

		let shader = device
			.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("dls-shader"), source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()) });

		let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("camera-uniform"),
			size: std::mem::size_of::<CameraUniform>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});

		let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("camera-bgl"),
			entries: &[wgpu::BindGroupLayoutEntry {
				binding: 0,
				visibility: wgpu::ShaderStages::VERTEX,
				ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
				count: None,
			}],
		});

		let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("camera-bg"),
			layout: &camera_bind_group_layout,
			entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() }],
		});

		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("dls-pipeline-layout"),
			bind_group_layouts: &[&camera_bind_group_layout],
			push_constant_ranges: &[],
		});

		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("dls-pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &shader,
				entry_point: "vs_main",
				buffers: &[Vertex::layout()],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			},
			fragment: Some(wgpu::FragmentState {
				module: &shader,
				entry_point: "fs_main",
				targets: &[Some(wgpu::ColorTargetState {
					format: surface_format,
					blend: Some(wgpu::BlendState::ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::ALL,
				})],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			}),
			primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
			depth_stencil: None,
			multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
			multiview: None,
			cache: None,
		});

		let vertex_capacity = 4096;
		let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("scene-vertices"),
			size: (vertex_capacity * std::mem::size_of::<Vertex>()) as u64,
			usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});

		let font_system = FontSystem::new();
		eprintln!("glyphon: {} font face(s) available", font_system.db().faces().count());
		let swash_cache = SwashCache::new();
		let text_cache = TextCache::new(&device);
		let text_viewport = TextViewport::new(&device, &text_cache);
		let mut text_atlas = TextAtlas::new(&device, &queue, &text_cache, surface_format);
		let text_renderer = TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);

		Renderer {
			device,
			queue,
			surface,
			surface_format,
			config,
			pipeline,
			camera_buffer,
			camera_bind_group,
			vertex_buffer,
			vertex_capacity,
			vertex_count: 0,
			font_system,
			swash_cache,
			text_atlas,
			text_viewport,
			text_renderer,
		}
	}

	pub fn resize(&mut self, width: u32, height: u32) {
		if width == 0 || height == 0 {
			return;
		}
		self.config.width = width;
		self.config.height = height;
		self.surface.configure(&self.device, &self.config);
	}

	fn ensure_vertex_capacity(&mut self, needed: usize) {
		if needed <= self.vertex_capacity {
			return;
		}
		self.vertex_capacity = needed.next_power_of_two();
		self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("scene-vertices"),
			size: (self.vertex_capacity * std::mem::size_of::<Vertex>()) as u64,
			usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
	}

	/// Lays out `labels` (world space, via `camera`) into glyphon text
	/// buffers and uploads any newly-needed glyphs into the atlas. Must be
	/// called before `begin_render_pass` (glyphon's `prepare` touches the
	/// device/queue directly, not a render pass), and the returned buffers
	/// must be kept alive until after `text_renderer.render(...)` is called
	/// -- `TextArea` borrows them, so `render()` builds them, prepares, and
	/// draws all in one scope rather than stashing them on `self`.
	fn prepare_text(&mut self, labels: &[TextLabel], camera: &Camera) -> Vec<TextBuffer> {
		self.text_viewport.update(&self.queue, Resolution { width: self.config.width, height: self.config.height });

		let mut buffers = Vec::with_capacity(labels.len());
		for label in labels {
			// World-space sizes -> screen pixels via the camera's current
			// zoom (screen pixels per world unit), matching how
			// `Camera::world_to_screen` scales world-space geometry.
			let font_px = (label.font_size * camera.zoom).max(1.0);
			let width_px = (label.width * camera.zoom).max(1.0);

			let family = Attrs::new().family(Family::SansSerif);
			let mut buffer = TextBuffer::new(&mut self.font_system, Metrics::new(font_px, font_px * 1.2));
			buffer.set_size(&mut self.font_system, Some(width_px), Some(font_px * 4.0));
			buffer.set_text(&mut self.font_system, &label.text, family, Shaping::Advanced);
			for line in buffer.lines.iter_mut() {
				line.set_align(Some(glyphon::cosmic_text::Align::Center));
			}
			buffer.shape_until_scroll(&mut self.font_system, false);
			buffers.push(buffer);
		}

		let areas: Vec<TextArea> = labels
			.iter()
			.zip(buffers.iter())
			.map(|(label, buffer)| {
				let screen = camera.world_to_screen(Vec2::new(label.pos.x, label.pos.y));
				let width_px = (label.width * camera.zoom).max(1.0);
				let font_px = (label.font_size * camera.zoom).max(1.0);
				let left = screen.x - width_px / 2.0;
				let top = screen.y - font_px * 1.2 / 2.0;
				TextArea {
					buffer,
					left,
					top,
					scale: 1.0,
					bounds: TextBounds {
						left: left.floor() as i32,
						top: top.floor() as i32,
						right: (left + width_px).ceil() as i32,
						bottom: (top + font_px * 1.2).ceil() as i32,
					},
					default_color: GlyphColour::rgba(
						(label.colour[0] * 255.0) as u8,
						(label.colour[1] * 255.0) as u8,
						(label.colour[2] * 255.0) as u8,
						(label.colour[3] * 255.0) as u8,
					),
					custom_glyphs: &[],
				}
			})
			.collect();

		if !areas.is_empty() {
			self.text_renderer
				.prepare(&self.device, &self.queue, &mut self.font_system, &mut self.text_atlas, &self.text_viewport, areas, &mut self.swash_cache)
				.expect("glyphon text prepare failed");
		}

		buffers
	}

	/// Draws one frame from back to front, as an ordered stack of
	/// `layers` (e.g. `[world, ui_overlay, context_menu]`) -- each
	/// layer's own triangles *and* text labels are fully drawn (as their
	/// own submitted GPU pass) before the next layer's are even
	/// uploaded, so a later layer's triangles reliably paint over an
	/// earlier layer's text, not just its shapes. Compare the old
	/// single-`SceneGeometry` `render`, which drew *all* triangles from
	/// every layer first and *all* text second -- meaning any label, no
	/// matter which logical layer it belonged to, always rendered on top
	/// of every triangle, including e.g. a modal popup's own background
	/// that was meant to cover it.
	///
	/// Each layer is deliberately its own `queue.submit()` (rather than
	/// one shared encoder/pass per frame) precisely so that layer *N*+1's
	/// `prepare_text` call -- which uploads into the same glyphon
	/// text-vertex buffer `prepare_text` reuses every call -- can't race
	/// layer *N*'s still-unexecuted `text_renderer.render` draw command.
	/// `wgpu::Queue` preserves strict submission order for everything
	/// pushed through it (`write_buffer`/`write_texture` calls *and*
	/// `submit()`s alike), so submitting layer *N* fully before even
	/// preparing layer *N*+1's text guarantees layer *N*'s glyphs are
	/// already consumed by the time they'd otherwise be overwritten.
	pub fn render(&mut self, layers: &[SceneGeometry], camera: &Camera, clear_colour: [f32; 4]) -> Result<(), wgpu::SurfaceError> {
		let camera_uniform = CameraUniform { view_proj: camera.view_proj_matrix() };
		self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera_uniform));

		let frame = self.surface.get_current_texture()?;
		let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

		let mut drew_anything = false;
		for layer in layers {
			if layer.triangles.is_empty() && layer.labels.is_empty() {
				continue;
			}
			self.render_layer(layer, camera, &view, clear_colour, !drew_anything);
			drew_anything = true;
		}
		// Still need to clear the frame even if every layer was empty
		// (e.g. a blank scene), or the previous frame's contents (or
		// undefined memory, on the very first frame) would show through.
		if !drew_anything {
			self.render_layer(&SceneGeometry::default(), camera, &view, clear_colour, true);
		}

		frame.present();

		// Evict glyphs that haven't been used recently so the atlas
		// doesn't grow unbounded as different chip names scroll through
		// view over a long session.
		self.text_atlas.trim();

		Ok(())
	}

	/// Draws one layer's triangles + text as a single submitted pass.
	/// `clear` selects whether this pass clears the target (the first
	/// non-empty layer of the frame) or loads/preserves whatever's
	/// already there (every layer after that) -- see `render`'s doc
	/// comment for why each layer gets its own `submit()` rather than
	/// sharing one encoder.
	fn render_layer(&mut self, layer: &SceneGeometry, camera: &Camera, view: &wgpu::TextureView, clear_colour: [f32; 4], clear: bool) {
		let vertices = scene_to_vertices(layer);
		self.ensure_vertex_capacity(vertices.len());
		if !vertices.is_empty() {
			self.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
		}
		self.vertex_count = vertices.len() as u32;

		// Must happen before the render pass begins -- glyphon's prepare() writes into the atlas texture
		// via the queue rather than recording into a pass. `_text_buffers` just needs to outlive the pass
		// below (the `TextArea`s handed to `text_renderer` during prepare borrow from it).
		let _text_buffers = self.prepare_text(&layer.labels, camera);
		let has_text = !layer.labels.is_empty();

		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dls-layer-encoder") });
		{
			let load = if clear {
				wgpu::LoadOp::Clear(wgpu::Color {
					r: clear_colour[0] as f64,
					g: clear_colour[1] as f64,
					b: clear_colour[2] as f64,
					a: clear_colour[3] as f64,
				})
			} else {
				wgpu::LoadOp::Load
			};

			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("dls-layer-pass"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view,
					resolve_target: None,
					ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
			});

			if self.vertex_count > 0 {
				pass.set_pipeline(&self.pipeline);
				pass.set_bind_group(0, &self.camera_bind_group, &[]);
				pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
				pass.draw(0..self.vertex_count, 0..1);
			}

			// Text draws on top of this layer's own shapes, in the same pass (glyphon is designed as
			// middleware over an existing pass -- no extra clear/load needed), but never leaks into
			// the next layer's pass, unlike the old single-pass renderer.
			if has_text {
				self.text_renderer.render(&self.text_atlas, &self.text_viewport, &mut pass).expect("glyphon text render failed");
			}
		}

		self.queue.submit(std::iter::once(encoder.finish()));
	}
}

/// Convenience used by `scene_to_vertices` tests below and by callers that
/// want to build an initial vertex buffer without going through `Renderer`.
pub fn upload_ready_bytes(vertices: &[Vertex]) -> &[u8] {
	bytemuck::cast_slice(vertices)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::render::theme;

	#[test]
	fn scene_vertex_converts_to_gpu_vertex() {
		let sv = SceneVertex { pos: Vec2::new(1.0, 2.0), colour: theme::PIN_COL };
		let v: Vertex = sv.into();
		assert_eq!(v.position, [1.0, 2.0]);
		assert_eq!(v.colour, theme::PIN_COL);
	}

	#[test]
	fn scene_to_vertices_preserves_triangle_count() {
		let mut geo = SceneGeometry::default();
		geo.add_rect(Vec2::ZERO, Vec2::new(1.0, 1.0), theme::CHIP_BODY_COL);
		let verts = scene_to_vertices(&geo);
		assert_eq!(verts.len(), 6);
	}

	#[test]
	fn vertex_bytes_round_trip_through_bytemuck() {
		let verts = vec![Vertex { position: [0.0, 0.0], colour: [1.0, 0.0, 0.0, 1.0] }];
		let bytes = upload_ready_bytes(&verts);
		assert_eq!(bytes.len(), std::mem::size_of::<Vertex>());
	}
}
