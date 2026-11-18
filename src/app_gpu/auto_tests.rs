//! `Renderer::Auto` where the platform defaults to the GPU, and a GPU that
//! finds its window hidden, on a stand-in GPU presenting to a stand-in
//! window's layer.

use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::gpu::ops::Plan;
use crate::gpu::renderer::{Api, Executor, GpuError, GpuRenderer};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::{ControlFlow, Event, GpuLayer, Window, WindowId};
use crate::prelude::*;
use crate::render::{Renderer, Surface};
use std::cell::Cell;
use std::rc::Rc;

/// What the stand-in GPU and window saw.
#[derive(Default)]
struct Seen {
    made: Cell<u32>,
    gpu: Cell<u32>,
    cpu: Cell<u32>,
    read: Cell<u32>,
    occlude: Cell<bool>,
    /// The GPU can't present to the window's layer (a compositor that
    /// takes none of its `dma-buf`s).
    refuse: Cell<bool>,
}

struct Gpu {
    device: &'static str,
    seen: Rc<Seen>,
}

impl Executor for Gpu {
    fn api(&self) -> Api {
        Api::Metal
    }
    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
        self.seen.read.set(self.seen.read.get() + 1);
        Ok(Pixmap::new(plan.size))
    }
    fn can_present(&self, _: &GpuLayer) -> bool {
        !self.seen.refuse.get()
    }
    fn present(
        &mut self,
        _: &mut Plan,
        _: &GpuLayer,
        _: ScaleFactor,
        _: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        self.seen.gpu.set(self.seen.gpu.get() + 1);
        if self.seen.occlude.get() {
            return Err(GpuError::Occluded);
        }
        Ok(())
    }
    fn uploads(&self) -> usize {
        0
    }
    fn device(&self) -> String {
        self.device.into()
    }
}

struct Counting(HeadlessSurface, Rc<Seen>);

impl Surface for Counting {
    fn resize(&mut self, size: PhysicalSize) {
        self.0.resize(size)
    }
    fn size(&self) -> PhysicalSize {
        self.0.size()
    }
    fn present(&mut self, pixmap: &Pixmap, damage: &[crate::geometry::Rect]) {
        self.1.cpu.set(self.1.cpu.get() + 1);
        self.0.present(pixmap, damage)
    }
}

/// A window with a GPU layer (or without one).
struct Win {
    seen: Rc<Seen>,
    layer: Option<GpuLayer>,
}

impl Window for Win {
    fn id(&self) -> WindowId {
        WindowId(1)
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
        Box::new(Counting(
            HeadlessSurface::new(self.inner_size()).0,
            self.seen.clone(),
        ))
    }
    fn gpu_layer(&self) -> Option<GpuLayer> {
        self.layer
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

fn app(renderer: Renderer) -> App<()> {
    App::new((), spinning)
        .no_font()
        .logical_size(Size::new(40.0, 30.0))
        .renderer(renderer)
}

/// A browser canvas's layer: any layer the stand-in GPU presents to.
const LAYER: Option<GpuLayer> = Some(GpuLayer::Web);

/// Run an app choosing `renderer`, where `Auto` tries the GPU, on a GPU
/// named `device`, in a window with a GPU layer (or not); `send` delivers an
/// event and says how many frames went to the GPU and to the CPU.
fn rig(
    renderer: Renderer,
    device: &'static str,
    layer: Option<GpuLayer>,
) -> (Rc<Seen>, impl FnMut(Event) -> (u32, u32)) {
    super::AUTO_TRIES_GPU.with(|a| a.set(Some(true)));
    let seen = Rc::new(Seen::default());
    let s = seen.clone();
    super::reset(Some(Box::new(move || {
        s.made.set(s.made.get() + 1);
        Ok(GpuRenderer::with_executor(Box::new(Gpu {
            device,
            seen: s.clone(),
        })))
    })));
    let (_, mut handler) = app(renderer).into_handler();
    let s = seen.clone();
    let send = move |event: Event| {
        let (gpu, cpu) = (s.gpu.get(), s.cpu.get());
        let win = Win {
            seen: s.clone(),
            layer,
        };
        assert_eq!(handler(event, &win), ControlFlow::Wait);
        (s.gpu.get() - gpu, s.cpu.get() - cpu)
    };
    (seen, send)
}

fn done() {
    super::AUTO_TRIES_GPU.with(|a| a.set(None));
    super::reset(None);
}

#[test]
fn auto_draws_on_a_hardware_gpu() {
    let (seen, mut send) = rig(Renderer::Auto, "NVIDIA GeForce RTX 5080", LAYER);
    assert_eq!(send(Event::RedrawRequested), (1, 0), "on the GPU");
    assert_eq!(send(Event::Wake), (1, 0));
    assert_eq!(seen.made.get(), 1);
    done();
}

#[test]
fn auto_stays_on_the_cpu_with_a_software_gpu() {
    for device in [
        "llvmpipe (LLVM 19.1.7, 256 bits)",
        "Microsoft Basic Render Driver",
        "Apple Paravirtual device",
        "Google SwiftShader",
        "",
    ] {
        let (seen, mut send) = rig(Renderer::Auto, device, LAYER);
        assert_eq!(send(Event::RedrawRequested), (0, 1), "{device:?}: the CPU");
        assert_eq!(send(Event::Wake), (0, 1));
        assert_eq!(seen.made.get(), 1, "{device:?}: asked once");
        done();
    }
    // Asked for, it draws there anyway.
    let (_, mut send) = rig(Renderer::Gpu, "llvmpipe (LLVM 19.1.7, 256 bits)", LAYER);
    assert_eq!(send(Event::RedrawRequested), (1, 0));
    done();
}

#[test]
fn auto_never_reads_back() {
    // A window the GPU can't present to draws on the CPU, and the GPU isn't
    // even set up.
    let (seen, mut send) = rig(Renderer::Auto, "Apple M5 Max", None);
    assert_eq!(send(Event::RedrawRequested), (0, 1));
    assert_eq!(seen.made.get(), 0);
    done();
    // Nor does headless rendering.
    super::AUTO_TRIES_GPU.with(|a| a.set(Some(true)));
    let made = Rc::new(Cell::new(0));
    let m = made.clone();
    super::reset(Some(Box::new(move || {
        m.set(m.get() + 1);
        Err(GpuError::Unavailable("not asked".into()))
    })));
    app(Renderer::Auto).render_once();
    assert_eq!(made.get(), 0, "render_once stays on the CPU");
    done();
}

/// A Wayland window's layer: frames go to the compositor as `dma-buf`s,
/// under a title bar Stipple draws or not (the backend hands out the layer
/// either way; the sink is the backend's, never touched by the stand-in).
#[cfg(target_os = "linux")]
const WAYLAND: Option<GpuLayer> = Some(GpuLayer::Wayland {
    surface: 3,
    sink: std::ptr::null(),
});

#[cfg(target_os = "linux")]
#[test]
fn auto_presents_to_a_wayland_window_on_a_hardware_gpu() {
    let (seen, mut send) = rig(Renderer::Auto, "NVIDIA GeForce RTX 5080", WAYLAND);
    assert_eq!(send(Event::RedrawRequested), (1, 0), "dma-bufs, on the GPU");
    assert_eq!(send(Event::Wake), (1, 0));
    assert_eq!(seen.read.get(), 0, "nothing read back");
    done();
    // On lavapipe, as on CI's runners, the CPU.
    let (seen, mut send) = rig(Renderer::Auto, "llvmpipe (LLVM 19.1.7, 256 bits)", WAYLAND);
    assert_eq!(send(Event::RedrawRequested), (0, 1));
    assert_eq!(seen.read.get(), 0);
    done();
}

#[test]
fn auto_draws_on_the_cpu_where_the_gpu_cant_present() {
    // A compositor that takes none of the GPU's dma-bufs (or a layer the
    // GPU can't present to): `Auto` draws on the CPU rather than read back.
    let (seen, mut send) = rig(Renderer::Auto, "NVIDIA GeForce RTX 5080", LAYER);
    seen.refuse.set(true);
    assert_eq!(send(Event::RedrawRequested), (0, 1), "the CPU");
    assert_eq!(send(Event::Wake), (0, 1));
    assert_eq!(seen.read.get(), 0, "nothing read back");
    done();
    #[cfg(target_os = "linux")]
    {
        let (seen, mut send) = rig(Renderer::Auto, "NVIDIA GeForce RTX 5080", WAYLAND);
        seen.refuse.set(true);
        assert_eq!(send(Event::RedrawRequested), (0, 1));
        assert_eq!(seen.read.get(), 0);
        done();
    }
}

#[test]
fn auto_falls_back_when_the_gpu_fails_to_start_or_is_lost() {
    super::AUTO_TRIES_GPU.with(|a| a.set(Some(true)));
    super::reset(Some(Box::new(|| {
        Err(GpuError::Unavailable("no device".into()))
    })));
    let seen = Rc::new(Seen::default());
    let (_, mut handler) = app(Renderer::Auto).into_handler();
    let win = Win {
        seen: seen.clone(),
        layer: LAYER,
    };
    handler(Event::RedrawRequested, &win);
    assert_eq!(seen.cpu.get(), 1, "no device: the CPU");
    assert!(super::given_up());
    done();

    let (seen, mut send) = rig(Renderer::Auto, "Apple M5 Max", LAYER);
    assert_eq!(send(Event::RedrawRequested), (1, 0));
    super::current(|r| r.expect("a renderer").lose_device());
    assert_eq!(send(Event::Wake), (0, 1), "lost: that frame on the CPU");
    assert_eq!(send(Event::Wake), (1, 0), "set up again");
    assert_eq!(seen.made.get(), 2);
    super::current(|r| r.expect("a renderer").lose_device());
    assert_eq!(send(Event::Wake), (0, 1));
    assert_eq!(send(Event::Wake), (0, 1), "lost twice: the CPU from now on");
    assert_eq!(seen.made.get(), 2);
    done();
}

#[test]
fn the_environment_and_the_app_still_win() {
    use super::{Want, resolve};
    super::AUTO_TRIES_GPU.with(|a| a.set(Some(true)));
    assert_eq!(resolve(Renderer::Auto, None), Want::Auto);
    assert_eq!(
        resolve(Renderer::Auto, Some(Want::Software)),
        Want::Software
    );
    assert_eq!(resolve(Renderer::Software, None), Want::Software);
    super::AUTO_TRIES_GPU.with(|a| a.set(Some(false)));
    assert_eq!(resolve(Renderer::Auto, None), Want::Software);
    super::AUTO_TRIES_GPU.with(|a| a.set(None));
}

#[test]
fn a_window_the_gpu_finds_hidden_stops_drawing_until_seen() {
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (seen, mut send) = rig(Renderer::Gpu, "Apple M5 Max", LAYER);
    assert_eq!(send(Event::RedrawRequested), (1, 0));
    seen.occlude.set(true);
    assert_eq!(send(Event::Wake), (1, 0), "presented, but went nowhere");
    assert_eq!(send(Event::Wake), (0, 0), "hidden: no frames");
    assert!(
        !crate::platform::frame::suspended(),
        "the GPU's word alone doesn't stop the clock"
    );
    seen.occlude.set(false);
    // A repaint the system asks for shows it again.
    assert_eq!(send(Event::RedrawRequested), (1, 0));
    assert_eq!(send(Event::Wake), (1, 0), "animating again");
    done();
}
