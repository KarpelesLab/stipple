#version 450

// Every scene program shares this vertex shader (the GLSL ES `VS` in
// `gl/scene.rs`, the MSL `vs` in `metal_scene.rs`): `v.xy` in device pixels,
// mapped to clip space by `xf` (scale.xy, offset.zw), and `v.zw` passed
// through. Vulkan's clip space has y down, so device row 0 is the top row.
layout(location = 0) in vec4 v;
layout(push_constant) uniform Push {
    vec4 xf;
} p;
layout(location = 0) out vec2 aux;

void main() {
    aux = v.zw;
    gl_Position = vec4(v.xy * p.xf.xy + p.xf.zw, 0.0, 1.0);
}
