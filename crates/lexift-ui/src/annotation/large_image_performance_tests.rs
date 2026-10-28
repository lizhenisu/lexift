//! Explicit Release benchmark; fixtures contain no user pixels.
use super::*;
fn fixture(width: i32, height: i32) -> Input {
    let bounds = Rect {
        left: -31,
        top: 17,
        right: width - 31,
        bottom: height + 17,
    };
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let a = if (x + y) % 19 == 0 { 96 } else { 255 };
            pixels.extend_from_slice(&[
                ((x * 13 + y * 3) % (a + 1)) as u8,
                ((x + y * 7) % (a + 1)) as u8,
                ((x * 5 + y) % (a + 1)) as u8,
                a as u8,
            ]);
        }
    }
    Input {
        snapshot: ExportSnapshot {
            session: Arc::new(Session::new()),
            monitor: bounds,
            scale: 1.,
            monitors: vec![(bounds, 1.)],
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        },
        capture: Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![lexift_core::ports::screen_capture::CapturedRegion { bounds, pixels }],
        }),
        crop: bounds,
        label: None,
        help: None,
        picker: None,
        hovered: None,
        crop_handles: false,
        crop_border: false,
        erase: None,
    }
}
#[test]
#[ignore = "Explicit same-machine Release performance matrix"]
fn large_image_matrix() {
    for (w, h) in [
        (1000, 1000),
        (2000, 4000),
        (4000, 5000),
        (5000, 8000),
        (600, 32768),
    ] {
        for half in [false, true] {
            let mut input = fixture(w, h);
            if half {
                Arc::make_mut(&mut input.snapshot.session).image_scale =
                    lexift_core::domain::image_resize::ImageScale::new(1, 2).unwrap();
            }
            let cancel = AtomicBool::new(false);
            let mut cache = Cache::default();
            let mut timings = Vec::new();
            let mut checksum = 0u64;
            for round in 0..21 {
                let started = Instant::now();
                let frames = paint_frames(&input, &cancel, &mut cache).unwrap();
                let ms = started.elapsed().as_secs_f64() * 1000.;
                if round > 0 {
                    timings.push(ms);
                }
                checksum = frames[0]
                    .data()
                    .iter()
                    .step_by(97)
                    .fold(0u64, |a, &b| a.wrapping_mul(31).wrapping_add(u64::from(b)));
                std::hint::black_box(frames);
            }
            timings.sort_by(f64::total_cmp);
            println!(
                "LARGE_IMAGE w={w} h={h} half={half} samples=20 median_ms={:.3} p95_ms={:.3} checksum={checksum}",
                timings[10], timings[18]
            );
        }
    }
}

#[test]
fn content_cache_ignores_feedback_but_invalidates_every_pixel_transform() {
    let mut input = fixture(37, 41);
    let cancel = AtomicBool::new(false);
    let mut cache = Cache::default();
    let first = render_frames(&input, &cancel, &mut cache).unwrap();
    let source = Arc::downgrade(&input.capture);
    input.hovered = Some(0);
    let next = render_frames(&input, &cancel, &mut cache).unwrap();
    assert!(Arc::ptr_eq(&first[0], &next[0]));
    assert!(!Arc::ptr_eq(&first[1], &next[1]));
    let mut previous = next[0].clone();
    for n in 0..6 {
        let session = Arc::make_mut(&mut input.snapshot.session);
        match n {
            0 => session.image_color.inverted = true,
            1 => {
                session.image_scale =
                    lexift_core::domain::image_resize::ImageScale::new(1, 2).unwrap()
            }
            2 => {
                session.image_crop = Some(Rect {
                    right: input.crop.right - 2,
                    ..input.crop
                })
            }
            3 => {
                session.image_orientation = session
                    .image_orientation
                    .action(lexift_core::domain::image_transform::Action::Right)
            }
            4 => input.capture = Arc::new((*input.capture).clone()),
            _ => input.snapshot.scale = 1.25,
        }
        let result = render_frames(&input, &cancel, &mut cache).unwrap();
        assert!(!Arc::ptr_eq(&previous, &result[0]));
        let reference = render_frames(&input, &cancel, &mut Cache::default()).unwrap();
        assert_eq!(result[0].data(), reference[0].data());
        assert_eq!(result[1].data(), reference[1].data());
        previous = result[0].clone();
    }
    drop(first);
    drop(previous);
    drop(next);
    drop(input);
    drop(cache);
    assert!(source.upgrade().is_none());
}
fn job(revision: u64) -> Job {
    Job {
        submitted: Instant::now(),
        generation: 1,
        session: 2,
        revision,
        inputs: vec![fixture(16, 16)],
        canvases: vec![3],
    }
}
#[test]
fn retirement_cancels_only_owned_work_and_normal_feedback_makes_progress() {
    let mut state = QueueState::default();
    let running = Arc::new(AtomicBool::new(false));
    state.running = Some((4, running.clone()));
    state.pending = Some(job(5));
    state.pending = Some(job(6));
    assert!(!running.load(Ordering::Acquire));
    assert_eq!(state.pending.as_ref().unwrap().revision, 6);
    state.retire_before(5);
    assert!(running.load(Ordering::Acquire));
    assert_eq!(state.pending.as_ref().unwrap().revision, 6);
    state.retire_before(7);
    assert!(state.pending.is_none());
    let current = Arc::new(AtomicBool::new(false));
    state.running = Some((7, current.clone()));
    state.retire_before(7);
    assert!(!current.load(Ordering::Acquire));
    state.stop();
    assert!(current.load(Ordering::Acquire));
    assert!(state.stopped);
}
#[test]
fn preview_workers_and_sources_release_after_twenty_close_reopen_rounds() {
    for _ in 0..20 {
        let preview = Preview::new().unwrap();
        let queue = Arc::downgrade(&preview.queue);
        let input = job(1);
        let source = Arc::downgrade(&input.inputs[0].capture);
        preview.submit(input);
        drop(preview);
        let until = Instant::now() + Duration::from_secs(5);
        while queue.upgrade().is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(queue.upgrade().is_none());
        assert!(source.upgrade().is_none());
    }
}

#[test]
#[ignore = "Release cancellation latency measurement, not native call latency"]
fn running_resample_cancellation_latency() {
    let image = fixture(4000, 5000).capture.regions[0].clone();
    let mut samples = Vec::new();
    for _ in 0..20 {
        let source = image.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_started = started.clone();
        let worker = std::thread::spawn(move || {
            worker_started.store(true, Ordering::Release);
            lexift_core::domain::image_resize::ImageScale::new(1, 2)
                .unwrap()
                .pixels(source, &worker_cancel)
        });
        while !started.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(2));
        let time = Instant::now();
        cancel.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        samples.push(time.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "CANCEL_CPU samples=20 median_ms={:.3} p95_ms={:.3}",
        samples[10], samples[18]
    );
    assert!(samples[18] < 100.);
}

#[test]
fn recomputable_content_cache_has_a_fixed_bound_without_rejecting_large_images() {
    let input = fixture(3000, 3000);
    let cancel = AtomicBool::new(false);
    let mut cache = Cache::default();
    let frames = render_frames(&input, &cancel, &mut cache).unwrap();
    assert!(frames[0].data().len() > MAX_CACHED_IMAGE_BYTES);
    assert!(cache.image.is_none());
    assert_eq!(frames[0].data(), input.capture.regions[0].pixels);
}
