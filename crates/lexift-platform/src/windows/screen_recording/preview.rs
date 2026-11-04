//! Media Engine and GPU objects are owned exclusively by the playback thread.
mod blit;
mod engine;
mod surface;
mod worker;
pub(super) use worker::Worker;
pub(crate) use worker::position;
#[cfg(test)]
mod audio_test;
#[cfg(test)]
mod resource_test;

#[cfg(test)]
mod tests {
    use super::engine::Engine;
    use std::time::{Duration, Instant};
    use windows::Win32::{Graphics::Dwm::DwmFlush, System::Com::*};

    #[test]
    #[ignore = "native playback gate; LEXIFT_PREVIEW_GATE_FILE and output directory required"]
    fn native_preview_engine_gate() {
        let source = std::env::var("LEXIFT_PREVIEW_GATE_FILE").unwrap();
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let mut player = Engine::open(std::path::Path::new(&source)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut first = None;
        while first.is_none() && Instant::now() < deadline {
            player.check_events().unwrap();
            first = player.frame().unwrap();
            unsafe { DwmFlush().unwrap() };
        }
        assert!(first.is_some(), "No paused first frame");
        assert!(unsafe { player.media.IsPaused().as_bool() });
        assert!(unsafe { player.media.GetCurrentTime() } < 0.1);
        let before = player.read_pixel(10, 500).unwrap();
        unsafe { player.media.Play().unwrap() };
        let until = Instant::now() + Duration::from_millis(600);
        let mut frames = 0;
        while Instant::now() < until {
            player.check_events().unwrap();
            frames += usize::from(player.frame().unwrap().is_some());
            unsafe { DwmFlush().unwrap() };
        }
        assert!(frames >= 10, "Playback did not advance: {frames}");
        unsafe { player.media.Pause().unwrap() };
        let target = unsafe { player.media.GetDuration() } * 0.5;
        unsafe { player.media.SetCurrentTime(target).unwrap() };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seeked = None;
        while seeked.is_none() && Instant::now() < deadline {
            player.check_events().unwrap();
            seeked = player
                .frame()
                .unwrap()
                .filter(|p| (*p - target).abs() < 0.1);
            unsafe { DwmFlush().unwrap() };
        }
        assert!(seeked.is_some(), "Seek did not present target frame");
        let points: Vec<_> = (0..12).map(|bit| (710 + bit * 24, 120)).collect();
        let middle = player.read_pixels(&points).unwrap();
        let tail_target = (unsafe { player.media.GetDuration() } - 0.05).max(0.);
        unsafe {
            player.media.SetCurrentTime(tail_target).unwrap();
        }
        let limit = Instant::now() + Duration::from_secs(3);
        let mut tail = None;
        while tail.is_none() && Instant::now() < limit {
            player.check_events().unwrap();
            tail = player
                .frame()
                .unwrap()
                .filter(|p| (*p - tail_target).abs() < 0.1);
            unsafe { DwmFlush().unwrap() };
        }
        assert!(tail.is_some(), "Tail seek did not present");
        let tail_pixels = player.read_pixels(&points).unwrap();
        eprintln!(
            "Preview frame oracle: {}",
            serde_json::json!({"middle_pts":seeked,"middle_bgra":middle,"tail_pts":tail,"tail_bgra":tail_pixels})
        );
        eprintln!(
            "Preview gate: first={first:?} frames={frames} seek={seeked:?} background={before:?} audio={}",
            unsafe { player.media.HasAudio().as_bool() }
        );
        drop(player);
        // An open decoder must not retain this private copy after Shutdown.
        let copy = std::path::Path::new(&source).with_extension("retirement.mp4");
        std::fs::copy(&source, &copy).unwrap();
        let player = Engine::open(&copy).unwrap();
        drop(player);
        std::fs::remove_file(copy).unwrap();
        unsafe { CoUninitialize() };
    }
}
