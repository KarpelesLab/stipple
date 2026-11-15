// The Direct3D 11 scene shaders (src/gpu/d3d/scene.rs), compiled with
// D3DCompile when the executor is created (vs_4_0 / ps_4_0). Each matches its
// MSL twin in src/gpu/metal_scene.rs and its GLSL twin in src/gpu/gl/scene.rs.
//
// One constant buffer feeds every stage: `xf` is the vertex shader's
// device-to-clip mapping, u1..u7 the pixel shader's own fields (packed by
// src/gpu/d3d/abi.rs's *_constants functions).

cbuffer Constants : register(b0) {
    float4 xf;
    float4 u1;
    float4 u2;
    float4 u3;
    float4 u4;
    float4 u5;
    float4 u6;
    float4 u7;
};

Texture2D tex0 : register(t0);
SamplerState smp_bilinear : register(s0);
SamplerState smp_nearest : register(s1);

struct VOut {
    float4 pos : SV_Position;
    float2 aux : TEXCOORD0;
};

// Every program shares one vertex shader: `xy` in device pixels mapped to
// clip space by `xf` (scale.xy, offset.zw), and a pass-through `aux`.
VOut vs_main(float4 d : POSITION) {
    VOut o;
    o.pos = float4(d.xy * xf.xy + xf.zw, 0.0, 1.0);
    o.aux = d.zw;
    return o;
}

// A box: `aux` is the device position. Coverage is the exact overlap of the
// pixel with a sharp box (or ring), or a signed distance for rounded corners.
// u1 = rect, u2 = color, u3.x = radius, u3.y = stroke.
float span1(float p, float h) {
    return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0);
}
float boxcov(float2 p, float2 h) {
    h = max(h, float2(0.0, 0.0));
    return span1(p.x, h.x) * span1(p.y, h.y);
}
float sdf(float2 p, float2 h, float r) {
    float2 d = abs(p) - (h - float2(r, r));
    return length(max(d, float2(0.0, 0.0))) + min(max(d.x, d.y), 0.0) - r;
}
float4 box_ps(VOut i) : SV_Target {
    float2 c = (u1.xy + u1.zw) * 0.5;
    float2 h = (u1.zw - u1.xy) * 0.5;
    float2 p = i.aux - c;
    float radius = u3.x;
    float stroke = u3.y;
    float w = stroke * 0.5;
    float cov;
    if (radius <= 0.0) {
        if (stroke > 0.0) {
            cov = boxcov(p, h + float2(w, w)) - boxcov(p, h - float2(w, w));
        } else {
            cov = boxcov(p, h);
        }
    } else {
        float r = min(radius, min(h.x, h.y));
        float d = sdf(p, h, r);
        if (stroke > 0.0) {
            cov = saturate(0.5 - (d - w)) - saturate(0.5 - (d + w));
        } else {
            cov = saturate(0.5 - d);
        }
    }
    return u2 * cov;
}

// A textured quad: `aux` is the local position inside the rect u1; u5.xy the
// device pixels per local unit along each axis (for edge coverage). u2 is the
// part of the texture spread over the rect, kept within u3; u4 (premultiplied)
// tints every texel. u5.z = opacity, u5.w = premultiplied, u6.x = antialiased
// edges, u6.y = nearest.
float4 tex_ps(VOut i) : SV_Target {
    float2 f = (i.aux - u1.xy) / (u1.zw - u1.xy);
    float2 uv = clamp(lerp(u2.xy, u2.zw, f), u3.xy, u3.zw);
    float4 c;
    if (u6.y > 0.5) {
        c = tex0.SampleLevel(smp_nearest, uv, 0.0);
    } else {
        c = tex0.SampleLevel(smp_bilinear, uv, 0.0);
    }
    if (u5.w < 0.5) {
        c.rgb *= c.a;
    }
    c *= u4;
    float cov = 1.0;
    if (u6.x > 0.5) {
        float2 d = min(i.aux - u1.xy, u1.zw - i.aux) * u5.xy;
        cov = saturate(d.x + 0.5) * saturate(d.y + 0.5);
    }
    return c * (u5.z * cov);
}

// A flat color (u1): the stencil and cover passes, and stencil clips.
float4 flat_ps() : SV_Target {
    return u1;
}

// Resolving a supersampled path coverage: `aux` is the position in scratch
// texels; four bilinear taps (u2.z apart) average the pixel's ss x ss block.
// u1 = color, u2.xy = 1 / scratch size.
float4 resolve_ps(VOut i) : SV_Target {
    float2 inv = u2.xy;
    float tap = u2.z;
    float c = tex0.SampleLevel(smp_bilinear, (i.aux + float2(-tap, -tap)) * inv, 0.0).r
            + tex0.SampleLevel(smp_bilinear, (i.aux + float2( tap, -tap)) * inv, 0.0).r
            + tex0.SampleLevel(smp_bilinear, (i.aux + float2(-tap,  tap)) * inv, 0.0).r
            + tex0.SampleLevel(smp_bilinear, (i.aux + float2( tap,  tap)) * inv, 0.0).r;
    return u1 * (c * 0.25);
}
