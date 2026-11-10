//! A color picker: a saturation/value area, a hue strip, an alpha strip, a hex
//! field and a row of swatches, inline ([`color_picker`]) or in a popup under
//! a swatch button ([`color_button`]).

use std::cell::RefCell;
use std::sync::Arc;

use crate::core::{Align, Anchor, Axis, BoxStyle, Cx, Element, KeyInput, OverlaySpec, Role};
use crate::geometry::{Insets, PhysicalSize};
use crate::render::{Color, ImageFit, Pixmap};
use crate::style::Theme;

use super::{EditBuffer, text_editor};

/// The saturation/value area's size, in logical pixels.
const AREA_W: f64 = 220.0;
const AREA_H: f64 = 140.0;
/// The hue and alpha strips' height.
const STRIP_H: f64 = 14.0;
/// The bitmaps are drawn at twice their logical size, for high-DPI screens.
const OVERSAMPLE: f64 = 2.0;

/// The state of a [`color_picker`] or [`color_button`]: the color as hue,
/// saturation, value and alpha (so the hue survives a trip through gray), the
/// hex field's text, and whether the button's popup is open. Keep one in your
/// app state per picker.
///
/// ```
/// use stipple::render::Color;
/// use stipple::widgets::ColorPicker;
/// let mut p = ColorPicker::new(Color::rgb(255, 0, 0));
/// p.set_saturation_value(0.0, 1.0); // white…
/// assert_eq!(p.color(), Color::WHITE);
/// p.set_saturation_value(1.0, 1.0); // …and back to red: the hue held
/// assert_eq!(p.color(), Color::rgb(255, 0, 0));
/// assert!(p.set_hex("#00ff0080"));
/// assert_eq!(p.color(), Color::rgba(0, 255, 0, 0x80));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ColorPicker {
    hue: f64,
    saturation: f64,
    value: f64,
    alpha: u8,
    alpha_enabled: bool,
    hex: EditBuffer,
    open: bool,
    swatches: Vec<Color>,
}

impl Default for ColorPicker {
    fn default() -> Self {
        Self::new(Color::WHITE)
    }
}

/// The swatches a picker offers unless given others.
const DEFAULT_SWATCHES: [Color; 10] = [
    Color::rgb(0xe5, 0x48, 0x4d),
    Color::rgb(0xf7, 0x6b, 0x15),
    Color::rgb(0xff, 0xc5, 0x3d),
    Color::rgb(0x46, 0xa7, 0x58),
    Color::rgb(0x12, 0xa5, 0x94),
    Color::rgb(0x00, 0x90, 0xff),
    Color::rgb(0x6e, 0x56, 0xcf),
    Color::rgb(0xd6, 0x40, 0x9f),
    Color::rgb(0x8b, 0x8d, 0x98),
    Color::rgb(0x1c, 0x20, 0x24),
];

impl ColorPicker {
    /// A picker showing `color`, with an alpha strip, closed.
    pub fn new(color: Color) -> Self {
        let mut p = Self {
            hue: 0.0,
            saturation: 0.0,
            value: 0.0,
            alpha: 255,
            alpha_enabled: true,
            hex: EditBuffer::new(),
            open: false,
            swatches: DEFAULT_SWATCHES.to_vec(),
        };
        p.set_color(color);
        p
    }

    /// Show the alpha strip (`true`, the default), or keep colors opaque.
    pub fn with_alpha(mut self, enabled: bool) -> Self {
        self.alpha_enabled = enabled;
        if !enabled {
            self.alpha = 255;
            self.sync_hex();
        }
        self
    }

    /// The swatches offered below the strips.
    pub fn with_swatches(mut self, swatches: Vec<Color>) -> Self {
        self.swatches = swatches;
        self
    }

    /// The color picked.
    pub fn color(&self) -> Color {
        Color::from_hsv(self.hue, self.saturation, self.value, self.alpha)
    }

    /// Show `color`. A gray keeps the hue the picker had.
    pub fn set_color(&mut self, color: Color) {
        let (h, s, v) = color.to_hsv();
        if s > 0.0 && v > 0.0 {
            self.hue = h;
        }
        self.saturation = s;
        self.value = v;
        self.alpha = if self.alpha_enabled { color.a } else { 255 };
        self.sync_hex();
    }

    /// The hue, in degrees (0..360).
    pub fn hue(&self) -> f64 {
        self.hue
    }

    /// Set the hue, in degrees.
    pub fn set_hue(&mut self, hue: f64) {
        self.hue = hue.clamp(0.0, 360.0);
        self.sync_hex();
    }

    /// Set the saturation and value (each 0..=1).
    pub fn set_saturation_value(&mut self, saturation: f64, value: f64) {
        self.saturation = saturation.clamp(0.0, 1.0);
        self.value = value.clamp(0.0, 1.0);
        self.sync_hex();
    }

    /// Set the alpha (0..=1); ignored when the alpha strip is off.
    pub fn set_alpha(&mut self, alpha: f64) {
        if self.alpha_enabled {
            self.alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            self.sync_hex();
        }
    }

    /// Set the color from a hex string (`#rgb`, `#rrggbb`, `#rrggbbaa`…).
    /// Returns whether it parsed; the hex field keeps what was typed either
    /// way.
    pub fn set_hex(&mut self, hex: &str) -> bool {
        let Some(c) = crate::render::Color::from_hex(hex.trim()) else {
            return false;
        };
        let typed = self.hex.clone();
        self.set_color(c);
        self.hex = typed;
        true
    }

    /// The hex field's text.
    pub fn hex(&self) -> &str {
        self.hex.text()
    }

    /// Whether [`color_button`]'s popup is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open or close [`color_button`]'s popup.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            // Leave the field showing the color, not a half-typed value.
            self.sync_hex();
        }
    }

    fn sync_hex(&mut self) {
        let mut hex = EditBuffer::from_text(self.color().to_hex());
        hex.clear_history();
        self.hex = hex;
    }

    /// Apply a key to the hex field: the color follows each edit that
    /// parses, and Enter puts the field back to the color's canonical form.
    fn hex_key(&mut self, key: &KeyInput) -> bool {
        match key {
            KeyInput::Enter => {
                self.sync_hex();
                true
            }
            KeyInput::SelectLine => {
                self.hex.select_all();
                false
            }
            other => {
                let before = self.color();
                self.hex.apply(other);
                let text = self.hex.text().to_string();
                self.set_hex(&text);
                self.color() != before
            }
        }
    }
}

thread_local! {
    /// The bitmaps last drawn, by what they show, so an unchanged picker
    /// reuses the same image (images compare by identity).
    static BITMAPS: RefCell<Vec<(Bitmap, Arc<Pixmap>)>> = const { RefCell::new(Vec::new()) };
}

/// What a bitmap shows; hue and positions quantized to what a pixel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bitmap {
    /// The saturation/value area for a hue, with the marker at (s, v).
    Area { hue: u16, s: u16, v: u16 },
    /// The hue strip, with the marker at a hue.
    Hue { hue: u16 },
    /// The alpha strip for an opaque color, with the marker at an alpha.
    Alpha { rgb: [u8; 3], alpha: u8 },
}

fn bitmap(which: Bitmap) -> Arc<Pixmap> {
    BITMAPS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, p)) = cache.iter().find(|(w, _)| *w == which) {
            return p.clone();
        }
        let p = Arc::new(draw(which));
        // A few pickers on screen at once, each with three bitmaps.
        if cache.len() >= 24 {
            cache.remove(0);
        }
        cache.push((which, p.clone()));
        p
    })
}

/// Draw `which` into a fresh pixmap.
fn draw(which: Bitmap) -> Pixmap {
    let (lw, lh) = match which {
        Bitmap::Area { .. } => (AREA_W, AREA_H),
        Bitmap::Hue { .. } | Bitmap::Alpha { .. } => (AREA_W, STRIP_H),
    };
    let (w, h) = ((lw * OVERSAMPLE) as u32, (lh * OVERSAMPLE) as u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    // Where the marker sits, in pixels.
    let (mx, my) = match which {
        Bitmap::Area { s, v, .. } => (
            s as f64 / 1000.0 * (w - 1) as f64,
            (1.0 - v as f64 / 1000.0) * (h - 1) as f64,
        ),
        Bitmap::Hue { hue } => (hue as f64 / 3600.0 * (w - 1) as f64, h as f64 / 2.0),
        Bitmap::Alpha { alpha, .. } => (alpha as f64 / 255.0 * (w - 1) as f64, h as f64 / 2.0),
    };
    let ring = 6.0 * OVERSAMPLE;
    for y in 0..h {
        for x in 0..w {
            let fx = x as f64 / (w - 1) as f64;
            let fy = y as f64 / (h - 1) as f64;
            let base = match which {
                Bitmap::Area { hue, .. } => Color::from_hsv(hue as f64 / 10.0, fx, 1.0 - fy, 255),
                Bitmap::Hue { .. } => Color::from_hsv(fx * 360.0, 1.0, 1.0, 255),
                Bitmap::Alpha { rgb, .. } => {
                    // The color over a checkerboard, transparent at the left.
                    let cell = (6.0 * OVERSAMPLE) as u32;
                    let check = if ((x / cell) + (y / cell)).is_multiple_of(2) {
                        Color::rgb(0xff, 0xff, 0xff)
                    } else {
                        Color::rgb(0xcc, 0xcc, 0xcc)
                    };
                    check.mix(Color::rgb(rgb[0], rgb[1], rgb[2]), fx)
                }
            };
            // The marker: a white ring with a dark edge (a vertical bar on a
            // strip), visible over any color.
            let px = match which {
                Bitmap::Area { .. } => {
                    let d = ((x as f64 - mx).powi(2) + (y as f64 - my).powi(2)).sqrt();
                    if (d - ring).abs() < OVERSAMPLE {
                        Color::WHITE
                    } else if (d - ring).abs() < 2.0 * OVERSAMPLE {
                        Color::rgb(0x20, 0x20, 0x20)
                    } else {
                        base
                    }
                }
                _ => {
                    let d = (x as f64 - mx).abs();
                    if d < 1.5 * OVERSAMPLE {
                        Color::WHITE
                    } else if d < 2.5 * OVERSAMPLE {
                        Color::rgb(0x20, 0x20, 0x20)
                    } else {
                        base
                    }
                }
            };
            data.extend_from_slice(&[px.r, px.g, px.b, 255]);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(w, h), data)
}

/// A bitmap shown at `w`×`h` logical pixels.
fn picture(p: Arc<Pixmap>, w: f64, h: f64, theme: &Theme) -> Element {
    Element::stack(
        Axis::Horizontal,
        vec![Element::image(p).fit(ImageFit::Fill).width(w).height(h)],
    )
    .border(theme.palette.border, 1.0)
    .radius(2.0)
    .clip()
}

/// An inline color picker for `state` (see [`ColorPicker`]): a
/// saturation/value area for the current hue, a hue strip, an alpha strip
/// (unless [`ColorPicker::with_alpha`] turned it off), a hex field and a row
/// of swatches. Dragging, typing a hex that parses or tapping a swatch changes
/// the color and calls `on_change` with it. `field` is an accessor returning
/// the [`ColorPicker`] inside the app state.
///
/// Self-drawn under every theme (the bitmaps are the same in all of them; the
/// frame, field and swatches follow the theme).
pub fn color_picker<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &ColorPicker,
    field: impl Fn(&mut S) -> &mut ColorPicker + Copy + 'static,
    on_change: impl Fn(&mut S, Color) + Clone + 'static,
) -> Element {
    let q = |f: f64| (f.clamp(0.0, 1.0) * 1000.0).round() as u16;
    let hue10 = (state.hue.clamp(0.0, 360.0) * 10.0).round() as u16;
    let on = on_change.clone();
    let area = picture(
        bitmap(Bitmap::Area {
            hue: hue10,
            s: q(state.saturation),
            v: q(state.value),
        }),
        AREA_W,
        AREA_H,
        theme,
    )
    .cursor(crate::platform::CursorIcon::Crosshair)
    .on_drag_motion(cx, move |s: &mut S, m| {
        let b = m.bounds;
        let sat = (m.position.x - b.min_x()) / b.width().max(1.0);
        let val = 1.0 - (m.position.y - b.min_y()) / b.height().max(1.0);
        let p = field(s);
        p.set_saturation_value(sat, val);
        let c = p.color();
        on(s, c);
    });
    let on = on_change.clone();
    let hue = picture(bitmap(Bitmap::Hue { hue: hue10 }), AREA_W, STRIP_H, theme).on_drag(
        cx,
        move |s: &mut S, f| {
            let p = field(s);
            p.set_hue(f * 360.0);
            let c = p.color();
            on(s, c);
        },
    );
    let mut rows = vec![area, hue];
    if state.alpha_enabled {
        let c = state.color();
        let on = on_change.clone();
        let alpha = picture(
            bitmap(Bitmap::Alpha {
                rgb: [c.r, c.g, c.b],
                alpha: c.a,
            }),
            AREA_W,
            STRIP_H,
            theme,
        )
        .on_drag(cx, move |s: &mut S, f| {
            let p = field(s);
            p.set_alpha(f);
            let c = p.color();
            on(s, c);
        });
        rows.push(alpha);
    }

    // The hex field beside a preview of the color.
    let on = on_change.clone();
    let hex = text_editor(cx, theme, &state.hex, move |s: &mut S| &mut field(s).hex)
        .width(AREA_W - 32.0 - theme.spacing.sm)
        .on_key(cx, move |s: &mut S, k| {
            let p = field(s);
            if p.hex_key(k) {
                let c = p.color();
                on(s, c);
            }
        })
        .a11y_label("Hex color");
    let preview = Element::boxed(BoxStyle {
        fill: Some(state.color()),
        radius: theme.radius,
        border: Some((theme.palette.border, 1.0)),
    })
    .width(32.0)
    .height(28.0);
    rows.push(
        Element::stack(Axis::Horizontal, vec![preview, hex])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center),
    );

    if !state.swatches.is_empty() {
        let current = state.color();
        let swatches = state
            .swatches
            .iter()
            .map(|&c| {
                let on = on_change.clone();
                let ring = if c == current {
                    theme.palette.focus_ring
                } else {
                    theme.palette.border
                };
                Element::boxed(BoxStyle {
                    fill: Some(c),
                    radius: 3.0,
                    border: Some((ring, if c == current { 2.0 } else { 1.0 })),
                })
                .width(18.0)
                .height(18.0)
                .hovered(move |e| e.border(ring.mix(Color::BLACK, 0.3), 2.0))
                .on_tap(cx, move |s: &mut S| {
                    field(s).set_color(c);
                    on(s, c);
                })
                .a11y_label(c.to_hex())
            })
            .collect();
        rows.push(
            Element::stack(Axis::Horizontal, swatches)
                .gap((AREA_W - 18.0 * 10.0) / 9.0)
                .align(Align::Start, Align::Center),
        );
    }

    Element::stack(Axis::Vertical, rows)
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Stretch)
        .width(AREA_W)
        .role(Role::Group)
        .a11y_label("Color picker")
}

/// A swatch button showing `state`'s color that opens [`color_picker`] in a
/// popup below it; a press outside closes it. `on_change` runs with each new
/// color, as for [`color_picker`].
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{ColorPicker, color_button};
/// struct App { tint: ColorPicker, applied: Color }
/// fn view(s: &App, cx: &mut Cx<App>) -> Element {
///     let theme = *cx.theme();
///     color_button(cx, &theme, &s.tint, |s: &mut App| &mut s.tint, |s: &mut App, c| s.applied = c)
/// }
/// # let theme = Theme::light();
/// # let mut cx = Cx::new(&theme);
/// # let app = App { tint: ColorPicker::new(Color::WHITE), applied: Color::WHITE };
/// # assert!(view(&app, &mut cx).action.is_some());
/// ```
pub fn color_button<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &ColorPicker,
    field: impl Fn(&mut S) -> &mut ColorPicker + Copy + 'static,
    on_change: impl Fn(&mut S, Color) + Clone + 'static,
) -> Element {
    let p = &theme.palette;
    let chip = Element::boxed(BoxStyle {
        fill: Some(state.color()),
        radius: 3.0,
        border: Some((p.border, 1.0)),
    })
    .width(22.0)
    .height(16.0);
    let chevron = Element::text("▾", theme.typography.body, p.text_muted);
    let hover = p.hover_overlay;
    let button = Element::stack(Axis::Horizontal, vec![chip, chevron])
        .gap(theme.spacing.xs)
        .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .align(Align::Start, Align::Center)
        .hovered(move |e| e.fill(p.surface.mix(hover.with_alpha(255), 0.08)))
        .on_tap(cx, move |s: &mut S| {
            let p = field(s);
            let open = !p.is_open();
            p.set_open(open);
        })
        .expanded(state.open)
        .a11y_label(format!("Color {}", state.color().to_hex()));
    if state.open
        && let Some(tap) = button.action
    {
        let picker = color_picker(cx, theme, state, field, on_change);
        let content = Element::stack(Axis::Vertical, vec![picker])
            .padding(Insets::uniform(theme.spacing.md))
            .fill(p.surface)
            .border(p.border, 1.0)
            .radius(theme.radius);
        let dismiss = cx.register(move |s: &mut S| field(s).set_open(false));
        cx.overlay(OverlaySpec {
            content,
            anchor: Anchor::Below(tap),
            modal: false,
            dismiss: Some(dismiss),
        });
    }
    button
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::{Point, Size};

    struct State {
        picker: ColorPicker,
        changes: Vec<Color>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> Element {
        let theme = *cx.theme();
        super::super::column(vec![color_button(
            cx,
            &theme,
            &s.picker,
            |s: &mut State| &mut s.picker,
            |s: &mut State, c| s.changes.push(c),
        )])
        .padding(Insets::uniform(10.0))
    }

    fn app() -> App<State> {
        let state = State {
            picker: ColorPicker::new(Color::rgb(0, 0x90, 0xff)),
            changes: Vec::new(),
        };
        App::new(state, view).logical_size(Size::new(320.0, 360.0))
    }

    #[test]
    fn hex_typing_follows_parses_only() {
        let mut p = ColorPicker::new(Color::rgb(255, 0, 0)).with_alpha(false);
        assert_eq!(p.hex(), "#ff0000");
        p.hex_key(&KeyInput::SelectAll);
        assert!(!p.hex_key(&KeyInput::Text("#0".into()))); // not a color yet
        assert_eq!(p.color(), Color::rgb(255, 0, 0));
        assert!(p.hex_key(&KeyInput::Text("f0".into()))); // "#0f0"
        assert_eq!(p.color(), Color::rgb(0, 255, 0));
        assert_eq!(p.hex(), "#0f0");
        p.hex_key(&KeyInput::Enter);
        assert_eq!(p.hex(), "#00ff00");
        // Alpha off: a translucent hex comes out opaque.
        p.set_hex("#11223344");
        assert_eq!(p.color().a, 255);
    }

    #[test]
    fn the_button_opens_a_popup_whose_area_and_strips_change_the_color() {
        let mut app = app();
        app.render_once();
        assert!(app.click_at(Point::new(30.0, 22.0)));
        assert!(app.state().picker.is_open());
        app.render_once();
        // The area sits under the button; drag its top-right (full
        // saturation and value) — the hue stays.
        let area = app
            .accessibility_tree()
            .unwrap()
            .descendants()
            .into_iter()
            .find(|n| n.name == "Color picker")
            .map(|n| n.bounds)
            .expect("the picker");
        let top_right = Point::new(area.min_x() + AREA_W - 1.0, area.min_y() + 1.0);
        assert!(app.drag_at_point(top_right));
        app.end_drag();
        let c = app.state().picker.color();
        let (h, s, v) = c.to_hsv();
        assert!(
            (h - Color::rgb(0, 0x90, 0xff).to_hsv().0).abs() < 1.0,
            "{h}"
        );
        assert!(s > 0.98 && v > 0.98, "{s} {v}");
        assert_eq!(app.state().changes.last(), Some(&c));
        // The hue strip: its left end is red.
        let hue_at = Point::new(
            area.min_x() + 1.0,
            area.min_y() + AREA_H + 8.0 + STRIP_H / 2.0,
        );
        assert!(app.drag_at_point(hue_at));
        app.end_drag();
        let (h, _, _) = app.state().picker.color().to_hsv();
        assert!(!(2.0..358.0).contains(&h), "{h}");
        // A press outside closes the popup.
        assert!(app.click_at(Point::new(300.0, 350.0)));
        assert!(!app.state().picker.is_open());
    }

    #[test]
    fn bitmaps_are_reused_while_unchanged() {
        let a = bitmap(Bitmap::Hue { hue: 1200 });
        let b = bitmap(Bitmap::Hue { hue: 1200 });
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(
            a.size(),
            PhysicalSize::new((AREA_W * OVERSAMPLE) as u32, (STRIP_H * OVERSAMPLE) as u32)
        );
        // The hue strip runs red to red through green and blue.
        let px = a.pixel(a.size().width * 2 / 3, 2).unwrap();
        assert!(px[2] > 200 && px[0] < 60, "{px:?}");
    }
}
