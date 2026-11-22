//! Filename rules shared by screenshot settings and unattended PNG output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix(String);

impl Default for Prefix {
    fn default() -> Self {
        Self("Lexift".into())
    }
}

impl Prefix {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if !(1..=32).contains(&trimmed.chars().count())
            || trimmed.ends_with('.')
            || value
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        {
            return None;
        }
        // A device name before an internal dot stays reserved after appending the timestamp.
        if let Some((first, _)) = trimmed.split_once('.') {
            let first = first.trim_end().to_ascii_uppercase();
            if matches!(
                first.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) || first
                .strip_prefix("COM")
                .or_else(|| first.strip_prefix("LPT"))
                .is_some_and(|n| {
                    matches!(
                        n,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            {
                return None;
            }
        }
        Some(Self(trimmed.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn filename(&self, timestamp: &str) -> String {
        format!("{}-{timestamp}.png", self.0)
    }

    /// Keep the timestamp frozen with the export snapshot.
    pub fn for_snapshot(&self, name: &str) -> String {
        match name
            .strip_prefix("Lexift-")
            .and_then(|name| name.strip_suffix(".png"))
        {
            Some(timestamp) => self.filename(timestamp),
            None => name.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_unicode_and_keeps_dots_inside_the_filename() {
        let prefix = Prefix::parse("  文档 2.0  ").unwrap();
        assert_eq!(prefix.as_str(), "文档 2.0");
        assert_eq!(
            prefix.for_snapshot("Lexift-20261122-143052.png"),
            "文档 2.0-20261122-143052.png"
        );
        assert!(Prefix::parse(&"图".repeat(32)).is_some());
    }

    #[test]
    fn rejects_unsafe_or_invalid_prefixes() {
        for value in [
            "",
            "  ",
            ".",
            "abc.",
            "../图",
            "A:B",
            "A\\B",
            "a\nb",
            "\tfoo",
            "CON.foo",
            "lpt².extra",
        ] {
            assert!(Prefix::parse(value).is_none(), "{value:?}");
        }
        assert!(Prefix::parse("CON").is_some());
        assert!(Prefix::parse(&"图".repeat(33)).is_none());
    }
}
