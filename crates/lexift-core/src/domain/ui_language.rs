//! Interface language, deliberately independent from translation source and target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UiLanguage {
    SimplifiedChinese,
    TraditionalChinese,
    #[default]
    EnglishUs,
    EnglishUk,
    Japanese,
    Korean,
    German,
    French,
    Spanish,
    Italian,
    PortuguesePortugal,
    PortugueseBrazil,
}
impl UiLanguage {
    pub const ALL: [Self; 12] = [
        Self::SimplifiedChinese,
        Self::TraditionalChinese,
        Self::EnglishUs,
        Self::EnglishUk,
        Self::Japanese,
        Self::Korean,
        Self::German,
        Self::French,
        Self::Spanish,
        Self::Italian,
        Self::PortuguesePortugal,
        Self::PortugueseBrazil,
    ];
    pub const fn code(self) -> &'static str {
        match self {
            Self::SimplifiedChinese => "zh-CN",
            Self::TraditionalChinese => "zh-TW",
            Self::EnglishUs => "en-US",
            Self::EnglishUk => "en-GB",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::German => "de",
            Self::French => "fr",
            Self::Spanish => "es",
            Self::Italian => "it",
            Self::PortuguesePortugal => "pt-PT",
            Self::PortugueseBrazil => "pt-BR",
        }
    }
    pub fn from_index(index: i32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }
    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|x| *x == self).unwrap_or(2) as i32
    }
}
impl std::str::FromStr for UiLanguage {
    type Err = crate::Error;
    fn from_str(code: &str) -> crate::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|x| x.code() == code)
            .ok_or_else(|| crate::Error::new(format!("Unsupported interface language {code}")))
    }
}
