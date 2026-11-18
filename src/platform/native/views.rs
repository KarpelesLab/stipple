//! Real OS controls in the window, under [`Theme::native`](crate::style::Theme::native).
//!
//! Where a window can host them (UIKit, AppKit, Win32), each OS-backed control
//! the app shows is a real `UIView` / `NSView` / child `HWND`, placed over the
//! control's laid-out box each frame. The App decides which controls are live
//! (see `core::native_host`) and hands the window the list, a [`NativeView`]
//! each; the window keeps one OS control per [`id`](NativeView::id), made the
//! first time it is seen, updated when its spec changes and destroyed when it
//! leaves the list ([`ViewSet`] does that bookkeeping for each backend). What
//! the user does to a control comes back as
//! [`Event::NativeView`](crate::platform::Event::NativeView).
//!
//! Everything else stays as it is: off screen, headless, and wherever a live
//! control can't show (something drawn over it, a transform, a fade, a
//! partial clip), the OS draws a picture of the control into the frame
//! instead (`native::render`).

use super::NativeControl;
use crate::geometry::Rect;
use std::collections::HashMap;

/// One live OS control, as the window should show it this frame.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct NativeView {
    /// What the control is known by across frames (see
    /// `core::native_host`): the window keeps one OS control per id.
    pub id: u64,
    /// Which control, with its value (checked, on, a slider's position).
    pub control: NativeControl,
    /// Where it goes, in the window's logical pixels.
    pub frame: Rect,
    /// A button's title (empty for the others).
    pub title: String,
    /// The size of the text the button's title is in the app's theme, in
    /// logical pixels (0 for the others): where the OS's own size is set
    /// per control (Win32's), the title takes this one, so it matches the
    /// app's text, and the picture it is in a frame Stipple draws.
    pub font_size: f64,
    /// The name a screen reader reads for it: its label, which the app may
    /// draw beside it (a checkbox's text).
    pub label: String,
    /// Whether it looks enabled and takes input.
    pub enabled: bool,
    /// Held down through the app's own input: where the app's view takes
    /// a control's touches (iOS's buttons), the control shows its pressed
    /// look from this.
    pub pressed: bool,
    /// Drawn in the dark appearance.
    pub dark: bool,
    /// Laid out right to left (a slider fills from the right).
    pub rtl: bool,
    /// A select's choices: the control is the OS's pop-up button (a
    /// `UIButton` with a `UIMenu`, an `NSPopUpButton`, a `COMBOBOX`), whose
    /// own menu or list picks one.
    pub menu: Option<NativeMenu>,
    /// The scroll areas it is in, outermost first: each is a clipping
    /// container the OS cuts the control off at (see [`NativeClip`]).
    pub clips: Vec<NativeClip>,
}

/// A select's choices, as its OS control shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeMenu {
    /// The options, in order.
    pub options: Vec<String>,
    /// The chosen one, if any (the app's state says which).
    pub selected: Option<usize>,
}

/// A scroll area a live control is in: the OS clips the control to it (a
/// `UIView` / `NSView` that clips its subviews, a window region on Win32),
/// so one that is half scrolled out stays live, cut at the edge.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct NativeClip {
    /// What the area is known by across frames (one container per id).
    pub id: u64,
    /// Its viewport, in the window's logical pixels.
    pub frame: Rect,
}

impl NativeView {
    /// The part of the control its scroll areas show, in the window's
    /// logical pixels (`None` when none of it is).
    pub fn visible(&self) -> Option<Rect> {
        self.clips
            .iter()
            .try_fold(self.frame, |r, c| r.intersection(c.frame))
    }
}

/// What the user did to a live control.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum NativeViewEvent {
    /// Pressed a button, or flipped a switch, checkbox or radio button.
    Activated,
    /// Moved a slider, to this position (`0..=1`).
    Value(f64),
    /// Picked this option of a select, from the OS's own menu or list.
    Picked(usize),
}

/// The OS side of a [`ViewSet`]: how a backend makes, changes and removes its
/// controls.
pub(crate) trait ViewBackend {
    /// The backend's handle on one control (a retained `UIView`, an `HWND`).
    type Handle;
    /// Make the control for `view`; `None` if the OS can't.
    fn create(&mut self, view: &NativeView) -> Option<Self::Handle>;
    /// Bring the control from `old` to `new` (same id).
    fn update(&mut self, handle: &mut Self::Handle, old: &NativeView, new: &NativeView);
    /// Remove the control.
    fn destroy(&mut self, handle: Self::Handle);
}

/// A window's live controls by id, kept in step with each frame's list.
pub(crate) struct ViewSet<H> {
    shown: HashMap<u64, (NativeView, H)>,
    /// The ids in the order they were last listed (paint order).
    order: Vec<u64>,
    /// Controls the user acted on since: the next sync brings them back to
    /// their spec even if it didn't change (a pick or a flip the app
    /// refused goes back).
    stale: std::collections::HashSet<u64>,
}

impl<H> Default for ViewSet<H> {
    fn default() -> Self {
        ViewSet {
            shown: HashMap::new(),
            order: Vec::new(),
            stale: Default::default(),
        }
    }
}

impl<H> ViewSet<H> {
    /// Show exactly `views`: make the new ones, update the changed ones,
    /// destroy the ones gone. Returns whether the order (z-order) changed,
    /// for backends that then restack their controls.
    pub(crate) fn sync<B: ViewBackend<Handle = H>>(
        &mut self,
        backend: &mut B,
        views: &[NativeView],
    ) -> bool {
        let wanted: std::collections::HashSet<u64> = views.iter().map(|v| v.id).collect();
        let gone: Vec<u64> = self
            .shown
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some((_, handle)) = self.shown.remove(&id) {
                backend.destroy(handle);
            }
        }
        let mut order = Vec::with_capacity(views.len());
        for view in views {
            match self.shown.get_mut(&view.id) {
                Some((old, handle)) => {
                    if self.stale.remove(&view.id) || old != view {
                        backend.update(handle, old, view);
                        *old = view.clone();
                    }
                }
                None => match backend.create(view) {
                    Some(handle) => {
                        self.shown.insert(view.id, (view.clone(), handle));
                    }
                    None => continue,
                },
            }
            order.push(view.id);
        }
        let restack = order != self.order;
        self.order = order;
        self.stale.clear();
        restack
    }

    /// The user acted on control `id`: have the next sync set it to its spec
    /// again, changed or not (the app's state decides what it shows).
    pub(crate) fn touch(&mut self, id: u64) {
        self.stale.insert(id);
    }

    /// The control shown for `id`, with its spec.
    pub(crate) fn get(&self, id: u64) -> Option<(&NativeView, &H)> {
        self.shown.get(&id).map(|(v, h)| (v, h))
    }

    /// The id of the control whose handle is `handle`.
    pub(crate) fn id_of(&self, handle: &H) -> Option<u64>
    where
        H: PartialEq,
    {
        self.shown
            .iter()
            .find(|(_, (_, h))| h == handle)
            .map(|(id, _)| *id)
    }

    /// The controls, in paint order (bottom first).
    pub(crate) fn in_order(&self) -> impl Iterator<Item = (&NativeView, &H)> {
        self.order.iter().filter_map(|id| self.get(*id))
    }

    /// How many controls are shown.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.shown.len()
    }
}

/// A live control as a self-test finds it in the window: its OS class (or
/// window class) and where it is, in the window's logical pixels.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LiveControl {
    /// The [`NativeView::id`] it shows.
    pub id: u64,
    /// The OS's class name: `UISwitch`, `NSButton`, `Button` (Win32), ...
    pub class: String,
    /// Its title as the OS has it (a button's), else empty.
    pub title: String,
    /// Its frame as the OS reports it, in logical pixels of the window.
    pub frame: Rect,
    /// The part of it the OS shows: its frame cut by the clipping
    /// containers it is in (all of it when none cuts it, empty when they
    /// hide it).
    pub visible: Rect,
    /// Whether a tap or click at the middle of its [visible](Self::visible)
    /// part reaches it (the OS's own hit test of the window's view
    /// hierarchy): it lands on the control, or, for one whose touches the
    /// app's view takes (iOS's buttons and switches), on that view, with
    /// nothing else over it.
    pub hit: bool,
    /// The OS class of the container it is in, when it is in a scroll
    /// area's (`StippleClipView`), else empty.
    pub container: String,
}

/// For self-tests (CI): the live controls the window shows now, read back
/// from the OS's view hierarchy. Empty where controls aren't live.
#[doc(hidden)]
pub fn live_controls() -> Vec<LiveControl> {
    super::os_views::live_controls()
}

/// For self-tests (CI): have the OS act on live control `id` as a user's tap
/// or click would (`sendActionsForControlEvents:`, `performClick:`,
/// `BM_CLICK`; a slider is set to `value`; a select picks option `value`
/// through its own menu: `sendAction:` with the `UIAction`,
/// `performActionForItemAtIndex:`, the `COMBOBOX`'s list). Returns whether
/// the control was there.
#[doc(hidden)]
pub fn poke_control(id: u64, value: Option<f64>) -> bool {
    super::os_views::poke(id, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mock {
        next: u32,
        log: Vec<String>,
    }

    impl ViewBackend for Mock {
        type Handle = u32;
        fn create(&mut self, view: &NativeView) -> Option<u32> {
            if view.title == "refuse" {
                return None;
            }
            self.next += 1;
            self.log
                .push(format!("create {} as {}", view.id, self.next));
            Some(self.next)
        }
        fn update(&mut self, handle: &mut u32, old: &NativeView, new: &NativeView) {
            self.log.push(format!(
                "update {handle}: {:?} -> {:?}",
                old.title, new.title
            ));
        }
        fn destroy(&mut self, handle: u32) {
            self.log.push(format!("destroy {handle}"));
        }
    }

    fn view(id: u64, title: &str) -> NativeView {
        NativeView {
            id,
            control: NativeControl::Button { primary: false },
            frame: Rect::from_xywh(0.0, 0.0, 80.0, 30.0),
            title: title.into(),
            font_size: 14.0,
            label: title.into(),
            enabled: true,
            pressed: false,
            dark: false,
            rtl: false,
            menu: None,
            clips: Vec::new(),
        }
    }

    #[test]
    fn controls_are_made_once_updated_on_change_and_destroyed_when_gone() {
        let mut set = ViewSet::default();
        let mut os = Mock::default();
        set.sync(&mut os, &[view(1, "Save"), view(2, "Cancel")]);
        assert_eq!(os.log, ["create 1 as 1", "create 2 as 2"]);
        os.log.clear();
        // The same list again: nothing reaches the OS.
        assert!(!set.sync(&mut os, &[view(1, "Save"), view(2, "Cancel")]));
        assert!(os.log.is_empty());
        // One changes, one goes, one comes.
        set.sync(&mut os, &[view(1, "Saved"), view(3, "Help")]);
        assert_eq!(
            os.log,
            [
                "destroy 2",
                "update 1: \"Save\" -> \"Saved\"",
                "create 3 as 3"
            ]
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set.id_of(&3), Some(3));
        assert_eq!(set.get(1).map(|(v, _)| v.title.as_str()), Some("Saved"));
        os.log.clear();
        set.sync(&mut os, &[]);
        assert_eq!(os.log.len(), 2);
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn a_new_order_asks_for_restacking_and_a_refused_control_is_skipped() {
        let mut set = ViewSet::default();
        let mut os = Mock::default();
        set.sync(&mut os, &[view(1, "a"), view(2, "refuse"), view(3, "b")]);
        assert_eq!(set.len(), 2);
        let ids: Vec<u64> = set.in_order().map(|(v, _)| v.id).collect();
        assert_eq!(ids, [1, 3]);
        assert!(set.sync(&mut os, &[view(3, "b"), view(1, "a")]));
        let ids: Vec<u64> = set.in_order().map(|(v, _)| v.id).collect();
        assert_eq!(ids, [3, 1]);
    }

    #[test]
    fn a_touched_control_is_set_back_to_its_spec() {
        let mut set = ViewSet::default();
        let mut os = Mock::default();
        set.sync(&mut os, &[view(1, "a")]);
        os.log.clear();
        // The user picked something the app didn't take: same spec, but the
        // OS control is updated to it again, once.
        set.touch(1);
        set.sync(&mut os, &[view(1, "a")]);
        assert_eq!(os.log, ["update 1: \"a\" -> \"a\""]);
        os.log.clear();
        set.sync(&mut os, &[view(1, "a")]);
        assert!(os.log.is_empty());
    }

    #[test]
    fn the_visible_part_is_what_every_clip_shows() {
        let mut v = view(1, "a");
        v.frame = Rect::from_xywh(0.0, 80.0, 80.0, 40.0);
        assert_eq!(v.visible(), Some(v.frame));
        v.clips = vec![
            NativeClip {
                id: 7,
                frame: Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
            },
            NativeClip {
                id: 8,
                frame: Rect::from_xywh(0.0, 50.0, 200.0, 200.0),
            },
        ];
        assert_eq!(v.visible(), Some(Rect::from_xywh(0.0, 80.0, 80.0, 20.0)));
        v.frame = Rect::from_xywh(0.0, 120.0, 80.0, 40.0);
        assert_eq!(v.visible(), None);
    }
}
