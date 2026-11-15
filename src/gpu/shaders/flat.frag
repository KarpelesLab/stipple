#version 450

// A flat color: the stencil and cover passes of a path, and stencil clips.
// The push-constant offset matches `FLAT_COLOR` in `src/gpu/vk/record.rs`.
layout(location = 0) out vec4 outColor;
layout(push_constant) uniform Push {
    layout(offset = 16) vec4 color;
} u;

void main() {
    outColor = u.color;
}
