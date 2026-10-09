//! Per-action copy requests; successful feedback is cleared by a guarded UI timer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyStatus {
    pub request: u64,
    pub pending: bool,
    pub completed: u64,
}
impl CopyStatus {
    pub fn begin(&mut self) -> Option<u64> {
        if self.pending {
            return None;
        }
        self.request = self.request.wrapping_add(1);
        self.pending = true;
        self.completed = 0;
        Some(self.request)
    }
    pub fn finish(&mut self, request: u64, success: bool) -> bool {
        if !self.pending || self.request != request {
            return false;
        }
        self.pending = false;
        self.completed = if success { request } else { 0 };
        true
    }
    pub fn clear(&mut self, request: u64) {
        if self.completed == request {
            self.completed = 0;
        }
    }
}
