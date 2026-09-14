use super::language::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslateRequest {
    pub text: String,
    pub target_language: Language,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslateResult {
    pub text: String,
}
