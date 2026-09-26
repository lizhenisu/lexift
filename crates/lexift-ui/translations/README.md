# Bundled interface translations

The 12 `*/LC_MESSAGES/lexift-ui.po` catalogs are the single source for Slint and Rust UI text.
`en-US` is the source/default; `en-GB` includes British spelling. Locale codes are independent of translation target languages.

- Mark Slint user-facing text with `@tr("English message")`. No component context is used, so common labels share translations.
- Add the same `msgid` to **every** catalog. Keep numbered placeholders such as `{0}` unchanged. Do not translate language endonyms, user content, font names, brands or shortcut combinations.
- Rust presentation code calls `i18n::tr` / `i18n::detail`. `build.rs` generates its table from these same PO files; no files are read at runtime.
- Core common messages use `domain::message::MessageId`; phase/settings feedback use their existing enums. External technical error details are preserved within a localized error wrapper.
- Compilation rejects missing/duplicate/empty/fuzzy entries and mismatched placeholders. Slint message coverage is validated against the English catalog. Unknown dynamic details retain an English fallback.

`i18n::select` runs on the UI thread before state mapping. Each newly created component applies the current bundled locale before showing. Open dropdowns close on a switch; window and tool state remain owned by their existing registries. Tray labels cross the Core port via the App composition root.

## Visual checks

Run `cargo test -p lexift-ui i18n::tests::all_languages`.
Set `LEXIFT_I18N_SNAPSHOT_DIR` to an output directory to additionally render all Settings pages at 960 and 580 logical pixels, light/dark Popup and annotation panels, main window, language menu and toolbar as BMP files. Without this variable the test creates no artifacts.

Integer input keeps its existing validation/step limits. Slint controls use the bundled locale’s decimal separator; magnification labels use localized numeric interpolation while tool values remain numeric indices.
