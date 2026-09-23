use lexift_core::{Error, Result};
use windows::Win32::System::{ProcessStatus::EmptyWorkingSet, Threading::GetCurrentProcess};

/// Asks Windows to evict this process's resident pages from physical memory.
///
/// This does not decommit allocations or release renderer resources; pages are faulted back in
/// when needed. Per-window renderer resources are released when their Slint windows are dropped.
pub(crate) fn trim_working_set() -> Result<()> {
    // SAFETY: GetCurrentProcess returns the pseudo-handle for this process, valid for this call.
    unsafe { EmptyWorkingSet(GetCurrentProcess()) }
        .map_err(|error| Error::new(format!("Could not trim process working set: {error}")))
}
