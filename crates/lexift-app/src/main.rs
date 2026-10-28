#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotation_export;
mod bootstrap;
mod clipboard_history;
mod controller;
mod lifecycle;
mod llm_credentials;
mod ocr;
mod runtime;
mod screenshot;
mod wiring;

// Startup is silent by default; the legacy --background argument remains accepted.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    bootstrap::run()
}
