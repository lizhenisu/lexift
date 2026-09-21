use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use lexift_core::{
    Error, Result,
    domain::translation::PopupSessionId,
    ports::speech::{SpeechEvent, SpeechEventHandler, SpeechPort, SpeechRequest},
};

pub(crate) struct WindowsSpeechPort {
    sender: mpsc::Sender<SpeechCommand>,
    handler: Arc<Mutex<Option<SpeechEventHandler>>>,
}

enum SpeechCommand {
    Speak(SpeechRequest),
    Stop(PopupSessionId),
}

impl WindowsSpeechPort {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(Mutex::new(None));
        let worker_handler = Arc::clone(&handler);
        thread::Builder::new()
            .name("lexift-speech".into())
            .spawn(move || run_worker(receiver, &worker_handler))
            .expect("failed to start speech worker");
        Self { sender, handler }
    }
}

impl SpeechPort for WindowsSpeechPort {
    fn set_event_handler(&self, handler: SpeechEventHandler) {
        *self
            .handler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handler);
    }

    fn speak(&self, request: SpeechRequest) -> Result<()> {
        self.sender
            .send(SpeechCommand::Speak(request))
            .map_err(|_| Error::new("Speech worker is unavailable"))
    }

    fn stop(&self, session_id: PopupSessionId) -> Result<()> {
        self.sender
            .send(SpeechCommand::Stop(session_id))
            .map_err(|_| Error::new("Speech worker is unavailable"))
    }
}

fn run_worker(
    receiver: mpsc::Receiver<SpeechCommand>,
    handler: &Mutex<Option<SpeechEventHandler>>,
) {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::{
        Win32::Media::Speech::{ISpVoice, ISpeechVoice, SpVoice},
        core::Interface,
    };

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let voice = if initialized {
        unsafe { CoCreateInstance::<_, ISpVoice>(&SpVoice, None, CLSCTX_ALL) }.ok()
    } else {
        None
    };
    let Some(voice) = voice else {
        while let Ok(command) = receiver.recv() {
            if let SpeechCommand::Speak(request) = command {
                notify(
                    handler,
                    SpeechEvent {
                        session_id: request.session_id,
                        source: request.source,
                        speaking: false,
                        error: Some("Windows speech synthesis is unavailable".into()),
                    },
                );
            }
        }
        if initialized {
            unsafe { CoUninitialize() };
        }
        return;
    };
    let automation_voice = voice.cast::<ISpeechVoice>().ok();

    let mut active: Option<(PopupSessionId, bool)> = None;
    loop {
        let command = if active.is_some() {
            match receiver.recv_timeout(Duration::from_millis(60)) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if speech_finished(&voice) {
                        if let Some((session_id, source)) = active.take() {
                            notify(
                                handler,
                                SpeechEvent {
                                    session_id,
                                    source,
                                    speaking: false,
                                    error: None,
                                },
                            );
                        }
                    }
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        let Some(command) = command else { continue };
        match command {
            SpeechCommand::Speak(request) => {
                purge(&voice);
                if let Some((session_id, source)) = active.take() {
                    notify(
                        handler,
                        SpeechEvent {
                            session_id,
                            source,
                            speaking: false,
                            error: None,
                        },
                    );
                }
                if let (Some(automation_voice), Some(language)) =
                    (&automation_voice, request.language.as_ref())
                {
                    select_voice(&voice, automation_voice, language);
                }
                match speak(&voice, &request.text) {
                    Ok(()) => {
                        active = Some((request.session_id, request.source));
                        notify(
                            handler,
                            SpeechEvent {
                                session_id: request.session_id,
                                source: request.source,
                                speaking: true,
                                error: None,
                            },
                        );
                    }
                    Err(error) => notify(
                        handler,
                        SpeechEvent {
                            session_id: request.session_id,
                            source: request.source,
                            speaking: false,
                            error: Some(error),
                        },
                    ),
                }
            }
            SpeechCommand::Stop(session_id) => {
                if active.is_some_and(|(active_id, _)| active_id == session_id) {
                    purge(&voice);
                    if let Some((session_id, source)) = active.take() {
                        notify(
                            handler,
                            SpeechEvent {
                                session_id,
                                source,
                                speaking: false,
                                error: None,
                            },
                        );
                    }
                }
            }
        }
    }
    purge(&voice);
    drop(automation_voice);
    drop(voice);
    unsafe { CoUninitialize() };
}

fn select_voice(
    voice: &windows::Win32::Media::Speech::ISpVoice,
    automation_voice: &windows::Win32::Media::Speech::ISpeechVoice,
    language: &lexift_core::domain::language::Language,
) {
    use windows::{
        Win32::Media::Speech::ISpObjectToken,
        core::{BSTR, Interface},
    };
    let Some(lcid) = language_lcid(&language.0) else {
        return;
    };
    let required = BSTR::from(format!("Language={lcid:x}"));
    let optional = BSTR::new();
    let token = unsafe {
        automation_voice
            .GetVoices(&required, &optional)
            .and_then(|tokens| tokens.Item(0))
            .and_then(|token| token.cast::<ISpObjectToken>())
    };
    if let Ok(token) = token {
        let _ = unsafe { voice.SetVoice(&token) };
    }
}

fn language_lcid(language: &str) -> Option<u16> {
    Some(match language {
        "zh-CN" => 0x0804,
        "zh-TW" => 0x0404,
        "en-US" | "en" => 0x0409,
        "en-GB" => 0x0809,
        "ja" => 0x0411,
        "ko" => 0x0412,
        "de" => 0x0407,
        "fr" => 0x040c,
        "es" => 0x0c0a,
        "it" => 0x0410,
        "pt-PT" => 0x0816,
        "pt-BR" => 0x0416,
        _ => return None,
    })
}

fn speak(
    voice: &windows::Win32::Media::Speech::ISpVoice,
    text: &str,
) -> std::result::Result<(), String> {
    use windows::Win32::Media::Speech::{SPF_ASYNC, SPF_PURGEBEFORESPEAK};
    let wide = text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    unsafe {
        voice
            .Speak(
                windows::core::PCWSTR(wide.as_ptr()),
                (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0) as u32,
                None,
            )
            .map_err(|error| format!("Could not start speech: {error}"))
    }
}

fn purge(voice: &windows::Win32::Media::Speech::ISpVoice) {
    use windows::Win32::Media::Speech::{SPF_ASYNC, SPF_PURGEBEFORESPEAK};
    unsafe {
        let _ = voice.Speak(
            windows::core::PCWSTR::null(),
            (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0) as u32,
            None,
        );
    }
}

fn speech_finished(voice: &windows::Win32::Media::Speech::ISpVoice) -> bool {
    use windows::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};
    unsafe { WaitForSingleObject(voice.SpeakCompleteEvent(), 0) == WAIT_OBJECT_0 }
}

fn notify(handler: &Mutex<Option<SpeechEventHandler>>, event: SpeechEvent) {
    if let Some(handler) = handler
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(Arc::clone)
    {
        handler(event);
    }
}

#[cfg(test)]
mod tests {
    use super::language_lcid;

    #[test]
    fn maps_supported_popup_languages_to_windows_locales() {
        assert_eq!(language_lcid("zh-CN"), Some(0x0804));
        assert_eq!(language_lcid("en-GB"), Some(0x0809));
        assert_eq!(language_lcid("pt-BR"), Some(0x0416));
        assert_eq!(language_lcid("unsupported"), None);
    }
}
