//! Windows that can't be seen: minimized, on another virtual desktop,
//! covered, their surface gone or suspended by the compositor
//! ([`Event::Occluded`](crate::platform::Event::Occluded)).
//!
//! A hidden window draws no animation frames, on the CPU or the GPU: its
//! wakes still run the state's updates and timers, but nothing is built or
//! presented, and what changed is drawn when it shows again. Once every
//! window is hidden (or the app is in the background) the frame clock is
//! suspended as well, so nothing asks the display for frames.
//!
//! A GPU can also say a frame went nowhere (`DXGI_STATUS_OCCLUDED` from a
//! swap chain's `Present`). That stops the window's animation frames too,
//! but not the clock, and any event for the window but a wake (a repaint
//! the system asks for, a resize, focus, input) ends it: no platform says
//! when such a window shows again.

use crate::App;

/// What hides a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Occlusion {
    /// The backend said it's hidden ([`Event::Occluded`](crate::platform::Event::Occluded)).
    pub(crate) by_window: bool,
    /// The GPU's last present went nowhere.
    pub(crate) by_gpu: bool,
}

impl Occlusion {
    /// Whether the window draws no frames.
    pub(crate) fn hidden(self) -> bool {
        self.by_window || self.by_gpu
    }
}

impl<S: 'static> App<S> {
    /// Whether frames are suspended: the app is in the background, or every
    /// window is hidden.
    pub(crate) fn frames_suspended(&self) -> bool {
        self.in_background()
            || (!self.panes.is_empty() && self.panes.iter().all(|p| p.occlusion.by_window))
    }

    /// Suspend or resume the frame clock to match.
    pub(crate) fn sync_frames(&self) {
        crate::platform::frame::set_suspended(self.frames_suspended());
    }

    /// Window `idx` was hidden (`true`) or shown again. Returns whether it
    /// should be drawn now, whole: it showed again.
    pub(crate) fn window_occluded(&mut self, idx: usize, hidden: bool) -> bool {
        let pane = &mut self.panes[idx];
        let was = pane.occlusion.hidden();
        pane.occlusion.by_window = hidden;
        if !hidden {
            pane.occlusion.by_gpu = false;
        }
        crate::app_dbg(format_args!("occluded {:?} {hidden}", pane.id));
        self.sync_frames();
        if was && !hidden {
            // What changed while hidden shows now.
            self.panes[idx].dirty = true;
            return !self.in_background();
        }
        false
    }

    /// Something happened to window `idx` that a hidden window doesn't get
    /// (a repaint, a resize, focus, input): a GPU's "occluded" is over.
    pub(crate) fn window_seen(&mut self, idx: usize) {
        let pane = &mut self.panes[idx];
        if std::mem::take(&mut pane.occlusion.by_gpu) {
            pane.dirty = true;
        }
    }

    /// Whether window `idx` draws no frames now.
    pub(crate) fn window_hidden(&self, idx: usize) -> bool {
        self.panes[idx].occlusion.hidden()
    }
}

#[cfg(test)]
mod tests {
    use crate::core::Cx;
    use crate::geometry::{PhysicalSize, ScaleFactor};
    use crate::platform::backend::headless::HeadlessSurface;
    use crate::platform::{ControlFlow, Event, Lifecycle, Window, WindowAttributes, WindowId};
    use crate::prelude::*;
    use crate::render::Surface;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A window that counts the frames presented to it.
    struct Counting {
        id: u64,
        presents: Rc<Cell<u32>>,
        next: Rc<Cell<u64>>,
    }

    struct CountingSurface(HeadlessSurface, Rc<Cell<u32>>);

    impl Surface for CountingSurface {
        fn resize(&mut self, size: PhysicalSize) {
            self.0.resize(size)
        }
        fn size(&self) -> PhysicalSize {
            self.0.size()
        }
        fn present(&mut self, pixmap: &Pixmap, damage: &[crate::geometry::Rect]) {
            self.1.set(self.1.get() + 1);
            self.0.present(pixmap, damage)
        }
    }

    impl Window for Counting {
        fn id(&self) -> WindowId {
            WindowId(self.id)
        }
        fn inner_size(&self) -> PhysicalSize {
            PhysicalSize::new(40, 30)
        }
        fn scale_factor(&self) -> ScaleFactor {
            ScaleFactor::IDENTITY
        }
        fn request_redraw(&self) {}
        fn set_title(&self, _: &str) {}
        fn create_surface(&self) -> Box<dyn Surface> {
            Box::new(CountingSurface(
                HeadlessSurface::new(self.inner_size()).0,
                self.presents.clone(),
            ))
        }
        fn open_window(&self, _: WindowAttributes) -> Option<WindowId> {
            let id = self.next.get() + 1;
            self.next.set(id);
            Some(WindowId(id))
        }
    }

    struct Rig<H> {
        handler: H,
        presents: Rc<Cell<u32>>,
        next: Rc<Cell<u64>>,
    }

    impl<H: FnMut(Event, &dyn Window) -> ControlFlow> Rig<H> {
        fn send(&mut self, id: u64, event: Event) -> u32 {
            let before = self.presents.get();
            let w = Counting {
                id,
                presents: self.presents.clone(),
                next: self.next.clone(),
            };
            (self.handler)(event, &w);
            self.presents.get() - before
        }
    }

    /// A view that animates: a new number each frame.
    fn spinning(_: &(), cx: &mut Cx<()>) -> Element {
        thread_local!(static FRAME: Cell<u32> = const { Cell::new(0) });
        let n = FRAME.with(|f| {
            f.set(f.get() + 1);
            f.get()
        });
        cx.request_frame();
        label(cx, format!("{n}"))
    }

    /// An app whose windows (one or two) always animate.
    fn rig(windows: usize) -> Rig<impl FnMut(Event, &dyn Window) -> ControlFlow> {
        let mut app = App::new((), spinning)
            .no_font()
            .logical_size(Size::new(40.0, 30.0));
        for _ in 1..windows {
            app.open_window(WindowAttributes::new(), spinning);
        }
        let (_, handler) = app.into_handler();
        let mut rig = Rig {
            handler,
            presents: Rc::new(Cell::new(0)),
            next: Rc::new(Cell::new(100)),
        };
        assert_eq!(rig.send(100, Event::RedrawRequested), 1);
        for id in 1..windows as u64 {
            rig.send(100 + id, Event::RedrawRequested);
        }
        rig
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::platform::frame::SUSPEND_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_hidden_window_draws_no_frames_until_it_shows() {
        let _lock = lock();
        let mut rig = rig(1);
        assert_eq!(rig.send(100, Event::Wake), 1, "animating: a frame a wake");
        assert_eq!(rig.send(100, Event::Occluded(true)), 0);
        assert!(crate::platform::frame::suspended(), "the clock stops");
        assert_eq!(rig.send(100, Event::Wake), 0, "hidden: no frames");
        assert_eq!(rig.send(100, Event::Wake), 0);
        assert_eq!(rig.send(100, Event::Occluded(false)), 1, "drawn on showing");
        assert!(!crate::platform::frame::suspended());
        assert_eq!(rig.send(100, Event::Wake), 1, "animating again");
        // Told twice is still one change.
        assert_eq!(rig.send(100, Event::Occluded(false)), 0);
    }

    #[test]
    fn the_clock_stops_only_once_every_window_is_hidden() {
        let _lock = lock();
        let mut rig = rig(2);
        rig.send(100, Event::Occluded(true));
        assert!(!crate::platform::frame::suspended(), "one still shows");
        assert_eq!(rig.send(100, Event::Wake), 0);
        assert_eq!(rig.send(101, Event::Wake), 1);
        rig.send(101, Event::Occluded(true));
        assert!(crate::platform::frame::suspended());
        rig.send(100, Event::Occluded(false));
        assert!(!crate::platform::frame::suspended());
        rig.send(101, Event::Occluded(false));
    }

    #[test]
    fn coming_back_from_the_background_keeps_a_hidden_window_hidden() {
        let _lock = lock();
        let mut rig = rig(1);
        rig.send(100, Event::Occluded(true));
        rig.send(100, Event::Lifecycle(Lifecycle::Background));
        assert!(crate::platform::frame::suspended());
        rig.send(100, Event::Lifecycle(Lifecycle::Foreground));
        assert!(
            crate::platform::frame::suspended(),
            "back from the background, but the only window is still hidden"
        );
        assert_eq!(rig.send(100, Event::Wake), 0);
        // Shown while in the background: drawn when the app comes back.
        rig.send(100, Event::Lifecycle(Lifecycle::Background));
        assert_eq!(rig.send(100, Event::Occluded(false)), 0);
        assert!(crate::platform::frame::suspended());
        assert_eq!(rig.send(100, Event::Lifecycle(Lifecycle::Foreground)), 1);
        assert!(!crate::platform::frame::suspended());
    }
}
