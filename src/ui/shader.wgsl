struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Full-screen triangle — no vertex buffer needed.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    var pos = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0),
        vec2( 3.0, -1.0),
        vec2(-1.0,  3.0),
    );
    // NDC y↑, texture y↓ — flip v.
    var uv = array<vec2<f32>, 3>(
        vec2(0.0, 1.0),
        vec2(2.0, 1.0),
        vec2(0.0, -1.0),
    );
    var out: VertexOut;
    out.pos = vec4(pos[idx], 0.0, 1.0);
    out.uv  = uv[idx];
    return out;
}

@group(0) @binding(0) var y_tex:  texture_2d<f32>;
@group(0) @binding(1) var u_tex:  texture_2d<f32>;
@group(0) @binding(2) var v_tex:  texture_2d<f32>;
@group(0) @binding(3) var samp:   sampler;

// BT.601 limited-range YCbCr → linear RGB.
@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let y = textureSample(y_tex, samp, in.uv).r;
    let u = textureSample(u_tex, samp, in.uv).r - 0.5;
    let v = textureSample(v_tex, samp, in.uv).r - 0.5;

    let r = clamp(y + 1.402 * v,                     0.0, 1.0);
    let g = clamp(y - 0.344136 * u - 0.714136 * v,   0.0, 1.0);
    let b = clamp(y + 1.772 * u,                     0.0, 1.0);

    return vec4(r, g, b, 1.0);
}
