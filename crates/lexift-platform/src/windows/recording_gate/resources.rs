//! Explicit gate ownership counters, independent of allocator retention.
use std::sync::atomic::{AtomicUsize, Ordering};
pub static CAPTURE: AtomicUsize = AtomicUsize::new(0);
pub static ENCODER: AtomicUsize = AtomicUsize::new(0);
pub static CONTROL: AtomicUsize = AtomicUsize::new(0);
pub static WINDOWS: AtomicUsize = AtomicUsize::new(0);
pub struct Lease(&'static AtomicUsize);
impl Lease {
    pub fn new(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub fn snapshot() -> serde_json::Value {
    serde_json::json!({"capture":CAPTURE.load(Ordering::Acquire),"encoder":ENCODER.load(Ordering::Acquire),"control":CONTROL.load(Ordering::Acquire),"windows":WINDOWS.load(Ordering::Acquire)})
}
pub fn assert_retired() {
    for counter in [&CAPTURE, &ENCODER, &CONTROL, &WINDOWS] {
        assert_eq!(
            counter.load(Ordering::Acquire),
            0,
            "Recording owner did not retire"
        );
    }
}
