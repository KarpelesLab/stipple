# Stipple — Roadmap

A cross-platform UI library and toolkit in Rust. Stipple draws **beautiful, fully
themeable** interfaces on Linux, macOS, Windows, Android, iOS, and the web:
pixel-identical across platforms under its own themes, or native under
`Theme::native()`. It stays **as close to the OS as possible** and depends on
**as little third-party code as possible**.

Stipple does not reinvent 2D content rendering — it builds on the
[`oxideav`](../oxideav-workspace) workspace, a mature pure-Rust media stack that
already provides the entire "vector + text + images → pixel buffer" pipeline.
Stipple adds everything *around* that: native windowing and input per OS, getting
the buffer onto the screen, and the declarative UI toolkit itself.

---

## 0. Implementation status

> The toolkit was merged from 11 crates into the single `stipple` crate, so
> `stipple::core` was once `stipple-core`, and so on; the wasm target that was
> `stipple-web` is now `examples/webdemo`. Entries below describe the work as it
> landed.

> Living checklist — updated as work lands. ✅ done · 🚧 in progress · ⬜ not started.

- ✅ **Workspace + 10 crates** scaffolded (edition 2024, rust 1.88 — floor set
  by the oxideav stack via `oxideav-png` → `compcol`), CI (lint +
  MSRV), `stipple::geometry`.
- ✅ **Rendering seam** (`stipple::render`): `Scene` → oxideav `VectorFrame` →
  `oxideav-raster` → `Pixmap`; `Surface` GPU-ready boundary.
- ✅ **Software rasterization** path verified end to end (off-screen PNGs).
- ✅ **Layout** (`stipple::layout`): flex/box solver. **Paint**: `Element` IR +
  measure/layout/paint passes.
- ✅ **Reactivity MVP** (`stipple::core`): retained `LayoutNode` tree, `hit_test`,
  `Cx` handler registry, `on_tap` dispatch → state mutation (the `clickdemo`
  example drives clicks through the real path).
- ✅ **Theming** (`stipple::style`) and **animation primitives** (`stipple::anim`).
- ✅ **Text rendering** via `oxideav-scribe`: `Font` (load + measure), glyph
  shaping → scene nodes with per-run color; `Text` element threaded through
  layout (intrinsic sizing) and paint; `label`/`button_labeled` widgets.
- ✅ **Keyboard + focus**: `FocusId`, click-to-focus, Tab traversal, `on_key`
  handlers, `KeyInput` routing; editable `text_field` widget (the `textinput`
  example types into a focused field through the real dispatch path).
- ✅ **Pointer drag**: `DragId` + `on_drag` handlers, press/move/release routing
  with fractional position; `slider` widget driven by drag.
- ✅ **Widgets** (scaffold): panel, row/column, button, labeled button, label,
  divider, swatch, spacer, text field, checkbox, switch, slider.
- ✅ **Golden-pixel conformance** tests (font-free, deterministic) over the
  headless path — the cross-platform "pixel-identical" guarantee, enforced in
  CI; native backends must match the same samples.
- ✅ **Native X11 backend** written directly against the wire protocol (pure
  sockets, no deps): connect + auth, window create/map/resize/close, present,
  pointer + raw key events. **CI-verified**: a `Visual` workflow runs the
  `window` example under Xvfb and screenshots the result
  (`docs/screenshots/stipple-x11.png`). `App::run` selects X11 when `$DISPLAY` is
  set, else headless.
- ✅ **Native Wayland backend** written directly against the wire protocol (no
  `libwayland`/`wayland-client`): connects to `$WAYLAND_DISPLAY`, binds
  `wl_compositor`/`wl_shm`/`xdg_wm_base` via the registry roundtrip, creates an
  `xdg_toplevel`, runs the `xdg-shell` configure/ack handshake, and presents the
  software `Pixmap` through a `memfd`-backed `wl_shm` buffer (the fd passed with
  a raw `sendmsg` `SCM_RIGHTS` control message). Backend selection prefers
  Wayland, then X11, then headless. **CI-verified** under headless `sway` +
  `grim` (`docs/screenshots/stipple-wayland.png`).
- ✅ **Wayland input** (`wl_seat`): binds the seat and lazily creates the
  keyboard/pointer once a `capabilities` event advertises them, re-acquiring the
  keyboard if the capability toggles (calling `get_keyboard` unconditionally is
  a protocol error on a device-less headless seat). Keys decode through the
  compositor's **xkb keymap** — captured as an fd via `recvmsg`/`SCM_RIGHTS`,
  `mmap`-ed, and parsed (keycode → keysym) — so text works for any layout, with
  a layout-independent evdev table as fallback. `wl_pointer` motion/buttons
  decode `wl_fixed` coordinates and BTN_LEFT/RIGHT/MIDDLE. Mappings are
  unit-tested; **CI-verified end to end** under headless `sway` — `wtype` types
  "stipple wl" into a focused field (`docs/screenshots/stipple-wayland-input.png`).
- ✅ **X11 MIT-SHM fast present**: when the server advertises MIT-SHM, frames go
  through a System V shared-memory segment the server maps directly, and
  `ShmPutImage` blits only the `Surface` damage rectangles — so an incremental
  repaint transfers no pixels over the socket. Set up before mapping
  (QueryExtension → shmget/shmat → ShmAttach → sync), with `IPC_RMID` auto-
  cleanup and a `PutImage` fallback when the extension is absent. The shm
  syscalls are the X11 backend's only `unsafe`/FFI. **CI-verified** under Xvfb
  (logs confirm `shm=true`; screenshots render correctly).
- ✅ **Native Windows backend** over raw Win32 FFI (`user32`/`gdi32`/`kernel32`,
  no `windows` crate): window create/show, `StretchDIBits` present. **CI-verified**
  — the Visual workflow's Windows job runs the example on the runner's desktop
  and screenshots it (`docs/screenshots/stipple-windows.png`). Input + live resize
  are follow-ups.
- ✅ **Native macOS backend** over raw `objc_msgSend` Cocoa FFI (no
  `objc`/`cocoa` crate): `NSWindow` + a custom `NSView` whose `drawRect:` blits
  a `CGImage` (CTM-flipped for top-left origin). **CI-verified** —
  `docs/screenshots/stipple-macos.png`.
- ✅ **Desktop trio native + CI-screenshot-verified**: X11, Win32, and Cocoa
  backends each render the demo on their own OS runner. The build matrix also
  compiles the whole workspace on all three.
- ✅ **Input on X11 + Win32** (pointer move/buttons/wheel, keys, text, resize);
  X11 resolves keysyms (`GetKeyboardMapping`) to text + editing keys and grabs
  focus so it works WM-less. The App re-renders + presents on every input.
  **Interaction CI-verified**: `xdotool` clicks a counter `0 → 2` and types
  `Stipple!` into a focused field, both screenshot-confirmed.
- ✅ **Cocoa input + live resize**: a manual `nextEventMatchingMask:` loop
  routes `NSEvent`s (mouse y-flipped, keys) and polls view bounds for resize.
  **Input CI-verified** — `cliclick` drives the counter `0 → 2`
  (`docs/screenshots/stipple-macos-clicks.png`). Desktop trio is now interactive
  (X11 + macOS pointer/keyboard screenshot-verified; Win32 build-verified).
- ✅ **Web target (Phase 5), interactive**: `stipple-web` (wasm32) holds a
  persistent `App` and a small C ABI; a hand-written JS shim (no wasm-bindgen)
  uploads a font, blits the `Pixmap` to a `<canvas>` via `putImageData`, and
  forwards canvas mouse/text events. **CI-verified** — headless Chrome loads
  the font, self-drives two clicks, and the screenshot shows "Clicks: 2"
  (`docs/screenshots/stipple-web.png`): text + input both work on web.
- ✅ **Focus ring + text caret**: the App overlays a primary-colored ring on
  the focused element and a caret at the end of a focused text field's text
  (CI-screenshot-verified via the X11 textinput job).
- ✅ **Hover highlight**: the App tracks the hovered tappable element and
  overlays a translucent highlight matching its shape, re-presenting on change
  (CI-verified — `xdotool` hovers one of two buttons, which lights up).
- ✅ **GPU present path (Phase 6 seam)**: `stipple::gpu` routes the software
  `Pixmap` through raw EGL + OpenGL ES 2 (texture upload → fullscreen-quad
  shader → offscreen FBO → readback). **CI-verified** on Mesa software GL
  (`docs/screenshots/stipple-gpu.png`). v1 composites the CPU frame on the GPU;
  GPU-native scene tessellation and Vulkan/Metal/D3D/WebGPU are future work.
- ✅ **Theme engine + customization**: a semantic `Palette` (roles, interaction
  states, status, overlays), a `Typography` scale, and a `Theme` builder —
  `with_accent` (recolor + derive hover/active + pick a readable on-color),
  `with_radius`, `with_font_size`, `high_contrast`. Widgets gained `heading`
  and `button_variant` (Primary/Secondary/Ghost/Danger); the App's focus ring
  and hover overlay read theme tokens. **CI-verified**: the `themegallery`
  example renders one card under four themes, montaged into
  `docs/screenshots/stipple-themes.png`.
- ✅ **Frame reconciliation (damage diffing)**: `stipple::core::diff_trees`
  compares the previously-presented `LayoutNode` tree against the freshly built
  one and returns a `Damage` region (changed rectangles, coalesced). The `App`
  retains the on-screen frame as a baseline and limits each present to the
  damaged region via the `Surface` damage seam — a state change repaints only
  what moved (expose/resize still force a full present). Unit-tested in
  `stipple::core` (localized/full/none cases) and `stipple` (incremental App frames).
- ✅ **Area-based partial rasterization**: damage diffing chose *what* to
  present, but the renderer could still only paint the whole canvas and any
  hover/focus change forced `Damage::Full` — so a pointer move crossing a
  tappable re-ran a full tree rebuild, full-window CPU rasterize, and full
  upload (multi-second pointer lag). Two new seams close the gap.
  `stipple::render`: `SoftwareRenderer::render_region` rasterizes only a logical
  sub-rect (via the frame's view box) into a caller-sized buffer, and
  `Pixmap::blit` composites it into a retained full-window pixmap — a region
  render is pixel-identical to the full render within that rect. `stipple`:
  `Pane::take_damage` localizes hover/focus changes to the affected element
  rects (focus inflated to cover the ring) instead of `Damage::Full`, falling
  back to Full only for overlays, resize, first frame, or a missing node; the
  run loop computes damage *before* rasterizing — skipping the rasterize
  entirely when nothing changed — and, on localized damage, re-rasterizes just
  those rects into the retained pixmap and uploads only them. A hover move now
  repaints two small button rects and no-op events do zero raster/upload work.
  (The GPU `frame_renderer` path keeps the full-frame route.) Unit-tested
  (`render_region` vs full, blit offset, hover-change localized damage) and
  exercised by the `visual-calculator` CI job.
- ✅ **Subtree memoization** (`Cx::memo`): `cx.memo(key, build)` returns the
  previous frame's `Element` for an unchanged `key`, skipping the build closure
  so unchanged static branches aren't rebuilt. The closure gets no `Cx` (so a
  memoized subtree can't register handlers whose ids would desync); the `App`
  threads the cache across frames and evicts untouched keys. Unit-tested
  (build-once-per-key, rebuild-on-change, eviction).
- ✅ **Caret-aware text editing**: a single-line `EditBuffer` with a
  UTF-8-boundary-safe caret (insert / backspace / delete at the caret;
  left / right / home / end motion; `apply(KeyInput)`). The `Element`/
  `LayoutNode` IR carries an optional caret byte index; `paint_focus` draws the
  caret bar at that index (prefix-measured), and the reconciler treats a caret
  move as damage. `text_editor` renders it; **CI-verified** on X11 (type
  "Stipple", arrow-left ×2, insert "XY" → "StippXYle" with a mid-string caret).
- ✅ **Text selection**: `EditBuffer` gains a selection anchor — Shift+arrows /
  Home / End extend the selection, plain motions collapse it, typing/delete
  replace it, and Ctrl/Cmd+A selects all (UTF-8-boundary-safe). The X11 backend
  reports key modifiers (shift/ctrl) and Home/End/Delete; `map_key` maps
  Shift+motion to `Select*`. The IR carries a selection byte range, `paint_focus`
  draws a themed translucent highlight behind the selected glyphs, and the
  reconciler treats a selection change as damage. **CI-verified** on X11
  (`docs/screenshots/stipple-x11-selection.png`).
- ✅ **Multi-line text rendering**: `Font::measure` and `Scene::fill_text` split
  on `\n` — measure returns the widest line's width and line-height × line count
  (a trailing newline adds an empty line), and `fill_text` places each line
  dropped by one line height. Editable fields stay single-line (caret/selection
  math unchanged). **CI-verified** — the `window` example's two-line caption
  (`docs/screenshots/stipple-x11.png`).
- ✅ **Pointer-drag text selection**: `stipple::core::caret_index_at` resolves a
  pointer x to the nearest caret byte index (prefix-measured); a `TextPosId` /
  `on_text_pos` handler routes presses (place caret) and drags (extend
  selection) through `text_pos_at`/`find_text_pos`. `EditBuffer` gains
  `place_caret`/`extend_to`; `text_editor` takes a `&mut EditBuffer` accessor and
  wires keyboard + pointer together. **CI-verified** on X11 — mouse drag selects
  the leading characters (`docs/screenshots/stipple-x11-dragselect.png`).
- ✅ **Word-wrapping**: `Font::wrap` greedily wraps text to a max width (breaking
  at spaces, honoring hard newlines, shaping each word once); a `wrap` flag on
  text elements wraps to the laid-out content width in both measure (growing
  height) and paint. New `paragraph` widget. **CI-verified** — the `window`
  example's paragraph wraps across three lines (`docs/screenshots/stipple-x11.png`).
- ✅ **Multi-line editing**: `EditBuffer` gains Enter→newline, Up/Down (keeping
  the byte column), and line-aware Home/End (plus the matching `Select*`).
  `paint_focus` positions the caret on its line and draws the selection as one
  rectangle per spanned line; `caret_index_at` takes a `Point` (line from y,
  column from x). New `text_area` widget. **CI-verified** on X11 — three typed
  lines with a cross-line selection (`docs/screenshots/stipple-x11-multiline.png`).
- ✅ **mobile portability**: the whole stack **cross-compiles for Android
  (`aarch64-linux-android`) and iOS (`aarch64-apple-ios`)** — oxideav is pure
  Rust, so no NDK is needed. **CI-verified** (the `mobile` job builds the
  umbrella crate for both). A native **iOS UIKit backend** (raw `objc_msgSend` +
  UIKit/CoreGraphics, no `objc`/`uikit` crate — the same approach as the macOS
  backend) now exists: `UIApplicationMain` boots a hand-built `StippleAppDelegate`
  that creates a `UIWindow` hosting a custom `UIView` whose `drawRect:` blits the
  software `Pixmap` as a `CGImage`. **Runtime-verified on the iOS simulator**:
  the CI `visual-ios` job bundles the `window` example into a `Stipple.app`, boots
  a simulator, launches it, and reads back the backend's runtime marker
  (`window shown, framebuffer 640x480`) from the app container — proving
  `UIApplicationMain` booted, the delegate built the `UIWindow`, and the Stipple
  handler rendered a frame on a real iOS surface. An **Android `ANativeWindow`**
  present path exists too — `present_to_native_window` blits the software
  `Pixmap` to the activity's surface via the NDK C ABI
  (`ANativeWindow_setBuffersGeometry`/`_lock`/`_unlockAndPost` from `libandroid`,
  no `ndk`/`ndk-glue` crate). It is reached through a hand-written
  `ANativeActivity_onCreate` (the `androiddemo` cdylib, `libstipple_android.so`)
  that registers an `onNativeWindowCreated` callback, builds a Stipple `App` at the
  surface size, and blits a rendered frame. **Runtime-verified on the Android
  emulator**: the CI `visual-android` job hand-packages a signed debug APK
  (`aapt` + `zipalign` + `apksigner`, no Gradle), installs it, and confirms via
  `logcat` that the `NativeActivity` presented a frame
  (`Stipple Android: window 320x640 presented=true`). Both mobile backends thus
  render on a real device surface — iOS on the simulator, Android on the
  emulator. (Touch input + the full lifecycle event loop are follow-up depth.)
- ✅ **a11y foundation**: `stipple::core::a11y::accessibility_tree` builds a pruned
  semantic `AccessNode` tree (Window/Group/Button/TextField/Text roles, names,
  focus) from the layout tree; `App::accessibility_tree()` exposes it.
  Unit-tested. On **Linux** the tree is wired to the OS accessibility API:
  `stipple_platform::a11y` is a **hand-written D-Bus client + server** (no
  `zbus`/`dbus`/`libdbus` — just a `UnixStream` and the wire protocol, like the
  X11/Wayland backends) that runs the SASL `EXTERNAL` handshake, calls `Hello`,
  reaches the **AT-SPI** bus via `org.a11y.Bus.GetAddress`, claims a name, and
  **serves the accessibility tree over `org.a11y.atspi.Accessible`** —
  hand-marshalling method returns, properties, and variants. **CI-verified**: a
  `dbus-send` client reads the Stipple UI's root as AT-SPI role 27 (Window→FRAME),
  `ChildCount` 2, and `Name` "Stipple" from our server. The **macOS** and
  **Windows** bridges are wired too, each hand-written with no helper crate: the
  Cocoa `StippleView` overrides the **NSAccessibility** protocol (an accessible
  `AXGroup` with the window's label), and `stipple_platform::uia` is a by-hand
  **UI Automation** `IRawElementProviderSimple` COM object (vtable + `IUnknown`
  refcounting; `GetPropertyValue` answers control-type and a `VT_BSTR` name).
  Both are **CI-verified** through their real OS dispatch (objc / COM vtable) on
  the macOS and Windows runners.
  **macOS full element tree:** the bridge now vends the *whole* hierarchy, not
  just the root. The App maps each frame's `AccessNode` tree into a
  platform-neutral `stipple_platform::A11yNode` and pushes it through
  `Window::set_accessibility_tree`; the `StippleView`'s `accessibilityChildren`
  builds native `NSAccessibilityElement`s (role/label/frame/parent) recursively
  from it — Stipple roles mapped to `AXButton`/`AXTextField`/`AXStaticText`/
  `AXGroup`. **CI-verified**: the `visual-macos` job recursively walks
  `-accessibilityChildren` through real objc dispatch and reads nested
  `AXStaticText` descendants (e.g. "Welcome to Stipple") under the window root.
  **Windows full element tree:** the UIA bridge likewise vends the whole
  hierarchy now. `stipple_platform::uia::UiaTree::build` turns an `A11yNode` tree
  into one hand-written COM provider per node — a combined
  `IRawElementProviderFragmentRoot` vtable (IUnknown → Simple → Fragment →
  FragmentRoot) answering `Navigate` (parent/sibling/child), `GetRuntimeId` (a
  `SAFEARRAY`), `BoundingRectangle`, `get_FragmentRoot`, and `GetFocus`; the
  Win32 `WM_GETOBJECT` handler returns the root via
  `UiaReturnRawElementProvider`. **CI-verified**: `uiademo` walks the tree
  through `Navigate` (FirstChild/NextSibling) over the real COM vtable and reads
  a nested Text (50020) and a Button (50000) under a Group, plus the focused
  field via `GetFocus`. Both desktops share the same plumbing: the App maps each
  frame's `AccessNode` tree into the neutral `stipple_platform::A11yNode` and
  pushes it through `Window::set_accessibility_tree`.
  **Linux full element tree:** the AT-SPI server matches — `serve_atspi_tree`
  exposes every node of an `AtspiTree` as its own D-Bus object (the root at
  `/org/stippleui/a11y`, the rest at `{root}/{i}`), answering `GetChildren` /
  `GetChildAtIndex` (`(so)` object references), `GetRole`, `GetIndexInParent`,
  and the `Name` / `Description` / `ChildCount` / `Parent` properties — the exact
  surface a screen reader walks. **CI-verified**: under `dbus-run-session`,
  `dbus-send` navigates the real Stipple UI's tree from the root through
  `GetChildren` to a nested "OK" label two levels down, and reads each node's
  role/name/parent. **All three desktops now expose the full hierarchy** (AT-SPI,
  NSAccessibility, UIA), each hand-written with no helper crate. (Remaining a11y
  depth: raising change events to live clients — `UiaRaiseStructureChangedEvent`,
  AT-SPI `children-changed` — on each tree swap. Bounds are now screen
  coordinates on Windows and macOS. On Linux the AT-SPI server isn't connected
  to app windows nor registered with the AT-SPI registry yet, so Orca can't
  see Stipple apps; see the 7f items.)
- ✅ **GPU-native drawing**: a live stipple `Scene` renders entirely on the GPU.
  The `Scene` records structured `DrawCmd`s; `stipple::gpu::render_scene` turns box
  primitives (sharp/rounded fills + stroked borders) into geometry shaded by a
  rounded-rect signed-distance-field GLES2 shader, and composites each text run
  as an alpha-blended glyph-coverage mask (not by compositing a whole CPU
  pixmap). **CI-verified** on Mesa: the box/text primitives
  (`docs/screenshots/stipple-gpu-rects.png`) and the actual widget-tree `Scene`,
  whose text is drawn from a packed **per-glyph atlas** (each unique glyph
  rasterized once into one shared texture; repeats reuse the slot)
  (`docs/screenshots/stipple-gpu-scene.png`). A complete **raw Vulkan render
  pipeline** (no `ash`/`vulkano` — just `libvulkan` + hand-written C structs)
  now runs end to end: `VkInstance` + physical-device enumeration → logical
  device + graphics queue → a `DEVICE_LOCAL` color image + memory → image view +
  single-attachment render pass + framebuffer → a fenced command-buffer submit
  that clears the image, copies it to a `HOST_VISIBLE` buffer, and reads it back
  to the CPU → and finally a full `VkGraphicsPipeline` with two committed
  **SPIR-V** shader modules that `vkCmdDraw`s a triangle. **CI-verified** on Mesa
  lavapipe: the read-back clear is stipple blue (`docs/screenshots/stipple-gpu-vk.png`)
  and the shader-drawn triangle's center pixel is stipple green
  (`docs/screenshots/stipple-gpu-vk-tri.png`). The **Metal** (macOS) and
  **Direct3D 11** (Windows) backends now match it: each is hand-written raw FFI
  (no `metal`/`objc`/`windows` crate — `objc_msgSend` by hand for Metal, COM
  vtable slots by hand for D3D), creates a device, clears a render target to
  stipple blue, and draws a triangle through a real shader pipeline (a runtime-
  compiled `.metal` library for Metal; `D3DCompile`d HLSL for D3D), reading each
  frame back to the CPU. **CI-verified** on the macOS runner's Metal device and
  the Windows runner's **WARP** software rasterizer: both read back stipple blue for
  the clear and stipple green at the triangle's center pixel. A **WebGPU** backend
  completes the set: a hand-written WGSL triangle (no `wgpu`/bindgen — the
  sanctioned web exception) drawn through the browser's WebGPU API, **CI-verified**
  in headless Chrome on the bundled **SwiftShader** Vulkan ICD (the screenshot's
  center pixel is stipple green). All four GPU backends — Vulkan, Metal, D3D11, and
  WebGPU — thus render a real shader pipeline off-screen and read it back.
  Finally, GPU rendering is **wired into the live on-screen present path**:
  `App::render_with` swaps the software rasterizer for any
  `Scene → Pixmap` renderer, and the `gpuwindow` example drives a real X11 window
  whose every frame is produced by `stipple::gpu::render_scene` (GLES SDF + glyph
  atlas) and presented through the platform `Surface`. **CI-verified** under
  Xvfb + Mesa: the window paints a full GPU-rendered frame with no software
  fallback (`docs/screenshots/stipple-gpu-window.png`). (A zero-copy swapchain
  present — binding the GPU surface directly to the window instead of reading
  back to a `Pixmap` — remains a future optimization.)

### Toolkit surface buildout (Phase 3 maturity, in progress)

- ✅ **Scroll containers**: a `clip` primitive in the `Scene` (nested oxideav
  clipped groups, with `DrawCmd::PushClip`/`PopClip` for a future GPU scissor),
  a `ScrollId` handler kind, and a `scroll(cx, height, content)` widget. Content
  taller than the viewport lays out at natural size, is clipped to the viewport,
  and wheel events adjust a per-container offset (re-applied + clamped each frame
  by `apply_scroll`). **CI-verified** (X11/Xvfb): `xdotool` wheel-scrolls a tall
  list and the before/after screenshots differ while staying clipped.
- ✅ **Overlays**: a floating layer drawn above the main tree — the view declares
  overlays via `Cx::overlay` (an `OverlaySpec` with an `Anchor` + `modal` flag),
  and the app composes them with the main tree under one synthetic root (a
  full-window catcher behind each — a dark scrim for a modal, an invisible
  click-catcher for a non-modal — carries the dismiss action), so the existing
  hit-test/paint/scroll routing treats overlays as topmost for free. Widgets:
  `menu`/`menu_item`/`open_menu` (dropdown), `open_dialog` (modal + scrim),
  `tooltip`, `tabs` (segmented control), plus `radio`/`progress_bar`/`spinner`.
  **Right-click context menus** are wired through a core `on_context` handler
  (a new `ContextId`, carrying the click position) that the app routes on the
  secondary button to open a menu at the cursor. **CI-verified** (X11): opening
  the dropdown changes the frame and the modal's scrim darkens it; the tabs demo
  switches the body on a tab click and opens a context menu on right-click.
- ✅ **Clipboard**: copy/cut/paste in text fields via `Ctrl`/`Cmd`+`C`/`X`/`V`
  (`map_key` → `KeyInput::Copy`/`Cut`/`Paste`, handled by `EditBuffer`). An
  in-process mirror (`stipple::core::clipboard`) makes copy/paste work in-app and
  headless; the app syncs it with the OS clipboard around each op through the
  `Window::clipboard`/`set_clipboard` seam. The X11 backend implements that seam
  by owning the `CLIPBOARD` selection and answering `SelectionRequest`
  (`UTF8_STRING`/`STRING`/`TARGETS`); the X11 keyboard path now delivers
  `Ctrl`/`Meta`+printable as an `Event::Key` shortcut rather than text.
  **CI-verified** (X11): the field text doubles after copy+paste, and `xclip`
  reads the copied text back off the OS `CLIPBOARD` selection.
  Pasting other applications' selections, the primary selection and rich
  formats followed (see 7d).
- ✅ **Native file dialogs**: `stipple::platform::dialog` (`open_file`/`save_file`/
  `pick_folder`) backed by each OS's own picker. On Linux it drives
  `org.freedesktop.portal.FileChooser` over the hand-written D-Bus client (the
  `a11y` module gained `call_with_body`, `add_match`, signal emit, an `a{sv}`
  marshaller, and a full type-skipping `ua{sv}` Response parser) — works in
  sandboxes and across desktops without GTK/Qt. macOS/Windows/web wire their
  native panels per backend. **CI-verified** (Linux): a built-in mock portal
  (owns `org.freedesktop.portal.Desktop`, answers `OpenFile`, emits the
  `Response` signal) round-trips a canned `file://` URI back through
  `dialog::open_file` to a `PathBuf`, inside `dbus-run-session`.
- ✅ **True OS multi-window**: the parent `App<S>` owns the global state and a
  `Vec<Pane<S>>` — one pane per OS window, each with its own view onto the shared
  state plus its own tree/focus/hover/scroll/damage. `App::open_window(attrs,
  view)` registers additional windows; `App::run` opens each as a real native
  window and routes every event to the pane that owns the window it arrived on,
  ending when the last window closes. The X11 backend drives multiple top-level
  windows on one connection (a shared `WindowReg` adopts windows opened mid-loop
  via the new `Window::open_window`/`close_window` seam; events route by XID),
  and `WindowAttributes::with_position` lays them out. **CI-verified** (X11): the
  multiwindow example opens a red and a blue window side by side and the root
  screenshot confirms both painted, each its own color.
- ✅ **Runtime windows, titles and geometry** (#18–#20): Wayland (one
  `xdg_toplevel` each), Windows (`CreateWindowExW`) and macOS (another
  `NSWindow`) drive several windows like X11. `AppHandle::open_window(attrs,
  view) -> WindowId` / `close_window(id)` open and close them from any thread
  while the app runs; `Cx::window_id` lets one view serve many windows;
  `App::exit_on_last_window_closed(false)` keeps the app running with none;
  `WindowAttributes::with_focus`. Backends wake every window, and an event
  that changes the state in one window re-renders the others. Titles: real on
  every desktop backend (`_NET_WM_NAME` + `WM_NAME`, `xdg_toplevel.set_title`,
  `SetWindowTextW`), declarative with `Cx::window_title` or through
  `WindowRequest::Title`. Geometry: `WindowRequest::{Position, InnerSize,
  ResizeIncrements, Minimized}`, `WindowInfo` (position, inner/outer size,
  scale, maximized/minimized/fullscreen, work area) from `Cx::window_info`,
  `AppHandle::window_info` and `App::on_window_change`, with
  `Event::WindowChanged`. **CI-verified** on X11 (xprop/xdotool) and Wayland
  (swaymsg); the Windows and macOS jobs check values read back from the OS.
- ✅ **Embedded GPU content (browser viewport)**: toward using Stipple as a web
  browser's UI, the chosen model is **Stipple-as-compositor with shared GPU
  textures** (the Chromium model): a separate, sandboxed content process renders
  the page into a GPU texture, exports it as a `dma-buf` (Linux) / `IOSurface`
  (macOS) / shared D3D handle (Windows), and Stipple imports it as a texture and
  composites it into a viewport element — so chrome (menus, tabs, dropdowns)
  draws over/around the page and input is routed by Stipple and forwarded to the
  content process. **Phase A (done):** `stipple::gpu` can export a GL texture as a
  `dma-buf` and re-import it (`EGL_MESA_image_dma_buf_export` /
  `EGL_EXT_image_dma_buf_import`), proving the zero-copy handoff; the
  `dmabuftest` spike self-tests the round-trip (surfaceless, run on a GPU box;
  CI build-verifies and probes extension availability under software Mesa).
  **Confirmed PASS on real GPU hardware** — the key subtlety: exported buffers
  are tiled, so the importer must echo the export's **DRM format modifier**
  (`EGL_DMA_BUF_PLANE0_MODIFIER_LO/HI_EXT`) or the image is incomplete; and an
  imported dma-buf texture is **sample-only** (not color-renderable), which is
  exactly how the compositor uses it.
  **Phase B (transport — done):** the buffer-handoff plumbing for the chosen
  **DRI3 + Present over raw X11** path is built and hardware-verified.
  **B.1** `stipple_platform::scm` (Linux) — `send_with_fds`/`recv_with_fds` built
  directly on `sendmsg`/`recvmsg` with a hand-assembled `SCM_RIGHTS` control
  message (no `nix`/`libc`, matching the rest of the platform layer); the same
  primitive carries a frame's dma-buf fd to the X server and the page-buffer fd
  from the sandboxed content process. Socketpair round-trip unit-tested (a real
  open description is transferred, not a byte copy); runs locally, no GPU
  needed. **B.2** X11 `dri3_open` — negotiates the DRI3 extension over the raw
  socket and performs `DRI3Open`, whose reply carries the **server's DRM device
  fd** as ancillary data (received via `scm`, handling the fd-bearing reply
  arriving split from its data); binding our GPU/EGL context to that exact
  device is what lets the server import the dma-bufs we render. Request encoding
  unit-tested; hardware-gated (Xvfb has no DRM). **B.3** `stipple::gpu` EGL-via-GBM
  — `gbm_create_device(drm_fd)` → `eglGetPlatformDisplay(EGL_PLATFORM_GBM)` →
  shared context, so we render on the **same GPU the X server uses**;
  `dmabuf_self_test_on_device(drm_fd)` runs the export/import/sample round-trip
  on that exact device (also fixes `EGL_SURFACE_TYPE` config selection per
  platform). **Confirmed PASS on real GPU** against a render node
  (`/dev/dri/renderD129`). The `dri3probe` example chains it end to end:
  `dri3_open_drm_fd()` → GBM-bind EGL to that fd → dma-buf round-trip, proving
  on real GPU + X hardware that the server's GPU can export and re-import the
  buffers the compositor will hand it (`cargo run -p dri3probe
  --features stipple/gl`).
  **Phase C (UI integration — done):** the toolkit-side compositor surface is
  wired and CI-verified on the software path (no GPU needed, so it runs under
  Xvfb). A **viewport element** (`Element::viewport(ViewportId)` /
  `widgets::viewport`) reserves a rect carried through measure/layout/paint as
  `NodeContent::Viewport`, painting a placeholder and recording a
  `DrawCmd::Viewport`. The `App` holds a content registry
  (`with_viewport_content` / `set_viewport_content`) and **composites** each
  viewport's externally-rendered pixels over its placeholder after rasterize
  (`Pane::composite_viewports`; `collect_viewports` locates the rects) — the CPU
  analog of a GPU backend sampling the imported texture into the rect.
  **Input forwarding**: `App::on_viewport_input` routes pointer
  press/release/move, wheel, and (while a viewport holds input focus) keys that
  land in a viewport to a sink as `ViewportEvent`s in viewport-local coordinates
  — what a real build hands the content process; pressing the content grabs
  keyboard focus. CI-verified by the `viewportdemo` (a cyan/magenta checkerboard
  composited into a 320×240 viewport; a click is screenshot-confirmed to forward
  a local-coord press).
  **Phase D (zero-copy present — wire layer):** the X11 protocol for flipping a
  GPU frame to the window with no readback is in place and tested. DRI3
  `PixmapFromBuffers` (minor 7) wraps a rendered dma-buf — geometry + format +
  up to 4 planes' stride/offset + the DRM format modifier the import must echo,
  the plane fds passed as SCM_RIGHTS ancillary data — and Present `PresentPixmap`
  (minor 1) flips it; both request encoders are unit-tested for exact wire layout
  (`stipple_platform::backend::x11::{pixmap_from_buffers_request,
  present_pixmap_request}`, over a public `DmabufImage`). The **Present extension
  negotiation** (`present_probe`) needs no GPU, so it's **CI-verified under
  Xvfb** (the `dri3-present` job asserts `Present X.Y available`); the dma-buf
  Pixmap it flips still needs real GPU hardware. The **GPU side of the producer**
  is also in place: `stipple_gpu::export_dmabuf_on_device` renders a frame on the X
  server's device (bound via GBM from the `DRI3Open` fd) and exports it as a
  single-plane dma-buf, returning a `DmabufExport` descriptor (fd + stride/offset
  + modifier + fourcc) whose fields map 1:1 onto the `DmabufImage` the
  `PixmapFromBuffers` encoder consumes — so producer (export) and transport
  (encoders) now meet.
  **Phase E (content process — CPU shm dual, done):** the full compositor
  architecture — a *separate* content process whose pixels are shared with the
  UI process and composited into a viewport, with input forwarded back — is
  implemented end to end over the CPU shared-memory path, the dual of GPU
  dma-buf (the path used when GPU sharing is unavailable), so it runs with **no
  GPU and is CI-verified headlessly**. `stipple_platform::shm::SharedBuffer` is a
  `memfd` mapped `MAP_SHARED`, shareable across processes by passing its fd over
  a socket (`scm`) and re-mapping in the peer. The `contentproc` example spawns a
  real content process (the socket inherited on fd 3 via `pre_exec`/`dup2`); the
  content process renders into a `SharedBuffer` and hands the UI its fd
  (`SCM_RIGHTS`); the UI maps the same memory, composites it into a viewport
  (checked via `App::render_once`), forwards a pointer press over the socket, and
  the content process redraws a marker into the shared buffer — which the UI then
  sees. **CI-verified** (the `content-process` job asserts `RESULT: PASS`). So
  process separation, fd-over-socket transport, cross-process compositing, and
  input forwarding are all proven; the GPU `dma-buf` variant swaps the shared
  buffer for a GPU texture. The content process is also **sandboxed**:
  `stipple_platform::sandbox::restrict()` installs a seccomp-BPF filter
  (`NO_NEW_PRIVS` + a hand-written BPF program) that makes
  `socket`/`connect`/`execve`/`execveat`/`ptrace` fail with `EPERM` while leaving
  the existing IPC fd + shared memory usable — so a compromised content process
  can't open the network or exec; `contentproc` applies it and **CI asserts** a
  new `socket()` is blocked while the loop still completes.
  **GPU present, end to end (implemented):** the on-window zero-copy present is
  now wired —
  `stipple_platform::backend::x11::dri3_present_dmabuf_self_test` connects, creates
  + maps a window, then DRI3 `PixmapFromBuffers` (plane fds over the socket) →
  Present `PresentPixmap` to flip it (a `GetInputFocus` round-trip surfaces any
  protocol error), and `dri3probe` composes the whole chain on real hardware:
  `DRI3Open` → `stipple_gpu::export_dmabuf_on_device` (render + export on the
  server's GPU) → that present. **Build-verified in CI** (with and without the
  `gl` feature); the DRI3 import + flip need a real GPU + DRM-capable X server
  (Xvfb reports DRI3 unavailable), so runtime is hardware-gated — run `dri3probe`
  on a GPU box to validate the pixels on screen. **Frame sync** closes the last
  protocol gap: `present_pixmap_request` takes a `wait_fence` (an `XSyncFence` the
  server waits on before sampling the pixmap), and `dri3_fence_from_fd_request`
  (DRI3 minor 4) wraps the producer's render-completion sync-file fd as that
  fence — so the compositor never reads a half-rendered buffer, GPU-synced with
  no CPU stall (both encoders unit-tested; runtime hardware-gated). **The Linux
  compositor is now complete** — viewport, cross-process compositing, input
  forwarding, the sandboxed content process, and *both* the GPU `dma-buf` present
  (DRI3 + Present, end to end) and the CPU `shm` buffer paths all land, with
  frame sync.
  **Cross-platform GPU buffer-sharing (parity):** the macOS and Windows analogs
  of the `dma-buf` export now exist, each raw FFI (no helper crate) on its OS's
  shared-texture primitive. **Windows** —
  `stipple_gpu::d3d11_export_shared_handle` builds a `D3D11_RESOURCE_MISC_SHARED`
  texture and `QueryInterface(IDXGIResource)` → `GetSharedHandle` for a
  cross-process `HANDLE`; **runtime-verified on the Windows runner** (the
  `visual-windows` job's `d3ddemo` prints a real handle even on software WARP).
  **macOS** — `stipple_gpu::metal_export_iosurface` builds a BGRA8 `IOSurface` via
  the CoreFoundation C API and returns its global `IOSurfaceID`; the
  `visual-macos` job asserts the surface is created on the runner's real Metal
  stack. Both are the exact analogs of `dma-buf` (the UI process re-opens the
  handle / id and binds it as a GPU texture), so the compositor's content path is
  portable across all three desktops. **Browser-compositor item: complete** — the
  full architecture (viewport, compositing, input forwarding, sandboxed content
  process, GPU + CPU buffer transport, present, frame sync) is implemented and
  CI-verified to the limit of each environment, with the GPU on-window present
  runtime-validated by `dri3probe` on real GPU hardware.

### Platform fidelity (November 2026)

- ✅ **Single crate**: the 11 crates merged into `stipple`, at the repository
  root; the wasm target became `examples/webdemo`.
- ✅ **Native theme** (`Theme::native()`): follows the system's dark mode and
  accent color everywhere (the XDG portal on Linux) and uses each platform's
  metrics. On Windows (`uxtheme`), macOS (offscreen AppKit controls) and iOS
  (offscreen UIKit controls) the OS draws buttons, checkboxes, radio buttons,
  switches, sliders, progress bars and text-field frames into cached bitmaps.
  `examples/nativegallery` shows them, and CI screenshots it per platform.
- ✅ **High-DPI** on Windows (per-monitor v2), macOS (`backingScaleFactor`), iOS
  (`UIScreen.scale`), X11 (`Xft.dpi`) and Wayland (output scale). X11 and Wayland
  run at 2x in CI; Windows and macOS await a real high-DPI display.
- ✅ **iOS** runs full-screen in the safe area, with touch input.
- ✅ **Title bars** match the content's light or dark on Windows and macOS.
- ✅ **Text** centers capitals in the line, whatever the font's metrics claim.
- ✅ **Published screenshots**: CI commits fresh, deterministic screenshots to
  `docs/screenshots/` after a green push to `master`.
- ✅ **Docs**: `docs/` holds a guide per platform, plus the native theme,
  high-DPI and CI.

---

## 1. Guiding principles

1. **Minimal third-party dependencies.** No `winit`, `wgpu`, `taffy`, `lyon`,
   `tiny-skia`, `cosmic-text`, GTK/Qt, etc. We talk to each OS directly through
   its lowest stable, idiomatic interface, and we own our layout, reactivity,
   and rendering glue. oxideav crates are first-party (same author/ecosystem)
   and are the one sanctioned heavy dependency.
2. **Close to the OS.** The OS provides only what only it can: a window, an
   input stream, a presentable surface, clipboard, IME, and an accessibility
   bridge. Everything visual is ours.
3. **Self-drawn, pixel-identical, themeable.** Every control is drawn by Stipple
   via `oxideav-raster`. One theme engine, one look, identical across every
   platform — and completely customizable.
4. **Declarative & reactive.** The public API is state-driven (SwiftUI / Jetpack
   Compose style): UI is a function of state; the runtime diffs and updates.
5. **Software-first rendering, GPU later.** v1 rasterizes on the CPU with
   `oxideav-raster` and blits to a native surface. A GPU backend lands later
   behind a stable `Surface` abstraction — without wgpu, using raw
   Metal / Vulkan / D3D12 / WebGPU.
6. **Portability is a layering discipline.** All OS-specific code lives behind
   `stipple::platform`. The rest of the stack is `#![forbid]`-clean of platform
   `cfg`s and is tested headlessly with golden images.

### Non-negotiable dependency policy

| Concern | Stipple's answer |
|---|---|
| 2D vector rasterization | `oxideav-raster` |
| Scene graph / primitives | `oxideav-core` (`VectorFrame`, `Node`, `Group`, `Transform2D`) |
| Font parsing + shaping | `oxideav-ttf`, `oxideav-otf`, `oxideav-scribe` |
| Image decode | `oxideav-image` over `oxideav-png`, `-mjpeg`, `-gif`, `-webp`, `-bmp` |
| SVG (icons) | `oxideav-svg` |
| Pixel conversion / blit prep | `oxideav-pixfmt` |
| Windowing, input, IME, clipboard | **Stipple, hand-written per OS** |
| Layout, reactivity, widgets, theming, animation | **Stipple** |

---

## 2. Architecture

Layered, bottom-up. Each layer is a module of the single `stipple` crate, at the
repository root. Stipple targets **edition 2024, rust 1.88** (the floor imposed
by the oxideav dependency chain), `version = 0.0.x`,
pure-Rust. (oxideav itself is edition 2021 / rust 1.80; Stipple consumes it as a
dependency but builds on the newer toolchain.)

```
                 ┌─────────────────────────────────────────┐
   app facade    │  stipple  (crate root: App, prelude)
                 └─────────────────────────────────────────┘
   widgets       │  stipple::widgets  Text Button TextField Stack Row Column …
   styling       │  stipple::style    design tokens, themes, typography scales
   animation     │  stipple::anim     clock, easing, springs, transitions
                 ├─────────────────────────────────────────┤
   runtime       │  stipple::core     View trait · element tree · reconcile ·
                 │                  state/signals · events · focus · hit-test
   layout        │  stipple::layout   fl/box layout solver, intrinsic sizing
                 ├─────────────────────────────────────────┤
   rendering     │  stipple::render   Scene builder → oxideav VectorFrame ·
                 │                  Surface trait · software backend · text run
                 │                  shaping bridge · damage/dirty regions ·
                 │                  layer + glyph caches
   platform      │  stipple::platform windows · event loop · input · IME ·
                 │                  clipboard · DPI · vsync · a11y bridge
   geometry      │  stipple::geometry Point Size Rect Insets Affine (logical px)
                 └─────────────────────────────────────────┘
                       ↑ depends on oxideav-{core,raster,scribe,svg,png,…}
```

### Layer responsibilities

- **`stipple::geometry`** — Logical-pixel math: `Point`, `Size`, `Rect`, `Insets`,
  `Affine`. Thin ergonomic layer; converts to/from `oxideav-core`'s
  `Transform2D` and physical pixels at the render boundary. Handles the
  logical↔physical DPI scale factor.

- **`stipple::render`** — The seam between the toolkit and oxideav. Builds a
  `Scene` (an `oxideav_core::VectorFrame`) from draw commands; shapes text runs
  through `oxideav-scribe` (`shape` → `PositionedGlyph` →
  `shape_to_paths` → `Node`s); rasterizes via
  `oxideav_raster::Renderer::render` into an Rgba `VideoFrame`; and presents
  through a `Surface` trait. Owns **damage tracking** (only repaint dirty
  rects), **layer caching** (cache subtree raster output), and a **glyph cache**.
  The `Surface` trait is the GPU-readiness seam:
  ```rust
  pub trait Surface {
      fn resize(&mut self, size: PhysicalSize);
      fn present(&mut self, frame: &VideoFrame, damage: &[Rect]);
  }
  ```

- **`stipple::platform`** — The only module with per-OS code, selected by `cfg`.
  Exposes `EventLoop`, `Window`, an input event stream, IME, clipboard, DPI,
  vsync/frame callbacks, lifecycle, and a `Surface` factory. Backends added in
  roadmap order: `linux` (Wayland `wl_shm` first, X11 MIT-SHM fallback),
  `macos` (AppKit), `windows` (Win32), then `android` (NDK), `ios` (UIKit),
  `web` (canvas).

- **`stipple::layout`** — Self-contained flex/box layout solver over the element
  tree: main/cross axis, flex grow/shrink/basis, alignment, gap, padding,
  min/max, and **intrinsic sizing** driven by `stipple::render` text measurement.
  No `taffy`.

- **`stipple::core`** — The reactive runtime ("the Compose/SwiftUI engine"):
  the `View` trait, building an element tree, diff/reconcile against the prior
  tree, fine-grained state (signals/state cells), effect scheduling, event
  dispatch + bubbling, focus management, and hit-testing. Drives layout and
  render each frame.

- **`stipple::anim`** — Frame clock (fed by platform vsync), easing curves,
  spring physics, and value transitions wired into the reactive runtime.

- **`stipple::style`** — Design tokens, theme definitions (light/dark + custom),
  color systems, typography scales, spacing, elevation/shadow, animation
  defaults. The single source of "the look."

- **`stipple::widgets`** — The standard library drawn on top of everything:
  layout (`Row`, `Column`, `Stack`, `Grid`, `Scroll`), content (`Text`,
  `Image`, `Icon` via `oxideav-svg`), input (`Button`, `TextField`, `Checkbox`,
  `Radio`, `Switch`, `Slider`, `Dropdown`), structure (`List`/virtualized,
  `Table`, `Tabs`), overlay (`Menu`, `Popover`, `Tooltip`, `Dialog`).

- **`stipple`** (crate root) — `App` builder, prelude, examples
  entry point.

- **`stipple-a11y`** (lands in Phase 3) — Backend-agnostic accessibility tree
  plus per-OS bridges: AT-SPI (Linux), UI Automation (Windows),
  NSAccessibility (macOS), and the mobile/web equivalents later.

### Reactive API shape (target)

```rust
fn view(state: &Counter) -> impl View {
    Column((
        Text(format!("{}", state.n)).font_size(48.0),
        Row((
            Button("−").on_tap(|s: &mut Counter| s.n -= 1),
            Button("+").on_tap(|s: &mut Counter| s.n += 1),
        )).gap(8.0),
    ))
    .padding(24.0)
    .align(Align::Center)
}

fn main() {
    stipple::App::new(Counter { n: 0 }, view)
        .title("Counter")
        .theme(Theme::system())
        .run();
}
```

---

## 3. Phased roadmap

Phases are sequenced for **earliest end-to-end proof, then breadth, then
depth**. Each phase ends with a runnable, demoable deliverable. The GPU track
(Phase 6) is cross-cutting and can start in parallel once the `Surface`
abstraction is frozen at the end of Phase 2.

### Phase 0 — Foundations & de-risking spikes
*Goal: prove the riskiest seam (CPU buffer → screen) before building a toolkit.*

- Cargo workspace scaffold, CI (fmt/clippy/test on Linux), license, oxideav
  dependency wiring (path deps now, versioned later).
- `stipple::geometry` core types + `Transform2D` interop.
- **Spike 1 — present path (Linux):** open a Wayland window, allocate a
  `wl_shm` buffer, render a solid `VectorFrame` rect via `oxideav-raster`, blit,
  present, handle resize + close. The "hello rectangle."
- **Spike 2 — text:** shape a string with `oxideav-scribe`, rasterize the glyph
  paths, present. Confirms the scribe→raster→surface chain.
- **Exit criteria:** a window on Linux showing anti-aliased shapes + text from
  oxideav, resizing cleanly.

### Phase 1 — Single-platform vertical slice (Linux)
*Goal: a real, themeable, animated app on one platform — the full stack thin.*

- `stipple::platform` Linux backend: Wayland (primary) + X11 (fallback) — window,
  resize, mouse, keyboard, scroll, DPI/scale, frame callbacks (vsync), basic
  clipboard, basic IME.
- `stipple::render`: `Scene` builder, software `Surface`, text-run shaping bridge,
  double buffering, damage tracking.
- `stipple::core` MVP: `View` trait, element tree, reconcile, signals/state, event
  dispatch, hit-testing, focus.
- `stipple::layout`: flex subset (row/column/grow/align/gap/padding) + text
  intrinsic sizing.
- `stipple::widgets` MVP: `Row`/`Column`/`Stack`, `Text`, `Button`, basic
  `TextField`, `Image`, `Scroll`.
- `stipple::style` MVP theme + `stipple::anim` clock/tween/spring.
- **Exit criteria:** demo apps (counter, todo, settings panel) run on Linux,
  themeable (light/dark), with at least one animated transition.

### Phase 2 — Desktop breadth (macOS, Windows)
*Goal: identical apps on all three desktops; freeze the platform/Surface API.*

- `stipple::platform` macOS: `NSWindow`/`NSView`, `CVDisplayLink` vsync, blit via
  `CGImage`/`IOSurface`, IME via `NSTextInputClient`, clipboard, per-display
  scale.
- `stipple::platform` Windows: `HWND`, `WM_PAINT` + GDI/DXGI blit, raw input, IME
  via TSF/IMM, clipboard, per-monitor-v2 DPI.
- Cross-platform **golden-image conformance suite** (headless render + pixel
  diff) so "pixel-identical" is enforced in CI.
- Multi-window; native menus, file dialogs, and message boxes (thin OS shims);
  HiDPI correctness on all three.
- **Exit criteria:** the Phase 1 demos run unmodified on Linux/macOS/Windows
  with matching golden images. `stipple::platform` and `Surface` APIs frozen.

### Phase 3 — Toolkit maturity
*Goal: a toolkit you'd actually ship a product with.*

- Full widget set: virtualized `List`/`Table`, `Tabs`, `Menu`/`Popover`/
  `Tooltip`, `Dialog`, `Slider`, `Checkbox`/`Radio`/`Switch`, `Dropdown`/
  combobox, `Progress`.
- Rich text editing: selection, caret, multi-line, undo/redo, clipboard, **bidi
  + complex-script** input leveraging scribe; font fallback via `FaceChain`.
- **Accessibility** (`stipple-a11y`): semantics tree + AT-SPI / UIA /
  NSAccessibility bridges; keyboard navigation + focus traversal.
- Styling depth: full theming/token system, transitions, gesture recognizers.
- i18n: RTL layout, locale-aware formatting, font fallback chains.
- ✅ **Fonts and decorations**: system fonts by family name (font directories
  read by hand; fontconfig's configuration and aliases on Linux), `Font` as a
  cheap handle over a family with real or synthesized bold and italic,
  per-element fonts, public fallback chains, terminal-grade `FontMetrics`, and
  text decorations (five underline styles, strikethrough, overline, their own
  color). See [`docs/fonts.md`](docs/fonts.md).
- ✅ **Custom painting with partial invalidation**: `Element::canvas(id, paint)`,
  a leaf whose callback draws into a `Canvas` (a `Scene` in local
  coordinates) when it is rasterized, asked for just the area being
  rasterized. `AppHandle::invalidate_canvas` / `invalidate_canvas_rect`
  repaint it, or a rectangle of it, from any thread without rebuilding the
  view; `AppHandle::set_viewport_content` does the same for viewports.
  Paths (Béziers, dashes, caps, joins), bitmaps drawn without copying, and
  `GlyphRun`s placed cluster by cluster with a font, color and decoration per
  run. Area renders drop what can't touch the area, and heavy full frames are
  rasterized in bands; `RasterStats` counts the pixels. `canvasdemo
  --selftest` checks in CI that only invalidated rows are rasterized. See
  [`docs/canvas.md`](docs/canvas.md).
- Performance: render thread, layer caching, glyph atlas.
- **Exit criteria:** a non-trivial reference application (e.g., a file/media
  browser using oxideav decoders) ships on all three desktops, accessible and
  localized.

### Phase 4 — Mobile (Android, iOS)
*Goal: the same toolkit on touch.*

- Touch & gesture model, on-screen keyboard, safe areas, density scaling,
  app lifecycle (suspend/resume), back-navigation.
- `stipple::platform` Android: NDK `NativeActivity`/`GameActivity`,
  `ANativeWindow` buffer blit, input/IME via minimal JNI, density.
- `stipple::platform` iOS: UIKit, `CADisplayLink`, `CALayer`/`CGImage` present,
  `UITextInput` IME, touch, lifecycle.
- **Exit criteria:** a demo app runs on Android + iOS hardware/simulators with
  the same `view` code.

### Phase 5 — Web (WASM)
*Goal: the "maybe" target, made real via the software path.*

- `wasm32-unknown-unknown` target; thin hand-written JS interop (no heavy
  bindgen-driven dep tree where avoidable) for canvas + events.
- Software present via `putImageData`; event/IME/clipboard bridging; DPR
  scaling.
- **Exit criteria:** a demo runs in the browser from the same `view` code.

### Phase 6 — GPU backends (cross-cutting, optional)
*Goal: smooth high-DPI/animation perf without sacrificing the dep policy.*

- Implement `Surface` (and a scene-upload compositor) on raw **Metal** (macOS/
  iOS), **D3D12** (Windows), **Vulkan** (Linux/Android), **WebGPU** (web).
  No wgpu. Glyph/mask atlases on GPU; oxideav-raster remains the CPU fallback.
- **Exit criteria:** GPU backend is a drop-in `Surface` with measurable
  frame-time wins; software path stays the default/fallback.

### Phase 7 — App-ready parity
*Goal: everything a real app needs, on every platform. Rendering, the native look
and platform plumbing are in place; this is the breadth an app depends on.*

The order matters: 7a is an architecture decision the rest builds on, and 7b is
the quickest visible win. [`docs/README.md`](docs/README.md#what-works-where)
tracks what works on each platform today.

#### 7a — An event loop that can wake itself
`ControlFlow` used to wait only for OS events, so `Tween` and `Spring` had
nothing to drive them and background work couldn't update the UI.

- ✅ Settle threading and async (§6): one UI thread, an `AppHandle`, optional tokio.
- ✅ Timers: `AppHandle::after`, `every`, `cancel`.
- ✅ `AppHandle::update` from any thread, waking the loop on X11, Wayland,
  Windows, macOS and iOS; `AppHandle::spawn` for futures with the `tokio`
  feature. `examples/asyncdemo --selftest` checks it in CI on the four desktops.
- ✅ Wakeups and timers on the web: one `setTimeout` at the earliest deadline
  calls back into the module, and `AppHandle::update` arms it at zero delay.
  Timers there run on `stipple::platform::time::Instant`, read from
  `performance.now()` (std's `Instant` panics on `wasm32-unknown-unknown`).
- ✅ Wakeups on Android: the `NativeActivity` backend's `ALooper` wakes with
  `ALooper_wake` and times out at the next timer. CI's emulator run checks
  `androiddemo`'s once-a-second timer ticks.
- ✅ A frame clock driven by the display (`CVDisplayLink`, `CADisplayLink`,
  `wl_surface.frame`, `DwmFlush`; a RandR-rate timer on X11), so animations
  tick at the refresh rate and stop when idle. `Cx::request_frame` and
  `Cx::frame_time`; see [`docs/animation.md`](docs/animation.md).
- ✅ Animated widgets: a turning spinner, and hover and press highlights that
  fade.
- ✅ Frames on the web: `requestAnimationFrame` is the frame clock's display
  source, requested only while something animates.
- ✅ Frames on Android, from `AChoreographer` (resolved at run time, as it's
  newer than API 21). CI's emulator run shows `androiddemo`'s spinner building
  frames between timer ticks; the rate isn't checked.
- ✅ Theme changes that cross-fade, over 200 ms (see Phase 8).
- ⬜ macOS CI animates at only ~15 fps (10–12 frames during asyncdemo's 600 ms
  load, against ~36 on X11 and ~18 on Wayland): find out whether
  `CVDisplayLink` is ticking or the 30 Hz fallback timer is, and what each
  frame costs.
  *Instrumented and fixed from reading the code, not yet measured on CI. The
  frame itself is cheap: on X11 at 2x the same run presents a 22×22 damage
  region per frame in under 1 ms on average, and the macOS runner is 1x. What
  was slow on macOS is how frames reached the screen and how they were paced:
  every present marked the whole view for redisplay, and `drawRect:` drew
  the whole window from a straight-alpha RGBA image in the device color
  space (a per-pixel premultiply, swizzle and color match in Core Graphics);
  a display link that doesn't tick left every frame to the fallback timer,
  two refreshes after the request (the ~30 Hz); and the link stopped after 3
  idle callbacks, so a slow frame stopped and restarted it between frames.
  Now presents mark only the damage (`setNeedsDisplayInRect:`), the buffer
  is kept as sRGB BGRX (Core Graphics' native layout) in an sRGB window
  backing store, so `drawRect:` is a copy; the frame clock stops waiting for
  a source after three misses in a row (`frame::MISS_LIMIT`) and paces at
  the refresh interval until it ticks again; and the link idles for ~200 ms
  before stopping. `frame::stats()` counts frames from the display and from
  the timer and times presents and `drawRect:`; `asyncdemo --selftest`
  prints them, and the macOS job fails below 10 frames and warns below 24.
  The next macOS run will tell which cause it was and whether this is
  enough.*
- **Exit criteria:** an example that loads data on a background thread shows a
  live spinner, then the result, on every platform. `asyncdemo --selftest`
  checks it in CI on the four desktops; iOS is unverified, Android's emulator
  run covers the timer and the spinner (not the background thread). The web has
  no threads, so `webdemo` loads on a timer instead: its selftest checks in
  headless Chrome that the spinner animates, then stops.

#### 7b — Input parity
Most input only works on X11 today.

- ✅ Scroll wheel and trackpad scrolling on Wayland, Windows and macOS.
- ✅ Modifier keys on every desktop backend, so Ctrl/Cmd shortcuts and
  Shift-selection work beyond X11.
- ✅ A full key list: PageUp/PageDown, Insert, F1–F12, and keys for shortcuts.
- ✅ The web: pointer events (captured during a press), `keydown` keys and
  modifiers, wheel scrolling in pixels, lines or pages, and cursor shapes as the
  canvas's `style.cursor`. `webdemo`'s selftest clicks, types and uses Ctrl
  shortcuts through DOM events in CI.
- ✅ Android touch and keyboard (`AInputQueue`): the first finger as the
  pointer, the mouse's buttons, hover and wheel, named keys and modifiers, and
  typed characters through the keyboard's `KeyCharacterMap` (JNI). The soft
  keyboard shows while a text input has focus (`Window::set_text_input_active`).
  CI taps and types (Tab, `adb shell input text`) on the emulator; the mouse and
  shortcuts are only build-checked.
- ✅ Touch scrolling and multi-touch events on Android and iOS: every finger is
  an `Event::Touch` (id, phase, position), and the first is still the pointer.
  In the toolkit a finger drag in a scroll container scrolls it past an 8 px
  slop (taps still click; the press is cancelled once it scrolls), flings with
  an exponential slow-down, stops at the content's ends, and a touch stops a
  fling. Mice and wheels are unchanged. The gesture's state machine and the app
  wiring are unit-tested with synthetic touches; CI swipes `androiddemo`'s list
  with `adb shell input swipe` on the emulator (it scrolls, no row is tapped)
  and checks on X11 that a mouse drag still doesn't scroll. On iOS the
  `touchdemo` self-test drives a synthetic finger through the backend's touch
  path (not real `UITouch`es). No pinch or two-finger gestures yet, and
  scrolling is vertical only.
- ✅ iOS keyboard (`UIKeyInput`): the view becomes first responder while a text
  input has focus (`Window::set_text_input_active`), `insertText:` types and
  `deleteBackward` deletes; hardware keys the text system doesn't type (arrows,
  Escape, Tab, F-keys, Ctrl/Cmd shortcuts) arrive through `pressesBegan:`.
  `touchdemo`'s self-test sends `insertText:`/`deleteBackward` by objc dispatch
  on the simulator in CI and checks the field and the first-responder state;
  hardware keys are build-checked only. Composition goes through
  `UITextInput` (7c).
- **Exit criteria:** the same scripted input test passes on every desktop backend.

#### 7c — Text input
- ✅ Input methods (IME) for Chinese, Japanese and Korean: a platform-neutral
  preedit / commit model (`Event::Ime`), the composition drawn inline and
  underlined in text fields, and `Window::set_ime` reporting the caret
  rectangle; `text-input-v3` on Wayland, IMM32 (now TSF, below) on Windows and
  `NSTextInputClient` on macOS. Unit-tested, and the macOS protocol is driven
  through objc dispatch in CI. Wayland runs against a real input method in
  CI (fcitx5 under sway, below); the Windows and macOS paths are verified
  end to end only by hand so far (CI attempts them, non-blocking). See
  [docs/ime.md](docs/ime.md).
- ✅ IME on X11: an XIM client over the wire (no libX11), in
  `x11/xim.rs`. It finds the server from `XIM_SERVERS` and `XMODIFIERS`
  (`@im=`), connects over `_XIM_XCONNECT` (client messages, properties for
  large messages), opens the IM, creates an on-the-spot input context
  (`XIMPreeditCallbacks`) per window when a text field takes focus, forwards
  keys (`XIM_FORWARD_EVENT`), and turns `XIM_COMMIT` and the preedit
  callbacks into the same `Event::Ime` preedit / commit; `Window::set_ime`
  drives focus and `spotLocation` (the candidate window). COMPOUND_TEXT is
  decoded in-crate (Latin, UTF-8 segments, JIS X 0208 / KS C 5601 / GB 2312).
  It never blocks: every wait has a deadline, and on a timeout, a setup error
  or the server quitting it switches off and keys take the direct path
  (keysyms, Stipple's compose). Unit-tested against byte fixtures and a
  scripted fake server; **CI-verified with a real XIM server**: IBus
  (`ibus-daemon --xim`) under Xvfb commits a dead-key `é` with its preedit,
  and IBus quitting mid-session falls back to compose. Japanese through Mozc
  (`nihon` → preedit `にほん` → `日本` committed) is required in CI (it ran
  as an informational check first and passed on every run). Not tried: Fcitx, uim, kinput2. Not done: a
  server started after the app, status callbacks, timing out asynchronous
  keys a stopped server never answers. See [docs/ime.md](docs/ime.md).
- ✅ TSF on Windows, next to IMM32: modern IMEs (the Microsoft Japanese and
  Chinese IMEs' newer modes, handwriting, voice typing, the touch keyboard's
  suggestions) talk TSF, and IMM32 only reaches them through its compatibility
  layer. Stipple is now a TSF client with hand-written COM vtables, in
  `windows/tsf/`: `ITfThreadMgr` activation (IMM32 when that fails, or with
  `STIPPLE_TSF=0`), a document manager and context per window tied to it with
  `AssociateFocus` while a text field or `ime_input` element has focus (an
  empty one otherwise), and an `ITextStoreACP` over the field's text,
  selection and caret rectangle, with `ITfContextOwnerCompositionSink` for
  compositions and an `ITfTextEditSink` reading display attributes (the target
  clause is the preedit's highlight). Locks are granted at once, queued while
  another is held or refused when synchronous, never waited for. The store's
  logic (ACP offsets, selection, inserts and replacements, the composition
  state machine, mapping the document onto preedit / commit /
  delete-surrounding, the App's reports) is platform-neutral in
  `platform/text_store.rs` and unit-tested on Linux. CI activates TSF on the
  Windows runner and drives a composition through the real TSF manager, the
  app playing the input method (edit sessions, `StartComposition`, `SetText`,
  `EndComposition`): the field must show the preedit `にほん`, then `日本`,
  then the commit. (CI found an app can only start a composition through
  the owner's `ITfContextOwnerCompositionServices`, not
  `ITfContextComposition`, and that an empty field reported its layout
  placeholder `" "` as its text until a composition started, which read as
  the App changing the text under it and ended the composition; both are
  fixed, and the App's echo of a composition shown as text counts as in
  step.) A failure in the TSF path
  (a panic in a store call, `AssociateFocus` failing) switches TSF off for
  the run and hands input to IMM32. No real IME runs there, so how the
  Microsoft IMEs behave
  against the store is unverified, as are the display attributes and the
  IMM32 fallback (build-checked). Not done: `GetACPFromPoint` (handwriting
  aims by point), text attributes and input scopes (`RequestSupportedAttrs`
  offers none), the keystroke manager (keys reach text services through TSF's
  own hooks). See [docs/ime.md](docs/ime.md).
- ✅ iOS composition: `UITextInput` on the view, so the Japanese, Chinese and
  Korean keyboards compose instead of committing raw kana or jamo, and
  autocorrect, dictation and QuickType have a document to read. The document
  is the field's text around the caret from `Window::set_ime`, in UTF-16
  offsets carried by `UITextPosition` / `UITextRange` subclasses built at run
  time (an instance variable per offset); `setMarkedText:selectedRange:` is
  `Ime::Preedit`, `insertText:` / `unmarkText` commit, `replaceRange:withText:`
  becomes `Ime::DeleteSurrounding` plus a commit, a collapsed
  `setSelectedTextRange:` arrow keys, and `firstRectForRange:` /
  `caretRectForPosition:` the caret rectangle; changes the keyboard didn't
  make are announced through its `inputDelegate`. The document model is
  unit-tested on every host; `touchdemo`'s simulator self-test drives the
  methods through objc dispatch (marked text reaching the field as a preedit,
  the document and `firstRectForRange:` read back, a commit, a replacement).
  The first `visual-ios` run passed these: the preedit `にほん` reached the field, the document, selection and `firstRectForRange:` read back right, and the commit and replacement gave `"HiJP"`. No real keyboard composes in CI. Not done: selection rectangles,
  point-to-position hit-testing, keyboard-made selections, hardware dead keys.
  See [docs/ime.md](docs/ime.md).
- ✅ Android composition: an `InputConnection`, from a host `View` the backend
  adds to the `NativeActivity`. With no Java toolchain, its classes are
  assembled at run time by a hand-written dex writer (`platform::dex`: every
  method `native` and bound with `RegisterNatives`, the constructors two
  instructions) and loaded with `InMemoryDexClassLoader` (`DexClassLoader`
  from a file before Android 8). `setComposingText` / `commitText` /
  `finishComposingText` / `deleteSurroundingText[InCodePoints]` /
  `setSelection` feed `Event::Ime` (and arrow keys); `setComposingRegion`
  (composing an existing word again) is declined, as Stipple only composes
  new text at the caret,
  `getTextBefore/AfterCursor` / `getSelectedText` / `getCursorCapsMode` answer
  in UTF-16 from the field's `ImeState`, the editor action maps to Enter (Done,
  Go, Send…), Tab (Next) and Shift+Tab (Previous), the context menu actions to
  Ctrl+A/X/C/V; `updateSelection` and `restartInput` keep the keyboard in
  step; password fields get a password `inputType`. The editable model is
  unit-tested on the host. **Verified on an API 34 emulator** (locally, and in
  the `visual-android` CI steps, which pass since the typing fix): Gboard connects (`inputType 0x4001`, action Done), keys
  typed with `adb shell input text` reach the field with or without an input
  method connected (CI's first run never saw "Stipple" arrive; the likely
  cause, not reproduced locally, is that Gboard re-composed each word
  through `setComposingRegion` and kept the typed text as a preedit, so that
  call is now declined), and the
  app-side self-test drives the real Java `InputConnection` (compose にほ,
  commit 日本🎉, delete the emoji's two UTF-16 units, compose and finish), the
  field ending as `Stipple日本!`. Not tried with a person typing on a real
  composing keyboard (Japanese, Chinese, Korean); no `getExtractedText`, no
  cursor-anchor info for the candidate bar; selection moves from a non-empty
  selection or mid-composition are refused. See
  [docs/platforms/android.md](docs/platforms/android.md).
- ⬜ Dead keys and compose on mobile hardware keyboards (iOS and Android).
- 🚧 IME verification with a real input method. **X11**: IBus (`--xim`)
  with its keyboard engine and Mozc, required in CI (Mozc was informational
  until it had passed on every run, over forty). **Wayland: verified with a
  real input method in CI**: fcitx5 under headless sway, binding sway's
  `input-method-v2` and relayed to Stipple's `text-input-v3`, with `wtype`'s
  keys going through fcitx5's keyboard grab
  (`.github/scripts/wayland-ime.sh`): Mozc (`nihon` → preedit `にほん` →
  Space → `日本` → Return commits it; `ka` then Escape cancels without
  committing), pinyin (`nihao` → preedit `ni hao` → Space commits `你好`, and
  the field's surrounding text reaches the input method), Hangul (`gksrmf`
  composes `한` and `글` syllable by syllable, Space commits). Passed locally
  against Ubuntu 26.04's fcitx5 5.1.19 and sway 1.11 (the runner's image) in
  a rootless container. **Windows and macOS: attempted, not yet seen
  passing**: a non-blocking `ime-windows` job installs the ja-JP typing
  feature if needed (`Add-WindowsCapability` / `Install-Language`), adds the
  Microsoft Japanese IME to the user's languages, switches the app's thread
  with `WM_INPUTLANGCHANGEREQUEST` and types with `SendInput`; a
  non-blocking step in `interaction-macos` selects Kotoeri with a Swift
  helper (`TISSelectInputSource`) and posts CGEvents. Whether the runner
  images allow installing the language, posting keys (Accessibility) and
  switching the input source is what their runs will show; each logs the
  stage it stopped at. Until then both are protocol wiring in CI (Windows:
  the app playing the input method through the real TSF manager; macOS:
  `setMarkedText:` / `insertText:` through objc dispatch). A manual
  per-platform checklist (Japanese, pinyin, Korean, the candidate window
  following the caret, cancel, reconvert, mixed input, password fields) is
  in [docs/ime.md](docs/ime.md#manual-checklist). Not covered by CI:
  candidate window placement (needs the input method's own popup),
  reconversion, a real input method on iOS, Android and the web.
- ✅ Raw keys for app-handled input (`Element::on_raw_key`): every press,
  repeat and release with the logical key, the physical key (a USB HID usage
  with its W3C `code` name, from evdev codes, Windows scan codes, macOS key
  codes, `KeyboardEvent.code`), keypad location, left/right modifiers, the
  typed text and the text with only Shift (`charactersIgnoringModifiers`), and
  a repeat flag, on every backend (`Event::RawKey` underneath). The focused
  element's handler runs after open menus and menu shortcuts and before Tab
  traversal, the menu bar's keys and the `KeyInput` mapping; `captures_tab`
  keeps Tab, and `option_as_meta` makes left/right Option a Meta key on
  macOS. X11 asks XKB for detectable auto-repeat; Wayland repeats keys on the
  client from `wl_keyboard.repeat_info`, on the loop's deadline. Precedence is
  unit-tested at App level; **CI-verified** on X11 (`xdotool`: F-keys,
  Ctrl+letters, Tab, a held key) and Wayland (`wtype` under sway, a held key
  repeating). Windows, macOS, web, iOS and Android are build-checked. See
  [docs/keyboard.md](docs/keyboard.md).
- ✅ Input methods for elements that aren't text fields (`Element::ime_input`,
  `ime_cursor_area`): a terminal gets preedit and commits through `on_key` and
  places the candidate window at its cursor, on every platform with an IME.
- ✅ Dead keys and compose sequences: X11 and Wayland parse the locale's libX11
  Compose file (`XCOMPOSEFILE`, `~/.XCompose`, includes) by hand and show the
  pending sequence as preedit; Windows gets them from `TranslateMessage` and
  macOS from `NSTextInputClient`. CI types dead keys and a compose sequence on
  X11 (`xdotool`) and Wayland (`wtype` under sway).
- ✅ Undo and redo, word-wise movement and deletion, double-click to select a word
  (triple-click selects the line). See [docs/text-editing.md](docs/text-editing.md).
- ✅ Right-to-left and bidirectional text, from `oxideav-scribe`'s bidi support:
  UAX #9 per paragraph (auto-detected or `Element::direction`), runs shaped in
  logical order and placed in visual order, Arabic joining, right-aligned and
  wrapped right-to-left paragraphs, visual caret movement, multi-rectangle
  selections and hit-testing in mixed text. CI screenshots `bididemo` on X11.
  See [docs/bidi.md](docs/bidi.md).
- ✅ Mirrored layouts for right-to-left UIs: `Theme::with_layout_direction`
  (left to right by default; `LayoutDirection::from_env` / `for_language` pick
  it from a locale) and `Element::layout_direction` for a subtree. Rows run
  right to left, horizontal start/end alignment and grid columns flip, the
  left padding inset is the start side, `Auto` text lines up on the right,
  sliders and progress bars fill from the right (drags are mirrored), tree
  triangles and submenu arrows point left, Left/Right swap in trees, split
  panes and menus, and popups open leftwards. Unit-tested (layout, text,
  drags, menus, tree); CI checks `bididemo --mirror` on X11 (screenshot regions
  and a slider click), and its off-screen `--mirror-check` plus a window
  screenshot on Windows and macOS. Only unit-tested on iOS, Android and the
  web. See [docs/bidi.md](docs/bidi.md#mirrored-layouts).

#### 7d — OS integration
- ✅ System clipboard on Windows (`CF_UNICODETEXT`), macOS (`NSPasteboard`) and
  iOS (`UIPasteboard`), in `stipple::platform::clipboard` and behind
  `Window::clipboard`; CI round-trips text on Windows and macOS and reads it back
  with `Get-Clipboard` / `pbpaste`.
- ✅ System clipboard on Wayland (`wl_data_device`: a `wl_data_source` serves
  copies, pastes read a `wl_data_offer` through a pipe without blocking the
  loop); CI round-trips text with `wl-copy` / `wl-paste` under sway.
- ✅ System clipboard on the web, through the `paste`, `copy` and `cut` events
  and `navigator.clipboard.writeText`; Ctrl/Cmd+V arrives with the pasted text.
  CI round-trips text with synthetic clipboard events.
- ✅ Rich clipboard: `ClipboardContent` (text, HTML, an image as a `Pixmap`)
  through `Window::read_clipboard` / `write_clipboard` and
  `platform::clipboard::{read, write}`: MIME types on X11 and Wayland,
  `CF_HTML` / `CF_DIBV5` / `PNG` on Windows, `public.html` / `public.png` on
  macOS, HTML on the web. Handlers use `core::write_clipboard` /
  `request_clipboard` and `App::on_clipboard`. CI checks X11 (`xclip`), Wayland
  (`wl-clipboard` under sway), Windows (.NET `Clipboard`, `Set-Clipboard
  -AsHtml`) and macOS (`osascript`).
- ✅ X11 paste from other applications: asynchronous `ConvertSelection`
  (`TARGETS`, then each format; `UTF8_STRING`/`STRING` fallback), `INCR` both
  ways, a two-second timeout, delivered as `Event::Clipboard`. CI pastes from
  `xclip`, including a large `INCR` transfer.
- ✅ Primary selection on X11 (`PRIMARY`) and Wayland
  (`zwp_primary_selection_device_manager_v1`): a field's selection becomes it,
  a middle click on editable text pastes it (`App::paste_primary_at`);
  in-process elsewhere. CI-verified with `xclip` (including the middle click)
  and `wl-paste --primary`.
- ✅ System clipboard on Android: `ClipboardManager` over hand-written JNI
  through the activity's `JavaVM`. Tried by hand on an emulator, not in CI.
- ✅ File dialogs on Windows (`IFileOpenDialog` / `IFileSaveDialog` over COM) and
  macOS (`NSOpenPanel` / `NSSavePanel`). Build-checked only: CI can't drive a
  modal dialog.
- ✅ File dialogs on the web, through new contents-based calls that work
  everywhere: `dialog::open_file_contents` (an `<input type=file>`; the callback
  runs later, from the event loop, and updates the state through the
  `AppHandle`) and `dialog::save_file_contents` (a download). The path-based
  calls return `None` there. CI checks both with the picker and the download
  stood in for.
- ✅ Menus: a declarative `MenuBar` / `Menu` / `MenuItem` model (`App::menu`,
  rebuilt from the state) with shortcuts (`Shortcut::cmd`, Cmd on macOS and
  Ctrl elsewhere), enabled and checked states, separators and submenus; a
  native `NSMenu` menu bar on macOS (standard application menu, key
  equivalents, `validateMenuItem:`) and `HMENU` on Windows (`WM_COMMAND`), and a
  menu bar Stipple draws elsewhere, with keyboard navigation (Alt, F10, arrows,
  Enter, Escape, mnemonics); context menus (`Element::context_menu`) on
  right-click, the Menu key or Shift+F10, drawn as overlays everywhere; the
  `MenuBar`, `Menu` and `MenuItem` roles. CI drives the drawn bar, a shortcut and
  a context menu on X11. See [docs/menus.md](docs/menus.md).
- ✅ Native context-menu popups on macOS (`NSMenu`
  `popUpMenuPositioningItem:atLocation:inView:`) and Windows
  (`TrackPopupMenuEx`), from the same model (`Window::popup_menu`); drawn on
  Linux, and everywhere with `menu_bar_in_window(true)`. Mnemonics underlined in
  the drawn menus while the keyboard drives them (Alt held, F10, Alt+letter, a
  key in an open menu), as on Windows (`Element::underline`), passed through to
  Win32 menus as `&`. Shortcuts run once on macOS: AppKit's key-equivalent
  match is dropped when Stipple's own matching took the key, so ⌘C in a text
  field doesn't also run the menu item. Runtime-tested in CI: on Windows,
  PowerShell reads the `HMENU` back, picks with `WM_COMMAND` and Alt+E plus a
  mnemonic, presses Ctrl+D, and opens and picks from the native context menu
  (`#32768`); on macOS, an in-process self-test (`STIPPLE_COCOA_MENU_SELFTEST`)
  prints the `NSMenu` bar, picks with `performActionForItemAtIndex:`, posts ⌘D
  and ⌘C (with and without a focused field) and opens and picks from the native
  context menu, each checked to run once (`osascript` System Events is tried
  but not required, for want of an accessibility grant); on X11, CI checks Alt
  underlines the mnemonics. Not verified: how the native popups behave with a
  real mouse and keyboard (CI posts events or messages).
- ✅ Window control: minimum and maximum size, maximize, fullscreen and the
  window icon, as `App` / `WindowAttributes` options and at run time through
  `AppHandle::window`, on X11, Wayland, Windows and macOS (the Dock icon there).
  CI reads the X11 hints and icon back with `xprop`. See
  [docs/windows-and-cursors.md](docs/windows-and-cursors.md).
- ✅ Window lifecycle: `App::on_close_requested` and `on_quit_requested`
  answer `CloseResponse::Close` or `Cancel` (then `AppHandle::close_window`
  or `exit` once the user confirms), from `WM_DELETE_WINDOW`,
  `xdg_toplevel.close`, `WM_CLOSE` / `WM_QUERYENDSESSION` and
  `windowShouldClose:` / `applicationShouldTerminate:` / ⌘Q. Window focus as
  `Cx::window_focused()` and `App::on_window_focus` (X11 `FocusIn`/`FocusOut`,
  Wayland `activated`, `WM_SETFOCUS`/`WM_KILLFOCUS`, key-window changes on
  macOS). Attention requests, the system beep and the Dock badge
  (`AppHandle::request_attention`, `beep`, `set_badge`). On Wayland,
  server-side decorations through xdg-decoration, or a title bar Stipple
  draws (move, resize, maximize, minimize, close, window menu);
  `WindowAttributes::with_decorations(false)` turns them off. CI cancels and
  confirms closes on X11, Wayland (sway), Windows and macOS, follows focus on
  X11 and Wayland, reads the X11 urgency hint and the macOS badge, and checks
  Weston gets a decorated buffer. Not yet: the Windows taskbar overlay badge
  and the Linux launcher badge. See
  [docs/window-lifecycle.md](docs/window-lifecycle.md).
- ✅ Mouse cursor shapes (`CursorIcon`): text inputs show the I-beam, tap and
  drag targets the pointing hand, and `.cursor(...)` picks any other (resize
  arrows, not-allowed, grab, crosshair, move, or `Hidden`, to hide it while
  the user types) on X11, Wayland (`cursor-shape-v1`), Windows and macOS. CI
  checks the cursor changes on X11.
- ✅ Raw pointer events for custom elements (#14): `Element::on_pointer` gets
  every button's `Down`/`Up` (middle and side buttons too), moves and a
  `Leave`, with the buttons held, the position in the element's coordinates
  (through transforms), the modifiers and the click count (the platform's
  double-click time and distance where readable: Windows, macOS, XSETTINGS);
  a press captures the pointer until the last button is up, on every backend.
  `Element::on_wheel` gets horizontal and vertical deltas in lines or pixels,
  with the trackpad gesture's phase (macOS, Wayland). Every desktop backend
  reports the pointer leaving the window (`LeaveNotify`, `wl_pointer.leave`,
  `WM_MOUSELEAVE`, a tracking area), which ends hover highlights and
  `.hovered` looks. CI drives `pointerdemo` with `xdotool` on X11 (middle and
  double clicks, buttons 6/7, the hidden cursor, leaving); the other backends
  are build-checked. See [docs/pointer-events.md](docs/pointer-events.md).
- ✅ Following dark mode and accent changes while the app runs:
  `Event::ThemeChanged` from `WM_SETTINGCHANGE` on Windows, the appearance
  notifications on macOS and the portal's `SettingChanged` on Linux; the App
  rebuilds `Theme::native()`, drops cached OS-drawn bitmaps, switches the title
  bar and repaints. Not exercised in CI on the desktops; on iOS see 7f.
- ✅ Drag and drop: drops of files and text from other applications over XDND
  (X11, by hand on the wire), `wl_data_device` (Wayland), an OLE
  `IDropTarget` with a hand-written vtable plus `WM_DROPFILES` (Windows) and
  `NSDraggingDestination` (macOS), as `Event::DragEnter` / `DragOver` /
  `DragLeave` / `Drop`; `Element::on_drop` / `on_drop_of` targets hit-tested
  like pointer events, a `DropState` views read through `Cx::drop_state`
  (over, accepted, position, an eased level) to animate the drop zone, and
  `Element::drag_source` for drags inside the app. CI drops on `dropdemo` with
  an XDND source on X11, drives the view's dragging methods with a stand-in
  drag on macOS, and drags a chip in-app on X11. Windows is build-checked: CI
  tries a real OLE `DoDragDrop` from another process, but it doesn't complete
  on the runner yet. Wayland is unit-tested only. See
  [docs/drag-and-drop.md](docs/drag-and-drop.md).
- ✅ Dragging out of the app: an `Element::drag_source` whose payload is
  `DropData::Files` or `DropData::Text` becomes a drag for other applications
  when the pointer leaves the window (`Window::start_drag`, called by the App;
  `Internal` payloads stay in the app). An XDND v5 source by hand on the wire
  on X11 (`XdndSelection` owner, pointer and keyboard grab, `XdndAware` window
  search, `XdndEnter` / `Position` / `Leave` / `Drop`, `SelectionRequest`s
  served, `XdndStatus` / `XdndFinished`, Escape cancels); a `wl_data_source`
  with `start_drag` on Wayland; OLE `DoDragDrop` with hand-written
  `IDataObject` (`CF_HDROP`, `CF_UNICODETEXT`) and `IDropSource` on Windows
  (and the left button now captures the mouse, so moves outside the window
  arrive); `beginDraggingSessionWithItems:event:source:` with `NSURL` /
  `NSString` items on macOS. CI drags dropdemo's file and text chips out onto
  an XDND target script on X11 and checks the URI list and the text arrive,
  that Escape cancels, and that a drag out and back drops on the app's own
  zone. Wayland is unit-tested against a scripted compositor; Windows and
  macOS are build-checked only. The app isn't told how a drag out ended. See
  [docs/drag-and-drop.md](docs/drag-and-drop.md).
- ✅ Opening URLs and files in their default apps (`platform::open_url`,
  `open_path`): the XDG portal's `OpenURI` over D-Bus with `xdg-open` as the
  fallback on Linux, `ShellExecuteW`, `NSWorkspace`, `UIApplication`, an
  `ACTION_VIEW` intent over JNI on Android, and `window.open` on the web. CI
  checks Linux (portal and `xdg-open`, against a mock), Windows (an HKCU URL
  protocol) and macOS (a `.txt` opens TextEdit); iOS, Android and the web are
  build-checked only.
- ✅ Notifications (`platform::notify`, `AppHandle::notify`):
  `org.freedesktop.Notifications` on Linux, a `Shell_NotifyIconW` balloon (a
  toast on Windows 10 and 11), `osascript` on macOS (the notification
  frameworks need an app bundle) and the web's `Notification` API. CI checks
  the Linux call reaches a mock server, and that Windows and macOS accept it; no
  popup is looked at. iOS and Android: see 7f. Clicks and actions on
  notifications are still to do.
- ✅ Message boxes (`AppHandle::message`, `platform::dialog::message`): a
  question with a kind (icon), buttons, a default and a cancel button and an
  optional (secure) text field, answered asynchronously as a state update.
  `TaskDialogIndirect` (common controls 6 activated through shell32's
  manifest) or `MessageBoxW` on a thread of its own on Windows, an `NSAlert`
  sheet with a hand-built completion block on macOS, `UIAlertController` on
  iOS, `alert`/`confirm` on the web, and a modal panel drawn in the theme on
  Linux and Android. CI answers the native alert in-process on Windows and
  macOS and drives the drawn one on X11. `dialog::pick_color` opens
  `ChooseColorW` on Windows. See [docs/os-integration.md](docs/os-integration.md#message-boxes).
- ✅ Tray icons: a declarative `Tray` (`App::tray`, rebuilt from the state)
  with a tooltip, an icon, a menu from the menu model and a click action;
  `Shell_NotifyIconW` with a popup `HMENU` on Windows, an `NSStatusItem` with an
  `NSMenu` on macOS, and a hand-written `StatusNotifierItem` plus
  `com.canonical.dbusmenu` service on Linux. Picks arrive as
  `Event::MenuCommand`, clicks as `Event::TrayActivated`. CI drives the Linux
  item end to end through a mock tray host (properties, menu layout, a menu
  click and an activation reach the app); on Windows and macOS it checks the
  icon is created, but the menus and clicks are build-checked only. No XEmbed
  tray on Linux. See [docs/os-integration.md](docs/os-integration.md).

#### 7e — Widgets and layout
- ✅ Image and SVG widgets: `image(...)` / `svg(...)` with fill / contain /
  cover fit; `Pixmap::decode` through the `oxideav-image` registry (PNG, JPEG,
  GIF, WebP, BMP); `Svg` parsed by `oxideav-svg` and rasterized at the physical
  size shown; `VideoFrame` ⇄ `Pixmap` interop; `imagedemo` + `visual-image` CI.
- ✅ Select / dropdown, number input, split pane, toolbar and toasts, with
  keyboard control and accessibility roles; see [docs/widgets.md](docs/widgets.md)
  and `examples/widgetsdemo`.
- ✅ Document tab bar (`tab_bar`): closable tabs, drag to reorder (in-app
  drags, with the shifted tabs springing into place), per-tab color and
  indicator (dot, bell, count), a context menu per tab, a new-tab button,
  scroll arrows and a list of every tab past what fits, keyboard navigation
  and `TabList`/`Tab` roles, animated with state overrides and transitions.
  Middle-click to close and double-click to rename (`on_rename`) through
  `Element::on_pointer`; dragging a tab out into a window of its own and
  macOS's native window tabs are still to do. Password field (`password_field`: masked, no copy or cut,
  no undo history, IME off, `EnableSecureEventInput` on macOS, a reveal
  button, the `PasswordField` role) and color picker (`color_picker` /
  `color_button`: HSV area, hue and alpha strips, hex field, swatches).
  `examples/sessionsdemo` + `visual-sessions` CI.
- ✅ Table, tree and data grid: `table(...)` with sortable headers (▲/▼),
  drag-to-resize columns, single/multi row selection (Shift/Ctrl+click,
  Shift+arrows) and striped rows; `tree(...)` with disclosure triangles,
  indentation and Left/Right collapse/expand; list, table, row, cell, tree and
  tree-item accessibility roles; see
  [docs/lists-and-tables.md](docs/lists-and-tables.md), `tabledemo` +
  `visual-table` CI.
- ✅ Grid layout (fixed, `fr` and auto tracks, gaps, spans), wrapping stacks,
  and minimum / maximum size constraints (with opt-in `shrink`); see
  [docs/layout.md](docs/layout.md) and `examples/layoutdemo`.
- ✅ Virtualized lists, so 10,000 rows don't build 10,000 elements:
  `virtual_list(...)` builds only the rows in and around the viewport (fixed or
  per-row heights) while the scroll extent covers them all; keyboard navigation
  scrolls the cursor into view (`Cx::scroll_offset` / `Cx::scroll_requests`).

#### 7f — Platform completeness
- ✅ Android: high-DPI (the density over 160 dpi; CI's emulator runs at
  2.625x), and the native theme's system colors: night mode, followed live on
  configuration changes, and the Android 12 wallpaper accent, read from
  `Resources` over JNI. The system bars take the app's background. Dark mode
  and the accent were tried by hand on an emulator, not in CI.
- ✅ Accessibility on iOS (UIAccessibility): the view is a
  `UIAccessibilityContainer` exposing one `UIAccessibilityElement` per node of
  the tree (a run-time subclass whose `accessibilityActivate` taps the node),
  with its label, value, traits and frame (`accessibilityFrame`, the node's
  bounds converted through the window to the screen's coordinate space, and
  updated when the view moves; `accessibilityFrameInContainerSpace` read back
  as a zero frame on the simulator, as UIKit only converts it while its
  accessibility runtime is loaded). Elements are kept and
  updated in place; `UIAccessibilityLayoutChangedNotification` follows an
  update VoiceOver should hear, `ScreenChangedNotification` the first tree and
  a dialog opening or closing. The tree's mapping and the notification choice
  are unit-tested on every host; `touchdemo`'s simulator self-test reads the
  elements through objc dispatch (the field's value and screen frame, the
  label's static-text trait, a row's button trait), activates a row through
  its element and checks it was tapped and the label's element followed.
  On its first CI run the labels, values and traits read back right (34
  elements) but every frame was zero, so the frame and activation checks
  failed; the frames are now set directly in screen coordinates, and the
  next run passed every check (frames, activation, the label following).
  An XCUITest runner now also drives it from outside the app (see the
  out-of-process checks below). Not yet tried with VoiceOver itself, on a
  simulator or a device; no adjustable or scroll actions.
- ✅ Accessibility on Android: the host view's `AccessibilityNodeProvider`
  (same run-time dex) gives a virtual view per node (ids in tree order), with
  the closest widget class name per role (`Button`, `EditText`, `TextView`,
  `CheckBox`, `Switch`, `ListView`…), text or content description, checked /
  selected / focused / password / editable state and screen bounds; actions:
  accessibility focus (with its events), click, focus and select (a tap at the
  node), scroll forward / backward (a wheel step over it) and set text (select
  all, then the new text). Tree changes send `TYPE_WINDOW_CONTENT_CHANGED`,
  focus moves `TYPE_VIEW_FOCUSED`; a finger's hover (TalkBack's explore by
  touch) goes to the host view, which announces the node under it. **Verified
  on an API 34 emulator** with `uiautomator dump` (the button, the field and
  the list's rows with their classes and bounds; CI checks it in the
  `visual-android` steps, which pass), and by an accessibility service
  standing in for TalkBack (see the out-of-process checks below). Not yet
  tried with TalkBack itself, on an emulator or a device.
- ✅ App lifecycle for apps, cross-platform API and iOS: `Event::Lifecycle`
  with `Lifecycle::{Background, Foreground, LowMemory}`, heard by
  `App::on_lifecycle(|state, change|)`. While in the background the App asks
  for no frames (the frame clock is suspended and the display link stopped)
  and neither builds nor repaints, even when timers change the state; coming
  back rebuilds and repaints every window; a memory warning also drops the
  cached OS-drawn controls. On iOS from `UIApplicationDidEnterBackground` /
  `WillEnterForeground` / `DidReceiveMemoryWarning` notifications (posted with
  or without scenes). The App side is unit-tested (the hook, the frame clock
  suspended, no repaint until coming back); CI sends `touchdemo` to the
  background on the simulator by launching another app and back by launching
  it again, and checks both events and that no view was built in between; the
  memory warning is posted in-process. These steps passed in CI once the accessibility fix landed.
- ✅ Mobile app lifecycle on Android: `onPause` as `Lifecycle::Background`
  (the callback waits until the app has handled it), `onResume` after it as
  `Foreground`, and `onLowMemory` and `onTrimMemory` (running low, critical,
  moderate, complete; heard through a run-time `ComponentCallbacks2`) as
  `LowMemory`, through the shared API (frames stop in the background).
  **Verified on an API 34 emulator** (Home and `am start`, `am
  send-trim-memory RUNNING_LOW`; checked in CI).
- ✅ Saving and restoring state across process death on Android:
  `App::save_state(|state| -> Vec<u8>)` when Android asks
  (`onSaveInstanceState`; `Event::SaveState` answered with
  `platform::save_state` for a raw handler), kept by `NativeActivity` in the
  saved `Bundle` as a byte array, and `App::restore_state(|state, bytes|)` from
  `ANativeActivity_onCreate`'s saved state before the first frame. No serde: the
  format is the app's. Unit-tested in the App; **verified on an API 34
  emulator** (Home, `am kill`, relaunch: `androiddemo`'s tap count comes back;
  CI steps added). iOS and the web: see the next item.
- ✅ Saved state on iOS and the web, through the same `App::save_state` /
  `restore_state`. iOS: each `didEnterBackground` (and `willTerminate`) asks
  the app and writes the bytes to a file in its `Library/Caches`; the next
  launch reads and deletes it before the first frame, and coming back to the
  foreground deletes it (stale). Chosen over UIKit's state restoration, which
  is tied to view controllers with restoration identifiers and saves when
  UIKit likes; unlike it, a swipe-away in the app switcher still restores (iOS
  doesn't tell a suspended app). Web: `stipple.js` asks on `pagehide` and on
  `visibilitychange` to hidden (`stipple_save_state`, answered through a
  `save_state` import) and keeps the bytes, base64, in the tab's
  `sessionStorage` under a per-module, per-canvas key (`stateKey` option);
  the next load in the tab hands them over (`stipple_restore_state`) before
  `stipple_start`. The file handling is unit-tested on every host; headless
  Chrome reloads `webdemo` (`index.html#restore`) and its count comes back
  (run locally; CI step added); CI's simulator backgrounds `touchdemo`, ends
  it with `simctl terminate` and relaunches it, checking the field's text came
  back (added, not yet run).
- ✅ Back navigation on Android: the Back key (and the back gesture, which
  sends it while the app doesn't opt into predictive back) arrives as
  `Event::BackRequested`; the App closes an open menu, then asks
  `App::on_back(|state| -> Handled)`; a raw handler answers with
  `platform::handle_back()`. Unhandled, the system's default runs
  (`Activity.onBackPressed`: the activity finishes, or on Android 12+ a root
  task moves to the back). A Back that first closes the soft keyboard stays
  with the keyboard. Unit-tested in the App; **verified on an API 34
  emulator** (`input keyevent BACK`, handled then not; CI steps added with
  this change).
- ✅ Predictive back on Android 13+: `App::back_enabled(|state| bool)`
  says beforehand whether the app would go back (an open menu counts; without
  it, an `on_back` hook means yes), re-evaluated after each event; the backend
  registers an `OnBackInvokedCallback` (a hand-written dex class, loaded on its
  own) on the activity's `OnBackInvokedDispatcher` only while it's true, so the
  system previews and performs its own back otherwise. On Android 14+ it's an
  `OnBackAnimationCallback`: `App::on_back_progress` hears
  `BackGesture::Started { edge }`, `Progressed { progress }` and `Cancelled`.
  Opt-in with `android:enableOnBackInvokedCallback` (the demo sets it); the
  Back key path stays for older releases and apps without the flag. Unit-tested
  in the App; **verified on an API 34 emulator** (key Back through the
  callback, an edge swipe with progress, a cancelled swipe, a declined Back
  going to the system; CI steps added); API 33 build-checked only.
- ✅ iOS's interactive edge swipe for going back: while `back_enabled` says
  yes, a finger from the leading edge (the left, the right under a
  right-to-left theme, which the App passes on) that moves inward is the back
  gesture, recognized by the backend in its touch path (a small state machine
  kept free of UIKit) rather than by a `UIScreenEdgePanGestureRecognizer`, so
  synthetic touches go through it too: `BackGesture::Started` / `Progressed`
  (inward distance over the width) / `Cancelled`, and `Event::BackRequested`
  (`on_back`) for a swipe past half way or a flick. The finger reaches the
  app until recognized (its moves wait while undecided) and is cancelled
  there once it is; a vertical drag, a second finger, or back being off
  leave the edge's touches to the app. A raw handler's edge stays its own
  until it calls `set_back_enabled(true)`. Unit-tested (the recognizer, and
  driving an `App` with a strip at the edge); `touchdemo`'s simulator
  self-test swipes over an edge-to-edge strip with back off (the strip
  scrolls), on (back, the strip stays) and taken back (cancelled). The iOS
  steps are added and haven't run yet. Edge measured from the view (the safe
  area), so a landscape notch inset isn't covered.
- ✅ Mobile gestures, recognized in the shared core (`core::touch`) from the
  platform's touches, so iOS, Android and the web behave alike: pinch,
  two-finger pan and rotation through `Element::on_pinch` (a `PinchEvent`
  with phase, scale, rotation, translation and centroid, each measured from
  the start; the first finger's press is cancelled); a long press (half a
  second within the 8-pixel slop) opening the element's `context_menu` or
  `on_context` handler where the finger is, cancelling the tap; horizontal
  scroll containers (`widgets::scroll_horizontal`, or a horizontal stack made
  `scrollable`; right-to-left ones start at the right) that a sideways drag
  scrolls and flings, a vertical drag inside one still scrolling the list
  around it, and a horizontal wheel scrolls too; and touch events in mobile
  browsers (`touchstart` / `move` / `end` / `cancel` to `stipple_touch`, the
  same path). Unit-tested with synthetic touch sequences, recognizer and App;
  `touchdemo`'s iOS simulator self-test long-presses a row and picks its menu
  item, drags a strip sideways and pinches with two synthetic fingers through
  the backend's touch path; the Android emulator long-presses (`input swipe x
  y x y 800`) and swipes sideways (no pinch: `adb` has one finger); headless
  Chrome taps and long-presses with synthetic `TouchEvent`s. The Android
  steps and the web selftest were run locally (API 34 emulator, headless
  Chrome) and pass; the iOS steps are added with this change and haven't run
  yet. Not yet tried on a real phone. See
  [docs/pointer-events.md](docs/pointer-events.md#touch-gestures).
- ⬜ Mobile text editing UI: touch caret placement and selection handles,
  the system's edit menu (cut / copy / paste / select all; `UIEditMenuInteraction`
  on iOS, the floating action mode on Android), and keeping the focused field
  above the on-screen keyboard: on iOS the keyboard can cover a field today
  (follow the keyboard frame notifications); Android already shrinks the
  content, but nothing scrolls the focused field into view.
- ✅ Menus on mobile: `Element::context_menu` opens on a long press as the
  menu Stipple draws, on iOS, Android and the web alike (chosen over `UIMenu`
  / `UIContextMenuInteraction` and `PopupMenu`: it follows the theme, keeps
  one behavior and needs no more native code); lifting the finger picks
  nothing, a tap picks an item. The app's menu bar is the drawn in-window bar,
  its menus opening on a tap; phone apps leave `App::menu` out (documented in
  [docs/menus.md](docs/menus.md#context-menus)). Checked as for gestures,
  above; the menu bar isn't run in CI on mobile.
- ✅ File pickers on mobile: `dialog::open_file_contents` /
  `save_file_contents` (and the new `save_file_contents_then`, which says
  whether the bytes were written) answer asynchronously from the event loop,
  as on the web. iOS presents a `UIDocumentPickerViewController`
  (`initForOpeningContentTypes:asCopy:` with `UTType`s made from
  `FileDialog::accept`, or `initForExportingURLs:asCopy:` for a tmp file
  holding the bytes) with a delegate class built at run time. Android starts
  `ACTION_OPEN_DOCUMENT` / `ACTION_CREATE_DOCUMENT` (MIME types from `accept`,
  the name from `file_name`) from a headless `Fragment` defined in the
  run-time dex, whose `onActivityResult` gets the document; it's read or
  written through `ContentResolver.openFileDescriptor` on a worker thread. An
  `ActivityLifecycleCallbacks` (also in the dex) drops the fragment from the
  saved state, so the activity survives process death under the picker. The
  `accept` mapping is unit-tested. CI's Android emulator drives DocumentsUI
  with uiautomator: picks a file and reads it, cancels, saves and reads the
  file back, and kills the process under the picker; the iOS simulator
  self-test checks both pickers come up and that cancelling reaches the
  callback, and, since choosing a file there can't be scripted, opens each
  again and hands its delegate a file URL itself
  (`documentPicker:didPickDocumentsAtURLs:`, `simulate::pick_document`): the
  open callback gets the written file's name and bytes (and the copy is
  deleted), the export's callback `true` (added, not yet run in CI). No folder
  picker.
- ✅ Notifications on iOS: `platform::notify` through
  `UNUserNotificationCenter`; the first call asks permission
  (`requestAuthorizationWithOptions:`, with a hand-built completion block) and
  the notification waits for the answer, later ones go straight through or
  fail once refused; a run-time delegate shows them as banners while the app
  is on screen. `touchdemo`'s simulator self-test checks `notify` hands the
  notification over and the center reports the permission
  (`getNotificationSettingsWithCompletionHandler:`); nobody answers the prompt
  in CI, so delivery after a grant is build-checked only. These steps passed in CI once the accessibility fix landed.
- ✅ Notifications on Android: `platform::notify` through
  `NotificationManager` on a channel of the app's (API 26+), with the app's
  icon and a launch `PendingIntent`. On API 33+ the first call without
  `POST_NOTIFICATIONS` asks (`requestPermissions` from the results fragment,
  answered in its `onRequestPermissionsResult`) and the notification waits;
  once refused `notify` fails. CI's API 34 emulator refuses the system prompt
  through uiautomator, checks the next `notify` fails, `pm grant`s the
  permission and finds the notification in `dumpsys notification`.
- ⬜ Drag and drop from other apps on mobile: `UIDropInteraction` on iOS
  (iPad split view) and `View.OnDragListener` on Android, feeding the existing
  `DragEnter` / `Drop` events.
- ✅ iOS: following appearance changes live: `traitCollectionDidChange:` on
  the view, when the screen's `userInterfaceStyle` changes (the screen's
  traits, as UIKit flips a backgrounded app's own to snapshot it), and on
  coming back to the foreground, → `Event::ThemeChanged`; the native theme is
  rebuilt and the area around the safe area takes the new background. CI
  switches the simulator with `xcrun simctl ui ... appearance dark` while
  `touchdemo` runs and checks its theme turned dark. These steps passed in CI once the accessibility fix landed.
- ⬜ iOS: trackpad / mouse scrolling on iPad (`UIPanGestureRecognizer` with
  indirect scroll input).
- ⬜ Mobile CI depth: run the iOS features that are only implemented (timers,
  clipboard, opening URLs) on the simulator, with real `UITouch`es rather than
  the synthetic touch path, and try both platforms on physical devices before
  calling them supported.
- ✅ Accessibility on the web (ARIA): `web/stipple.js` mirrors the
  accessibility tree in an invisible overlay of ARIA elements over the canvas
  (roles, names, `aria-selected` / `aria-expanded` / `aria-checked`), updated
  only where the tree changed; a click on an element (a screen reader's
  "activate") taps its node, and focus follows the app's and, for text fields,
  the screen reader's. The tree gained checkbox and switch roles and a
  `checked` state (`Element::checked`). CI's web selftest checks the roles and
  names, activation and `aria-checked` in headless Chrome; not yet tried with a
  real screen reader.
- ✅ Accessibility checked from outside the app, as close to a screen reader
  as CI gets (see [docs/accessibility.md](docs/accessibility.md), which says
  what each check proves):
  - **iOS**: an XCUITest bundle (an Xcode project XcodeGen generates on the
    runner, `.github/a11y/ios`) drives `touchdemo` from the test runner's
    process over the AX channel VoiceOver uses: reads the tree (button, static
    text and text field counts, the whole snapshot), checks "Row 0" has a
    frame on screen and is hittable, taps it and sees "Rows tapped" follow,
    taps the field, types "Hi" on the software keyboard's keys and reads the
    field's value. For it, a text field's element now adds the traits a
    `UITextField` reports (UIKit has no public text-entry trait), so XCUITest
    sees a `textField` and VoiceOver says "text field". Added to the
    `visual-ios` job; not yet run in CI.
  - **Android**: TalkBack isn't on the emulator images (it comes from the
    Play Store), so a small accessibility service stands in for it
    (`.github/a11y/android`, built with javac/d8/aapt, no Gradle), enabled with
    `settings put secure enabled_accessibility_services` as TalkBack is. From
    its own process it walks the window's nodes, moves accessibility focus
    through every node TalkBack would stop on (checking `findFocus` and the
    events after each), clicks "Tap me", sets the field's text, and tries a
    scroll action. **Run on a local API 34 emulator**: 19 of 19 stops took
    focus, the click and the text reached the app. It found two things, one
    fixed: an empty text field read as " " (the space `text_editor` lays out
    to keep its height; now empty), and a plain scroll container offers no
    scroll actions (below). Added to `visual-android`.
  - **Windows**: `.github/scripts/uia-walk.ps1` walks a running `touchdemo`
    from PowerShell's process with the .NET UI Automation client (the path
    Narrator and NVDA use): the control-view tree from the desktop root, the
    button / field / label with bounds inside the window on screen, a hit
    test, the button invoked through its Invoke pattern (the app and the tree
    follow), and the focused element after a Tab. For it the provider tree
    gained what an out-of-process client needs: the root is hosted by the
    window's provider (`UiaHostProviderFromHwnd`), bounds are screen pixels
    (they were logical client coordinates), hit testing finds the deepest
    node, buttons have the Invoke pattern (a click at the node), keyboard
    focusability and focus are reported, the tree is rebuilt only when it
    changes, and a replaced tree is kept while a client holds its elements
    (it was freed every frame under them). Added to `visual-windows`; not yet
    run in CI.
  - **macOS**: `axprobe` (`.github/a11y/macos/axprobe.swift`) reads a running
    `touchdemo` through the AX API (`AXUIElementCreateApplication`) from its
    own process, as VoiceOver does: roles, labels, frames, the focused
    element, and an `AXPress` attempt. Reading another app needs TCC's
    accessibility permission; the step reports `AXIsProcessTrusted`, tries
    adding the probe to the system TCC database when refused (SIP is off on
    GitHub's runners), and warns rather than fails if it still can't read.
    Element frames are now in screen coordinates (they were the node's
    logical bounds). Added to `visual-macos`; not yet run in CI.
- ⬜ Accessibility on Linux for real: the AT-SPI server
  (`stipple::platform::a11y`) is only used by `a11ydemo`. X11 and Wayland
  windows ignore `Window::set_accessibility_tree`, and nothing registers with
  the AT-SPI registry (`org.a11y.atspi.Socket.Embed` on the accessibility
  bus), so Orca sees no Stipple app. Needed: serve the live tree from the event
  loop on the accessibility bus, register with the registry, implement
  `Application`, `Component` (extents, hit testing), `Action` (activate),
  `Text`/`EditableText` and `State`, emit `children-changed` / focus events,
  then check it from outside with a real AT-SPI client (pyatspi, as Orca uses)
  under Xvfb and sway.
- ⬜ Accessibility depth found by the out-of-process checks: a plain
  scrollable container (`Element::scrollable`) is a group with no scroll
  actions on every platform (only lists, trees and tables have them); UIA has
  no Value, Toggle, Selection, ExpandCollapse, Scroll or Text patterns and
  raises no events; macOS elements have no press action (VoiceOver's VO-Space
  does nothing) and no value for fields; no platform posts announcements for
  live regions (toasts).
- ⬜ Try the real screen readers: VoiceOver (macOS and iOS), TalkBack,
  Narrator / NVDA, Orca, by a person, on each platform.
- ✅ Images on the GPU path, so OS-drawn controls render under `App::render_with`;
  and with them the rest of the scene, so the GL scene renderer no longer falls
  back to the CPU for transforms, opacity, paths or canvases.
  `stipple::gpu::render_scene_scaled(scene, background, scale)` walks the
  scene's node tree in paint order into device-space operations
  (`src/gpu/ops.rs`, backend-neutral) that GLES2 executes on a persistent
  context: upright boxes through an analytic-coverage shader; every other fill
  and stroke flattened and stroked as `oxideav-raster` does, then filled by
  stencil-and-cover with 4×4 supersampled coverage; bitmaps as textured quads
  whose textures are cached by the image's `Arc` (uploaded once, freed when
  the image is dropped or unused for 120 frames; images stay `Pixmap`s, so the
  oxideav conversions are unchanged); text from the scene's own glyph outlines,
  each glyph rasterized once per size, color, transform and sub-pixel position
  and kept across frames; layers with group opacity in pooled offscreen
  targets; clips as scissors or stencil polygons; canvases painted for the
  part on screen. Only gradients, soft masks and concave clips are rasterized
  on the CPU, for their own bounds. The scale is now honored (it rendered at
  1x before), and text follows the scene's fonts and bidi. Unit tests render
  boxes, paths, images, clips, layers, text, a canvas and an app (through
  `App::render_with`) both ways at scales 1–2 and compare them: under 0.25 of
  255 mean difference per channel on Mesa's llvmpipe and on an NVIDIA GPU;
  CI's GPU job runs them on llvmpipe (`STIPPLE_REQUIRE_GPU`). Linux has no
  OS-drawn controls, so those are covered as ordinary images. Not done: the
  other GPU backends (Vulkan, D3D11, Metal, WebGPU) executing the same
  operations, presenting without the read-back, and a glyph atlas. See
  [docs/gpu.md](docs/gpu.md).
- ✅ The web's exports in the library: `stipple::web_app!(app)` generates them,
  and `web/stipple.js` is the whole page side, so a web app provides only its
  `App`. `App::run` on the web keeps the handler and returns; the page drives
  it from DOM events.
- ✅ High-DPI on the web: `devicePixelRatio` is the scale factor, the canvas's
  backing store is its CSS size times it, and size and ratio changes re-render.
  CI runs the web selftest at a ratio of 2.
- ✅ The web: IME and dead keys, following `prefers-color-scheme`, and more
  than one app per page. Keyboard focus is a hidden `<textarea>` placed at the
  caret: composition events become `Ime::Preedit` / `Ime::Commit`, and `input`
  events carry dead-key characters, while `keydown` keeps named keys and
  shortcuts. `Theme::native()` follows `prefers-color-scheme` and its `change`
  event (`Event::ThemeChanged`), with the CSS `AccentColor` where the browser
  has one. Each `start` instantiates the module afresh, so canvases run apps
  side by side (`examples/webdemo/multi.html`). CI's selftest checks
  composition and a dead key with synthetic events, runs once with a dark and
  once with a light scheme (a live switch is simulated through the same export
  the `change` listener calls), and starts a second app on the page; no real
  IME was run.

### Phase 8 — A rich, animated, reactive UI
*Goal: interfaces that respond to everything the user does, with motion, and
without hand-written frame bookkeeping in every view. Example: a drop area that
grows and glows while a file is dragged over it, tints red when the payload
isn't accepted, and springs back when the drag leaves or the file lands.*

Today a view can animate, but only by sampling `Tween` / `Spring` at
`Cx::frame_time()` itself and calling `Cx::request_frame()`; only hover and
press highlights animate on their own. Phase 8 makes motion and state-driven
styling part of the toolkit.

- ✅ Interaction states as values a view can read and style from: hovered,
  pressed, focused, focus-visible, disabled, selected, dragged, and drag-over
  (with whether the payload is accepted), per element, through `Cx` and as
  per-state style overrides (`.hovered(|e| …)`, `.drag_over(|e| …)`).
  *`Interaction` (a set of states); `Cx::interaction(key)` for keyed elements;
  `.hovered` / `.pressed` / `.focused` / `.focus_visible` / `.dragged` /
  `.drag_over` / `.drag_refused` and `.when(states, f)` overrides (fill,
  radius, border, padding, fixed size, text color, cursor, opacity,
  transform), resolved when the
  tree is lowered for layout; `Element::key` for identity (else the tree
  path); `.disabled(bool)` (look only: handlers still run) and `.selected`.
  States come from the frame on screen and are recomputed on pointer, button,
  focus and drag events; the app rebuilds only when a state some element (or
  the view, through `Cx`) uses changed. Focus and drag states count elements
  inside; `focused` is focus-within. The pointer leaving the window (reported
  by every backend) ends the hover. See `docs/animation.md`.*
- ✅ Implicit transitions: an element's animatable properties (colors, opacity,
  corner radius, border, shadow, padding, size, transform) declare a
  `Transition` (duration and easing, or a spring), and changing them between
  builds animates instead of jumping. The App keeps the in-flight values by
  element identity and asks for frames only while something moves.
  *`.transition(Transition::ease / tween / spring)` and
  `.transition_for(AnimProp, …)`; `AnimProp` covers fill, border (color and
  width), radius, padding, fixed width/height (between fixed sizes only), text
  color, opacity and transform; fills and borders fade in and out. Retargeting
  continues from the current value and velocity (each property is an
  `anim::Animated`, a `Transition` wraps a `Motion`, so reduced motion applies). Unit-tested at
  fixed frame times; `dropdemo`'s zones use `.drag_over` + transitions. No
  shadows yet (the scene has none); a property is added by an `AnimProp`
  variant in `src/core/transition.rs`.*
- ✅ Transforms and opacity in the scene: scale, rotate and translate about an
  origin, and group opacity, for any element and its subtree, with hit-testing
  that follows the transform. `Element::transform(Transform)` and
  `Element::opacity(f32)` lower to `Scene::push_layer` (an oxideav group with
  a transform and an offscreen-composited opacity on the CPU path); every
  pointer query maps through the inverse transforms, handlers get positions in
  the element's own space, the hover/focus/drop overlays and damage follow the
  transform, and text renders from outlines under scale and rotation.
  `transformdemo` shows it. Not covered: transformed embedded-viewport
  content (moved only), the IME preedit underline and caret rect, skew/3D, and
  transformed bounds in the accessibility tree. See
  [docs/transforms.md](docs/transforms.md).
- ✅ Enter and exit transitions for keyed elements (fade, slide, scale), so
  appearing and disappearing items animate; exiting elements stay drawn until
  their transition ends.
  *`.enter(Effect)` / `.exit(Effect)` / `.appear(Effect)`; `Effect::fade`,
  `slide`, `scale`, `custom(opacity, transform)`, combined with `.and` and
  timed with `.transition(Transition)`. Enter plays when an element appears
  after the window's first frame. An element that leaves is kept as a ghost
  (the last drawn copy, without handlers, keys or semantics: no hover, clicks,
  focus or accessibility node, and no room in the layout) under its old
  parent at its old index and place until its exit ends; built again
  meanwhile, it turns round from where it is. Applied on the laid-out tree as
  a transform and opacity around the element's own (`src/core/presence.rs`),
  on `Animated` values, so reduced motion snaps (no ghost at all).
  Unit-tested at fixed frame times (mid-way, ghost drawn then dropped, coming
  back, reduced motion) and through `App` (clicks pass the ghost);
  `examples/listmotion --selftest` in CI, and on X11 CI screenshots
  `listmotion --demo` mid-flight and checks it then rests with no CPU use.
  See [docs/animation.md](docs/animation.md#enter-exit-and-layout-animations).*
- ✅ Layout animations: when an element's position or size changes because the
  layout did (a list reordering, a panel opening), it moves there smoothly.
  *`.animate_layout(Transition)` (position and size, the size by scaling) and
  `.animate_position(Transition)` (position only), FLIP-style: each frame
  compares the element's laid-out box with the last one's and draws it
  translated (and scaled) back to where it was, animating to none.
  Measured from the nearest layout-animated ancestor, else the scroll
  container's content, else the window, so nested elements don't move twice,
  scrolling doesn't animate and a window resize snaps. Retargets mid-flight
  from the current place and velocity. Unit-tested (reorder mid-flight,
  retarget, size, scroll and resize, reduced motion); `listmotion` shows it.
  Not covered: a moving element is drawn in its new tree order (it can pass
  under its siblings), and a size change scales content rather than
  re-laying it out.*
- ✅ Interruptible, velocity-preserving animations: a new target mid-flight
  continues from the current value and speed; gestures (drags, flings, touch
  scrolling) can drive an animation directly and hand off to a spring on
  release. `anim::Animated<T>` retargets (`animate_to` / `spring_to` /
  `tween_to`) from the current value and velocity — springs take it as is,
  tweens blend it in with a correction that fades out — and takes gestures
  (`drag_to`, then `release` / `release_with_velocity` into a spring or
  tween, or `fling` into a friction `Decay`). Generic over the
  `Animatable` trait (`f64`, `Color` in premultiplied sRGB, `Point`,
  `Vec2`, `Size`, `Rect`, `Insets`, `[f64; N]`, tuples; open to other
  types); `Tween` and `Spring` are generic too, `Spring` in closed form with
  `SpringParams` presets. `Cx::animate` reads one at the frame time and asks
  for frames only while it moves. Not yet wired into touch scrolling; the
  built-in widgets use it (below).
- ✅ Keyframes and sequences/staggers for richer effects (a shake on invalid
  input, a pulsing drop target, staggered list entrance). Every animation is a
  pure `Animation` (`sample(t)`, `velocity(t)`, `is_finished(t)`):
  `Keyframes` with per-segment easing, `.delay`, `.repeat(n)` /
  `.repeat_forever()` / `.alternate()`, `.then` sequences, tuples as
  parallel groups, `stagger` / `stagger_within`, CSS `cubic-bezier` easing
  with named curves, and the `shake`, `pulse` and `entrance` effects.
  `Cx::play` drives one from a start time. `examples/motiondemo` shows them,
  checked at fixed times by `--selftest`; see
  [docs/animation.md](docs/animation.md).
- ⬜ Finer-grained reactivity: derived/memoized values and subtrees that rebuild
  only when their inputs change, so large animated UIs stay cheap; damage
  limited to what actually moved.
- ✅ Theme changes that cross-fade (also listed under 7a): when the theme views
  see changes (the system's look under `Theme::native()`, or the new
  `App::theme_with` deriving it from the state), the App blends the new frame
  over the one that was on screen for 200 ms (`core::THEME_FADE`), asking for
  frames only meanwhile and re-blending without rasterizing when the view is
  unchanged. It works on finished pixels, so a GPU renderer set with
  `App::render_with` fades too. Unit-tested at fixed frame times (the blend,
  restarts, resizes) and through `App::render_frame`; `webdemo`'s selftest
  checks in headless Chrome that a scheme change animates. Seen nowhere else
  in CI, which can't switch the system's look.
- ✅ Respect the OS reduced-motion setting: Windows (`SPI_GETCLIENTAREAANIMATION`,
  followed through `WM_SETTINGCHANGE`), macOS (`NSWorkspace`'s
  `accessibilityDisplayShouldReduceMotion` and its change notification), iOS
  (`UIAccessibilityIsReduceMotionEnabled` and its notification), Android
  (`animator_duration_scale` over JNI, read again on resume), Linux (GNOME's
  `enable-animations` or KDE's `AnimationDurationFactor` through the desktop
  portal, followed through `SettingChanged`; GTK's `settings.ini` without a
  portal) and the web (`prefers-reduced-motion` and its `change` event).
  `anim::motion_scale()` / `anim::motion_duration()` scale durations (0 when
  reduced, Android's own scale otherwise), `anim::reduced_motion()` and
  `Cx::reduced_motion()` ask, and `anim::set_reduced_motion` overrides it for
  apps and tests. `Animated`/`Motion` honor it centrally (tweens scale,
  springs, flings and played effects snap). Under it, highlight and drop fades
  and theme changes snap,
  touch scrolling doesn't fling, and the spinner turns at half speed. CI checks
  the web in headless Chrome with `--force-prefers-reduced-motion` (the app sees
  it and a theme change snaps); the shared behavior is unit-tested; the
  desktop and mobile readers are only build-checked, as CI can't switch the
  setting.
- ✅ Built-in widgets use all of this: drop areas, buttons, switches,
  checkboxes, tabs, menus, toasts, dialogs and disclosure triangles animate by
  default under every theme (and natively-matched curves under
  `Theme::native()`).
  *Per-theme motion tokens (`Theme::motion`, a `MotionTokens`: `state`,
  `press`/`press_scale`, `toggle`, `indicator`, `popup`, `dialog`, `toast_*`,
  `disclosure`), with presets `standard()` (the light/dark themes),
  `material()` (Material 3 themes, Android), `fluent()` (Windows), `macos()`,
  `ios()` and `adwaita()` (Linux) that `Theme::native()` picks per platform,
  their sources documented. Buttons fade their hover/press looks and shrink
  slightly (standard only); switches slide their thumb and fade the track;
  check marks and radio dots pop in; segmented `tabs` slide their accent
  fill to the new tab (a keyed, layout-animated indicator in a layer between
  the fills and the labels, through the new `Element::layers`) and fade the
  rest; the tab bar's accent strip slides between tabs; select lists, drawn
  menus, `open_menu` and the tab bar's list grow in and fade/shrink back out
  (`popup_exit`); tooltips fade in and out; toasts slide in, fade back out
  (kept drawn while leaving) and glide into place; the drawn message box and
  `open_dialog` scale in and back out with their scrim (`dialog_exit`); tree
  disclosure triangles turn; the app's drop-target highlight fades with the
  drag-over level. All through state overrides + transitions, `Animated`,
  transforms and opacity, and the core's enter, exit and layout animations:
  an overlay's content with an exit effect is kept as a ghost over the
  window after the view stops declaring it (under a node holding the
  window's layers; overlays are identified per kind of anchor, so one
  closing doesn't rename the others), taking no clicks, focus, hover or
  screen-reader attention while it leaves, and a modal's scrim fades with
  it; reduced motion snaps all of it. Nothing changes at rest (layouts and
  pixels are identical; checked under Xvfb against the previous build for
  `widgetsdemo`, `sessionsdemo`, `tabsdemo`, `bididemo`, `material3demo` and
  `themegallery`) and what's open in a widget's first build doesn't animate
  in. Unit-tested at fixed frame times (`widgets::motion_tests`,
  `core::presence`), with App-level tests that a closing select list and a
  closing dialog's scrim let clicks through and that the sliding tab
  indicator, presented through damage regions only, leaves no stale pixels
  (which caught borders' anti-aliased edges missing from the damage). A
  modal's scrim also fades in, on its dialog's enter motion (the drawn
  message box now comes in through the core's enter animation too, keyed by
  box, so the next one comes in as the last leaves); at rest the frames are
  identical (checked under Xvfb against the previous build with
  `sessionsdemo`'s message box and `overlaydemo`'s dialog open). Controls the
  OS draws under `Theme::native()` cross-fade from the old state's picture to
  the new one (`core::native_fade`: hover, press and focus on `state`, a
  check, radio or switch flipping on `toggle`; slider values and dark-mode
  switches snap), unit-tested only, as CI can't hover native controls on
  Windows or macOS. See [docs/widgets.md](docs/widgets.md#motion).*
- **Exit criteria:** a showcase example (drop area, animated list, interactive
  controls) whose animations are unit-tested at fixed frame times, with CI
  screenshots of mid-transition frames on the desktops and the web, and no CPU
  use once everything is at rest.

---

## 4. Cross-cutting tracks (continuous)

- **Testing:** golden-image rendering tests, layout solver unit tests, input/
  event simulation, per-platform smoke tests in CI.
- **Docs & examples:** an examples gallery that doubles as the conformance
  corpus; rustdoc; platform and topic guides in [`docs/`](docs/README.md).
- **Tooling (stretch):** a UI inspector/devtools overlay; hot-reload of `view`.
- **Packaging:** app bundling per platform (`.app`, MSIX, APK, IPA, wasm).

---

## 5. Key risks & mitigations

| Risk | Mitigation |
|---|---|
| CPU rasterization too slow for large/animated/4K UIs | Damage tracking + layer/glyph caching in Phase 1; GPU `Surface` (Phase 6) behind the same trait. |
| Per-OS windowing/IME is a deep, hand-written surface | Confine to `stipple::platform`; freeze the trait after Phase 2; ship one OS fully before porting. |
| Reactive runtime + Rust ownership friction | Prototype the `View`/state model in Phase 1 against real demos before widening the widget set. |
| Accessibility is hard when self-drawing everything | A semantics tree built from the element tree, with hand-written bridges to AT-SPI, UI Automation, NSAccessibility, UIAccessibility (iOS) and ARIA (the web), and an `AccessibilityNodeProvider` on Android. |
| The native theme leans on OS internals | Windows' dark control themes (`DarkMode_*`) are undocumented, and AppKit and UIKit restyle controls between releases. Controls are drawn by the OS at run time, so they follow its current look; the self-drawn decoration stays as the fallback; CI screenshots each platform. |
| oxideav API churn | Pin via path deps now; track upstream; the `stipple::render` seam isolates oxideav from the rest. |
| Web with "minimal deps" constraint | Accept a thin, hand-audited JS-interop shim as the one web exception. |

---

## 6. Decisions locked / still open

**Locked:** software-first rendering with a GPU-ready `Surface` seam;
reactive/declarative public API; self-drawn widgets, with `Theme::native()` as
the opt-in exception where the OS draws a control's chrome; a single crate, with
optional parts behind features; platform order desktop-trio → mobile → web.

**Locked: threading and async (November 2026).** The UI runs on one thread, the
main thread, which stays the OS's event loop: macOS and iOS require it. Changes
from elsewhere go through an `AppHandle`: closures over the state, queued from
any thread and run on the UI thread between events, so the state needs no locks.
Each backend's wait also wakes for the handle and for timers. Futures run on
tokio, behind an optional `tokio` feature, on tokio's own threads, with their
output applied through the handle; tokio never owns the main thread. Other
runtimes post through the handle the same way. See [`docs/async.md`](docs/async.md).

**Still open:**
- State/reactivity primitives: signals vs. message/`update`-reducer vs. hybrid.
  (Updates stay closures over the state for now.)
- Styling authoring: pure-Rust builder API only, or an optional declarative
  style/theme description format.
```
