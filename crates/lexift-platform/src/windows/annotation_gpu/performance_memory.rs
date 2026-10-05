//! Test-only allocation observations. No memory contents or addresses are
//! recorded; page categories and the default heap are non-atomic snapshots.
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::{
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            Memory::{
                GetProcessHeap, HEAP_SUMMARY, HeapLock, HeapUnlock, MEM_COMMIT, MEM_IMAGE,
                MEM_MAPPED, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PROCESS_HEAP_ENTRY,
                VirtualQuery,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
            SystemInformation::{GetSystemInfo, SYSTEM_INFO},
            Threading::GetCurrentProcess,
        },
    },
    core::{BOOL, s, w},
};

pub(super) fn process_bytes() -> (usize, usize) {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            counters.cb,
        )
        .unwrap();
    }
    (counters.PrivateUsage, counters.WorkingSetSize)
}

/// Query only the process-owned default heap. Enumerated third-party private
/// heap handles may retire concurrently and must not be passed to HeapSummary.
/// Resolve the optional API dynamically so older Windows test binaries load.
fn default_heap_bytes() -> Option<(usize, usize, usize)> {
    type Summary = unsafe extern "system" fn(HANDLE, u32, *mut HEAP_SUMMARY) -> BOOL;
    unsafe {
        let module = GetModuleHandleW(w!("kernel32.dll")).ok()?;
        let entry = GetProcAddress(module, s!("HeapSummary"))?;
        let summary: Summary = std::mem::transmute(entry);
        let mut bytes = HEAP_SUMMARY {
            cb: std::mem::size_of::<HEAP_SUMMARY>() as u32,
            ..Default::default()
        };
        summary(GetProcessHeap().ok()?, 0, &mut bytes)
            .as_bool()
            .then_some((bytes.cbAllocated, bytes.cbCommitted, bytes.cbReserved))
    }
}

/// A bounded size histogram on the default heap, without reading payloads. Do
/// not allocate or format while holding its lock. This deliberately perturbs
/// other threads and belongs only in retired resource fixtures, never timing.
fn report_heap_sizes(label: &str, round: usize) {
    type Walk = unsafe extern "system" fn(HANDLE, *mut PROCESS_HEAP_ENTRY) -> BOOL;
    // A resident device has more size classes than a fully retired one. Keep
    // bounded stack storage, and still report incomplete if it is exhausted.
    let mut buckets = [(0u32, 0u32); 512];
    let mut complete = false;
    // PROCESS_HEAP_ENTRY_BUSY from the SDK's minwinbase.h (0x0004); avoid
    // enabling another Windows binding module solely for this test constant.
    const BUSY: u16 = 0x0004;
    let available = unsafe {
        (|| {
            let module = GetModuleHandleW(w!("kernel32.dll")).ok()?;
            let entry = GetProcAddress(module, s!("HeapWalk"))?;
            // The BOOL ABI avoids an allocating Error conversion while locked.
            let walk: Walk = std::mem::transmute(entry);
            let heap = GetProcessHeap().ok()?;
            HeapLock(heap).ok()?;
            struct Locked(HANDLE);
            impl Drop for Locked {
                fn drop(&mut self) {
                    let _ = unsafe { HeapUnlock(self.0) };
                }
            }
            let lock = Locked(heap);
            let mut block = PROCESS_HEAP_ENTRY::default();
            for _ in 0..65_536 {
                if !walk(heap, &mut block).as_bool() {
                    complete = windows::Win32::Foundation::GetLastError()
                        == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
                    break;
                }
                if block.wFlags & BUSY == 0 {
                    continue;
                }
                let matching = buckets
                    .iter()
                    .position(|&(size, count)| count != 0 && size == block.cbData);
                let slot = matching.or_else(|| buckets.iter().position(|&(_, count)| count == 0));
                let Some(slot) = slot else { break };
                buckets[slot].0 = block.cbData;
                buckets[slot].1 += 1;
            }
            drop(lock);
            Some(())
        })()
        .is_some()
    };
    println!("heap_histogram={label} round={round} available={available} complete={complete}");
    if available {
        buckets.sort_unstable_by_key(|&(size, _)| size);
        for (size, count) in buckets.into_iter().filter(|&(_, count)| count != 0) {
            println!("heap_size={label} round={round} size_bytes={size} count={count}");
        }
    }
}

/// VirtualQuery classifies committed address ranges, not private commit charge:
/// copy-on-write image/mapped pages keep their original region type. Never
/// subtract these totals from PrivateUsage to claim exact ownership.
pub(super) fn report(label: &str, round: usize) {
    let heap = default_heap_bytes();
    let mut system = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut system) };
    let maximum = system.lpMaximumApplicationAddress as usize;
    let mut address = 0usize;
    let mut totals = [0usize; 3];
    let mut regions = 0usize;
    let mut complete = true;
    while address <= maximum {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let read = unsafe {
            VirtualQuery(
                Some(address as *const _),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if read != std::mem::size_of::<MEMORY_BASIC_INFORMATION>() {
            complete = false;
            break;
        }
        regions += 1;
        if info.State == MEM_COMMIT {
            let slot = match info.Type {
                MEM_PRIVATE => Some(0),
                MEM_MAPPED => Some(1),
                MEM_IMAGE => Some(2),
                _ => None,
            };
            if let Some(slot) = slot {
                totals[slot] = totals[slot].saturating_add(info.RegionSize);
            }
        }
        let Some(next) = (info.BaseAddress as usize).checked_add(info.RegionSize) else {
            complete = false;
            break;
        };
        if next <= address {
            complete = false;
            break;
        }
        address = next;
    }
    println!(
        "allocation={label} round={round} default_heap_allocated_committed_reserved={heap:?} committed_private_mapped_image={totals:?} regions={regions} complete={complete}"
    );
    if round == 0 || round == 63 {
        report_heap_sizes(label, round);
    }
}
