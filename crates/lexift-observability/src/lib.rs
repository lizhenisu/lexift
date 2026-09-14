mod panic;
pub mod performance;
mod tracing;

pub fn init() {
    tracing::init();
    panic::install();
}
