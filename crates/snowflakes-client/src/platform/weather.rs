//! Native GPU presentation of the physical snow model. No input hooks or timelines.
use super::{
    scene::Tracker,
    system_events::{Conditions, Observer},
};
use crate::{
    control::Status,
    geometry::{Point, Scene, Screen},
    snow::{Cursor, World},
    timing::StepClock,
};
use snowflakes_protocol::{MonitorSelection, Settings, WeatherMode};
use std::{
    collections::BTreeMap,
    f32::consts::TAU,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{Interface, Result, w},
};
use windows_numerics::{Matrix3x2, Vector2};

const CLASS: windows::core::PCWSTR = w!("SnowflakesWeatherLayer");
fn point(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}
fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, message, w, l) },
    }
}
struct Gpu {
    context: ID2D1DeviceContext,
    factory: ID2D1Factory1,
    brush: ID2D1SolidColorBrush,
    snow_brush: ID2D1LinearGradientBrush,
    crystals: Vec<ID2D1PathGeometry1>,
    swap: IDXGISwapChain1,
    _bitmap: ID2D1Bitmap1,
    _d2d: ID2D1Device,
    _d3d: ID3D11Device,
    _visual: IDCompositionVisual,
    _target: IDCompositionTarget,
    _composition: IDCompositionDevice,
}
impl Gpu {
    fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let mut device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.ok_or_else(windows::core::Error::from_thread)?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let description = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&device, &description, None)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = d2d_factory.CreateDevice(&dxgi)?;
            let context = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let surface: IDXGISurface = swap.GetBuffer(0)?;
            let properties = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = context.CreateBitmapFromDxgiSurface(&surface, Some(&properties))?;
            context.SetTarget(&bitmap);
            context.SetDpi(96.0, 96.0);
            let brush = context.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)?;
            let render_target: ID2D1RenderTarget = context.cast()?;
            let stops = render_target.CreateGradientStopCollection(
                &[
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: color(1.0, 1.0, 1.0, 1.0),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: color(0.73, 0.83, 0.94, 1.0),
                    },
                ],
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let snow_brush = context.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: point(0.0, 0.0),
                    endPoint: point(0.0, 20.0),
                },
                None,
                &stops,
            )?;
            let mut crystals = Vec::new();
            for variant in 0..8 {
                let path = d2d_factory.CreatePathGeometry()?;
                let sink = path.Open()?;
                for arm in 0..6 {
                    let a = arm as f32 * TAU / 6.0;
                    let (s, c) = a.sin_cos();
                    sink.BeginFigure(point(0.0, 0.0), D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddLine(point(c, s));
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    for t in [0.38 + variant as f32 * 0.012, 0.69] {
                        let length = if t < 0.6 { 0.25 } else { 0.21 };
                        for side in [-1.0, 1.0] {
                            sink.BeginFigure(point(c * t, s * t), D2D1_FIGURE_BEGIN_HOLLOW);
                            sink.AddLine(point(
                                c * (t + length * 0.5) - s * side * length * 0.866,
                                s * (t + length * 0.5) + c * side * length * 0.866,
                            ));
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                    }
                }
                sink.Close()?;
                crystals.push(path);
            }
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
            Ok(Self {
                context,
                factory: d2d_factory,
                brush,
                snow_brush,
                crystals,
                swap,
                _bitmap: bitmap,
                _d2d: d2d,
                _d3d: device,
                _visual: visual,
                _target: target,
                _composition: composition,
            })
        }
    }
    fn draw(&self, screen: &Screen, world: &World, settings: &Settings) -> Result<()> {
        unsafe {
            let dc = &self.context;
            dc.BeginDraw();
            dc.SetTransform(&Matrix3x2::identity());
            dc.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let opacity = settings.weather.opacity_percent as f32 / 100.0;
            self.brush.SetColor(&color(0.94, 0.97, 1.0, opacity * 0.94));
            for drift in world.drifts.values() {
                if drift.mass.iter().all(|&m| m < 0.001)
                    || drift.support.draw_clips.is_empty()
                    || drift.support.y < screen.rect.top
                    || drift.support.y > screen.rect.bottom
                {
                    continue;
                }
                let path = self.factory.CreatePathGeometry()?;
                let sink = path.Open()?;
                let base = drift.support.y - screen.rect.top;
                sink.BeginFigure(
                    point(drift.support.left - screen.rect.left, base),
                    D2D1_FIGURE_BEGIN_FILLED,
                );
                for index in 0..drift.mass.len() {
                    let x = drift.x(index);
                    let y = base - drift.height(index, &world.scene);
                    sink.AddLine(point(x - screen.rect.left, y));
                }
                sink.AddLine(point(drift.support.right - screen.rect.left, base));
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                // A new local peak must not recolor the whole snow surface.
                let depth = 24.0 * screen.scale;
                self.snow_brush.SetStartPoint(point(0.0, base - depth));
                self.snow_brush.SetEndPoint(point(0.0, base));
                self.snow_brush.SetOpacity(opacity * 0.97);
                self.brush
                    .SetColor(&color(0.36, 0.48, 0.66, opacity * 0.18));
                for region in &drift.support.draw_clips {
                    let left = region.left.max(screen.rect.left);
                    let right = region.right.min(screen.rect.right);
                    let top = region.top.max(screen.rect.top);
                    let bottom = region.bottom.min(screen.rect.bottom);
                    if right <= left || bottom <= top {
                        continue;
                    }
                    let clip = D2D_RECT_F {
                        left: left - screen.rect.left,
                        top: top - screen.rect.top,
                        right: right - screen.rect.left,
                        bottom: bottom - screen.rect.top,
                    };
                    // Adjacent mask strips must meet without antialiased seams.
                    // The snow contour itself still uses normal antialiasing.
                    dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                    dc.DrawGeometry(&path, &self.brush, 1.5, None);
                    dc.FillGeometry(&path, &self.snow_brush, None);
                    dc.PopAxisAlignedClip();
                }
            }
            for flake in &world.flakes {
                let scale = world.scene.scale_at(flake.position);
                if let Some(slab) = &flake.slab {
                    let outline = slab.outline();
                    let path = self.factory.CreatePathGeometry()?;
                    let sink = path.Open()?;
                    let project = |local| {
                        let p = flake.transformed_point(local, scale);
                        point(p.x - screen.rect.left, p.y - screen.rect.top)
                    };
                    sink.BeginFigure(project(outline[0]), D2D1_FIGURE_BEGIN_FILLED);
                    for &local in &outline[1..] {
                        sink.AddLine(project(local));
                    }
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    sink.Close()?;
                    dc.SetTransform(&Matrix3x2::identity());
                    let top = project(crate::geometry::Point::new(0.0, -24.0));
                    let bottom = project(crate::geometry::Point::new(0.0, 0.0));
                    self.snow_brush.SetStartPoint(top);
                    self.snow_brush.SetEndPoint(bottom);
                    self.snow_brush.SetOpacity(opacity * 0.97);
                    self.brush
                        .SetColor(&color(0.36, 0.48, 0.66, opacity * 0.18));
                    dc.DrawGeometry(&path, &self.brush, 1.5, None);
                    dc.FillGeometry(&path, &self.snow_brush, None);
                    continue;
                }
                let radius = flake.radius * scale;
                if flake.position.x + radius < screen.rect.left
                    || flake.position.x - radius > screen.rect.right
                    || flake.position.y + radius < screen.rect.top
                    || flake.position.y - radius > screen.rect.bottom
                {
                    continue;
                }
                let visibility = settings.snow.flake_visibility_percent as f32 / 100.0;
                let alpha =
                    (opacity * flake.opacity * (flake.age / 0.7).min(1.0) * visibility).min(1.0);
                let x = flake.position.x - screen.rect.left;
                let y = flake.position.y - screen.rect.top;
                if flake.radius < 2.0 {
                    dc.SetTransform(&Matrix3x2::identity());
                    self.brush.SetColor(&color(0.94, 0.97, 1.0, alpha * 0.35));
                    dc.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: point(x, y),
                            radiusX: radius * 0.75,
                            radiusY: radius * 0.75,
                        },
                        &self.brush,
                    );
                    self.brush.SetColor(&color(1.0, 1.0, 1.0, alpha));
                    dc.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: point(x, y),
                            radiusX: radius * 0.43,
                            radiusY: radius * 0.43,
                        },
                        &self.brush,
                    );
                } else {
                    let (s, c) = flake.angle.sin_cos();
                    dc.SetTransform(&Matrix3x2 {
                        M11: c * radius,
                        M12: s * radius,
                        M21: -s * radius,
                        M22: c * radius,
                        M31: x,
                        M32: y,
                    });
                    let crystal =
                        &self.crystals[((flake.shape * 100.0) as usize) % self.crystals.len()];
                    if visibility > 1.0 {
                        self.brush.SetColor(&color(
                            0.24,
                            0.32,
                            0.44,
                            (visibility - 1.0) * 0.28 * opacity,
                        ));
                        dc.DrawGeometry(crystal, &self.brush, 0.24, None);
                    }
                    if flake.radius > 4.0 {
                        self.brush.SetColor(&color(0.88, 0.95, 1.0, alpha * 0.18));
                        dc.DrawGeometry(crystal, &self.brush, 0.30, None);
                    }
                    self.brush.SetColor(&color(1.0, 1.0, 1.0, alpha));
                    dc.DrawGeometry(crystal, &self.brush, 0.13, None);
                }
            }
            dc.SetTransform(&Matrix3x2::identity());
            dc.EndDraw(None, None)?;
            self.swap.Present(0, DXGI_PRESENT(0)).ok()
        }
    }
}
struct Layer {
    hwnd: HWND,
    screen: Screen,
    gpu: Option<Gpu>,
    visible: bool,
}
impl Layer {
    fn new(mut screen: Screen) -> Result<Self> {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_NOREDIRECTIONBITMAP
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                CLASS,
                w!("Snowflakes"),
                WS_POPUP,
                screen.rect.left as i32,
                screen.rect.top as i32,
                screen.rect.width() as i32,
                screen.rect.height() as i32,
                None,
                None,
                Some(instance),
                None,
            )?;
            let mut layer = Self {
                hwnd,
                screen: screen.clone(),
                gpu: None,
                visible: false,
            };
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            screen.scale = (GetDpiForWindow(hwnd) as f32 / 96.0).max(0.5);
            layer.screen = screen;
            layer.gpu = Some(Gpu::new(
                hwnd,
                layer.screen.rect.width() as u32,
                layer.screen.rect.height() as u32,
            )?);
            Ok(layer)
        }
    }
    fn show(&mut self, show: bool) {
        if self.visible == show {
            return;
        }
        // SetWindowPos avoids a launcher's ShowWindow startup flags and activation.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOACTIVATE
                    | if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW },
            );
        }
        self.visible = show;
    }
}
impl Drop for Layer {
    fn drop(&mut self) {
        drop(self.gpu.take());
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[derive(Clone)]
pub struct Report {
    pub renderer: &'static str,
    pub rendering: bool,
    pub particles: u32,
    pub monitors: u32,
}
impl Default for Report {
    fn default() -> Self {
        Self {
            renderer: "not_connected",
            rendering: false,
            particles: 0,
            monitors: 0,
        }
    }
}
pub struct Weather {
    tracker: Tracker,
    world: World,
    layers: BTreeMap<u64, Layer>,
    clock: StepClock,
    next: Instant,
    retry: Instant,
    failures: u8,
    epoch: u64,
    seeded: bool,
    cursor: Option<(Point, Instant)>,
    last_enabled: bool,
    was_paused: bool,
    native_revision: u64,
}
impl Weather {
    pub fn new() -> Result<Self> {
        let instance = unsafe { HINSTANCE(GetModuleHandleW(None)?.0) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(windows::core::Error::from_thread());
        }
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64)
            ^ std::process::id() as u64;
        Ok(Self {
            tracker: Tracker::new()?,
            world: World::new(Scene::default(), seed),
            layers: BTreeMap::new(),
            clock: StepClock::default(),
            next: Instant::now(),
            retry: Instant::now(),
            failures: 0,
            epoch: 0,
            seeded: false,
            cursor: None,
            last_enabled: false,
            was_paused: false,
            native_revision: 0,
        })
    }
    pub fn wait_ms(&self) -> u32 {
        self.next
            .saturating_duration_since(Instant::now())
            .as_millis()
            .clamp(1, 100) as u32
    }
    pub fn frame(&mut self, status: &Status, conditions: &Conditions) -> Report {
        let now = Instant::now();
        let revision = conditions.revision();
        if revision != self.native_revision {
            self.native_revision = revision;
            self.clock.reset();
            self.cursor = None;
            self.tracker.refresh();
        }
        let supported = status
            .settings
            .as_ref()
            .is_some_and(|s| s.weather.mode == WeatherMode::Snow);
        let enabled = status.desired_effects_enabled && supported;
        if !enabled && self.last_enabled {
            self.failures = 0;
            self.seeded = false;
        }
        self.last_enabled = enabled;
        if status.clear_epoch != self.epoch {
            self.world.clear();
            self.epoch = status.clear_epoch;
        }
        let paused = !status.pauses.is_empty() || conditions.paused(status.settings.as_ref());
        if !enabled || paused {
            for layer in self.layers.values_mut() {
                layer.show(false);
            }
            self.clock.reset();
            self.cursor = None;
            self.was_paused = true;
            self.next = now + Duration::from_millis(100);
            return Report {
                renderer: if !conditions.ready() {
                    "system_events_unavailable"
                } else if supported {
                    "d3d11_direct2d_idle"
                } else {
                    "not_connected"
                },
                rendering: false,
                particles: self.world.flakes.len() as u32,
                monitors: self.layers.values().filter(|layer| layer.visible).count() as u32,
            };
        }
        if self.was_paused {
            self.tracker.refresh();
            self.was_paused = false;
        }
        let settings = status.settings.as_ref().unwrap();
        if now < self.next {
            return Report {
                renderer: "d3d11_direct2d",
                rendering: self.layers.values().any(|l| l.visible),
                particles: self.world.flakes.len() as u32,
                monitors: self.layers.values().filter(|layer| layer.visible).count() as u32,
            };
        }
        self.next = now + Duration::from_secs_f64(1.0 / settings.performance.target_fps as f64);
        let result = self.update(now, settings);
        if let Err(error) = result {
            eprintln!("weather_graphics_error: {:08x}", error.code().0);
            self.layers.clear();
            self.clock.reset();
            self.failures = self.failures.saturating_add(1);
            self.retry = now + Duration::from_secs(2_u64.pow(self.failures.min(3) as u32));
            return Report {
                renderer: if self.failures >= 3 {
                    "graphics_unavailable"
                } else {
                    "graphics_recovering"
                },
                rendering: false,
                particles: self.world.flakes.len() as u32,
                monitors: self.layers.values().filter(|layer| layer.visible).count() as u32,
            };
        }
        Report {
            renderer: if self.failures >= 3 {
                "graphics_unavailable"
            } else {
                "d3d11_direct2d"
            },
            rendering: self.layers.values().any(|l| l.visible),
            particles: self.world.flakes.len() as u32,
            monitors: self.layers.values().filter(|layer| layer.visible).count() as u32,
        }
    }
    fn update(&mut self, now: Instant, settings: &Settings) -> Result<()> {
        if let Some(mut scene) = self.tracker.poll(now)? {
            scene.screens.retain(|s| match settings.display.selection {
                MonitorSelection::All => true,
                MonitorSelection::Primary => s.rect.contains(Point::new(0.0, 0.0)),
                MonitorSelection::Selected => settings.display.monitor_ids.contains(&s.id()),
            });
            self.layers.retain(|key, layer| {
                scene
                    .screens
                    .iter()
                    .any(|s| s.key == *key && s.rect == layer.screen.rect)
            });
            for screen in &mut scene.screens {
                if !self.layers.contains_key(&screen.key) && now >= self.retry && self.failures < 3
                {
                    let layer = Layer::new(screen.clone())?;
                    self.layers.insert(screen.key, layer);
                }
                if let Some(layer) = self.layers.get_mut(&screen.key) {
                    screen.scale = unsafe { GetDpiForWindow(layer.hwnd) } as f32 / 96.0;
                    layer.screen = screen.clone();
                }
            }
            self.world.set_scene(scene);
        }
        if self.layers.is_empty() {
            self.clock.reset();
            return Ok(());
        }
        self.world.set_flake_size(settings.snow.flake_size_percent);
        self.world
            .set_paused_monitors(if settings.performance.pause_fullscreen {
                self.tracker.fullscreen.clone()
            } else {
                Default::default()
            });
        if !self.seeded {
            self.world.seed_air(settings);
            self.seeded = true;
        }
        let mut mouse = POINT::default();
        let cursor = if unsafe { GetCursorPos(&mut mouse) }.is_ok() {
            let position = Point::new(mouse.x as f32, mouse.y as f32);
            let velocity = self.cursor.map_or(Point::default(), |(previous, time)| {
                let dt = now.duration_since(time).as_secs_f32().max(0.001);
                let scale = self.world.scene.scale_at(position);
                Point::new(
                    (position.x - previous.x) / dt / scale,
                    (position.y - previous.y) / dt / scale,
                )
            });
            self.cursor = Some((position, now));
            Some(Cursor { position, velocity })
        } else {
            None
        };
        let all_fullscreen = settings.performance.pause_fullscreen
            && self
                .layers
                .keys()
                .all(|id| self.tracker.fullscreen.contains(id));
        for _ in 0..self.clock.advance(now, !all_fullscreen).count {
            self.world.step(settings, cursor);
        }
        for layer in self.layers.values_mut() {
            let visible = !settings.performance.pause_fullscreen
                || !self.tracker.fullscreen.contains(&layer.screen.key);
            if visible {
                layer
                    .gpu
                    .as_ref()
                    .unwrap()
                    .draw(&layer.screen, &self.world, settings)?;
            }
            layer.show(visible);
        }
        Ok(())
    }
}
impl Drop for Weather {
    fn drop(&mut self) {
        self.layers.clear();
        unsafe {
            if let Ok(module) = GetModuleHandleW(None) {
                let _ = UnregisterClassW(CLASS, Some(HINSTANCE(module.0)));
            }
        }
    }
}

/// HWND, COM and hooks are created/dropped on this dedicated rendering thread.
/// Native menus on the GUI thread cannot stall snow or its input-transparent layer.
pub struct Worker {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(
        view: std::sync::Arc<std::sync::Mutex<crate::ui::View>>,
        report: std::sync::Arc<std::sync::Mutex<Report>>,
        conditions: std::sync::Arc<Conditions>,
        session_id: u32,
    ) -> Self {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            unsafe {
                let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
            let mut weather = match Weather::new() {
                Ok(weather) => weather,
                Err(_) => {
                    if let Ok(mut current) = report.lock() {
                        current.renderer = "graphics_unavailable";
                    }
                    return;
                }
            };
            let mut observer = None;
            let mut retry_events = Instant::now();
            while !stopping.load(Ordering::Acquire) {
                // Process power/session messages before issuing another GPU frame.
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        if message.message == WM_QUIT {
                            return;
                        }
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                let now = Instant::now();
                if observer.is_none() && now >= retry_events {
                    observer = Observer::new(session_id, conditions.clone()).ok();
                    retry_events = now + Duration::from_secs(2);
                }
                if let Some(observer) = &mut observer {
                    observer.poll(now);
                }
                let status = view.lock().ok().map(|v| v.status.clone());
                if let Some(status) = status {
                    let result = weather.frame(&status, &conditions);
                    if let Ok(mut current) = report.lock() {
                        *current = result;
                    }
                }
                unsafe {
                    MsgWaitForMultipleObjectsEx(
                        None,
                        weather.wait_ms(),
                        QS_ALLINPUT,
                        MWMO_INPUTAVAILABLE,
                    );
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
