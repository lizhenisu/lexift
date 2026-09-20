use std::{fmt, str::FromStr};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl HotkeyModifiers {
    pub const fn alt() -> Self {
        Self {
            control: false,
            alt: true,
            shift: false,
            meta: false,
        }
    }

    pub const fn is_empty(self) -> bool {
        !self.control && !self.alt && !self.shift && !self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyKey {
    Letter(char),
    Digit(u8),
    Function(u8),
}

impl HotkeyKey {
    pub fn parse_event_text(value: &str) -> Option<Self> {
        let mut chars = value.chars();
        let value = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        match value {
            'a'..='z' | 'A'..='Z' => Some(Self::Letter(value.to_ascii_uppercase())),
            '0'..='9' => Some(Self::Digit(value as u8 - b'0')),
            ')' => Some(Self::Digit(0)),
            '!' => Some(Self::Digit(1)),
            '@' => Some(Self::Digit(2)),
            '#' => Some(Self::Digit(3)),
            '$' => Some(Self::Digit(4)),
            '%' => Some(Self::Digit(5)),
            '^' => Some(Self::Digit(6)),
            '&' => Some(Self::Digit(7)),
            '*' => Some(Self::Digit(8)),
            '(' => Some(Self::Digit(9)),
            '\u{f704}'..='\u{f70f}' => Some(Self::Function((value as u32 - 0xf704 + 1) as u8)),
            _ => None,
        }
    }
}

impl fmt::Display for HotkeyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Letter(value) => write!(formatter, "{value}"),
            Self::Digit(value) => write!(formatter, "{value}"),
            Self::Function(value) => write!(formatter, "F{value}"),
        }
    }
}

impl FromStr for HotkeyKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let normalized = value.trim().to_ascii_uppercase();
        if normalized.len() == 1 {
            return Self::parse_event_text(&normalized)
                .ok_or_else(|| Error::new("Hotkey key must be A-Z or 0-9"));
        }
        if let Some(number) = normalized.strip_prefix('F')
            && let Ok(number) = number.parse::<u8>()
            && (1..=12).contains(&number)
        {
            return Ok(Self::Function(number));
        }
        Err(Error::new("Hotkey key must be A-Z, 0-9, or F1-F12"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub modifiers: HotkeyModifiers,
    pub key: HotkeyKey,
}

impl HotkeyConfig {
    pub fn new(modifiers: HotkeyModifiers, key: HotkeyKey) -> Result<Self> {
        if modifiers.is_empty() {
            return Err(Error::new("Hotkey must include Ctrl, Alt, Shift, or Win"));
        }
        Ok(Self { modifiers, key })
    }

    pub fn from_key_event(
        text: &str,
        control: bool,
        alt: bool,
        shift: bool,
        meta: bool,
    ) -> Result<Self> {
        let key = HotkeyKey::parse_event_text(text)
            .ok_or_else(|| Error::new("Press A-Z, 0-9, or F1-F12"))?;
        Self::new(
            HotkeyModifiers {
                control,
                alt,
                shift,
                meta,
            },
            key,
        )
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            modifiers: HotkeyModifiers::alt(),
            key: HotkeyKey::Letter('X'),
        }
    }
}

impl fmt::Display for HotkeyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::with_capacity(5);
        if self.modifiers.control {
            parts.push("Ctrl".to_owned());
        }
        if self.modifiers.alt {
            parts.push("Alt".to_owned());
        }
        if self.modifiers.shift {
            parts.push("Shift".to_owned());
        }
        if self.modifiers.meta {
            parts.push("Win".to_owned());
        }
        parts.push(self.key.to_string());
        formatter.write_str(&parts.join(" + "))
    }
}

impl FromStr for HotkeyConfig {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut modifiers = HotkeyModifiers {
            control: false,
            alt: false,
            shift: false,
            meta: false,
        };
        let mut key = None;
        for part in value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.control = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "win" | "meta" => modifiers.meta = true,
                _ if key.is_none() => key = Some(part.parse()?),
                _ => return Err(Error::new("Hotkey contains more than one key")),
            }
        }
        Self::new(
            modifiers,
            key.ok_or_else(|| Error::new("Hotkey key is missing"))?,
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProviderConfig {
    #[default]
    DeepL,
}

impl ProviderConfig {
    pub const fn id(self) -> &'static str {
        match self {
            Self::DeepL => "deepl",
        }
    }
}

impl fmt::Display for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DeepL => "DeepL",
        })
    }
}

impl FromStr for ProviderConfig {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deepl" => Ok(Self::DeepL),
            _ => Err(Error::new("Translation provider is not available")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub hotkey: HotkeyConfig,
    pub provider: ProviderConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_round_trips_through_its_display_label() {
        for value in ["Alt + X", "Ctrl + Shift + 7", "Win + F12"] {
            let parsed: HotkeyConfig = value.parse().unwrap();
            assert_eq!(parsed.to_string().parse::<HotkeyConfig>().unwrap(), parsed);
        }
    }

    #[test]
    fn rejects_unmodified_or_unsupported_keys() {
        assert!("X".parse::<HotkeyConfig>().is_err());
        assert!("Alt + Escape".parse::<HotkeyConfig>().is_err());
        assert!("Alt + F13".parse::<HotkeyConfig>().is_err());
    }

    #[test]
    fn shifted_number_row_text_maps_back_to_digits() {
        let config = HotkeyConfig::from_key_event("&", true, false, true, false).unwrap();
        assert_eq!(config.to_string(), "Ctrl + Shift + 7");
    }
}
