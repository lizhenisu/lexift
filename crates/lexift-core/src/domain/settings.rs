use super::language::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub target_language: Language,
    /// Reference to a system credential. This value is never the credential secret.
    pub deepl_credential_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_language: Language("zh-CN".into()),
            deepl_credential_id: None,
        }
    }
}
