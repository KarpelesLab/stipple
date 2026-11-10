# Transforms and opacity

Any element can be drawn moved, scaled or rotated, and faded, together with
everything inside it. Neither changes layout: the element keeps its laid-out
box, and its siblings don't move. Pointer input follows the transform, so a
button is pressed where it's drawn.

```rust
use stipple::prelude::*;

fn badge(theme: &Theme, t: f64) -> Element {
    label(theme, "New")
        .padding(Insets::uniform(6.0))
        .fill(theme.palette.primary)
        // Tilted, pulsing about its center, and half faded in.
        .transform(
            Transform::IDENTITY
                .rotate_degrees(-8.0)
                .scale(1.0 + 0.1 * (t * 6.0).sin()),
        )
        .opacity(0.5)
}
```

The `transformdemo` example spins, pulses and fades cards, and has a tilted
button that counts clicks.

## `Transform`

`stipple::geometry::Transform` (also in the prelude) keeps its parts apart so
each can be animated:

| Part | Builder | Meaning |
|---|---|---|
| `translate` | `.translate(dx, dy)` | an offset in logical pixels (adds up) |
| `scale` | `.scale(s)`, `.scale_xy(sx, sy)` | scale factors (multiply) |
| `rotate` | `.rotate(radians)`, `.rotate_degrees(deg)` | clockwise on screen (adds up) |
| `origin` | `.origin(x, y)` | the fixed point of scale and rotation, as fractions of the box; `(0.5, 0.5)`, the center, by default |

Whatever order the builders are called in, a point is scaled, then rotated,
both about the origin, then translated, like CSS's `scale`, `rotate` and
`translate` properties. `Transform::lerp(a, b, t)` interpolates each part, so
tweening from no rotation to `2π` turns a full circle. `Transform::to_affine(bounds)`
gives the matrix (an `Affine`) for an element laid out at `bounds`.

Transforms nest: a child's transform applies inside its parent's.

## Opacity

`Element::opacity(f32)` fades an element and its subtree **as a group**: the
subtree is drawn on its own at full opacity, then blended once. Where children
overlap there's no darker seam, and a panel's fill doesn't show through the
controls on it, which multiplying each child's alpha would do. An element at
opacity `0` isn't drawn at all; like CSS's `opacity: 0`, it still takes
pointer input.

## Pointer input

Every pointer query (taps, focus on click, cursors, context menus, drags, drop
targets, text selection, scroll wheels, embedded viewports) reads the pointer
through the inverse of the transforms above the element. A clipping element
clips in its own transformed space, so a turned scroll area clips to its turned
box. An element scaled to nothing can't be hit.

Positions handed to handlers are in the element's own (layout) space: a slider
turned half a turn fills from the right, as it's drawn, and a drop target's
`DropState::position` is relative to its box as laid out.

Two helpers in `stipple::core` read a tree the same way: `local_point` maps a
window point into a node's layout space, and `node_to_window` gives the matrix
(and the product of group opacities) from a node's layout space to the window.

## Text

Glyphs are drawn from their outlines through the transform, so scaled text is
crisp at its new size and rotated text is rotated, not a stretched or rotated
bitmap. Text is still shaped (and hinted, where it is) at its laid-out size,
so very large scales show the small size's spacing.

## How it draws

`Scene::push_layer(affine, opacity)` / `pop_layer()` bracket a subtree;
`paint` emits one around every element with a transform or an opacity below 1.
On the CPU (every platform's default path), layers are `oxideav` groups with a
transform and an opacity, which the rasterizer composites through an offscreen
image when the opacity is below 1. Layers record `DrawCmd::PushLayer` /
`PopLayer` for other backends.

The hover highlight, focus ring and caret, and the drop-target highlight are
drawn through the transforms (and opacity) of the element they belong to.

Damage follows the transform: a change inside a transformed element repaints
the box around where it's drawn, and a change of transform or opacity
repaints the subtree where it was and where it is now.

## Not covered yet

- The GPU scene renderer (`stipple::gpu::render_scene`, experimental) doesn't
  draw layers natively: a scene with any transform or translucent layer is
  rasterized on the CPU and only composited on the GPU.
- Embedded-content viewports: the placeholder transforms, but registered
  content is blitted unscaled and unrotated at the moved top-left corner.
- The input method's composition underline (preedit) and the caret position
  reported to the input method don't follow a transform yet.
- Transforms are static per build: animate them by building with new values
  each frame (see [animation.md](animation.md)); implicit transitions are
  their own roadmap item.
- No skew or 3D (perspective) transforms.
- The accessibility tree reports elements' laid-out bounds, not their
  transformed ones.
