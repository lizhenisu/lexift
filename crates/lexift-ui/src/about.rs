//! Fixed About-page destinations and window-owned browser feedback.
use slint::ComponentHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutLink {
    Project,
    Feedback,
    License,
}

impl AboutLink {
    pub fn url(self) -> &'static str {
        match self {
            Self::Project => "https://github.com/lizhenisu/lexift",
            Self::Feedback => "https://github.com/lizhenisu/lexift/issues",
            Self::License => "https://github.com/lizhenisu/lexift/blob/main/LICENSE",
        }
    }
}

pub(crate) fn wire(settings: &crate::SettingsWindow, open: std::rc::Rc<dyn Fn(AboutLink) -> bool>) {
    settings.set_about_version(env!("CARGO_PKG_VERSION").into());
    let weak = settings.as_weak();
    settings.on_about_link_requested(move |index| {
        let link = match index {
            0 => AboutLink::Project,
            1 => AboutLink::Feedback,
            2 => AboutLink::License,
            _ => return,
        };
        if let Some(settings) = weak.upgrade() {
            settings.set_about_link_failed(!open(link));
        }
    });
}
