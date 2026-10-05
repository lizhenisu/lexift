//! Bounded test-only observations on the capture worker. Times describe frame
//! dequeue and comparison observation, not WGC arrival or physical display.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// Event time is callback entry; notification completion excludes diagnostic locking.
    /// Notifications have no frame identity and must not be matched to a version by proximity.
    Arrived {
        notified: Instant,
        woke: bool,
    },
    Frame {
        ticks: i64,
        dequeue_qpc: Option<i64>,
    },
    Confirm {
        elapsed: Duration,
        started: Instant,
        candidate: Option<Published>,
        changed: bool,
    },
    Pending,
    /// CPU query-readiness observation, not an absolute GPU completion time.
    QueryPoll {
        candidate: Published,
        began: Instant,
        finished: Instant,
        ready: bool,
    },
    GpuComparison {
        candidate: Published,
        copy_ms: f64,
        compare_ms: f64,
        observed_ms: f64,
    },
    GpuTimingInvalid {
        candidate: Published,
    },
    GpuTimingSkipped {
        candidate: Published,
    },
}
#[derive(Clone, Copy)]
pub(crate) struct Event {
    pub token: isize,
    pub sequence: u64,
    pub at: Instant,
    pub kind: Kind,
}
/// Version of the texture actually published after capture comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Published {
    pub token: isize,
    pub sequence: u64,
    pub ticks: i64,
}
#[derive(Default)]
pub(crate) struct Source {
    sequence: u64,
    latest: Option<Published>,
    candidate: Option<Published>,
    published: Option<Published>,
    started: Option<Instant>,
    events: VecDeque<Event>,
    pub dropped: usize,
    pub gpu_enabled: bool,
    gpu: super::super::gpu_timing::Profiler,
    gpu_candidate: Option<Published>,
}
impl Source {
    pub fn record(&mut self, token: isize, kind: Kind) {
        self.record_version(token, self.sequence, kind);
    }
    fn record_version(&mut self, token: isize, sequence: u64, kind: Kind) {
        if self.events.len() == 128 {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(Event {
            token,
            sequence,
            at: Instant::now(),
            kind,
        });
    }
    pub fn frame(&mut self, token: isize, ticks: i64) {
        self.sequence += 1;
        self.latest = Some(Published {
            token,
            sequence: self.sequence,
            ticks,
        });
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "QueryPerformanceCounter"]
            fn query_counter(value: *mut i64) -> i32;
        }
        let mut qpc = 0;
        // Test-only independent clock check at dequeue; no production API changes.
        let dequeue_qpc = (unsafe { query_counter(&mut qpc) } != 0).then_some(qpc);
        self.record(token, Kind::Frame { ticks, dequeue_qpc });
    }
    /// One outstanding timing owns its identity until its queries retire. A
    /// later comparison may proceed uninstrumented rather than rebind it.
    pub(super) fn start_gpu(
        &mut self,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    ) -> windows::core::Result<Option<super::super::gpu_timing::Trace>> {
        let Some(candidate) = self.latest.filter(|_| self.gpu_enabled) else {
            return Ok(None);
        };
        if self.gpu.pending_count() != 0 {
            self.record_version(
                candidate.token,
                candidate.sequence,
                Kind::GpuTimingSkipped { candidate },
            );
            return Ok(None);
        }
        let trace = self.gpu.begin(device, context)?;
        if trace.is_some() {
            self.gpu_candidate = Some(candidate);
        }
        Ok(trace)
    }
    pub(super) fn finish_gpu(
        &mut self,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
        trace: super::super::gpu_timing::Trace,
    ) {
        self.gpu.finish(context, trace);
    }
    pub(super) fn poll_gpu(
        &mut self,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    ) -> windows::core::Result<()> {
        let samples = self.gpu.poll(context)?;
        if let Some(sample) = samples.into_iter().next() {
            let candidate = self
                .gpu_candidate
                .take()
                .expect("timing has one frozen source");
            self.record_version(
                candidate.token,
                candidate.sequence,
                Kind::GpuComparison {
                    candidate,
                    copy_ms: sample.stages_ms[0],
                    compare_ms: sample.stages_ms[1],
                    observed_ms: sample.observed_ms,
                },
            );
        } else if self.gpu.pending_count() == 0
            && let Some(candidate) = self.gpu_candidate.take()
        {
            self.record_version(
                candidate.token,
                candidate.sequence,
                Kind::GpuTimingInvalid { candidate },
            );
        }
        Ok(())
    }

    pub fn begin(&mut self) {
        self.started = Some(Instant::now());
        self.candidate = self.latest;
    }
    pub fn query_poll(&mut self, poll: Option<(Instant, Instant, bool)>) {
        if let Some((began, finished, ready)) = poll
            && let Some(candidate) = self.candidate
        {
            self.record_version(
                candidate.token,
                candidate.sequence,
                Kind::QueryPoll {
                    candidate,
                    began,
                    finished,
                    ready,
                },
            );
        }
    }
    pub fn confirm(&mut self, token: isize, changed: bool) {
        if let Some(started) = self.started.take() {
            let candidate = self.candidate;
            if changed {
                self.publish_candidate();
            }
            self.record_version(
                token,
                candidate.map_or(self.sequence, |candidate| candidate.sequence),
                Kind::Confirm {
                    elapsed: started.elapsed(),
                    started,
                    candidate,
                    changed,
                },
            );
        }
    }
    pub fn cancel(&mut self) {
        self.started = None;
        self.candidate = None;
    }
    pub fn publish_current(&mut self) {
        self.published = self.latest;
    }
    pub fn publish_candidate(&mut self) {
        if let Some(candidate) = self.candidate.take() {
            self.published = Some(candidate);
        }
    }
    pub fn published(&self) -> Option<Published> {
        self.published
    }
    pub fn drain(&mut self) -> impl Iterator<Item = Event> + '_ {
        self.events.drain(..)
    }
}
#[test]
fn observations_are_bounded_and_confirmation_belongs_to_candidate() {
    let mut source = Source::default();
    source.frame(7, 0);
    source.begin();
    source.record(7, Kind::Pending);
    source.confirm(7, true);
    let events: Vec<_> = source.drain().collect();
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .all(|event| event.sequence == 1 && event.token == 7)
    );
    assert!(matches!(
        events[2].kind,
        Kind::Confirm { changed: true, .. }
    ));
    source.begin();
    source.cancel();
    source.confirm(7, false);
    assert_eq!(source.drain().count(), 0);
    for _ in 0..140 {
        source.frame(7, 0);
    }
    assert_eq!(source.dropped, 12);
    assert_eq!(source.drain().count(), 128);
}

#[test]
fn published_version_follows_frozen_texture_and_roi_policy() {
    let mut source = Source::default();
    source.frame(7, 100);
    source.publish_current();
    assert_eq!(source.published().unwrap().sequence, 1);
    source.frame(7, 200);
    source.begin();
    source.frame(7, 300);
    source.confirm(7, true);
    assert_eq!(source.published().unwrap().ticks, 200);
    let confirmation = source
        .drain()
        .find(|event| matches!(event.kind, Kind::Confirm { .. }))
        .unwrap();
    assert_eq!(
        confirmation.sequence, 2,
        "confirmation must not inherit the newer frame's sequence"
    );
    let Kind::Confirm {
        candidate, started, ..
    } = confirmation.kind
    else {
        unreachable!()
    };
    assert_eq!(
        candidate,
        Some(Published {
            token: 7,
            sequence: 2,
            ticks: 200
        })
    );
    assert!(started <= confirmation.at);
    source.begin();
    source.confirm(7, false);
    assert_eq!(source.published().unwrap().ticks, 200);
    // An equal ROI may still hide changed pixels outside it. Expanding the ROI
    // publishes that retained candidate, not the older texture version.
    source.publish_candidate();
    assert_eq!(source.published().unwrap().ticks, 300);
    source.frame(7, 400);
    source.begin();
    source.cancel();
    source.confirm(7, true);
    assert_eq!(source.published().unwrap().ticks, 300);
}

#[test]
#[ignore = "requires a Windows D3D11 hardware device"]
fn gpu_timing_identity_survives_cancel_and_later_frames() {
    let backend = super::capture::Backend::new().unwrap();
    let device = backend.device();
    let context = backend.context();
    let mut source = Source {
        gpu_enabled: true,
        ..Default::default()
    };
    for ticks in [100, 200] {
        source.frame(7, ticks);
        let candidate = source.latest.unwrap();
        let mut trace = source.start_gpu(device, context).unwrap().unwrap();
        trace.mark(context, 1);
        trace.mark(context, 2);
        source.finish_gpu(context, trace);
        unsafe {
            context.Flush();
        }
        source.begin();
        source.cancel();
        source.frame(7, ticks + 1);
        assert!(
            source.start_gpu(device, context).unwrap().is_none(),
            "later frame must not rebind pending timing"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while source.gpu_candidate.is_some() {
            source.poll_gpu(context).unwrap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let events: Vec<_> = source.drain().collect();
        assert!(events.iter().any(|event| matches!(event.kind, Kind::GpuComparison { candidate: actual, .. } if actual == candidate)));
        assert!(events.iter().any(|event| matches!(event.kind, Kind::GpuTimingSkipped { candidate: actual } if actual.ticks == ticks + 1)));
        assert!(source.published().is_none());
        assert_eq!(source.gpu.pending_count(), 0);
        assert_eq!(source.gpu.invalid, 0);
    }
}
