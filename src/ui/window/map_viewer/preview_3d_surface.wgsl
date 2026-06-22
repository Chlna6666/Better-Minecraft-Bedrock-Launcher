struct Preview3dCamera {
    view_proj_model: mat4x4<f32>,
};

struct Preview3dVertex {
    @location(0) position: vec3<f32>,
    @location(1) premultiplied_color: vec4<f32>,
};

struct Preview3dVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) premultiplied_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Preview3dCamera;

@vertex
fn vs_preview_3d(vertex: Preview3dVertex) -> Preview3dVarying {
    var out: Preview3dVarying;
    out.position = camera.view_proj_model * vec4<f32>(vertex.position, 1.0);
    out.premultiplied_color = vertex.premultiplied_color;
    return out;
}

@fragment
fn fs_preview_3d(input: Preview3dVarying) -> @location(0) vec4<f32> {
    return input.premultiplied_color;
}
