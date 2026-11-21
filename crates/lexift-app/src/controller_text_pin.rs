use super::{AppController, AppEvent};
use lexift_core::domain::text_pin::{self, Command, Event, Failure};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Resources {
    text: Mutex<BTreeMap<u64, Arc<str>>>,
    writer: Mutex<()>,
}
impl Resources {
    pub(super) fn clear(&self) {
        self.text.lock().unwrap().clear();
    }
}

impl AppController {
    pub(super) fn execute_text_pin(self: &Arc<Self>, command: Command) {
        match command {
            Command::Read(id) => {
                let controller = self.clone();
                self.runtime.spawn_blocking(move || {
                    let snapshot = controller
                        .clipboard
                        .as_ref()
                        .ok_or(Failure::Read)
                        .and_then(|port| port.read_text().map_err(|_| Failure::Read));
                    let snapshot = snapshot.and_then(|text| {
                        text_pin::validate(text.as_deref())?;
                        text.ok_or(Failure::Empty)
                    });
                    let result = {
                        let state = controller.state.lock().unwrap();
                        if !state
                            .text_pins
                            .creation
                            .is_some_and(|creation| creation.id == id)
                        {
                            return;
                        }
                        snapshot.map(|text| {
                            controller
                                .text_pins
                                .text
                                .lock()
                                .unwrap()
                                .insert(id, Arc::from(text));
                        })
                    };
                    controller.dispatch(AppEvent::TextPin(Event::Read { id, result }));
                });
            }
            Command::Present(creation) => {
                let text = self
                    .text_pins
                    .text
                    .lock()
                    .unwrap()
                    .get(&creation.id)
                    .cloned();
                if text.is_none() {
                    self.dispatch(AppEvent::TextPin(Event::Presented {
                        id: creation.id,
                        success: false,
                    }));
                } else {
                    self.ui.text_pin(command, text);
                }
            }
            Command::Copy {
                id,
                request,
                selection,
            } => {
                let controller = self.clone();
                self.runtime.spawn_blocking(move || {
                    let success = {
                        let _write = controller.text_pins.writer.lock().unwrap();
                        let text = {
                            let state = controller.state.lock().unwrap();
                            if !state
                                .text_pins
                                .pins
                                .get(&id)
                                .is_some_and(|pin| pin.copy == Some(request))
                            {
                                return;
                            }
                            controller.text_pins.text.lock().unwrap().get(&id).cloned()
                        };
                        text.as_deref()
                            .and_then(|text| text_pin::selection(text, selection))
                            .is_some_and(|text| {
                                controller
                                    .clipboard
                                    .as_ref()
                                    .is_some_and(|port| port.write_sensitive_text(text).is_ok())
                            })
                    };
                    controller.dispatch(AppEvent::TextPin(Event::Copied {
                        id,
                        request,
                        success,
                    }));
                });
            }
            Command::Drop(id) => {
                self.text_pins.text.lock().unwrap().remove(&id);
                self.ui.text_pin(command, None);
            }
            Command::Visibility(_) | Command::Feedback { .. } => self.ui.text_pin(command, None),
        }
    }
}
