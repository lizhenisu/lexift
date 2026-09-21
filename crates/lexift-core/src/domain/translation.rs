use super::language::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TranslationTaskId(u64);

impl TranslationTaskId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupSessionId(u64);

impl PopupSessionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslateRequest {
    pub text: String,
    pub target_language: Language,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslateResult {
    pub text: String,
    pub detected_source_language: Option<Language>,
}
