#version 450

// A self-contained triangle: positions come from gl_VertexIndex (no vertex
// buffers), so the pipeline needs no vertex input state. Vulkan clip space has
// y pointing down; this triangle is centered, apex up.
void main() {
    vec2 positions[3] = vec2[](
        vec2( 0.0, -0.6),
        vec2( 0.6,  0.6),
        vec2(-0.6,  0.6)
    );
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
}
