//! Stable identifiers carried by existing message channels; technical details stay opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    Copied,
    EmptyApiKey,
    MissingApiKey,
    EmptyTranslation,
    NothingToRead,
}
impl MessageId {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Copied => "lexift.copied",
            Self::EmptyApiKey => "lexift.credentials.empty",
            Self::MissingApiKey => "lexift.credentials.missing",
            Self::EmptyTranslation => "lexift.translation.empty",
            Self::NothingToRead => "lexift.speech.empty",
        }
    }
}
