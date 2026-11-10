//! App-level tests of custom-paint canvases: painting without a rebuild,
//! partial invalidation and presentation, revisions, hit-testing, and viewport
//! content set through the handle.

use crate::core::{CanvasId, ViewportId};
use crate::geometry::{PhysicalSize, Point, Rect, Size};
use crate::prelude::*;
use crate::render::Surface;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A surface that remembers what was presented.
#[derive(Default)]
struct Recorder {
    size: PhysicalSize,
    /// The damage list of each present (empty: all of it).
    presents: Vec<Vec<Rect>>,
    last: Option<Pixmap>,
}

impl Surface for Recorder {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, damage: &[Rect]) {
        self.presents.push(damage.to_vec());
        self.last = Some(pixmap.clone());
    }
}

impl<S: 'static> App<S> {
    /// What the run loop does to present the primary window: repaint without
    /// a rebuild if that's all that's wanted, else build; then rasterize and
    /// present what changed.
    fn present_to(&mut self, surface: &mut Recorder) {
        self.refresh_theme();
        let App {
            state,
            theme,
            font,
            panes,
            viewport_content,
            ..
        } = self;
        let pane = &mut panes[0];
        let (scene, damage) = match pane.repaint(theme, font.as_ref(), false) {
            Some(frame) => frame,
            None => {
                let scene = pane.build_frame(state, theme, font.as_ref());
                (scene, pane.take_damage())
            }
        };
        let phys = pane.scale.to_physical(pane.attrs.logical_size);
        pane.present_scene(scene, damage, theme, viewport_content, surface, phys, false);
    }

    /// What the run loop does on a wake: run queued updates, take canvas and
    /// viewport changes, and present.
    fn wake_to(&mut self, surface: &mut Recorder) {
        let (ran, _) = self
            .handle
            .pump(&mut self.state, crate::platform::time::Instant::now());
        let content = self.pump_content();
        if ran {
            self.panes[0].dirty = true;
        } else if content {
            self.panes[0].repaint_only = true;
        }
        self.present_to(surface);
    }
}

const GRID: CanvasId = CanvasId(1);
const ROW: f64 = 10.0;

struct Grid {
    /// Builds of the view.
    builds: Rc<Cell<u32>>,
    /// The area each paint call was asked for.
    painted: Rc<RefCell<Vec<Rect>>>,
    /// The color of each of the canvas's 10 rows.
    rows: Rc<RefCell<Vec<Color>>>,
    /// The canvas's revision.
    revision: u64,
    taps: u32,
}

/// The app, its build count, the areas painted, and the rows' colors.
type GridApp = (
    App<Grid>,
    Rc<Cell<u32>>,
    Rc<RefCell<Vec<Rect>>>,
    Rc<RefCell<Vec<Color>>>,
);

fn grid_app() -> GridApp {
    let builds = Rc::new(Cell::new(0));
    let painted = Rc::new(RefCell::new(Vec::new()));
    let rows = Rc::new(RefCell::new(vec![Color::rgb(0, 0, 255); 10]));
    let state = Grid {
        builds: builds.clone(),
        painted: painted.clone(),
        rows: rows.clone(),
        revision: 0,
        taps: 0,
    };
    let app = App::new(state, |s: &Grid, cx: &mut Cx<Grid>| {
        s.builds.set(s.builds.get() + 1);
        let (painted, rows) = (s.painted.clone(), s.rows.clone());
        // The canvas is 100×100 at (20, 0): ten rows of 10 px.
        row(vec![
            Element::boxed(Default::default()).width(20.0).height(100.0),
            Element::canvas(GRID, move |c: &mut Canvas| {
                painted.borrow_mut().push(c.dirty());
                for (i, color) in rows.borrow().iter().enumerate() {
                    let r = Rect::from_xywh(0.0, i as f64 * ROW, 100.0, ROW);
                    if c.needs_paint(r) {
                        c.fill_rect(r, *color);
                    }
                }
            })
            .revision(s.revision)
            .width(100.0)
            .height(100.0)
            .on_tap(cx, |s: &mut Grid| s.taps += 1),
        ])
    })
    .logical_size(Size::new(120.0, 100.0));
    (app, builds, painted, rows)
}

#[test]
fn invalidating_a_rect_repaints_only_it_without_a_rebuild() {
    let (mut app, builds, painted, rows) = grid_app();
    let mut screen = Recorder::default();
    app.present_to(&mut screen);
    assert_eq!(builds.get(), 1);
    assert_eq!(
        screen.presents,
        [Vec::<Rect>::new()],
        "the first frame is whole"
    );
    assert!(!painted.borrow().is_empty(), "the canvas painted");
    painted.borrow_mut().clear();

    // Row 3 turns red, from "another thread".
    rows.borrow_mut()[3] = Color::rgb(255, 0, 0);
    let handle = app.handle();
    handle.invalidate_canvas_rect(GRID, Rect::from_xywh(0.0, 30.0, 100.0, ROW));
    app.wake_to(&mut screen);

    assert_eq!(builds.get(), 1, "no rebuild");
    assert_eq!(
        screen.presents.last().unwrap(),
        &[Rect::from_xywh(20.0, 30.0, 100.0, 10.0)],
        "only the row is presented"
    );
    assert_eq!(
        *painted.borrow(),
        [Rect::from_xywh(0.0, 30.0, 100.0, ROW)],
        "the paint was asked for the row alone"
    );
    let shown = screen.last.as_ref().unwrap();
    assert_eq!(shown.pixel(50, 35), Some([255, 0, 0, 255]), "row 3 is red");
    assert_eq!(shown.pixel(50, 45), Some([0, 0, 255, 255]), "row 4 isn't");
}

#[test]
fn invalidating_the_whole_canvas_presents_its_box() {
    let (mut app, builds, _, rows) = grid_app();
    let mut screen = Recorder::default();
    app.present_to(&mut screen);
    *rows.borrow_mut() = vec![Color::rgb(0, 255, 0); 10];
    app.handle().invalidate_canvas(GRID);
    app.wake_to(&mut screen);
    assert_eq!(builds.get(), 1, "no rebuild");
    assert_eq!(
        screen.presents.last().unwrap(),
        &[Rect::from_xywh(20.0, 0.0, 100.0, 100.0)]
    );
    let shown = screen.last.as_ref().unwrap();
    assert_eq!(shown.pixel(30, 90), Some([0, 255, 0, 255]));
}

#[test]
fn a_rebuild_leaves_the_canvas_alone_until_its_revision_changes() {
    let (mut app, builds, painted, _) = grid_app();
    let mut screen = Recorder::default();
    app.present_to(&mut screen);
    painted.borrow_mut().clear();

    // A state change that doesn't touch the canvas: rebuilt, nothing to show.
    app.handle().update(|s: &mut Grid| s.taps += 0);
    app.wake_to(&mut screen);
    assert_eq!(builds.get(), 2);
    assert!(
        painted.borrow().is_empty(),
        "nothing asked the canvas to paint"
    );

    // A new revision repaints it whole.
    app.handle().update(|s: &mut Grid| s.revision += 1);
    app.wake_to(&mut screen);
    assert_eq!(
        screen.presents.last().unwrap(),
        &[Rect::from_xywh(20.0, 0.0, 100.0, 100.0)]
    );
    assert_eq!(*painted.borrow(), [Rect::from_xywh(0.0, 0.0, 100.0, 100.0)]);
}

#[test]
fn an_invalidation_during_a_rebuild_is_kept() {
    let (mut app, builds, painted, _) = grid_app();
    let mut screen = Recorder::default();
    app.present_to(&mut screen);
    painted.borrow_mut().clear();
    // An update and an invalidation arrive together: the update rebuilds, and
    // the row still repaints.
    let handle = app.handle();
    handle.update(|s: &mut Grid| s.taps += 0);
    handle.invalidate_canvas_rect(GRID, Rect::from_xywh(0.0, 50.0, 100.0, ROW));
    app.wake_to(&mut screen);
    assert_eq!(builds.get(), 2);
    assert_eq!(
        screen.presents.last().unwrap(),
        &[Rect::from_xywh(20.0, 50.0, 100.0, 10.0)]
    );
    assert_eq!(*painted.borrow(), [Rect::from_xywh(0.0, 50.0, 100.0, ROW)]);
}

#[test]
fn a_canvas_is_hit_like_any_leaf() {
    let (mut app, ..) = grid_app();
    app.render_once();
    assert!(app.click_at(Point::new(60.0, 50.0)));
    assert!(!app.click_at(Point::new(10.0, 50.0)), "left of the canvas");
    assert_eq!(app.state().taps, 1);
}

#[test]
fn viewport_content_from_the_handle_presents_just_the_viewport() {
    let vid = ViewportId(4);
    let builds = Rc::new(Cell::new(0));
    let counter = builds.clone();
    let mut app = App::new((), move |_: &(), _: &mut Cx<()>| {
        counter.set(counter.get() + 1);
        column(vec![
            Element::boxed(Default::default()).height(20.0),
            Element::viewport(vid).width(40.0).height(30.0),
        ])
    })
    .logical_size(Size::new(100.0, 100.0));
    let mut screen = Recorder::default();
    app.present_to(&mut screen);

    let content = Pixmap::from_rgba8(PhysicalSize::new(40, 30), [9, 200, 9, 255].repeat(40 * 30));
    // From a renderer thread.
    let handle = app.handle();
    std::thread::spawn(move || handle.set_viewport_content(vid, content))
        .join()
        .unwrap();
    app.wake_to(&mut screen);
    assert_eq!(builds.get(), 1, "no rebuild");
    assert_eq!(
        screen.presents.last().unwrap(),
        &[Rect::from_xywh(0.0, 20.0, 40.0, 30.0)]
    );
    let shown = screen.last.as_ref().unwrap();
    assert_eq!(shown.pixel(10, 30), Some([9, 200, 9, 255]));
}

/// A canvas drawn moved by a transform is invalidated where it is drawn.
#[test]
fn a_transformed_canvas_is_damaged_where_it_is_drawn() {
    let painted = Rc::new(RefCell::new(Vec::new()));
    let log = painted.clone();
    let mut app = App::new((), move |_: &(), _: &mut Cx<()>| {
        let log = log.clone();
        column(vec![
            Element::canvas(GRID, move |c: &mut Canvas| {
                log.borrow_mut().push(c.dirty());
                let b = c.bounds();
                c.fill_rect(b, Color::WHITE);
            })
            .width(20.0)
            .height(20.0)
            .transform(Transform::IDENTITY.translate(50.0, 40.0)),
        ])
    })
    .logical_size(Size::new(100.0, 100.0));
    let mut screen = Recorder::default();
    app.present_to(&mut screen);
    painted.borrow_mut().clear();
    app.handle()
        .invalidate_canvas_rect(GRID, Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    app.wake_to(&mut screen);
    let presented = screen.presents.last().unwrap();
    assert_eq!(presented.len(), 1);
    assert!(
        presented[0].contains(Point::new(55.0, 45.0)),
        "{presented:?} is where the invalidated corner is drawn"
    );
    assert!(!presented[0].contains(Point::new(5.0, 5.0)));
    assert_eq!(painted.borrow().len(), 1);
}
