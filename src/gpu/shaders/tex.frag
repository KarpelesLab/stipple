#version 450

// A textured quad: `aux` is the local position inside `rect`, `px` the device
// pixels per local unit along each axis (for edge coverage). `uv` is the part
// of the texture spread over the rect (swapped ends mirror it), kept within
// `clampr`; `tint` (premultiplied) multiplies every texel. Nearest or bilinear
// is the sampler bound with the texture. The push-constant offsets match
// `TEX_*` in `src/gpu/vk/record.rs`.
layout(location = 0) in vec2 aux;
layout(location = 0) out vec4 outColor;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(push_constant) uniform Push {
    layout(offset = 16) vec4 rect;
    layout(offset = 32) vec4 uv;
    layout(offset = 48) vec4 clampr;
    layout(offset = 64) vec4 tint;
    layout(offset = 80) vec2 px;
    layout(offset = 88) float opacity;
    layout(offset = 92) float premul;
    layout(offset = 96) float aa;
} u;

void main() {
    vec2 f = (aux - u.rect.xy) / (u.rect.zw - u.rect.xy);
    vec2 uv = clamp(mix(u.uv.xy, u.uv.zw, f), u.clampr.xy, u.clampr.zw);
    vec4 t = texture(tex, uv);
    if (u.premul < 0.5) {
        t.rgb *= t.a;
    }
    t *= u.tint;
    float cov = 1.0;
    if (u.aa > 0.5) {
        vec2 d = min(aux - u.rect.xy, u.rect.zw - aux) * u.px;
        cov = clamp(d.x + 0.5, 0.0, 1.0) * clamp(d.y + 0.5, 0.0, 1.0);
    }
    outColor = t * (u.opacity * cov);
}
