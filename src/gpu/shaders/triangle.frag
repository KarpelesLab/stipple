#version 450

// Solid forma green (0x34, 0xd3, 0x99) so a CI readback can tell the drawn
// triangle apart from the dark cleared background.
layout(location = 0) out vec4 outColor;

void main() {
    outColor = vec4(0x34 / 255.0, 0xd3 / 255.0, 0x99 / 255.0, 1.0);
}
