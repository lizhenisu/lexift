use std::{collections::BTreeMap, env, fs, path::Path};

fn placeholders(text: &str) -> Vec<&str> {
    let mut values: Vec<_> = text
        .split('{')
        .skip(1)
        .map(|s| s.split('}').next().unwrap())
        .collect();
    values.sort_unstable();
    values
}

fn validate_ui_messages(path: &Path, expected: &BTreeMap<&str, Vec<&str>>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            validate_ui_messages(&path, expected);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "slint")
        {
            let source = fs::read_to_string(&path).unwrap();
            for fragment in source.split("@tr(\"").skip(1) {
                let key = fragment.split('"').next().unwrap();
                assert!(
                    expected.contains_key(key),
                    "{}: missing catalog entry {key}",
                    path.display()
                );
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=translations");
    let mut generated = String::from(
        "fn lookup(language: &str, key: &str) -> Option<&'static str> { match (language, key) {\n",
    );
    let base = rspolib::pofile(Path::new("translations/en-US/LC_MESSAGES/lexift-ui.po")).unwrap();
    let expected: BTreeMap<_, _> = base
        .entries
        .iter()
        .map(|e| (e.msgid.as_str(), placeholders(&e.msgid)))
        .collect();
    validate_ui_messages(Path::new("ui"), &expected);
    let mut directories: Vec<_> = fs::read_dir("translations")
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| e.path().is_dir())
        .collect();
    directories.sort_by_key(|e| e.file_name());
    assert_eq!(
        directories.len(),
        12,
        "all supported locales must be bundled"
    );
    for directory in directories {
        let language = directory.file_name().to_string_lossy().into_owned();
        let catalog =
            rspolib::pofile(directory.path().join("LC_MESSAGES/lexift-ui.po").as_path()).unwrap();
        assert_eq!(
            catalog.entries.len(),
            expected.len(),
            "{language}: incomplete catalog"
        );
        let mut seen = std::collections::BTreeSet::new();
        for entry in &catalog.entries {
            let translation = entry.msgstr.as_deref().unwrap_or("");
            assert!(seen.insert(&entry.msgid), "duplicate message in {language}");
            assert!(
                !translation.is_empty() && !entry.flags.iter().any(|f| f == "fuzzy"),
                "{language}: untranslated {}",
                entry.msgid
            );
            assert_eq!(
                expected.get(entry.msgid.as_str()),
                Some(&placeholders(translation)),
                "{language}: invalid placeholders for {}",
                entry.msgid
            );
            generated.push_str(&format!(
                "({language:?}, {:?}) => Some({translation:?}),\n",
                entry.msgid
            ));
        }
    }
    generated.push_str("_ => None } }\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("catalog.rs"),
        generated,
    )
    .unwrap();
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/main.slint", config).expect("failed to compile Slint UI");
}
