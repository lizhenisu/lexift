use std::sync::Arc;

use crate::domain::{language::Language, translation::PopupSessionId};

#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub session_id: PopupSessionId,
    pub source: bool,
    pub text: String,
    pub language: Option<Language>,
}

#[derive(Debug, Clone)]
pub struct SpeechEvent {
    pub session_id: PopupSessionId,
    pub source: bool,
    pub speaking: bool,
    pub error: Option<String>,
}

pub type SpeechEventHandler = Arc<dyn Fn(SpeechEvent) + Send + Sync>;

/// Plays short text utterances without blocking the application event loop.
pub trait SpeechPort: Send + Sync {
    fn set_event_handler(&self, handler: SpeechEventHandler);
    fn speak(&self, request: SpeechRequest) -> crate::Result<()>;
    fn stop(&self, session_id: PopupSessionId) -> crate::Result<()>;
}
