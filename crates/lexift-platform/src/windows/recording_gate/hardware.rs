//! Enumerate native hardware MFT capabilities without treating activation as support.
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::CoTaskMemFree},
    core::*,
};
static FACTORY_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn factory_calls() -> usize {
    FACTORY_CALLS.load(std::sync::atomic::Ordering::Acquire)
}

fn enumerate(flags: MFT_ENUM_FLAG) -> Result<Vec<IMFActivate>> {
    unsafe {
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut list = std::ptr::null_mut();
        let mut count = 0;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&output),
            &mut list,
            &mut count,
        )?;
        let result = if count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts_mut(list, count as usize)
                .iter_mut()
                .filter_map(Option::take)
                .collect()
        };
        CoTaskMemFree(Some(list.cast()));
        Ok(result)
    }
}

pub struct Selection {
    control: IMFPluginControl,
    prior: Vec<(GUID, bool)>,
    factory: Option<windows::Win32::System::Com::IClassFactory>,
    activation: Option<IMFActivate>,
}
impl Selection {
    pub fn from_environment() -> Result<Option<Self>> {
        let Some(value) = std::env::var_os("LEXIFT_RECORDING_GATE_ENCODER") else {
            return Ok(None);
        };
        let text = value.to_string_lossy().replace(['-', '{', '}'], "");
        let selected =
            GUID::from_u128(u128::from_str_radix(&text, 16).map_err(|e| {
                Error::new(windows::Win32::Foundation::E_INVALIDARG, e.to_string())
            })?);
        unsafe {
            let control = MFGetPluginControl()?;
            let mut guard = Self {
                control,
                prior: Vec::new(),
                factory: None,
                activation: None,
            };
            let output = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            let ordinal = std::env::var("LEXIFT_RECORDING_GATE_DEVICE_INDEX")
                .unwrap_or_else(|_| "0".into())
                .parse::<usize>()
                .map_err(|e| Error::new(windows::Win32::Foundation::E_INVALIDARG, e.to_string()))?;
            let mut seen = 0;
            for item in enumerate(MFT_ENUM_FLAG_ALL)? {
                let matches = item.GetGUID(&MFT_TRANSFORM_CLSID_Attribute).ok() == Some(selected);
                let choose = matches && seen == ordinal;
                if matches {
                    seen += 1;
                }
                if choose && guard.factory.is_none() {
                    let factory: windows::Win32::System::Com::IClassFactory = Factory {
                        activation: item.clone(),
                    }
                    .into();
                    let input = MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: MFVideoFormat_NV12,
                    };
                    MFTRegisterLocal(
                        &factory,
                        &MFT_CATEGORY_VIDEO_ENCODER,
                        w!("Lexift gate explicit hardware encoder"),
                        MFT_ENUM_FLAG_ASYNCMFT.0 as u32,
                        Some(&[input]),
                        Some(&[output]),
                    )?;
                    guard.factory = Some(factory);
                    guard.activation = Some(item.clone());
                }
                if let Ok(clsid) = item.GetGUID(&MFT_TRANSFORM_CLSID_Attribute)
                    && !guard.prior.iter().any(|(id, _)| *id == clsid)
                {
                    let disabled = guard
                        .control
                        .IsDisabled(MF_Plugin_Type_MFT.0 as u32, &clsid)
                        .is_ok();
                    guard.prior.push((clsid, disabled));
                    guard.control.SetDisabled(
                        MF_Plugin_Type_MFT.0 as u32,
                        &clsid,
                        clsid != selected,
                    )?;
                }
            }
            if guard.factory.is_none() {
                return Err(Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Selected encoder was not enumerated",
                ));
            }
            Ok(Some(guard))
        }
    }
}
impl Drop for Selection {
    fn drop(&mut self) {
        unsafe {
            if let Some(factory) = &self.factory {
                let _ = MFTUnregisterLocal(factory);
            }
            // ActivateObject caches the MFT. Releasing the factory alone does
            // not perform its shutdown; retire it only after the writer drops.
            if let Some(activation) = &self.activation {
                // Some activators do not forward shutdown to the asynchronous
                // MFT. Its event queue must be shut down explicitly as well.
                if let Ok(transform) = activation.ActivateObject::<IMFTransform>()
                    && let Ok(shutdown) = transform.cast::<IMFShutdown>()
                {
                    let _ = super::encoder::stage("shutdown_hardware_transform", || {
                        shutdown.Shutdown()
                    });
                }
                let _ =
                    super::encoder::stage("shutdown_activation", || activation.ShutdownObject());
            }
            for (id, disabled) in &self.prior {
                let _ = self
                    .control
                    .SetDisabled(MF_Plugin_Type_MFT.0 as u32, id, *disabled);
            }
        }
    }
}

#[implement(windows::Win32::System::Com::IClassFactory)]
struct Factory {
    activation: IMFActivate,
}
#[allow(non_snake_case)]
impl windows::Win32::System::Com::IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        out: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        if !outer.is_null() {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::CLASS_E_NOAGGREGATION,
            ));
        }
        unsafe {
            FACTORY_CALLS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let t = self.activation.ActivateObject::<IMFTransform>()?;
            t.query(iid, out).ok()
        }
    }
    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

#[test]
#[ignore = "requires native MF hardware and LEXIFT_RECORDING_GATE_DIR"]
fn inventory() {
    let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
    let _media = super::encoder::Media::new().unwrap();
    let mut entries = Vec::new();
    unsafe {
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut list = std::ptr::null_mut();
        let mut count = 0;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &mut list,
            &mut count,
        )
        .unwrap();
        if count > 0 {
            for entry in std::slice::from_raw_parts_mut(list, count as usize) {
                let a = entry.take().unwrap();
                let mut name = [0u16; 512];
                let _ = a.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut name, None);
                let name = String::from_utf16_lossy(
                    &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())],
                );
                let clsid = a.GetGUID(&MFT_TRANSFORM_CLSID_Attribute).ok();
                let activated = a.ActivateObject::<IMFTransform>();
                let mut inputs = Vec::new();
                let mut colors = Vec::new();
                let error = match activated {
                    Ok(t) => {
                        for index in 0..128 {
                            match t.GetInputAvailableType(0, index) {
                                Ok(mt) => {
                                    inputs.push(format!("{:?}", mt.GetGUID(&MF_MT_SUBTYPE).ok()))
                                }
                                Err(_) => break,
                            }
                        }
                        if let Ok(codec) = t.cast::<ICodecAPI>() {
                            for key in [
                                CODECAPI_AVEncVideoOutputColorPrimaries,
                                CODECAPI_AVEncVideoOutputColorTransferMatrix,
                                CODECAPI_AVEncVideoOutputColorTransferFunction,
                            ] {
                                colors.push(serde_json::json!({"key":format!("{key:?}"),"support":codec.IsSupported(&key).is_ok()}));
                            }
                        }
                        None
                    }
                    Err(e) => Some(e.to_string()),
                };
                let _ = a.ShutdownObject();
                entries.push(serde_json::json!({"name":name,"clsid":format!("{clsid:?}"),"input_subtypes":inputs,"color_properties":colors,"activation_error":error}));
            }
            CoTaskMemFree(Some(list.cast()));
        }
    }
    let folder = std::path::PathBuf::from(std::env::var_os("LEXIFT_RECORDING_GATE_DIR").unwrap());
    std::fs::write(
        folder.join("hardware-inventory.json"),
        serde_json::to_vec_pretty(&entries).unwrap(),
    )
    .unwrap();
}
