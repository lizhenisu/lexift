//! Owned native lifecycle checks, separate from throughput measurement. Resource
//! counts cover the fixture process, not the complete application's UI/IME.
use super::*;
#[path = "performance_memory.rs"]
mod memory_probe;
use memory_probe::process_bytes as memory_bytes;

pub(super) fn counts() -> (u32, u32, u32) {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    let process = unsafe { GetCurrentProcess() };
    let mut handles = 0;
    unsafe {
        GetProcessHandleCount(process, &mut handles).unwrap();
    }
    (
        handles,
        unsafe { GetGuiResources(process, GR_GDIOBJECTS) },
        unsafe { GetGuiResources(process, GR_USEROBJECTS) },
    )
}

/// Run each isolation case in a fresh test process. The same observation delay
/// as clear/reopen keeps asynchronous retirement out of immediate counts; it
/// does not assert that Windows/driver caches finish retiring within that time.
fn observe_retirement(label: &str, mut action: impl FnMut()) {
    if cfg!(debug_assertions) {
        panic!("run lifecycle verification with --release");
    }
    // Match the retained render worker: its apartment survives per-scene
    // backends. Otherwise this probe would also measure full COM retirement.
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    // Opt-in batch observations distinguish delayed system retirement from
    // growth across successive batches without changing production teardown.
    let idle_ms = std::env::var("LEXIFT_RESOURCE_BATCH_IDLE_MS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .expect("invalid resource idle duration")
        });
    let rounds = if idle_ms.is_some() { 60 } else { 64 };
    if idle_ms.is_some() {
        action();
        observe_idle(label, "warmup", Duration::from_millis(250));
    }
    for round in 0..rounds {
        action();
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        println!(
            "isolation={label} round={round} private_working_set_bytes={:?} handle_gdi_user={:?}",
            memory_bytes(),
            counts()
        );
        if round == 0 || round % 16 == 15 {
            memory_probe::report(label, round);
        }
        if let Some(idle_ms) = idle_ms
            && (round + 1) % 20 == 0
        {
            observe_idle(
                label,
                &format!("batch{}", (round + 1) / 20),
                Duration::from_millis(idle_ms),
            );
        }
    }
}

/// Keep the fixture's COM apartment pumping during the observation interval;
/// blocking its messages would itself delay native resource retirement.
fn observe_idle(label: &str, stage: &str, duration: Duration) {
    let acknowledgement = std::env::var_os("LEXIFT_RESOURCE_OBSERVER_DIR")
        .map(|directory| std::path::PathBuf::from(directory).join(format!("{label}-{stage}.ack")));
    if let Some(path) = &acknowledgement {
        assert!(!path.exists(), "stale lifecycle observer acknowledgement");
    }
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        surface::pump_messages();
        thread::sleep(Duration::from_millis(5));
    }
    println!(
        "isolation={label} stage={stage} idle_ms={} private_working_set_bytes={:?} handle_gdi_user={:?}",
        duration.as_millis(),
        memory_bytes(),
        counts()
    );
    // The external classifier must finish before the next batch allocates or
    // the final fixture exits. A missing observer fails instead of reading a
    // dead process as zero resources or sampling the next batch's peak.
    if let Some(path) = acknowledgement {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(Instant::now() < deadline, "lifecycle observer timed out");
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "D3D11 backend retirement isolation; hardware Windows and Release required"]
fn lifecycle_memory_backend_without_capture_sessions() {
    observe_retirement("backend", || drop(Backend::new().unwrap()));
}

#[test]
#[ignore = "WGC capability activation isolation; Windows and Release required"]
fn lifecycle_memory_support_without_pool() {
    observe_retirement("support", || {
        assert!(super::super::super::screen_capture::activation::supported().unwrap());
    });
}

#[test]
#[ignore = "temporary capture apartments without pools; Windows and Release required"]
fn lifecycle_memory_support_on_fresh_threads() {
    observe_retirement("support_threads", || {
        thread::spawn(|| {
            assert!(super::super::super::screen_capture::activation::supported().unwrap());
        })
        .join()
        .unwrap();
    });
}

#[test]
#[ignore = "one-shot capture Port construction without UI or element requests; Windows and Release required"]
fn lifecycle_memory_capture_port_without_queries() {
    observe_retirement("capture_port", || {
        thread::spawn(|| {
            drop(super::super::super::screen_capture::WindowsScreenCapturePort::new().unwrap());
        })
        .join()
        .unwrap();
    });
}

#[test]
#[ignore = "raw D3D11 device retirement without WinRT adapters; Windows and Release required"]
fn lifecycle_memory_raw_device_without_winrt() {
    observe_raw_retirement(false, false);
}

#[test]
#[ignore = "raw D3D11 texture retirement without WinRT adapters; Windows and Release required"]
fn lifecycle_memory_raw_texture_without_winrt() {
    observe_raw_retirement(true, false);
}

#[test]
#[ignore = "WARP device retirement without hardware D3D initialization; Windows and Release required"]
fn lifecycle_memory_warp_device_without_texture() {
    observe_raw_retirement(false, true);
}

#[test]
#[ignore = "WARP texture retirement hardware-driver isolation; Windows and Release required"]
fn lifecycle_memory_warp_device_with_texture() {
    observe_raw_retirement(true, true);
}

#[test]
#[ignore = "raw textures on one retained D3D11 device; Windows and Release required"]
fn lifecycle_memory_raw_textures_on_retained_device() {
    observe_retained_raw_device(false);
}

#[test]
#[ignore = "retained D3D11 deferred destruction isolation; Windows and Release required"]
fn lifecycle_memory_retained_textures_with_flush() {
    observe_retained_raw_device(true);
}

#[test]
#[ignore = "empty joined threads without GPU initialization; Windows and Release required"]
fn lifecycle_memory_empty_threads_without_gpu() {
    observe_empty_threads(false);
}

#[test]
#[ignore = "empty joined threads with a retained GPU device; Windows and Release required"]
fn lifecycle_memory_empty_threads_with_gpu() {
    observe_empty_threads(true);
}

/// Keep one device/resource resident while testing thread attach/detach alone.
/// The workers do not initialize COM, capture, touch D3D, or retain resources.
/// Seventeen threads per round contrasts the earlier fixed allocation count;
/// matching a block count still would not prove its allocation call stack.
fn observe_empty_threads(with_gpu: bool) {
    use windows::{Win32::System::LibraryLoader::GetModuleHandleW, core::w};
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let gpu = with_gpu.then(|| {
        let (device, context) = raw_device();
        let texture = raw_texture(&device);
        (device, context, texture)
    });
    let label = if with_gpu {
        "empty_threads_gpu"
    } else {
        "empty_threads_control"
    };
    println!(
        "isolation={label} nvidia_driver_loaded={}",
        unsafe { GetModuleHandleW(w!("nvwgf2umx.dll")) }.is_ok()
    );
    observe_retirement(label, || {
        for _ in 0..17 {
            thread::spawn(|| {}).join().unwrap();
        }
    });
    println!(
        "isolation={label} nvidia_driver_loaded_after={}",
        unsafe { GetModuleHandleW(w!("nvwgf2umx.dll")) }.is_ok()
    );
    if let Some((device, context, texture)) = gpu {
        drop(texture);
        unsafe {
            context.ClearState();
            context.Flush();
        }
        drop(context);
        drop(device);
    }
}

/// Contrast deferred retirement on one device, without drawing or capture.
/// ClearState/Flush is a test intervention, not a production close policy or a
/// GPU completion wait. Device reconstruction has a separate allocation probe.
fn observe_retained_raw_device(flush: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Initialize before device creation, matching all other retirement probes.
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let device_destroyed = Arc::new(AtomicUsize::new(0));
    let texture_destroyed = Arc::new(AtomicUsize::new(0));
    let (device, context) = raw_device();
    let device_registered = register_destruction(&device, &device_destroyed);
    let mut registered = 0;
    let label = if flush {
        "raw_retained_flush"
    } else {
        "raw_retained_device"
    };
    observe_retirement(label, || {
        let texture = raw_texture(&device);
        registered += usize::from(register_destruction(&texture, &texture_destroyed));
        drop(texture);
        if flush {
            unsafe {
                context.ClearState();
                context.Flush();
            }
        }
    });
    assert_eq!(texture_destroyed.load(Ordering::Acquire), registered);
    drop(context);
    drop(device);
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        surface::pump_messages();
        thread::sleep(Duration::from_millis(5));
    }
    let observed = (
        device_destroyed.load(Ordering::Acquire),
        texture_destroyed.load(Ordering::Acquire),
    );
    println!(
        "isolation={label} destruction_registered=({}, {registered}) destruction_observed={observed:?}",
        usize::from(device_registered)
    );
    assert_eq!(observed, (usize::from(device_registered), registered));
    memory_probe::report(&format!("{label}_released"), 64);
}

fn observe_raw_retirement(with_texture: bool, warp: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let device_destroyed = Arc::new(AtomicUsize::new(0));
    let texture_destroyed = Arc::new(AtomicUsize::new(0));
    let mut registered = (0, 0);
    let label = match (with_texture, warp) {
        (false, false) => "raw_device",
        (true, false) => "raw_texture",
        (false, true) => "warp_device",
        (true, true) => "warp_texture",
    };
    observe_retirement(label, || {
        let supported =
            retire_raw_device(with_texture, warp, &device_destroyed, &texture_destroyed);
        registered.0 += usize::from(supported.0);
        registered.1 += usize::from(supported.1);
    });
    let destroyed = (
        device_destroyed.load(Ordering::Acquire),
        texture_destroyed.load(Ordering::Acquire),
    );
    println!(
        "isolation={label} destruction_registered={registered:?} destruction_observed={destroyed:?}"
    );
    assert_eq!(
        destroyed, registered,
        "registered destruction callbacks missing after retirement observation"
    );
}

/// The callback owns only its counter, never the retiring COM object. A failed
/// registration returns that reference locally; successful registration releases
/// it in the destruction callback, which must not access the destroyed object.
fn register_destruction<T: windows::core::Interface>(
    object: &T,
    counter: &Arc<std::sync::atomic::AtomicUsize>,
) -> bool {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::Win32::Graphics::Direct3D::ID3DDestructionNotifier;
    unsafe extern "system" fn destroyed(data: *mut std::ffi::c_void) {
        let counter = unsafe { Arc::from_raw(data.cast::<AtomicUsize>()) };
        counter.fetch_add(1, Ordering::Release);
    }
    let Ok(notifier) = object.cast::<ID3DDestructionNotifier>() else {
        return false;
    };
    let data = Arc::into_raw(Arc::clone(counter));
    if unsafe { notifier.RegisterDestructionCallback(Some(destroyed), data.cast()) }.is_err() {
        unsafe {
            drop(Arc::from_raw(data));
        }
        return false;
    }
    true
}

/// Isolate the native device's first resource creation from WinRT wrapping,
/// capture item/factory activation, application renderer and shader compilation.
fn retire_raw_device(
    with_texture: bool,
    warp: bool,
    device_destroyed: &Arc<std::sync::atomic::AtomicUsize>,
    texture_destroyed: &Arc<std::sync::atomic::AtomicUsize>,
) -> (bool, bool) {
    let (device, context) = raw_device_for_driver(warp);
    let device_registered = register_destruction(&device, device_destroyed);
    let mut texture_registered = false;
    if with_texture {
        let texture = raw_texture(&device);
        texture_registered = register_destruction(&texture, texture_destroyed);
        drop(texture);
    }
    drop(context);
    drop(device);
    (device_registered, texture_registered)
}

fn raw_device() -> (
    windows::Win32::Graphics::Direct3D11::ID3D11Device,
    windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
) {
    raw_device_for_driver(false)
}

/// WARP changes only device creation in the resource isolation fixture. It is
/// not a production fallback or evidence about hardware presentation latency.
fn raw_device_for_driver(
    warp: bool,
) -> (
    windows::Win32::Graphics::Direct3D11::ID3D11Device,
    windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
) {
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D11::*,
        },
    };
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            if warp {
                D3D_DRIVER_TYPE_WARP
            } else {
                D3D_DRIVER_TYPE_HARDWARE
            },
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .unwrap();
    }
    let device = device.unwrap();
    if warp {
        use windows::{
            Win32::Graphics::Dxgi::{DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIDevice},
            core::Interface,
        };
        let dxgi: IDXGIDevice = device.cast().unwrap();
        let adapter: IDXGIAdapter1 = unsafe { dxgi.GetAdapter() }.unwrap().cast().unwrap();
        let description = unsafe { adapter.GetDesc1() }.unwrap();
        assert_ne!(description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32, 0);
    }
    (device, context.unwrap())
}

fn raw_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
) -> windows::Win32::Graphics::Direct3D11::ID3D11Texture2D {
    use windows::Win32::Graphics::{
        Direct3D11::*,
        Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC},
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: 1280,
                    Height: 720,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        ..Default::default()
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut texture),
            )
            .unwrap();
    }
    texture.unwrap()
}

#[test]
#[ignore = "GPU renderer retirement without capture/presentation; Windows and Release required"]
fn lifecycle_memory_renderer_without_capture_sessions() {
    observe_retirement("renderer", || drop(Renderer::new().unwrap()));
}

#[test]
#[ignore = "runtime shader compiler allocation isolation; Windows and Release required"]
fn lifecycle_memory_shader_compiler_without_gpu_device() {
    use windows::core::s;
    let source = include_str!("blit.hlsl");
    observe_retirement("shader_compiler", || {
        for entry in [
            s!("grid_main"),
            s!("expand_main"),
            s!("mask_main"),
            s!("gaussian_main"),
            s!("down_main"),
            s!("up_main"),
            s!("mix_main"),
            s!("lens_main"),
            s!("pixel_main"),
        ] {
            drop(super::super::super::d3d_shader::compile(source, entry, s!("ps_5_0")).unwrap());
        }
        drop(
            super::super::super::d3d_shader::compile(source, s!("vertex_main"), s!("vs_5_0"))
                .unwrap(),
        );
    });
}

#[test]
#[ignore = "WGC pool retirement without StartCapture; Windows and Release required"]
fn lifecycle_memory_frame_pool_without_capture_sessions() {
    observe_retirement("frame_pool", || retire_frame_pool(false));
}

#[test]
#[ignore = "explicit WinRT device Close after pool Close; Windows and Release required"]
fn lifecycle_memory_frame_pool_with_closed_device() {
    observe_retirement("pool_closed_device", || retire_frame_pool(true));
}

#[test]
#[ignore = "SDK-only WGC reproduction without a Lexift capture backend; Windows and Release required"]
fn lifecycle_memory_sdk_frame_pool_without_backend() {
    assert!(super::super::super::screen_capture::activation::supported().unwrap());
    observe_retirement("sdk_frame_pool", sdk_frame_pool_once);
}

#[test]
#[ignore = "SDK WGC with a fully retired owner thread each round; Windows and Release required"]
fn lifecycle_memory_sdk_frame_pool_on_fresh_threads() {
    assert!(super::super::super::screen_capture::activation::supported().unwrap());
    observe_retirement("sdk_pool_threads", || {
        thread::spawn(sdk_frame_pool_once).join().unwrap();
    });
}

fn sdk_frame_pool_once() {
    use windows::{
        Graphics::{
            Capture::Direct3D11CaptureFramePool,
            DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
            SizeInt32,
        },
        Win32::{
            Foundation::HMODULE,
            Graphics::{Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, Dxgi::IDXGIDevice},
            System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
        },
        core::Interface,
    };
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let mut device = None;
    let mut context = None;
    unsafe {
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
        )
        .unwrap();
    }
    let dxgi: IDXGIDevice = device.unwrap().cast().unwrap();
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .unwrap()
        .cast()
        .unwrap();
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        SizeInt32 {
            Width: 1280,
            Height: 720,
        },
    )
    .unwrap();
    pool.Close().unwrap();
    drop(pool);
    winrt.Close().unwrap();
}

#[test]
#[ignore = "pool factory reuse without retained pixels; Windows and Release required"]
fn lifecycle_memory_frame_pools_reused_factory() {
    use windows::{
        Graphics::{
            Capture::IDirect3D11CaptureFramePoolStatics2, DirectX::Direct3D11::IDirect3DDevice,
        },
        Win32::{
            Graphics::Dxgi::IDXGIDevice,
            System::WinRT::{
                Direct3D11::CreateDirect3D11DeviceFromDXGIDevice, RoGetActivationFactory,
            },
        },
        core::{HSTRING, Interface},
    };
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let factory: IDirect3D11CaptureFramePoolStatics2 = unsafe {
        RoGetActivationFactory(&HSTRING::from(
            "Windows.Graphics.Capture.Direct3D11CaptureFramePool",
        ))
    }
    .unwrap();
    observe_retirement("pool_reused_factory", || {
        let backend = Backend::new().unwrap();
        let dxgi: IDXGIDevice = backend.device().cast().unwrap();
        let device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .unwrap()
            .cast()
            .unwrap();
        let mut raw = std::ptr::null_mut();
        unsafe {
            (factory.vtable().CreateFreeThreaded)(
                factory.as_raw(),
                device.as_raw(),
                windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                windows::Graphics::SizeInt32 {
                    Width: 1280,
                    Height: 720,
                },
                &mut raw,
            )
            .ok()
            .unwrap();
            let pool = windows::Graphics::Capture::Direct3D11CaptureFramePool::from_raw(raw);
            pool.Close().unwrap();
            drop(pool);
        }
    });
}

fn retire_frame_pool(close_device: bool) {
    use windows::{
        Graphics::{
            DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
            SizeInt32,
        },
        Win32::{
            Graphics::Dxgi::IDXGIDevice,
            System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
        },
        core::Interface,
    };
    let backend = Backend::new().unwrap();
    let dxgi: IDXGIDevice = backend.device().cast().unwrap();
    let device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .unwrap()
        .cast()
        .unwrap();
    let pool = super::super::super::screen_capture::activation::create_pool(
        &device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        SizeInt32 {
            Width: 1280,
            Height: 720,
        },
    )
    .unwrap();
    pool.Close().unwrap();
    drop(pool);
    if close_device {
        device.Close().unwrap();
    }
    drop(device);
    drop(dxgi);
    drop(backend);
}

#[test]
#[ignore = "active WGC retirement without effects/presentation; interactive Windows and Release required"]
fn lifecycle_memory_active_capture_without_renderer() {
    // Match the full clear/reopen source dimensions, but omit annotation
    // surfaces, shader/effect work, and the render worker entirely.
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let painter = SourceFrame::new(source.0);
    surface::pump_messages();
    painter.paint(37);
    painter.verify(37);
    drop(bootstrap);
    let token = source.0.0 as isize;
    let request = ScreenCaptureRequest {
        revision: 1,
        regions: vec![bounds],
        paused_regions: Vec::new(),
        excluded_windows: Vec::new(),
    };
    observe_retirement("active_capture", || {
        let mut backend = Backend::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            surface::pump_messages();
            if backend.sample_gpu(&request).unwrap().is_some_and(|frame| {
                frame
                    .layers
                    .iter()
                    .any(|layer| layer.source.is_some_and(|version| version.token == token))
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "owned WGC first frame missing");
            thread::sleep(Duration::from_millis(1));
        }
        println!("active_capture_source_count={}", backend.capture_count());
        drop(backend);
    });
    drop(painter);
    drop(source);
}

#[test]
#[ignore = "retains one native GPU Port across clear/reopen; interactive Windows and Release required"]
fn gpu_clear_reopen_retained_port_releases_scene_and_window_resources() {
    if cfg!(debug_assertions) {
        panic!("run lifecycle verification with --release");
    }
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    let source_frame = SourceFrame::new(source.0);
    source_frame.paint(37);
    source_frame.verify(37);
    drop(bootstrap);
    let port = WindowsAnnotationRenderPort::new().unwrap();
    let mut retired = Vec::new();
    let mut retired_memory = Vec::new();
    println!(
        "retained_port_pid={} initial_private_working_set_bytes={:?}",
        std::process::id(),
        memory_bytes()
    );
    for round in 0..64 {
        let mut requested = scene(bounds, input.0, round % 2);
        requested.stamp.session = 200 + round as u64;
        let stamp = requested.stamp;
        let requested = Arc::new(requested);
        let weak = Arc::downgrade(&requested);
        port.submit(requested).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            surface::pump_messages();
            if port
                .latest()
                .unwrap()
                .is_some_and(|frame| frame.presented && frame.stamp == stamp)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no matching presentation in round {round}"
            );
            thread::sleep(Duration::from_millis(1));
        }
        port.clear();
        // clear wakes the retained worker. Wait for the last worker-local scene
        // reference to retire, rather than assuming destruction is synchronous.
        let deadline = Instant::now() + Duration::from_secs(5);
        while weak.upgrade().is_some() {
            surface::pump_messages();
            assert!(
                Instant::now() < deadline,
                "clear retained scene round {round}"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            port.latest().unwrap().is_none(),
            "old completion survived clear"
        );
        let settled = Instant::now() + Duration::from_millis(250);
        while Instant::now() < settled {
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        retired.push(counts());
        let memory = memory_bytes();
        retired_memory.push(memory);
        println!("retained_port_round={round} private_working_set_bytes={memory:?}");
        if round == 0 || round % 16 == 15 {
            memory_probe::report("retained_port", round);
        }
    }
    println!("retained_port_clear_handle_gdi_user_counts={retired:?}");
    println!("retained_port_clear_private_working_set_bytes={retired_memory:?}");
    let baseline = retired[2];
    for &(handles, gdi, user) in &retired[3..] {
        assert!(
            handles <= baseline.0 + 4,
            "handles grow across retained-Port clear"
        );
        assert!(gdi <= baseline.1, "GDI resources survive clear");
        assert!(user <= baseline.2, "native window resources survive clear");
    }
    drop(port);
    let owned = counts();
    let owned_memory = memory_bytes();
    drop(source_frame);
    drop(input);
    drop(source);
    surface::pump_messages();
    let released = counts();
    println!("retained_port_fixture_release_before={owned:?} after={released:?}");
    println!(
        "retained_port_fixture_release_private_working_set_bytes_before={owned_memory:?} after={:?}",
        memory_bytes()
    );
    assert!(released.1 < owned.1, "fixture bitmap/DC not released");
    assert!(released.2 < owned.2, "fixture HWND not released");
}
