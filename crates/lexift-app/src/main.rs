mod bootstrap;
mod controller;
mod lifecycle;
mod wiring;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    bootstrap::run()
}
