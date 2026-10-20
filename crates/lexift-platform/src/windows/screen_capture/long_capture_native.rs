//! Interactive gate: real changing HWND pixels through the production continuous capture adapter.
use super::*;
use lexift_core::{
    domain::{
        geometry::Rect,
        long_capture::{Feedback, Stitcher},
    },
    ports::screen_capture::ScreenCapturePort,
};
use std::{path::PathBuf, sync::atomic::AtomicBool};

#[test]
#[ignore = "requires the owned target/screenshot-long-capture/fixture.ps1 scene"]
fn long_capture_native_pixels_register_without_missing_or_duplicate_rows() {
    let folder =
        PathBuf::from(std::env::var("LEXIFT_LONG_FIXTURE_DIR").expect("owned fixture directory"));
    let expected = std::fs::read(folder.join("source.rgba")).unwrap();
    let port = WindowsScreenCapturePort::new().unwrap();
    port.request(ScreenCaptureRequest {
        revision: 941,
        regions: vec![Rect {
            left: 300,
            top: 200,
            right: 900,
            bottom: 600,
        }],
        paused_regions: vec![],
        excluded_windows: vec![],
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    let mut stitch = None::<Stitcher>;
    let mut last = 0;
    for y in [
        0usize, 1, 31, 131, 260, 360, 560, 760, 960, 1160, 1360, 1560, 1760, 1960, 2000,
    ] {
        std::fs::write(folder.join("position.txt"), y.to_string()).unwrap();
        let start = Instant::now();
        let frame = loop {
            if let Ok(Some(f)) = port.latest()
                && f.serial != last
                && f.regions[0].pixels == expected[y * 600 * 4..(y + 400) * 600 * 4]
            {
                break f;
            }
            assert!(
                start.elapsed() < Duration::from_secs(6),
                "real capture never reached physical row {y}: {:?}",
                port.latest().err()
            );
            thread::sleep(Duration::from_millis(20));
        };
        last = frame.serial;
        let image = Arc::new(frame.regions[0].clone());
        let at = Instant::now();
        if let Some(s) = &mut stitch {
            assert_eq!(
                s.push(image, &cancel).unwrap(),
                Feedback::Accepted,
                "offset {y}"
            );
        } else {
            stitch = Some(Stitcher::new(image).unwrap());
        }
        let output = stitch.as_ref().unwrap().flatten(&cancel).unwrap();
        assert_eq!(
            output.pixels,
            expected[..(y + 400) * 600 * 4],
            "real pixel mismatch at offset {y}"
        );
        println!(
            "row={y} observed_capture_ms={:.2} registration_and_validation_ms={:.2}",
            start.elapsed().as_secs_f64() * 1000.,
            at.elapsed().as_secs_f64() * 1000.
        );
    }
    port.clear();
    std::fs::write(
        folder.join("native-gate.rgba"),
        stitch.unwrap().flatten(&cancel).unwrap().pixels,
    )
    .unwrap();
}
