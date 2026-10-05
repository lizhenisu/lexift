//! Nonblocking D3D11 query reads shared by completion and test timestamp queries.
use std::mem::size_of;
use windows::{
    Win32::{Foundation::S_FALSE, Graphics::Direct3D11::*},
    core::{Interface, Result},
};

/// The generated Result wrapper treats S_FALSE as success; use the HRESULT to
/// distinguish unavailable metadata from a completed timestamp with value zero.
pub(super) fn data<T: Default>(
    context: &ID3D11DeviceContext,
    query: &ID3D11Query,
) -> Result<Option<T>> {
    let mut value = T::default();
    let result = unsafe {
        (context.vtable().GetData)(
            context.as_raw(),
            query.as_raw(),
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
            D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
        )
    };
    if result == S_FALSE {
        return Ok(None);
    }
    result.ok()?;
    Ok(Some(value))
}
