#version 450

// A box: `aux` is the device position. Coverage is the exact overlap of the
// pixel with a sharp box (or ring), or a signed distance for rounded corners.
// The push-constant offsets match `BOX_*` in `src/gpu/vk/record.rs`.
layout(location = 0) in vec2 aux;
layout(location = 0) out vec4 outColor;
layout(push_constant) uniform Push {
    layout(offset = 16) vec4 rect;
    layout(offset = 32) vec4 color;
    layout(offset = 48) float radius;
    layout(offset = 52) float stroke;
} u;

float span(float p, float h) {
    return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0);
}

float boxcov(vec2 p, vec2 h) {
    h = max(h, vec2(0.0));
    return span(p.x, h.x) * span(p.y, h.y);
}

float sdf(vec2 p, vec2 h, float r) {
    vec2 d = abs(p) - (h - vec2(r));
    return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

void main() {
    vec2 c = (u.rect.xy + u.rect.zw) * 0.5;
    vec2 h = (u.rect.zw - u.rect.xy) * 0.5;
    vec2 p = aux - c;
    float w = u.stroke * 0.5;
    float cov;
    if (u.radius <= 0.0) {
        if (u.stroke > 0.0) {
            cov = boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w));
        } else {
            cov = boxcov(p, h);
        }
    } else {
        float r = min(u.radius, min(h.x, h.y));
        float d = sdf(p, h, r);
        if (u.stroke > 0.0) {
            cov = clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0);
        } else {
            cov = clamp(0.5 - d, 0.0, 1.0);
        }
    }
    outColor = u.color * cov;
}
