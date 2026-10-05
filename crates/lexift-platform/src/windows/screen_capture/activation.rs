//! Apartment-scoped WGC activation. Generated static factory caches can outlive
//! RoUninitialize and point into an unloaded GraphicsCapture DLL after reopening.
use super::capture::Apartment;
use windows::{
    Graphics::{
        Capture::{
            Direct3D11CaptureFramePool, GraphicsCaptureSession,
            IDirect3D11CaptureFramePoolStatics2, IGraphicsCaptureSessionStatics,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::System::WinRT::RoGetActivationFactory,
    core::{HSTRING, Interface, Result, RuntimeName},
};

pub(crate) fn supported() -> Result<bool> {
    super::runtime::keep_code_loaded()?;
    let _apartment = Apartment::for_query()?;
    unsafe {
        let factory: IGraphicsCaptureSessionStatics =
            RoGetActivationFactory(&HSTRING::from(GraphicsCaptureSession::NAME))?;
        let mut supported = false;
        (factory.vtable().IsSupported)(factory.as_raw(), &mut supported).map(|| supported)
    }
}

/// The caller's capture backend owns an active apartment. Keep its factory local
/// to this call and use the published interface ABI, without a process-wide cache.
pub(crate) fn create_pool(
    device: &IDirect3DDevice,
    format: DirectXPixelFormat,
    buffers: i32,
    size: SizeInt32,
) -> Result<Direct3D11CaptureFramePool> {
    super::runtime::keep_code_loaded()?;
    unsafe {
        let factory: IDirect3D11CaptureFramePoolStatics2 =
            RoGetActivationFactory(&HSTRING::from(Direct3D11CaptureFramePool::NAME))?;
        let mut pool = core::ptr::null_mut();
        (factory.vtable().CreateFreeThreaded)(
            factory.as_raw(),
            device.as_raw(),
            format,
            buffers,
            size,
            &mut pool,
        )
        .ok()?;
        windows::core::Type::from_abi(pool)
    }
}

#[test]
fn capability_query_borrows_and_preserves_existing_sta() {
    std::thread::spawn(|| {
        use windows::Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize},
        };
        struct Sta;
        impl Drop for Sta {
            fn drop(&mut self) {
                unsafe {
                    RoUninitialize();
                }
            }
        }
        unsafe {
            RoInitialize(RO_INIT_SINGLETHREADED).unwrap();
        }
        let _sta = Sta;
        assert!(Apartment::for_query().unwrap().is_none());
        supported().unwrap();
        assert_eq!(Apartment::new().err().unwrap().code(), RPC_E_CHANGED_MODE);
    })
    .join()
    .unwrap();
}

#[test]
#[ignore = "repeated fresh WGC activation requires a Windows D3D11 hardware device"]
fn pool_and_support_factories_survive_full_apartment_retirement() {
    use windows::Win32::{
        Graphics::Dxgi::IDXGIDevice,
        System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
    };
    for _ in 0..8 {
        assert!(supported().unwrap());
        let backend = super::capture::Backend::new().unwrap();
        let dxgi: IDXGIDevice = backend.device().cast().unwrap();
        let device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .unwrap()
            .cast()
            .unwrap();
        let pool = create_pool(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            SizeInt32 {
                Width: 32,
                Height: 32,
            },
        )
        .unwrap();
        pool.Close().unwrap();
        drop(pool);
        drop(device);
        drop(dxgi);
        drop(backend);
    }
}
