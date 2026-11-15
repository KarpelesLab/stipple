#version 450

// Batched boxes and textured quads (`quad_fs` in `metal_scene.rs`; the
// vertex format is in `src/gpu/batch.rs`). A textured quad: `v_p` is the
// texture coordinate, kept within `v_a2`; `v_a1` the distances to its edges
// in device pixels (for their coverage); `v_a3` the tint times the opacity
// (premultiplied). Nearest or bilinear is the sampler bound with the
// texture.
layout(location = 0) in vec2 v_p;
layout(location = 1) in vec4 v_a1;
layout(location = 2) in vec4 v_a2;
layout(location = 3) in vec4 v_a3;
layout(location = 4) in float v_mode;
layout(location = 0) out vec4 outColor;
layout(set = 0, binding = 0) uniform sampler2D tex;

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
    if (v_mode < 0.5) {
        outColor = quadBox();
        return;
    }
    vec4 t = texture(tex, clamp(v_p, v_a2.xy, v_a2.zw));
    if (v_mode < 1.5) {
        t.rgb *= t.a;
    }
    t *= v_a3;
    float cov = clamp(min(v_a1.x, v_a1.z) + 0.5, 0.0, 1.0)
              * clamp(min(v_a1.y, v_a1.w) + 0.5, 0.0, 1.0);
    outColor = t * cov;
}
