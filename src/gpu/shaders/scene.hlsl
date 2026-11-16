// The Direct3D 11 scene shaders (src/gpu/d3d/scene.rs), compiled with
// D3DCompile when the executor is created (vs_4_0 / ps_4_0). Each matches its
// MSL twin in src/gpu/metal_scene.rs and its GLSL twin in src/gpu/gl/scene.rs.
//
// One constant buffer feeds every stage: `xf` is the vertex shaders'
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
// NV12's chroma (`quad_yuv_ps`), tex0 its luma.
Texture2D tex1 : register(t1);
SamplerState smp_bilinear : register(s0);
SamplerState smp_nearest : register(s1);

struct VOut {
    float4 pos : SV_Position;
    float2 aux : TEXCOORD0;
};

// The paths, clips and resolves share one vertex shader: `xy` in device
// pixels mapped to clip space by `xf` (scale.xy, offset.zw), and a
// pass-through `aux`.
VOut vs_main(float4 d : POSITION) {
    VOut o;
    o.pos = float4(d.xy * xf.xy + xf.zw, 0.0, 1.0);
    o.aux = d.zw;
    return o;
}

// Batched boxes and textured quads (src/gpu/batch.rs): every input is a
// vertex attribute, QUAD0..QUAD4 (a0..a4), so a run of them is one draw.
// `a0.xy` in device pixels is mapped to clip space by `xf`.
struct QIn {
    float4 a0 : QUAD0;
    float4 a1 : QUAD1;
    float4 a2 : QUAD2;
    float4 a3 : QUAD3;
    float4 a4 : QUAD4;
};

struct QOut {
    float4 pos : SV_Position;
    float2 p : TEXCOORD0;
    float4 a1 : TEXCOORD1;
    float4 a2 : TEXCOORD2;
    float4 a3 : TEXCOORD3;
    float mode : TEXCOORD4;
};

QOut quad_vs(QIn d) {
    QOut o;
    o.pos = float4(d.a0.xy * xf.xy + xf.zw, 0.0, 1.0);
    o.p = d.a0.zw;
    o.a1 = d.a1;
    o.a2 = d.a2;
    o.a3 = d.a3;
    o.mode = d.a4.x;
    return o;
}

// A box: `p` is the device position, `a1` the rect, `a2` the color, `a3`
// the radius and stroke. Coverage is the exact overlap of the pixel with a
// sharp box (or ring), or a signed distance for rounded corners.
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
float4 quad_box(QOut i) {
    float2 c = (i.a1.xy + i.a1.zw) * 0.5;
    float2 h = (i.a1.zw - i.a1.xy) * 0.5;
    float2 p = i.p - c;
    float radius = i.a3.x;
    float stroke = i.a3.y;
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
    return i.a2 * cov;
}

// Boxes only (a batch without a texture): textured quads are left out.
float4 quadbox_ps(QOut i) : SV_Target {
    if (i.mode < 0.5) {
        return quad_box(i);
    }
    return float4(0.0, 0.0, 0.0, 0.0);
}

// Boxes and textured quads: `p` is the texture coordinate, kept within `a2`;
// `a1` the distances to the edges in device pixels (for their coverage);
// `a3` the tint times the opacity (premultiplied). u1.x = nearest.
float4 quad_ps(QOut i) : SV_Target {
    if (i.mode < 0.5) {
        return quad_box(i);
    }
    float2 uv = clamp(i.p, i.a2.xy, i.a2.zw);
    float4 c;
    if (u1.x > 0.5) {
        c = tex0.SampleLevel(smp_nearest, uv, 0.0);
    } else {
        c = tex0.SampleLevel(smp_bilinear, uv, 0.0);
    }
    if (i.mode < 1.5) {
        c.rgb *= c.a;
    }
    c *= i.a3;
    float cov = saturate(min(i.a1.x, i.a1.z) + 0.5) * saturate(min(i.a1.y, i.a1.w) + 0.5);
    return c * cov;
}

// `quad_ps` sampling NV12 (an external image): luma from tex0 (.r), chroma
// from tex1 (.rg: Cb, Cr), turned into RGB by the matrix rows u2..u4 (taking
// [y, cb, cr, 1] to r, g and b). u1.x = nearest.
float4 quad_yuv_ps(QOut i) : SV_Target {
    if (i.mode < 0.5) {
        return quad_box(i);
    }
    float2 uv = clamp(i.p, i.a2.xy, i.a2.zw);
    float4 yuv;
    if (u1.x > 0.5) {
        yuv = float4(tex0.SampleLevel(smp_nearest, uv, 0.0).r,
                     tex1.SampleLevel(smp_nearest, uv, 0.0).rg, 1.0);
    } else {
        yuv = float4(tex0.SampleLevel(smp_bilinear, uv, 0.0).r,
                     tex1.SampleLevel(smp_bilinear, uv, 0.0).rg, 1.0);
    }
    float4 c = float4(saturate(float3(dot(u2, yuv), dot(u3, yuv), dot(u4, yuv))), 1.0);
    c *= i.a3;
    float cov = saturate(min(i.a1.x, i.a1.z) + 0.5) * saturate(min(i.a1.y, i.a1.w) + 0.5);
    return c * cov;
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
