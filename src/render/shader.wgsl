// Chip canvas shader: transforms world-space 2D geometry (chip bodies, pin
// dots, wires -- all pre-triangulated on the CPU in render::scene) by the
// camera's view-projection matrix, and passes the per-vertex colour through
// untouched (all shading/state-colouring happens on the CPU side, matching
// the flat-colour look of the original Unity draw calls).

struct CameraUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 0.0, 1.0);
    out.colour = in.colour;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.colour;
}
