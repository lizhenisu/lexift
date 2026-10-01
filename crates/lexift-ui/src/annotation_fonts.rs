//! Installed-font defaults and per-tool preferences; object fonts remain independent.
use super::*;
use lexift_core::domain::ui_language::UiLanguage;
use std::collections::HashMap;

type Aliases = HashMap<String, Vec<(String, fontdb::Language)>>;

#[derive(Default)]
pub(super) struct Preferences {
    manual: [Option<String>; 3],
    aliases: Aliases,
}

fn name_language(language: UiLanguage) -> fontdb::Language {
    use fontdb::Language as L;
    match language {
        UiLanguage::SimplifiedChinese => L::Chinese_PeoplesRepublicOfChina,
        UiLanguage::TraditionalChinese => L::Chinese_Taiwan,
        UiLanguage::EnglishUs => L::English_UnitedStates,
        UiLanguage::EnglishUk => L::English_UnitedKingdom,
        UiLanguage::Japanese => L::Japanese_Japan,
        UiLanguage::Korean => L::Korean_Korea,
        UiLanguage::German => L::German_Germany,
        UiLanguage::French => L::French_France,
        UiLanguage::Spanish => L::Spanish_ModernSort_Spain,
        UiLanguage::Italian => L::Italian_Italy,
        UiLanguage::PortuguesePortugal => L::Portuguese_Portugal,
        UiLanguage::PortugueseBrazil => L::Portuguese_Brazil,
    }
}

/// Index aliases once, merging regular/bold faces without changing the sorted menu.
fn alias_index(families: Vec<Vec<(String, fontdb::Language)>>) -> Aliases {
    let mut result = Aliases::new();
    for family in families {
        for (name, _) in &family {
            let aliases = result.entry(name.to_lowercase()).or_default();
            for alias in &family {
                if !aliases.contains(alias) {
                    aliases.push(alias.clone());
                }
            }
            aliases.sort_by_cached_key(|(name, _)| (name.to_lowercase(), name.clone()));
        }
    }
    result
}

fn local_name(names: &[String], aliases: &Aliases, name: String, language: UiLanguage) -> String {
    aliases
        .get(&name.to_lowercase())
        .into_iter()
        .flatten()
        .filter(|(_, lang)| *lang == name_language(language))
        .find_map(|(alias, _)| {
            names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(alias))
                .cloned()
        })
        .unwrap_or(name)
}

fn candidates(language: UiLanguage) -> &'static [&'static str] {
    match language {
        UiLanguage::SimplifiedChinese => &["Microsoft YaHei", "Noto Sans CJK SC"],
        UiLanguage::TraditionalChinese => &["Microsoft JhengHei", "Noto Sans CJK TC"],
        UiLanguage::Japanese => &["Yu Gothic", "Meiryo", "Noto Sans CJK JP"],
        UiLanguage::Korean => &["Malgun Gothic", "Noto Sans CJK KR"],
        _ => &["Segoe UI", "Noto Sans", "Arial"],
    }
}

fn resolve(
    names: &[String],
    aliases: &Aliases,
    language: UiLanguage,
    sans: Option<&str>,
) -> String {
    let name = candidates(language)
        .iter()
        .copied()
        .chain(sans)
        .find_map(|candidate| {
            names
                .iter()
                .find(|name| name.eq_ignore_ascii_case(candidate))
                .cloned()
        })
        .or_else(|| names.first().cloned())
        .unwrap_or_default();
    local_name(names, aliases, name, language)
}

pub(super) fn preferred(r: &Registry, tool: usize) -> String {
    r.font_preferences.manual[tool - 8]
        .clone()
        .unwrap_or_else(|| {
            resolve(
                &r.fonts,
                &r.font_preferences.aliases,
                crate::i18n::language(),
                crate::annotation_text::system_sans_family().as_deref(),
            )
        })
}

pub(super) fn ensure(r: &mut Registry) {
    if r.fonts.is_empty() {
        r.fonts = crate::annotation_text::font_families();
        r.font_preferences.aliases = alias_index(crate::annotation_text::font_family_aliases());
        reset_defaults(r);
    }
}

/// Refresh only idle tool defaults; selected objects and active drafts keep their actual fonts.
pub(super) fn reset_defaults(r: &mut Registry) {
    for tool in 8..=10 {
        let selected = r
            .session
            .selected
            .and_then(|i| r.session.objects.get(i))
            .is_some_and(|o| {
                (tool == 8 && o.kind == Kind::Text) || (tool == 10 && o.kind == Kind::Sequence)
            });
        let editing = r.text_edit.as_ref().is_some_and(|e| {
            if tool == 8 {
                !editing_sequence(r)
                    && e.index.is_none_or(|i| {
                        r.session
                            .objects
                            .get(i)
                            .is_some_and(|o| o.kind == Kind::Text)
                    })
            } else {
                tool == 10 && editing_sequence(r)
            }
        });
        if selected || editing || (tool == 9 && r.session.watermark.is_some()) {
            continue;
        }
        let name = preferred(r, tool);
        if let Some(index) = r.fonts.iter().position(|f| f.eq_ignore_ascii_case(&name)) {
            r.values[tool].font = index as i32;
        }
    }
}

pub(super) fn choose(r: &mut Registry, tool: usize, index: i32) {
    if (8..=10).contains(&tool)
        && let Some(name) = r.fonts.get(index.max(0) as usize)
    {
        r.font_preferences.manual[tool - 8] = Some(name.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_changes_preserve_manual_preferences_and_existing_objects() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.fonts = [
                "Arial",
                "Malgun Gothic",
                "Microsoft JhengHei",
                "Microsoft YaHei",
                "Segoe UI",
                "Yu Gothic",
            ]
            .map(String::from)
            .to_vec();
            for language in UiLanguage::ALL {
                crate::i18n::select(language);
                reset_defaults(r);
                for tool in 8..=10 {
                    assert_eq!(chosen_font(r, tool), candidates(language)[0]);
                }
            }
            r.fonts.push("微软雅黑".into());
            r.font_preferences.aliases = alias_index(vec![vec![
                (
                    "Microsoft YaHei".into(),
                    fontdb::Language::English_UnitedStates,
                ),
                (
                    "微软雅黑".into(),
                    fontdb::Language::Chinese_PeoplesRepublicOfChina,
                ),
            ]]);
            crate::i18n::select(UiLanguage::SimplifiedChinese);
            reset_defaults(r);
            for tool in 8..=10 {
                assert_eq!(chosen_font(r, tool), "微软雅黑");
            }
            choose(r, 8, 3);
            reset_defaults(r);
            assert_eq!(chosen_font(r, 8), "Microsoft YaHei");
            assert_eq!(chosen_font(r, 9), "微软雅黑");
            choose(r, 8, 0);
            crate::i18n::select(UiLanguage::Japanese);
            reset_defaults(r);
            assert_eq!(chosen_font(r, 8), "Arial");
            assert_eq!(chosen_font(r, 9), "Yu Gothic");
            let mut object = crate::annotation_sequence::tests::badge(1.25);
            if let Extra::Sequence(s) = &mut object.extra {
                s.caption.font_family = "Arial".into();
            }
            crate::annotation_sequence::layout(&mut object);
            assert!(r.session.add(object));
            r.pending_displays = vec![Rect {
                left: -400,
                top: -300,
                right: 600,
                bottom: 500,
            }];
            create_canvas(r, 0);
            adopt_selected_style(r, 0);
            assert!(r.font_preferences.manual[2].is_none());
            crate::i18n::select(UiLanguage::Korean);
            reset_defaults(r);
            assert_eq!(chosen_font(r, 10), "Arial");
            assert_eq!(preferred(r, 10), "Malgun Gothic");
            let new = sequence_from_values(r, (0., 0.), 1.25);
            let Extra::Sequence(s) = new.extra else {
                unreachable!()
            };
            assert_eq!(s.caption.font_family, "Malgun Gothic");
            let Extra::Sequence(existing) = &r.session.objects[0].extra else {
                unreachable!()
            };
            assert_eq!(existing.caption.font_family, "Arial");
            r.close();
            assert_eq!(preferred(r, 8), "Arial");
        });
        shutdown();
    }

    #[test]
    fn all_languages_choose_installed_defaults_and_respect_candidate_priority() {
        let names = [
            "Arial",
            "Microsoft YaHei",
            "Microsoft JhengHei",
            "Yu Gothic",
            "Meiryo",
            "Malgun Gothic",
            "Segoe UI",
        ]
        .map(String::from);
        for language in UiLanguage::ALL {
            assert_eq!(
                resolve(&names, &Aliases::new(), language, Some("Arial")),
                candidates(language)[0]
            );
            let remaining = names
                .iter()
                .filter(|n| n.as_str() != candidates(language)[0])
                .cloned()
                .collect::<Vec<_>>();
            let resolved = resolve(&remaining, &Aliases::new(), language, Some("Arial"));
            assert!(remaining.contains(&resolved));
        }
        assert_eq!(
            resolve(
                &["Generic".into()],
                &Aliases::new(),
                UiLanguage::Japanese,
                Some("Generic")
            ),
            "Generic"
        );
        assert_eq!(resolve(&[], &Aliases::new(), UiLanguage::Korean, None), "");
    }

    #[test]
    fn localized_aliases_respect_all_languages_catalog_and_fallback() {
        for language in UiLanguage::ALL {
            let candidate = candidates(language)[0].to_owned();
            let localized = if language == UiLanguage::EnglishUs {
                candidate.clone()
            } else {
                format!("local-{}", language.code())
            };
            let families = vec![vec![
                (candidate.clone(), fontdb::Language::English_UnitedStates),
                (localized.clone(), name_language(language)),
            ]];
            let aliases = alias_index(families.clone());
            let names = vec![candidate.clone(), localized.clone()];
            // English-US records are indistinguishable here; use the real canonical name.
            let expected = if language == UiLanguage::EnglishUs {
                &candidate
            } else {
                &localized
            };
            assert_eq!(resolve(&names, &aliases, language, None), *expected);
            assert_eq!(
                resolve(std::slice::from_ref(&candidate), &aliases, language, None),
                candidate
            );
            let unrelated = ["Unrelated".to_owned()];
            assert_eq!(resolve(&unrelated, &aliases, language, None), "Unrelated");
            let generic = "Generic Sans".to_owned();
            let fallback_aliases = alias_index(vec![vec![
                (generic.clone(), fontdb::Language::English_UnitedStates),
                (localized.clone(), name_language(language)),
            ]]);
            let names = vec![generic.clone(), localized.clone()];
            let expected = if language == UiLanguage::EnglishUs {
                &generic
            } else {
                &localized
            };
            assert_eq!(
                resolve(&names, &fallback_aliases, language, Some(&generic)),
                *expected
            );
            let mut duplicated = families.clone();
            duplicated.extend(families);
            assert_eq!(alias_index(duplicated), aliases);
        }
    }
}
