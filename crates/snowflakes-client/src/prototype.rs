//! Bounded P1 experiments. This is not the production simulation or window tracker.
//! Win32/COM resources stay on the creating thread; no borrowed pointer outlives a call.
use std::{
    cell::Cell,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
            Gdi::*,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{BOOL, Interface, Result, w},
};

type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

static REBUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Monitor {
    rect: RECT,
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: the caller passes a live Vec for the synchronous enumeration.
    unsafe {
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            (&mut *(data.0 as *mut Vec<Monitor>)).push(Monitor {
                rect: info.rcMonitor,
            });
        }
    }
    BOOL(1)
}

fn monitors() -> Result<Vec<Monitor>> {
    let mut result: Vec<Monitor> = Vec::new();
    // SAFETY: callback and mutable Vec are valid until EnumDisplayMonitors returns.
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut result as *mut _ as isize),
        )
        .ok()?;
    }
    Ok(result)
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            REBUILD.store(true, std::sync::atomic::Ordering::Relaxed);
            LRESULT(0)
        }
        // SAFETY: default processing receives the original Win32 arguments.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

struct Renderer {
    context: ID3D11DeviceContext1,
    view: ID3D11RenderTargetView,
    swap: IDXGISwapChain1,
    _visual: IDCompositionVisual,
    _target: IDCompositionTarget,
    _composition: IDCompositionDevice,
    _device: ID3D11Device,
}

impl Renderer {
    fn create(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        // SAFETY: all out pointers are initialized Options; COM objects remain on this thread.
        unsafe {
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(windows::core::Error::from_thread)?;
            let context: ID3D11DeviceContext1 = context
                .ok_or_else(windows::core::Error::from_thread)?
                .cast()?;
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
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut view = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut view))?;
            let view = view.ok_or_else(windows::core::Error::from_thread)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
            Ok(Self {
                context,
                view,
                swap,
                _visual: visual,
                _target: target,
                _composition: composition,
                _device: device,
            })
        }
    }

    fn draw(&self, width: i32, height: i32, scale: f64, elapsed: f64) -> Result<()> {
        // Rectangles intentionally test alpha composition without a shader dependency.
        let rectangles: Vec<RECT> = (0..80)
            .map(|index| {
                let x = ((index * 719 % width.max(1)) as f64 + (elapsed * 8.0).sin() * 12.0 * scale)
                    as i32;
                let y = ((index * 233) as f64 + elapsed * 65.0 * scale) as i32 % height.max(1);
                let size = (4.0 * scale).round().max(1.0) as i32;
                RECT {
                    left: x,
                    top: y,
                    right: x + size,
                    bottom: y + size,
                }
            })
            .collect();
        let marker = RECT {
            left: width / 2 - (60.0 * scale) as i32,
            top: height / 2 - (40.0 * scale) as i32,
            right: width / 2 + (60.0 * scale) as i32,
            bottom: height / 2 + (40.0 * scale) as i32,
        };
        // SAFETY: resource views belong to this device and remain live for the calls.
        unsafe {
            self.context
                .ClearRenderTargetView(&self.view, &[0.0, 0.0, 0.0, 0.0]);
            self.context
                .ClearView(&self.view, &[0.8, 0.8, 0.8, 0.8], Some(&rectangles));
            // Premultiplied RGB: translucent cyan marker for the click-through test.
            self.context
                .ClearView(&self.view, &[0.0, 0.3, 0.4, 0.4], Some(&[marker]));
            self.swap.Present(0, DXGI_PRESENT(0)).ok()
        }
    }
}

struct Overlay {
    hwnd: HWND,
    renderer: Option<Renderer>,
    width: i32,
    height: i32,
    scale: f64,
}

impl Overlay {
    fn create(monitor: Monitor, instance: HINSTANCE) -> Result<Self> {
        let width = monitor.rect.right - monitor.rect.left;
        let height = monitor.rect.bottom - monitor.rect.top;
        // SAFETY: the registered class lives for the process; creation data is null.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_NOREDIRECTIONBITMAP
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                w!("SnowflakesOverlayPrototype"),
                w!("Snowflakes Overlay Prototype"),
                WS_POPUP,
                monitor.rect.left,
                monitor.rect.top,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        let mut overlay = Self {
            hwnd,
            renderer: None,
            width,
            height,
            scale: 1.0,
        };
        // SAFETY: HWND is owned by this struct, and ShowWindow explicitly avoids activation.
        unsafe {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            overlay.scale = f64::from(GetDpiForWindow(hwnd)) / 96.0;
            overlay.renderer = Some(Renderer::create(hwnd, width as u32, height as u32)?);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(overlay)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // Drop the entire COM graph before destroying its target HWND.
        drop(self.renderer.take());
        // SAFETY: this struct exclusively owns the window on the creating thread.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn make_overlays(instance: HINSTANCE) -> AnyResult<Vec<Overlay>> {
    let monitors = monitors()?;
    if monitors.is_empty() {
        return Err("no desktop monitors".into());
    }
    let result: Result<Vec<Overlay>> = monitors
        .into_iter()
        .map(|monitor| Overlay::create(monitor, instance))
        .collect();
    Ok(result?)
}

/// Bounded experiment, with a synthetic pause and full resource rebuild if requested.
pub fn overlay(seconds: u64, exercise_recovery: bool) -> AnyResult<()> {
    if !(1..=600).contains(&seconds) {
        return Err("prototype duration must be 1..600 seconds".into());
    }
    // SAFETY: DPI context is set before any windows; all following windows share this thread.
    let instance = unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        HINSTANCE(GetModuleHandleW(None)?.0)
    };
    let class = WNDCLASSW {
        lpfnWndProc: Some(overlay_proc),
        hInstance: instance,
        lpszClassName: w!("SnowflakesOverlayPrototype"),
        ..Default::default()
    };
    // SAFETY: class data is copied by Windows and its procedure has process lifetime.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread().into());
    }
    let mut overlays = make_overlays(instance)?;
    println!("D3D11 composition prototype: {} monitor(s)", overlays.len());
    for (index, overlay) in overlays.iter().enumerate() {
        println!(
            "monitor {index}: {}x{}, scale {:.2}",
            overlay.width, overlay.height, overlay.scale
        );
    }
    let started = Instant::now();
    let mut previous = started;
    let mut simulated = 0.0;
    let mut frames = 0_u64;
    let mut paused = false;
    let mut recovered = false;
    let mut activated = false;
    let mut quit = false;
    while started.elapsed() < Duration::from_secs(seconds) && !quit {
        let frame_started = Instant::now();
        let elapsed = frame_started.duration_since(started).as_secs_f64();
        let should_pause = exercise_recovery && (3.0..5.0).contains(&elapsed);
        if paused != should_pause {
            paused = should_pause;
            for overlay in &overlays {
                // SAFETY: live owned windows; resuming never activates them.
                unsafe {
                    let _ = ShowWindow(
                        overlay.hwnd,
                        if paused { SW_HIDE } else { SW_SHOWNOACTIVATE },
                    );
                }
            }
            println!("prototype pause={paused}, frames={frames}");
        }
        let rebuild = REBUILD.swap(false, std::sync::atomic::Ordering::Relaxed)
            || (exercise_recovery && elapsed >= 8.0 && !recovered);
        if rebuild {
            overlays.clear();
            overlays = make_overlays(instance)?;
            recovered = true;
            println!("prototype resources rebuilt");
        }
        if !paused {
            simulated += frame_started
                .duration_since(previous)
                .as_secs_f64()
                .min(0.1);
            for overlay in &overlays {
                if let Some(renderer) = &overlay.renderer {
                    renderer.draw(overlay.width, overlay.height, overlay.scale, simulated)?;
                }
            }
            frames += 1;
        }
        previous = frame_started;
        // SAFETY: read-only foreground query and standard message pumping on this thread.
        unsafe {
            activated |= overlays
                .iter()
                .any(|overlay| overlay.hwnd == GetForegroundWindow());
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    quit = true;
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let wait = Duration::from_millis(16)
                .saturating_sub(frame_started.elapsed())
                .as_millis() as u32;
            MsgWaitForMultipleObjectsEx(None, wait.max(1), QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
    println!(
        "prototype complete: frames={frames}, overlay_activated={activated}, resource_rebuild={recovered}"
    );
    if activated {
        return Err("overlay unexpectedly became foreground".into());
    }
    Ok(())
}

thread_local! {
    static CLICKS: Cell<u32> = const { Cell::new(0) };
    static KEYS: Cell<u32> = const { Cell::new(0) };
    static WHEEL: Cell<i32> = const { Cell::new(0) };
}

unsafe fn update_probe(hwnd: HWND) {
    let title: Vec<u16> = format!(
        "Snowflakes Input Probe | clicks: {} | keys: {} | wheel: {}\0",
        CLICKS.get(),
        KEYS.get(),
        WHEEL.get()
    )
    .encode_utf16()
    .collect();
    // SAFETY: the title is null-terminated and remains alive until SetWindowTextW returns.
    unsafe {
        let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(title.as_ptr()));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe extern "system" fn probe_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Win32 invokes this procedure on its owning thread with a live HWND.
    unsafe {
        match message {
            WM_LBUTTONDOWN => {
                CLICKS.set(CLICKS.get() + 1);
                update_probe(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                KEYS.set(KEYS.get() + 1);
                update_probe(hwnd);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                WHEEL.set(WHEEL.get() + ((wparam.0 >> 16) as i16 as i32));
                update_probe(hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let brush = CreateSolidBrush(COLORREF(0x302018));
                FillRect(dc, &rect, brush);
                let _ = DeleteObject(brush.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0xeeeeee));
                rect.left += 32;
                rect.top += 48;
                let mut text: Vec<u16> = "Snowflakes Input Probe\n\nSynthetic test window: no personal content.\n\nClick the translucent cyan rectangle, type and scroll.\nThe title counts input received by this separate process.\nDrag the title bar to test movement.\n\nThe overlay must never take keyboard focus.".encode_utf16().collect();
                DrawTextW(dc, &mut text, &mut rect, DT_LEFT | DT_WORDBREAK);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

pub fn input_probe() -> AnyResult<()> {
    // SAFETY: independent test process, standard visible Win32 window and message loop.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(probe_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("SnowflakesInputProbe"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SnowflakesInputProbe"),
            w!("Snowflakes Input Probe | clicks: 0 | keys: 0 | wheel: 0"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            650,
            None,
            None,
            Some(instance),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        let mut message = MSG::default();
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status == -1 {
                return Err(windows::core::Error::from_thread().into());
            }
            if status == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}
