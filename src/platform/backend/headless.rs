//! A windowless backend used for tests, golden-image rendering, and CI.
//!
//! It implements the full platform vocabulary without touching any OS
//! windowing API: a [`HeadlessWindow`] satisfies [`Window`], a
//! [`HeadlessSurface`] satisfies [`Surface`] by retaining the last presented
//! frame, and [`run`] drives a handler over a scripted event sequence. This is
//! the harness the roadmap's cross-platform conformance suite builds on.

use std::sync::{Arc, Mutex};

use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::event::{Event, WindowId};
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

/// A handle to inspect the most recent frame a [`HeadlessSurface`] received.
#[derive(Clone, Debug, Default)]
pub struct FrameProbe {
    last: Arc<Mutex<Option<Pixmap>>>,
}

// The probe and surface are test seams: the crate's tests read frames back.
#[cfg_attr(not(test), allow(dead_code))]
impl FrameProbe {
    /// The most recently presented frame, if any.
    pub fn last_frame(&self) -> Option<Pixmap> {
        self.last.lock().unwrap().clone()
    }
}

/// A [`Surface`] that stores the last presented [`Pixmap`] instead of blitting
/// to a screen.
#[derive(Debug)]
pub struct HeadlessSurface {
    size: PhysicalSize,
    sink: Arc<Mutex<Option<Pixmap>>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl HeadlessSurface {
    /// A surface of `size`, and the probe that reads back what it presents.
    pub fn new(size: PhysicalSize) -> (Self, FrameProbe) {
        let sink = Arc::new(Mutex::new(None));
        (
            Self {
                size,
                sink: sink.clone(),
            },
            FrameProbe { last: sink },
        )
    }
}

impl Surface for HeadlessSurface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }

    fn size(&self) -> PhysicalSize {
        self.size
    }

    fn present(&mut self, pixmap: &Pixmap, _damage: &[crate::geometry::Rect]) {
        *self.sink.lock().unwrap() = Some(pixmap.clone());
    }
}

/// A window with no on-screen presence.
#[derive(Debug)]
pub struct HeadlessWindow {
    id: WindowId,
    size: PhysicalSize,
    scale: ScaleFactor,
    probe: FrameProbe,
}

impl HeadlessWindow {
    #[cfg_attr(any(target_os = "ios", target_arch = "wasm32"), allow(dead_code))] // no fallback there
    fn new(attrs: &WindowAttributes) -> Self {
        let scale = ScaleFactor::IDENTITY;
        let size = scale.to_physical(attrs.logical_size);
        Self {
            id: WindowId(1),
            size,
            scale,
            probe: FrameProbe::default(),
        }
    }

    /// Inspect frames presented into surfaces created from this window.
    #[allow(dead_code)]
    pub fn frame_probe(&self) -> FrameProbe {
        self.probe.clone()
    }
}

impl Window for HeadlessWindow {
    fn id(&self) -> WindowId {
        self.id
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size
    }
    fn scale_factor(&self) -> ScaleFactor {
        self.scale
    }
    fn request_redraw(&self) {}
    fn set_title(&self, _title: &str) {}
    fn create_surface(&self) -> Box<dyn Surface> {
        // Share the window's probe so callers can read what was presented.
        let surface = HeadlessSurface {
            size: self.size,
            sink: self.probe.last.clone(),
        };
        Box::new(surface)
    }
}

/// Drive `handler` over a scripted `events` sequence against a fresh headless
/// window, stopping early if the handler returns [`ControlFlow::Exit`].
///
/// Returns the window so the caller can read its [`FrameProbe`] afterwards.
#[cfg_attr(any(target_os = "ios", target_arch = "wasm32"), allow(dead_code))] // no fallback there
pub fn run<H>(
    attrs: WindowAttributes,
    events: impl IntoIterator<Item = Event>,
    mut handler: H,
) -> HeadlessWindow
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let window = HeadlessWindow::new(&attrs);
    for event in events {
        if matches!(handler(event, &window), ControlFlow::Exit) {
            break;
        }
    }
    window
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Size;

    #[test]
    fn surface_retains_last_frame() {
        let (mut surface, probe) = HeadlessSurface::new(PhysicalSize::new(4, 4));
        assert!(probe.last_frame().is_none());
        let pm = Pixmap::new(PhysicalSize::new(4, 4));
        surface.present(&pm, &[]);
        assert_eq!(probe.last_frame().unwrap().size(), PhysicalSize::new(4, 4));
    }

    #[test]
    fn run_stops_on_exit() {
        let attrs = WindowAttributes::new().with_logical_size(Size::new(10.0, 10.0));
        let mut seen = 0;
        run(
            attrs,
            [
                Event::RedrawRequested,
                Event::CloseRequested,
                Event::RedrawRequested,
            ],
            |ev, _w| {
                seen += 1;
                if matches!(ev, Event::CloseRequested) {
                    ControlFlow::Exit
                } else {
                    ControlFlow::Wait
                }
            },
        );
        assert_eq!(seen, 2); // third event never delivered
    }
}
