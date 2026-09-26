//! Per-open native menu rendering. Windows keeps tracking, focus and keyboard navigation.
use std::{cell::RefCell, mem::size_of};
use windows::{
    Win32::{
        Foundation::{COLORREF, LPARAM, POINT},
        Graphics::Gdi::*,
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        UI::{
            Accessibility::{MSAA_MENU_SIG, MSAAMENUINFO},
            Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DEFAULT, ODS_SELECTED, ODT_MENU},
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::*,
        },
    },
    core::{PWSTR, w},
};

thread_local! { static STYLE: RefCell<Option<Style>> = const { RefCell::new(None) }; }

struct Style {
    dark: bool,
    dpi: u32,
    background: HBRUSH,
    hover: HBRUSH,
    font: HFONT,
    bold: HFONT,
    // Boxed so Win32's itemData pointers remain stable even if Style moves.
    names: Box<[MSAAMENUINFO; 3]>,
    _text: [Vec<u16>; 3],
    width: u32,
}
impl Drop for Style {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.background.into());
            let _ = DeleteObject(self.hover.into());
            let _ = DeleteObject(self.font.into());
            let _ = DeleteObject(self.bold.into());
        }
    }
}
fn rgb(hex: u32) -> COLORREF {
    COLORREF(((hex >> 16) & 255) | (hex & 0xff00) | ((hex & 255) << 16))
}
fn pixels(value: u32, dpi: u32) -> u32 {
    (value * dpi + 48) / 96
}
fn resolve_dark(preference: u8, system_light: Option<u32>) -> bool {
    preference == 1 || (preference == 2 && system_light == Some(0))
}
fn system_light() -> Option<u32> {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
        .then_some(value)
    }
}

pub(super) struct MenuAppearance;
impl MenuAppearance {
    pub(super) fn begin(preference: u8, labels: &lexift_core::ports::tray::TrayMenuLabels) -> Self {
        let dark = resolve_dark(
            preference,
            if preference == 2 {
                system_light()
            } else {
                None
            },
        );
        let mut cursor = POINT::default();
        let mut dpi = 96;
        let mut dpi_y = 96;
        unsafe {
            if GetCursorPos(&mut cursor).is_ok() {
                let _ = GetDpiForMonitor(
                    MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
                    MDT_EFFECTIVE_DPI,
                    &mut dpi,
                    &mut dpi_y,
                );
            }
        }
        let letters = format!("{}{}{}", labels.open, labels.settings, labels.quit);
        let face = if letters
            .chars()
            .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))
        {
            w!("Yu Gothic UI")
        } else if letters
            .chars()
            .any(|c| ('\u{ac00}'..='\u{d7af}').contains(&c))
        {
            w!("Malgun Gothic")
        } else if letters
            .chars()
            .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
        {
            w!("Microsoft YaHei UI")
        } else {
            w!("Segoe UI")
        };
        let font = |weight| unsafe {
            CreateFontW(
                -(pixels(14, dpi) as i32),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                face,
            )
        };
        let text = [&labels.open, &labels.settings, &labels.quit]
            .map(|s| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
        let names = std::array::from_fn(|i| MSAAMENUINFO {
            dwMSAASignature: MSAA_MENU_SIG as u32,
            cchWText: text[i].len() as u32 - 1,
            pszWText: PWSTR(text[i].as_ptr().cast_mut()),
        });
        let font = font(400);
        let bold = unsafe {
            let mut description = LOGFONTW::default();
            GetObjectW(
                font.into(),
                size_of::<LOGFONTW>() as i32,
                Some((&mut description as *mut LOGFONTW).cast()),
            );
            description.lfWeight = 600;
            CreateFontIndirectW(&description)
        };
        // Use the same fonts and monitor DPI as drawing, including the bold default item.
        let width = unsafe {
            let dc = GetDC(None);
            let previous = SelectObject(dc, bold.into());
            let width = text
                .iter()
                .map(|text| {
                    let mut size = windows::Win32::Foundation::SIZE::default();
                    let _ = GetTextExtentPoint32W(dc, &text[..text.len() - 1], &mut size);
                    size.cx.max(0) as u32
                })
                .max()
                .unwrap_or(0)
                + pixels(48, dpi);
            SelectObject(dc, previous);
            ReleaseDC(None, dc);
            width.max(pixels(176, dpi))
        };
        STYLE.with(|slot| {
            *slot.borrow_mut() = Some(Style {
                dark,
                dpi,
                background: unsafe {
                    CreateSolidBrush(rgb(if dark { 0x24272c } else { 0xffffff }))
                },
                hover: unsafe { CreateSolidBrush(rgb(if dark { 0x293f60 } else { 0xe8f0fe })) },
                font,
                bold,
                width,
                _text: text,
                names: Box::new(names),
            })
        });
        Self
    }

    pub(super) fn configure(&self, menu: HMENU) {
        STYLE.with(|slot| {
            let slot = slot.borrow();
            let Some(style) = slot.as_ref() else { return };
            unsafe {
                let info = MENUINFO {
                    cbSize: size_of::<MENUINFO>() as u32,
                    fMask: MIM_BACKGROUND,
                    hbrBack: style.background,
                    ..Default::default()
                };
                let _ = SetMenuInfo(menu, &info);
                for (index, name) in style.names.iter().enumerate() {
                    let item = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_FTYPE | MIIM_DATA,
                        fType: MFT_OWNERDRAW,
                        dwItemData: name as *const MSAAMENUINFO as usize,
                        ..Default::default()
                    };
                    let _ = SetMenuItemInfoW(menu, index as u32 + 1, false, &item);
                }
                let separator = MENUITEMINFOW {
                    cbSize: size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE,
                    fType: MFT_SEPARATOR | MFT_OWNERDRAW,
                    ..Default::default()
                };
                let _ = SetMenuItemInfoW(menu, 2, true, &separator);
            }
        });
    }
}
impl Drop for MenuAppearance {
    fn drop(&mut self) {
        STYLE.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

/// Match the displayed label's initial; cycle duplicate initials without executing.
pub(super) fn menu_character(
    key: u16,
    menu: LPARAM,
) -> Option<windows::Win32::Foundation::LRESULT> {
    let key = char::from_u32(key as u32)?.to_lowercase().to_string();
    STYLE.with(|slot| {
        let slot = slot.borrow();
        let style = slot.as_ref()?;
        let matches: Vec<_> = style
            ._text
            .iter()
            .enumerate()
            .filter_map(|(i, text)| {
                let initial = String::from_utf16_lossy(text)
                    .chars()
                    .next()?
                    .to_lowercase()
                    .to_string();
                (initial == key).then_some(if i == 2 { 3 } else { i as u32 })
            })
            .collect();
        let mut chosen = *matches.first()?;
        if matches.len() > 1 {
            for (i, position) in matches.iter().enumerate() {
                let flags =
                    unsafe { GetMenuState(HMENU(menu.0 as *mut _), *position, MF_BYPOSITION) };
                if flags & MF_HILITE.0 != 0 {
                    chosen = matches[(i + 1) % matches.len()];
                    break;
                }
            }
        }
        let action = if matches.len() == 1 {
            MNC_EXECUTE
        } else {
            MNC_SELECT
        };
        Some(windows::Win32::Foundation::LRESULT(
            ((action as isize) << 16) | chosen as isize,
        ))
    })
}

/// Only dereference Win32's draw structures for the messages and menu we own.
pub(super) fn handle_draw_message(message: u32, lparam: LPARAM) -> bool {
    if !matches!(message, WM_MEASUREITEM | WM_DRAWITEM) || lparam.0 == 0 {
        return false;
    }
    STYLE.with(|slot| {
        let slot = slot.borrow();
        let Some(style) = slot.as_ref() else {
            return false;
        };
        unsafe {
            if message == WM_MEASUREITEM {
                let item = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return false;
                }
                item.itemWidth = style.width;
                item.itemHeight = pixels(if item.itemID == 0 { 9 } else { 34 }, style.dpi);
            } else {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return false;
                }
                let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
                FillRect(
                    item.hDC,
                    &item.rcItem,
                    if selected {
                        style.hover
                    } else {
                        style.background
                    },
                );
                let mut rect = item.rcItem;
                rect.left += pixels(16, style.dpi) as i32;
                rect.right -= pixels(16, style.dpi) as i32;
                if item.itemID == 0 {
                    rect.top = (rect.top + rect.bottom) / 2;
                    rect.bottom = rect.top + 1;
                    let brush = CreateSolidBrush(rgb(if style.dark { 0x444a54 } else { 0xdadce0 }));
                    FillRect(item.hDC, &rect, brush);
                    let _ = DeleteObject(brush.into());
                } else if let Some(name) = item
                    .itemID
                    .checked_sub(1)
                    .and_then(|index| style.names.get(index as usize))
                {
                    let saved = SaveDC(item.hDC);
                    SelectObject(
                        item.hDC,
                        if item.itemState.0 & ODS_DEFAULT.0 != 0 {
                            style.bold
                        } else {
                            style.font
                        }
                        .into(),
                    );
                    SetBkMode(item.hDC, TRANSPARENT);
                    SetTextColor(item.hDC, rgb(if style.dark { 0xe8eaed } else { 0x202124 }));
                    let mut text =
                        std::slice::from_raw_parts(name.pszWText.as_ptr(), name.cchWText as usize)
                            .to_vec();
                    DrawTextW(
                        item.hDC,
                        &mut text,
                        &mut rect,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    let _ = RestoreDC(item.hDC, saved);
                }
            }
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_modes_override_system_and_unknown_defaults_light() {
        for system in [None, Some(0), Some(1)] {
            assert!(!resolve_dark(0, system));
            assert!(resolve_dark(1, system));
            assert_eq!(resolve_dark(2, system), system == Some(0));
        }
    }
    #[test]
    fn native_menu_exposes_accessible_labels_and_cleans_up() {
        let appearance = MenuAppearance::begin(1, &Default::default());
        let menu = unsafe { CreatePopupMenu().unwrap() };
        unsafe {
            AppendMenuW(menu, MF_STRING, 1, w!("Open Lexift")).unwrap();
            AppendMenuW(menu, MF_STRING, 2, w!("Settings")).unwrap();
            AppendMenuW(menu, MF_SEPARATOR, 0, None).unwrap();
            AppendMenuW(menu, MF_STRING, 3, w!("Quit")).unwrap();
        }
        appearance.configure(menu);
        for (id, expected) in [(1, "Open Lexift"), (2, "Settings"), (3, "Quit")] {
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_DATA | MIIM_FTYPE,
                ..Default::default()
            };
            unsafe {
                GetMenuItemInfoW(menu, id, false, &mut item).unwrap();
                assert_ne!(item.fType.0 & MFT_OWNERDRAW.0, 0);
                let name = &*(item.dwItemData as *const MSAAMENUINFO);
                assert_eq!(name.dwMSAASignature, MSAA_MENU_SIG as u32);
                assert_eq!(
                    String::from_utf16_lossy(std::slice::from_raw_parts(
                        name.pszWText.as_ptr(),
                        name.cchWText as usize
                    )),
                    expected
                );
            }
        }
        unsafe {
            DestroyMenu(menu).unwrap();
        }
        drop(appearance);
        STYLE.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn localized_labels_are_owned_and_long_labels_expand_the_menu() {
        use lexift_core::ports::tray::TrayMenuLabels;
        let labels = TrayMenuLabels {
            open: "打开 Lexift".into(),
            settings: "Paramètres et préférences de l’application".into(),
            quit: "退出".into(),
        };
        let appearance = MenuAppearance::begin(0, &labels);
        drop(labels);
        STYLE.with(|slot| {
            let slot = slot.borrow();
            let style = slot.as_ref().unwrap();
            assert!(style.width > pixels(176, style.dpi));
            let name = &style.names[0];
            let text = unsafe {
                std::slice::from_raw_parts(name.pszWText.as_ptr(), name.cchWText as usize)
            };
            assert_eq!(String::from_utf16_lossy(text), "打开 Lexift");
        });
        drop(appearance);
    }

    #[test]
    fn menu_dimensions_scale_with_monitor_dpi() {
        assert_eq!(pixels(34, 96), 34);
        assert_eq!(pixels(34, 144), 51);
        assert_eq!(pixels(176, 192), 352);
    }
}
