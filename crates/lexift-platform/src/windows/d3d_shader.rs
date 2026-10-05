//! Shared runtime shader compilation for device-local annotation and capture work.
use windows::{
    Win32::Graphics::Direct3D::{Fxc::*, ID3DBlob},
    core::{PCSTR, Result, s},
};
pub(super) fn bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

pub(super) fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let (mut code, mut diagnostics) = (None, None);
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("Lexift.Annotation"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut diagnostics),
        )
    };
    if let Err(error) = result {
        let message = diagnostics
            .as_ref()
            .map(|blob| String::from_utf8_lossy(bytes(blob)).into_owned())
            .unwrap_or_else(|| error.to_string());
        return Err(windows::core::Error::new(error.code(), message));
    }
    Ok(code.unwrap())
}
