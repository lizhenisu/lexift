//! One coalesced source wake owned by the capture backend, never a frame queue.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use windows::{Foundation::TypedEventHandler, Graphics::Capture::Direct3D11CaptureFramePool};

pub(crate) struct Notice {
    pending: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
    #[cfg(test)]
    observations: std::sync::Mutex<std::collections::VecDeque<super::diagnostics::Event>>,
    #[cfg(test)]
    dropped: std::sync::atomic::AtomicUsize,
}
impl Notice {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            wake: Box::new(wake),
            #[cfg(test)]
            observations: Default::default(),
            #[cfg(test)]
            dropped: Default::default(),
        })
    }
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
    fn notify(&self, token: isize, #[cfg(test)] entered: std::time::Instant) {
        #[cfg(not(test))]
        let _ = token;
        let woke = !self.pending.swap(true, Ordering::AcqRel);
        if woke {
            (self.wake)();
        }
        #[cfg(test)]
        {
            let notified = std::time::Instant::now();
            let mut observations = self.observations.lock().unwrap();
            if observations.len() == 4096 {
                observations.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            observations.push_back(super::diagnostics::Event {
                token,
                sequence: 0,
                at: entered,
                kind: super::diagnostics::Kind::Arrived { notified, woke },
            });
        }
    }
    #[cfg(test)]
    pub fn drain(&self) -> (Vec<super::diagnostics::Event>, usize) {
        (
            self.observations.lock().unwrap().drain(..).collect(),
            self.dropped.swap(0, Ordering::Relaxed),
        )
    }
}

/// Revoke before closing the pool. An in-flight callback only owns a weak notice;
/// it cannot retain the renderer or deliver data into a replacement backend.
pub(crate) struct Subscription {
    pool: Direct3D11CaptureFramePool,
    token: i64,
    active: Arc<AtomicBool>,
}
impl Subscription {
    pub fn new(
        pool: &Direct3D11CaptureFramePool,
        notice: &Arc<Notice>,
        source: isize,
    ) -> windows::core::Result<Self> {
        let weak = Arc::downgrade(notice);
        let active = Arc::new(AtomicBool::new(true));
        let alive = active.clone();
        let handler = TypedEventHandler::new(move |_, _| {
            #[cfg(test)]
            let entered = std::time::Instant::now();
            if alive.load(Ordering::Acquire)
                && let Some(notice) = weak.upgrade()
            {
                notice.notify(
                    source,
                    #[cfg(test)]
                    entered,
                );
            }
            Ok(())
        });
        let token = pool.FrameArrived(&handler)?;
        Ok(Self {
            pool: pool.clone(),
            token,
            active,
        })
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        let _ = self.pool.RemoveFrameArrived(self.token);
    }
}
#[test]
fn wakes_coalesce_without_retaining_or_replaying_old_notices() {
    use std::sync::atomic::AtomicUsize;
    let wakes = Arc::new(AtomicUsize::new(0));
    let observed = wakes.clone();
    let notice = Notice::new(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    for _ in 0..10 {
        notice.notify(7, std::time::Instant::now());
    }
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert!(notice.take());
    assert!(!notice.take());
    notice.notify(7, std::time::Instant::now());
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
    for _ in 0..4200 {
        notice.notify(7, std::time::Instant::now());
    }
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        2,
        "notifications queued extra wakeups"
    );
    let (events, dropped) = notice.drain();
    assert_eq!(events.len(), 4096);
    assert_eq!(dropped, 115);
    let weak = Arc::downgrade(&notice);
    drop(notice);
    assert!(weak.upgrade().is_none());
    let replacement = Notice::new(|| {});
    assert!(!replacement.take());
}

#[test]
fn callback_time_precedes_notification_and_diagnostic_recording() {
    let notice = Notice::new(|| {});
    let entered = std::time::Instant::now() - std::time::Duration::from_millis(5);
    notice.notify(8, entered);
    notice.notify(8, entered);
    let (events, dropped) = notice.drain();
    assert_eq!(dropped, 0);
    assert_eq!(events.len(), 2);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.at, entered);
        let super::diagnostics::Kind::Arrived { notified, woke } = event.kind else {
            panic!("arrival must retain callback entry and notification completion");
        };
        assert!(notified >= event.at);
        assert_eq!(woke, index == 0);
    }
    assert!(notice.take());
    assert!(!notice.take());
}
