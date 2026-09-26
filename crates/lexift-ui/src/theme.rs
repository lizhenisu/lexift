//! Applies the persisted preference to independently owned Slint component globals.
use lexift_core::domain::settings::ThemePreference;
use slint::ComponentHandle;
use std::cell::Cell;

thread_local! { static CURRENT: Cell<ThemePreference> = const { Cell::new(ThemePreference::Light) }; }

pub(crate) fn index(theme: ThemePreference) -> i32 {
    match theme {
        ThemePreference::Light => 0,
        ThemePreference::Dark => 1,
        ThemePreference::System => 2,
    }
}
pub(crate) fn from_index(index: i32) -> ThemePreference {
    match index {
        1 => ThemePreference::Dark,
        2 => ThemePreference::System,
        _ => ThemePreference::Light,
    }
}
pub(crate) fn set_current(theme: ThemePreference) {
    CURRENT.set(theme);
}
pub(crate) fn apply<T: ComponentHandle>(window: &T)
where
    for<'a> crate::Colors<'a>: slint::Global<'a, T>,
{
    window
        .global::<crate::Colors>()
        .set_preference(index(CURRENT.get()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::rc::Rc;

    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }

    #[test]
    fn system_theme_updates_all_component_globals_and_explicit_modes_override_it() {
        use i_slint_core::items::ColorScheme;
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let popup = crate::TranslationPopup::new().unwrap();
        let menu = crate::PopupLanguageMenuWindow::new().unwrap();
        let selection = crate::SelectionToolbarWindow::new().unwrap();
        let toolbar = crate::AnnotationToolbar::new().unwrap();
        let panel = crate::AnnotationPanel::new().unwrap();
        let main = crate::AppWindow::new().unwrap();
        popup.set_source_text("unsaved input".into());
        let set_system = |scheme| {
            i_slint_core::context::with_global_context(
                || unreachable!(),
                |context| context.set_color_scheme(scheme),
            )
            .unwrap();
        };
        macro_rules! check {
            ($dark:expr) => {
                assert_eq!(settings.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(popup.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(menu.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(selection.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(toolbar.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(panel.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(main.global::<crate::Colors>().get_dark(), $dark);
                assert_eq!(
                    settings.get_resize_fallback_color(),
                    settings.global::<crate::Colors>().get_background()
                );
                assert_eq!(
                    popup.get_resize_fallback_color(),
                    popup.global::<crate::Colors>().get_surface()
                );
            };
        }
        for preference in [
            ThemePreference::System,
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
        ] {
            set_current(preference);
            apply(&settings);
            apply(&popup);
            apply(&menu);
            apply(&selection);
            apply(&toolbar);
            apply(&panel);
            apply(&main);
            for scheme in [ColorScheme::Unknown, ColorScheme::Dark, ColorScheme::Light] {
                set_system(scheme);
                check!(
                    preference == ThemePreference::Dark
                        || (preference == ThemePreference::System && scheme == ColorScheme::Dark)
                );
            }
        }
        assert_eq!(popup.get_source_text().as_str(), "unsaved input");
        let weak = settings.as_weak();
        drop(settings);
        assert!(weak.upgrade().is_none());
        set_current(ThemePreference::Dark);
        let reopened = crate::SettingsWindow::new().unwrap();
        apply(&reopened);
        assert!(reopened.global::<crate::Colors>().get_dark());
    }
}
