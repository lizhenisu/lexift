//! Opt-in, bounded first-500ms drag diagnostics. Never logs individual pointer events.
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

const LIMIT: usize = 128;
const WINDOW: Duration = Duration::from_millis(500);
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Input,
    Compile,
    Raster,
    Submit,
    Confirm,
    Render,
}
impl Phase {
    fn index(self) -> usize {
        self as usize
    }
}
struct Drag {
    at: Instant,
    last_input: Instant,
    last_frame: Option<Instant>,
    generation: u64,
    canvas: usize,
    objects: usize,
    phases: [Vec<f64>; 6],
    gaps: Vec<f64>,
    frames: Vec<f64>,
}
thread_local! { static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) }; }
fn push(samples: &mut Vec<f64>, duration: Duration) {
    if samples.len() < LIMIT {
        samples.push(duration.as_secs_f64() * 1000.);
    }
}
fn stats(samples: &[f64]) -> [f64; 4] {
    if samples.is_empty() {
        return [0.; 4];
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    [0.5, 0.95, 0.99, 1.]
        .map(|p| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)])
}
pub(super) struct Span {
    phase: Phase,
    at: Instant,
    generation: u64,
    finish: bool,
}
impl Span {
    pub fn new(phase: Phase) -> Option<Self> {
        DRAG.with(|slot| {
            let slot = slot.borrow();
            let drag = slot.as_ref()?;
            (drag.at.elapsed() <= WINDOW).then(|| Self {
                phase,
                at: Instant::now(),
                generation: drag.generation,
                finish: false,
            })
        })
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        DRAG.with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return;
            };
            let Some(drag) = slot.as_mut().filter(|d| d.generation == self.generation) else {
                return;
            };
            if self.at.duration_since(drag.at) <= WINDOW {
                push(&mut drag.phases[self.phase.index()], self.at.elapsed());
            }
            if matches!(self.phase, Phase::Render) && drag.at.elapsed() <= WINDOW {
                let now = Instant::now();
                if let Some(previous) = drag.last_frame {
                    push(&mut drag.frames, now.duration_since(previous));
                }
                drag.last_frame = Some(now);
            }
            if self.finish {
                report(slot.take().unwrap());
            }
        });
    }
}
fn report(drag: Drag) {
    tracing::debug!(target: "lexift_ui::annotation_drag_timing",
        session = drag.generation, canvas = drag.canvas, objects = drag.objects,
        samples = drag.phases[0].len(),
        input_ms = ?stats(&drag.phases[0]), compile_ms = ?stats(&drag.phases[1]),
        raster_ms = ?stats(&drag.phases[2]), submit_ms = ?stats(&drag.phases[3]),
        confirm_ms = ?stats(&drag.phases[4]), render_ms = ?stats(&drag.phases[5]),
        input_gap_ms = ?stats(&drag.gaps), frame_gap_ms = ?stats(&drag.frames),
        "Annotation drag first 500ms; statistics=[median,p95,p99,max], CPU confirmation is not photon latency");
}
pub(super) fn pointer(event: i32, generation: u64, canvas: usize, objects: usize) -> Option<Span> {
    if event == 0
        && tracing::enabled!(target: "lexift_ui::annotation_drag_timing", tracing::Level::DEBUG)
    {
        let now = Instant::now();
        DRAG.with(|slot| {
            *slot.borrow_mut() = Some(Drag {
                at: now,
                last_input: now,
                last_frame: None,
                generation,
                canvas,
                objects,
                phases: std::array::from_fn(|_| Vec::with_capacity(LIMIT)),
                gaps: Vec::with_capacity(LIMIT),
                frames: Vec::with_capacity(LIMIT),
            })
        });
    }
    DRAG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let drag = slot.as_mut().filter(|d| d.generation == generation)?;
        let now = Instant::now();
        if event == 1 && now.duration_since(drag.at) <= WINDOW {
            push(&mut drag.gaps, now.duration_since(drag.last_input));
            drag.last_input = now;
        }
        (now.duration_since(drag.at) <= WINDOW || matches!(event, 2 | 3)).then(|| Span {
            phase: Phase::Input,
            at: now,
            generation,
            finish: matches!(event, 2 | 3),
        })
    })
}
pub(super) fn clear() {
    // Registry teardown can run after this TLS slot's destructor on thread exit.
    let _ = DRAG.try_with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_retirement_after_diagnostic_tls_destruction_is_safe() {
        struct Owner;
        impl Drop for Owner {
            fn drop(&mut self) {
                clear();
            }
        }
        thread_local! { static OWNER: Owner = const { Owner }; }
        std::thread::spawn(|| {
            OWNER.with(|_| {});
            DRAG.with(|_| {}); // Initialized last, destroyed before its owner.
        })
        .join()
        .unwrap();
    }
    #[test]
    fn bounded_samples_and_percentiles_are_explicit() {
        let mut samples = Vec::new();
        for value in 1..=200 {
            push(&mut samples, Duration::from_millis(value));
        }
        assert_eq!(samples.len(), LIMIT);
        assert_eq!(stats(&samples), [64., 122., 127., 128.]);
        assert_eq!(stats(&[]), [0.; 4]);
        clear();
        assert!(Span::new(Phase::Render).is_none());
    }
    #[test]
    fn expired_sampling_and_reentrant_retirement_do_not_affect_input() {
        let at = Instant::now() - Duration::from_secs(1);
        DRAG.with(|slot| {
            *slot.borrow_mut() = Some(Drag {
                at,
                last_input: at,
                last_frame: None,
                generation: 7,
                canvas: 0,
                objects: 0,
                phases: std::array::from_fn(|_| Vec::new()),
                gaps: Vec::new(),
                frames: Vec::new(),
            })
        });
        assert!(pointer(1, 7, 0, 0).is_none());
        let terminal = pointer(2, 7, 0, 0).unwrap();
        clear(); // A window can retire synchronously inside its input handler.
        drop(terminal);
        assert!(Span::new(Phase::Render).is_none());
    }
}
