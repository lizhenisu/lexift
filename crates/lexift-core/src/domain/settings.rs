use super::language::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub target_language: Language,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_language: Language("zh-CN".into()),
        }
    }
}
