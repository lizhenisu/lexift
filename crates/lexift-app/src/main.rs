#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bootstrap;
mod controller;
mod lifecycle;
mod runtime;
mod wiring;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupMode {
    Interactive,
    Background,
}

impl StartupMode {
    fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        if args.into_iter().any(|argument| argument == "--background") {
            Self::Background
        } else {
            Self::Interactive
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    bootstrap::run(StartupMode::from_args(std::env::args().skip(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_flag_controls_initial_visibility() {
        assert_eq!(
            StartupMode::from_args(["--background".into()]),
            StartupMode::Background
        );
        assert_eq!(
            StartupMode::from_args(Vec::<String>::new()),
            StartupMode::Interactive
        );
    }
}
