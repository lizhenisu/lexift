//! Owned static desktop fixture for scene movement, independent of source paints.
//! Port submission and OS sync exclude UI input, scene compilation and photon time.
use super::super::display_timing::{
    Observation, latency_bounds, match_commits_where, presented_window,
};
use super::*;

#[derive(Default)]
struct MotionSamples {
    stamps: HashSet<RenderStamp>,
    lower: Vec<f64>,
    upper: Vec<f64>,
    invalid: usize,
    rejected_clock_observations: usize,
    recovered_counts: usize,
    invalid_examples: Vec<String>,
}

/// Join an exact displayed counter to its authorized scene and request. Skipped
/// counters never acknowledge intermediate revisions; invalid clocks stay invalid.
fn motion_samples(
    observations: &[Observation],
    commits: &[Presentation],
    requests: &HashMap<RenderStamp, Instant>,
) -> MotionSamples {
    let mut result = MotionSamples::default();
    let mut rejected_counts = HashSet::new();
    let matched = match_commits_where(observations, commits, |observation, commit| {
        if !requests.contains_key(&commit.stamp) {
            return false;
        }
        if presented_window(observation).is_none() {
            result.rejected_clock_observations += 1;
            rejected_counts.insert(observation.count);
            return false;
        }
        true
    });
    for (observation, commit) in matched {
        let Some(&requested) = requests.get(&commit.stamp) else {
            continue;
        };
        if result.stamps.contains(&commit.stamp) {
            continue;
        }
        let bounds = latency_bounds(observation, requested);
        if commit.counts.len() != 1 || commit.at < requested || bounds.is_none() {
            result.invalid += 1;
            if result.invalid_examples.len() < 3 {
                result.invalid_examples.push(format!("stamp={:?} count={:?} commit_before_request={} surface_counts={} valid_sync_window={} sync_before_request={} refresh=({},{}) qpc={} frequency={} anchor_present={}",
                    commit.stamp, observation.count, commit.at < requested, commit.counts.len(),
                    super::super::display_timing::presented_window(observation).is_some(),
                    super::super::display_timing::presented_window(observation).is_some_and(|(before, _)| before < requested),
                    observation.present_refresh, observation.sync_refresh, observation.qpc, observation.frequency, observation.clock.is_some()));
            }
            continue;
        }
        let (lower, upper) = bounds.unwrap();
        result.recovered_counts += usize::from(rejected_counts.remove(&observation.count));
        result.stamps.insert(commit.stamp);
        result.lower.push(lower);
        result.upper.push(upper);
    }
    result
}

fn marker_color(revision: u64) -> [u8; 4] {
    [(revision % 251) as u8, 100, 180, 255]
}

/// Translating immutable masks preserves their raster identity and dimensions.
/// A separate marker makes every requested scene visibly distinct.
fn moving_scene(initial: &AnnotationScene, revision: u64) -> AnnotationScene {
    let view = initial.surfaces[0].bounds;
    let mut updated = initial.clone();
    updated.stamp.revision = revision;
    let mut layer = match &initial.layers[0] {
        RenderLayer::Mosaic(layer) => layer.clone(),
        _ => unreachable!(),
    };
    let left = view.left + 40 + (revision % 200) as i32;
    let top = view.top + 80;
    let bounds = Rect {
        left,
        top,
        right: left + 960,
        bottom: top + 480,
    };
    let mut object = (*layer.object).clone();
    object.bounds = Bounds::from_corners(
        (left as f32, top as f32),
        (bounds.right as f32, bounds.bottom as f32),
    );
    layer.object = Arc::new(object);
    let mut mask = layer.masks[0].clone();
    mask.bounds = bounds;
    layer.masks = vec![mask].into();
    updated.layers = vec![RenderLayer::Mosaic(layer)].into();
    updated.interaction = vec![RasterLayer {
        key: RasterKey { asset: 2, revision },
        bounds: Rect {
            left: view.left + 8,
            top: view.top + 8,
            right: view.left + 40,
            bottom: view.top + 40,
        },
        pixels: marker_color(revision).repeat(32 * 32).into(),
    }]
    .into();
    updated
}

#[test]
fn motion_latency_joins_scene_stamp_and_rejects_skipped_counts_and_bad_clocks() {
    use super::super::display_timing::{ClockAnchor, Count};
    let at = Instant::now();
    let stamp = |revision| RenderStamp {
        session: 10,
        revision,
    };
    let count = |present| Count { chain: 1, present };
    let commit = |revision| Presentation {
        stamp: stamp(revision),
        native_commit: None,
        counts: vec![count(revision as u32)],
        at: at + Duration::from_millis(7),
        sources: Vec::new(),
    };
    let commits = [commit(1), commit(2), commit(3)];
    let requests = HashMap::from([(stamp(1), at), (stamp(2), at), (stamp(3), at)]);
    let observation = |present| Observation {
        count: count(present),
        frequency: 1000,
        at: at + Duration::from_millis(25),
        qpc: 995,
        present_refresh: 10,
        sync_refresh: 10,
        clock: Some(ClockAnchor {
            before: at + Duration::from_millis(20),
            after: at + Duration::from_millis(21),
            qpc: 1000,
        }),
    };
    let observations = [observation(1), observation(3)];
    let samples = motion_samples(&observations, &commits, &requests);
    assert_eq!(samples.stamps, HashSet::from([stamp(1), stamp(3)]));
    assert_eq!(samples.lower, vec![15., 15.]);
    assert_eq!(samples.upper, vec![16., 16.]);
    let mut invalid = observation(1);
    invalid.clock = None;
    let unavailable = motion_samples(std::slice::from_ref(&invalid), &commits, &requests);
    assert_eq!(unavailable.invalid, 0);
    assert_eq!(unavailable.rejected_clock_observations, 1);
    let recovered = motion_samples(&[invalid, observation(1)], &commits, &requests);
    assert_eq!(recovered.stamps, HashSet::from([stamp(1)]));
    assert_eq!(recovered.recovered_counts, 1);
    assert_eq!(recovered.rejected_clock_observations, 1);
    let future = HashMap::from([(stamp(1), at + Duration::from_millis(30))]);
    assert_eq!(motion_samples(&observations, &commits, &future).invalid, 1);
    let duplicate = [commit(1), commit(1)];
    assert!(
        motion_samples(&observations, &duplicate, &requests)
            .stamps
            .is_empty()
    );
    let other_session = HashMap::from([(
        RenderStamp {
            session: 11,
            revision: 1,
        },
        at,
    )]);
    assert!(
        motion_samples(&observations, &commits, &other_session)
            .stamps
            .is_empty()
    );
}

#[test]
#[ignore = "owned moving-scene OS sync fixture; requires Release, WGC and unobstructed desktop"]
fn gpu_static_source_moving_blur_request_to_os_sync() {
    if cfg!(debug_assertions) {
        panic!("run with --release");
    }
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    let source_mode = monitor_mode(source.0);
    let input_mode = monitor_mode(input.0);
    assert!(
        source_mode.is_ok() && source_mode == input_mode,
        "fixture windows require one reported output"
    );
    let source_dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(source.0) };
    let input_dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(input.0) };
    let source_frame = SourceFrame::new(source.0);
    source_frame.paint(37);
    source_frame.verify(37);
    drop(bootstrap);
    let port = WindowsAnnotationRenderPort::new().unwrap();
    let mut initial = scene(bounds, input.0, 0);
    let RenderLayer::Mosaic(layer) = &mut Arc::make_mut(&mut initial.layers)[0] else {
        unreachable!()
    };
    Arc::make_mut(&mut layer.masks)[0].pixels = vec![255; 960 * 480 * 4].into();
    let warmup = moving_scene(&initial, 1);
    warmup.validate().unwrap();
    port.submit(Arc::new(warmup)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        surface::pump_messages();
        if port.latest().unwrap().is_some_and(|frame| frame.presented) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "moving fixture failed to initialize"
        );
        thread::sleep(Duration::from_millis(1));
    }
    port.shared.0.lock().unwrap().record_presentations = true;
    let mut requests = HashMap::new();
    let period = Duration::from_secs_f64(1. / 60.);
    let start = Instant::now();
    let mut next = start;
    for revision in 2..=241 {
        while Instant::now() < next {
            surface::pump_messages();
            port.latest().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let updated = moving_scene(&initial, revision);
        updated.validate().unwrap();
        assert!(requests.insert(updated.stamp, Instant::now()).is_none());
        port.submit(Arc::new(updated)).unwrap();
        next += period;
    }
    let drain = Instant::now() + Duration::from_millis(300);
    while Instant::now() < drain {
        surface::pump_messages();
        port.latest().unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    let (commits, observations, dropped) = {
        let mut state = port.shared.0.lock().unwrap();
        state.record_presentations = false;
        (
            std::mem::take(&mut state.source_presentations),
            std::mem::take(&mut state.display_observations),
            state.source_presentations_dropped + state.display_observations_dropped,
        )
    };
    let samples = motion_samples(&observations, &commits, &requests);
    assert_eq!(
        source_mode,
        monitor_mode(source.0),
        "source output mode changed during run"
    );
    assert_eq!(
        input_mode,
        monitor_mode(input.0),
        "input output mode changed during run"
    );
    let mut environment = String::from(
        "motion_environment_scope=reported_window_output_and_dpi_not_measured_refresh_or_cross_screen_coverage\n",
    );
    append_monitor_mode(&mut environment, "motion_source", &source_mode);
    append_monitor_mode(&mut environment, "motion_input", &input_mode);
    println!(
        "{environment}motion_source_dpi={source_dpi}\nmotion_input_dpi={input_dpi}\nmotion_scene_scale=1\nmotion_view_pixels=1280x720\nmotion_mask_pixels=960x480\nmotion_requested_rate_hz=60\nmotion_effect=blur\nmotion_strength=100"
    );
    let metric = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values.get(values.len() * 95 / 100).copied()
    };
    println!(
        "motion_scope=owned_static_source_port_request_to_same_scene_os_sync_not_ui_input_compile_full_frame_or_photon requests={} matched={} unobserved={} invalid={} dropped={} request_to_sync_lower_p95_ms={:?} request_to_sync_upper_p95_ms={:?}",
        requests.len(),
        samples.stamps.len(),
        requests.len() - samples.stamps.len(),
        samples.invalid,
        dropped,
        metric(samples.lower),
        metric(samples.upper)
    );
    assert_eq!(dropped, 0);
    println!("motion_invalid_examples={:?}", samples.invalid_examples);
    println!(
        "motion_rejected_clock_observations={} motion_recovered_counts={} scope=unusable_snapshot_retry_same_native_count_not_relaxed_clock_conversion",
        samples.rejected_clock_observations, samples.recovered_counts
    );
    assert_eq!(samples.invalid, 0);
    assert!(
        samples.stamps.len() >= 120,
        "too few displayed versions to characterize movement"
    );
    assert!(
        samples.stamps.contains(&RenderStamp {
            session: initial.stamp.session,
            revision: 241
        }),
        "final request was not matched to OS sync"
    );
    // Read only after the timed loop; this is final desktop marker correctness,
    // not a per-frame readback or physical display-latency measurement.
    let marker = marker_color(241);
    let actual = unsafe {
        let dc = GetDC(None);
        assert!(!dc.is_invalid());
        let color = GetPixel(dc, bounds.left + 16, bounds.top + 16);
        ReleaseDC(None, dc);
        [color.0 as u8, (color.0 >> 8) as u8, (color.0 >> 16) as u8]
    };
    assert_eq!(actual, [marker[0], marker[1], marker[2]]);
    println!(
        "motion_final_desktop_marker_revision=241 exact_match=true sampled_pixels=1 not_full_frame=true"
    );
    port.clear();
    drop(port);
}
