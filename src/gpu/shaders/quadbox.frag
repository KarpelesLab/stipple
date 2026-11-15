#version 450

// Batched boxes only (a batch without a texture; `quadbox_fs` in
// `metal_scene.rs`): `quad.frag` without the texture, textured quads left
// out.
layout(location = 0) in vec2 v_p;
layout(location = 1) in vec4 v_a1;
layout(location = 2) in vec4 v_a2;
layout(location = 3) in vec4 v_a3;
layout(location = 4) in float v_mode;
layout(location = 0) out vec4 outColor;

// A box: `v_p` is the device position, `v_a1` the rect, `v_a2` the color,
// `v_a3` the radius and stroke. Coverage is the exact overlap of the pixel
// with a sharp box (or ring), or a signed distance for rounded corners.
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

vec4 quadBox() {
    float radius = v_a3.x;
    float stroke = v_a3.y;
    vec2 c = (v_a1.xy + v_a1.zw) * 0.5;
    vec2 h = (v_a1.zw - v_a1.xy) * 0.5;
    vec2 p = v_p - c;
    float w = stroke * 0.5;
    float cov;
    if (radius <= 0.0) {
        if (stroke > 0.0) {
            cov = boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w));
        } else {
            cov = boxcov(p, h);
        }
    } else {
        float r = min(radius, min(h.x, h.y));
        float d = sdf(p, h, r);
        if (stroke > 0.0) {
            cov = clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0);
        } else {
            cov = clamp(0.5 - d, 0.0, 1.0);
        }
    }
    return v_a2 * cov;
}

void main() {
    outColor = v_mode < 0.5 ? quadBox() : vec4(0.0);
}
