#version 450

// Batched boxes and textured quads (`src/gpu/batch.rs`; the MSL `quad_vs`
// in `metal_scene.rs`): every input is a vertex attribute, so a run of
// them is one draw. `a0.xy` in device pixels is mapped to clip space by
// `xf` (scale.xy, offset.zw); the rest is passed through.
layout(location = 0) in vec4 a0;
layout(location = 1) in vec4 a1;
layout(location = 2) in vec4 a2;
layout(location = 3) in vec4 a3;
layout(location = 4) in vec4 a4;
layout(push_constant) uniform Push {
    vec4 xf;
} p;
layout(location = 0) out vec2 v_p;
layout(location = 1) out vec4 v_a1;
layout(location = 2) out vec4 v_a2;
layout(location = 3) out vec4 v_a3;
layout(location = 4) out float v_mode;

void main() {
    v_p = a0.zw;
    v_a1 = a1;
    v_a2 = a2;
    v_a3 = a3;
    v_mode = a4.x;
    gl_Position = vec4(a0.xy * p.xf.xy + p.xf.zw, 0.0, 1.0);
}
