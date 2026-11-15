#version 450

// Resolving a supersampled path coverage: `aux` is the position in scratch
// texels; four bilinear taps average the pixel's ss×ss block. The
// push-constant offsets match `RESOLVE_*` in `src/gpu/vk/record.rs`.
layout(location = 0) in vec2 aux;
layout(location = 0) out vec4 outColor;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(push_constant) uniform Push {
    layout(offset = 16) vec4 color;
    layout(offset = 32) vec2 inv;
    layout(offset = 40) float tap;
} u;

void main() {
    float c = texture(tex, (aux + vec2(-u.tap, -u.tap)) * u.inv).r
            + texture(tex, (aux + vec2( u.tap, -u.tap)) * u.inv).r
            + texture(tex, (aux + vec2(-u.tap,  u.tap)) * u.inv).r
            + texture(tex, (aux + vec2( u.tap,  u.tap)) * u.inv).r;
    outColor = u.color * (c * 0.25);
}
