//! Opt-in timestamp evidence. Queries never block, flush, or enter release UI.
use super::d3d_query::data;
use std::{collections::VecDeque, time::Instant};
use windows::{Win32::Graphics::Direct3D11::*, core::Result};

const CAPACITY: usize = 8;

struct Queries {
    disjoint: ID3D11Query,
    points: [ID3D11Query; 6],
}
impl Queries {
    fn new(device: &ID3D11Device) -> Result<Self> {
        let create = |kind| -> Result<ID3D11Query> {
            let mut query = None;
            unsafe {
                device.CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: kind,
                        MiscFlags: 0,
                    },
                    Some(&mut query),
                )?;
            }
            Ok(query.unwrap())
        };
        Ok(Self {
            disjoint: create(D3D11_QUERY_TIMESTAMP_DISJOINT)?,
            points: [
                create(D3D11_QUERY_TIMESTAMP)?,
                create(D3D11_QUERY_TIMESTAMP)?,
                create(D3D11_QUERY_TIMESTAMP)?,
                create(D3D11_QUERY_TIMESTAMP)?,
                create(D3D11_QUERY_TIMESTAMP)?,
                create(D3D11_QUERY_TIMESTAMP)?,
            ],
        })
    }
}

pub(super) struct Trace {
    queries: Queries,
    at: Instant,
    submitted: Option<Instant>,
    marked: u8,
}
impl Trace {
    pub fn mark(&mut self, context: &ID3D11DeviceContext, point: usize) {
        unsafe {
            context.End(&self.queries.points[point]);
        }
        self.marked |= 1 << point;
    }
}

pub(super) struct Sample {
    pub at: Instant,
    pub stages_ms: [f64; 5],
    pub full_frame: bool,
    /// CPU time to observe completed query results, including poll cadence.
    /// This is not GPU queue delay or input-to-photon latency.
    pub observed_ms: f64,
}

#[derive(Default)]
pub(super) struct Profiler {
    free: Vec<Queries>,
    pending: VecDeque<Trace>,
    pub dropped: usize,
    pub invalid: usize,
}
impl Profiler {
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn begin(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Option<Trace>> {
        if self.pending.len() >= CAPACITY {
            self.dropped += 1;
            return Ok(None);
        }
        let queries = if let Some(queries) = self.free.pop() {
            queries
        } else {
            Queries::new(device)?
        };
        let mut trace = Trace {
            queries,
            at: Instant::now(),
            submitted: None,
            marked: 0,
        };
        unsafe {
            context.Begin(&trace.queries.disjoint);
        }
        trace.mark(context, 0);
        Ok(Some(trace))
    }
    pub fn finish(&mut self, context: &ID3D11DeviceContext, mut trace: Trace) {
        // Early exits have no effect/composition stage; mark the remaining
        // query objects once so they can all be safely recycled after completion.
        let marked = trace.marked;
        for point in 1..6 {
            if trace.marked & (1 << point) == 0 {
                trace.mark(context, point);
            }
        }
        trace.marked = marked;
        unsafe {
            context.End(&trace.queries.disjoint);
        }
        trace.submitted = Some(Instant::now());
        self.pending.push_back(trace);
    }
    pub fn poll(&mut self, context: &ID3D11DeviceContext) -> Result<Vec<Sample>> {
        let mut samples = Vec::new();
        while let Some(trace) = self.pending.front() {
            let Some(disjoint) =
                data::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>(context, &trace.queries.disjoint)?
            else {
                break;
            };
            let mut ticks = [0; 6];
            let mut complete = true;
            for (tick, query) in ticks.iter_mut().zip(&trace.queries.points) {
                if let Some(value) = data::<u64>(context, query)? {
                    *tick = value;
                } else {
                    complete = false;
                    break;
                }
            }
            if !complete {
                break;
            }
            let trace = self.pending.pop_front().unwrap();
            if let Some(stages_ms) = elapsed(disjoint, ticks) {
                samples.push(Sample {
                    at: trace.at,
                    stages_ms,
                    full_frame: trace.marked & (1 << 4) != 0,
                    observed_ms: trace.submitted.unwrap().elapsed().as_secs_f64() * 1000.,
                });
            } else {
                self.invalid += 1;
            }
            self.free.push(trace.queries);
        }
        Ok(samples)
    }
}

fn elapsed(clock: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT, ticks: [u64; 6]) -> Option<[f64; 5]> {
    if clock.Disjoint.as_bool()
        || clock.Frequency == 0
        || ticks.windows(2).any(|pair| pair[1] < pair[0])
    {
        return None;
    }
    Some(std::array::from_fn(|i| {
        (ticks[i + 1] - ticks[i]) as f64 * 1000. / clock.Frequency as f64
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disjoint_zero_frequency_and_reversed_counters_are_not_timing_samples() {
        let clock = |frequency, disjoint: bool| D3D11_QUERY_DATA_TIMESTAMP_DISJOINT {
            Frequency: frequency,
            Disjoint: disjoint.into(),
        };
        assert_eq!(
            elapsed(clock(1000, false), [10, 11, 13, 16, 20, 25]),
            Some([1., 2., 3., 4., 5.])
        );
        assert!(elapsed(clock(1000, true), [10, 11, 13, 16, 20, 25]).is_none());
        assert!(elapsed(clock(0, false), [10, 11, 13, 16, 20, 25]).is_none());
        assert!(elapsed(clock(1000, false), [10, 9, 13, 16, 20, 25]).is_none());
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn timestamp_queries_are_bounded_nonblocking_and_reusable() {
        use crate::windows::screen_capture::capture::Backend;
        use std::{thread, time::Duration};
        use windows::Win32::Graphics::Dxgi::Common::*;
        let backend = Backend::new().unwrap();
        let device = backend.device();
        let context = backend.context();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 16,
            Height: 16,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut output = None;
        let mut view = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut output))
                .unwrap();
            device
                .CreateRenderTargetView(output.as_ref().unwrap(), None, Some(&mut view))
                .unwrap();
        }
        let clear = || unsafe {
            context.ClearRenderTargetView(view.as_ref().unwrap(), &[0., 0., 0., 0.]);
        };
        let mut profiler = Profiler::default();
        for _ in 0..2 {
            for _ in 0..CAPACITY {
                let mut trace = profiler.begin(device, context).unwrap().unwrap();
                trace.mark(context, 1);
                clear();
                trace.mark(context, 2);
                clear();
                trace.mark(context, 3);
                clear();
                trace.mark(context, 4);
                profiler.finish(context, trace);
            }
            assert!(profiler.begin(device, context).unwrap().is_none());
            // Only this isolated verification submits a flush. The benchmark
            // relies on the production capture/presentation command stream.
            unsafe {
                context.Flush();
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut samples = Vec::new();
            while samples.len() < CAPACITY {
                samples.extend(profiler.poll(context).unwrap());
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert!(samples.iter().all(|sample| {
                sample.full_frame
                    && sample
                        .stages_ms
                        .iter()
                        .all(|time| time.is_finite() && *time >= 0.)
            }));
            assert!(profiler.pending.is_empty());
            assert_eq!(profiler.free.len(), CAPACITY);
        }
        assert_eq!(profiler.dropped, 2);
        assert_eq!(profiler.invalid, 0);
    }
}
