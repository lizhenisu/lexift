//! PP-OCRv5 preprocessing, DB regions and CTC decoding on the CPU.
use image::{
    RgbImage,
    imageops::{self, FilterType},
};
use lexift_core::{Error, Result, ports::screen_capture::CapturedRegion};
use ort::{session::Session, value::Tensor};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn error(e: impl std::fmt::Display) -> Error {
    Error::new(format!("OCR recognition: {e}"))
}
fn cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Acquire) {
        Err(error("cancelled"))
    } else {
        Ok(())
    }
}

pub(super) struct Engine {
    folder: PathBuf,
    detector: Session,
    readers: HashMap<&'static str, (Session, Vec<String>)>,
}
impl Engine {
    pub fn new(folder: &Path) -> Result<Self> {
        let dll = folder
            .join("onnxruntime.dll")
            .canonicalize()
            .map_err(error)?;
        ort::init_from(dll.to_string_lossy())
            .with_name("Lexift OCR")
            .commit()
            .map_err(error)?;
        Ok(Self {
            folder: folder.into(),
            detector: session(&folder.join("det.onnx"))?,
            readers: HashMap::new(),
        })
    }
    pub fn recognize(
        &mut self,
        input: CapturedRegion,
        language: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        cancelled(cancel)?;
        let w = (input.bounds.right - input.bounds.left) as u32;
        let h = (input.bounds.bottom - input.bounds.top) as u32;
        if w == 0
            || h == 0
            || u64::from(w) * u64::from(h) > 40_000_000
            || input.pixels.len() != w as usize * h as usize * 4
        {
            return Err(error("invalid image"));
        }
        let rgb: Vec<u8> = input
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let image = RgbImage::from_raw(w, h, rgb).ok_or_else(|| error("invalid image"))?;
        let ratio = (960. / w.max(h) as f32).min(1.);
        let dw = ((w as f32 * ratio / 32.).round() as u32 * 32).max(32);
        let dh = ((h as f32 * ratio / 32.).round() as u32 * 32).max(32);
        let resized = imageops::resize(&image, dw, dh, FilterType::Triangle);
        let tensor = Tensor::from_array((
            [1usize, 3, dh as usize, dw as usize],
            normalize(&resized, true),
        ))
        .map_err(error)?;
        let outputs = self.detector.run(ort::inputs![tensor]).map_err(error)?;
        let (shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
        if shape.len() != 4
            || shape[0] != 1
            || shape[1] != 1
            || shape[2] <= 0
            || shape[3] <= 0
            || shape[2].checked_mul(shape[3]) != Some(map.len() as i64)
        {
            return Err(error("invalid detection output"));
        }
        let mh = shape[shape.len() - 2] as usize;
        let mw = shape[shape.len() - 1] as usize;
        let boxes = regions(map, mw, mh);
        drop(outputs);
        let key = lexift_core::domain::ocr::model_for(language);
        if !self.readers.contains_key(key) {
            let mut dict = vec![String::new()];
            dict.extend(
                std::fs::read_to_string(self.folder.join(format!("{key}.txt")))
                    .map_err(error)?
                    .lines()
                    .map(str::to_owned),
            );
            if dict.last().is_none_or(|s| s != " ") {
                dict.push(" ".into());
            }
            self.readers.insert(
                key,
                (session(&self.folder.join(format!("{key}.onnx")))?, dict),
            );
        }
        let (reader, dict) = self.readers.get_mut(key).unwrap();
        let mut lines = Vec::new();
        for b in boxes {
            cancelled(cancel)?;
            let left = (b.0 as f32 / mw as f32 * w as f32).floor().max(0.) as u32;
            let top = (b.1 as f32 / mh as f32 * h as f32).floor().max(0.) as u32;
            let right = (b.2 as f32 / mw as f32 * w as f32).ceil().min(w as f32) as u32;
            let bottom = (b.3 as f32 / mh as f32 * h as f32).ceil().min(h as f32) as u32;
            if right <= left || bottom <= top {
                continue;
            }
            let crop = imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image();
            let width =
                ((crop.width() as f32 / crop.height() as f32 * 48.).ceil() as u32).clamp(8, 3200);
            let crop = imageops::resize(&crop, width, 48, FilterType::Triangle);
            let tensor =
                Tensor::from_array(([1usize, 3, 48, width as usize], normalize(&crop, false)))
                    .map_err(error)?;
            let outputs = reader.run(ort::inputs![tensor]).map_err(error)?;
            let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
            let classes = *shape
                .last()
                .ok_or_else(|| error("invalid recognition output"))?
                as usize;
            if classes != dict.len() {
                return Err(error("model dictionary mismatch"));
            }
            let text = decode(values, classes, dict);
            if !text.trim().is_empty() {
                lines.push(text);
            }
        }
        cancelled(cancel)?;
        Ok(lines.join("\n"))
    }
}
fn session(path: &Path) -> Result<Session> {
    Session::builder()
        .map_err(error)?
        .with_intra_threads(2)
        .map_err(error)?
        .commit_from_file(path)
        .map_err(error)
}
fn normalize(image: &RgbImage, detection: bool) -> Vec<f32> {
    let n = image.width() as usize * image.height() as usize;
    let mut output = vec![0.; n * 3];
    for (i, p) in image.pixels().enumerate() {
        for c in 0..3 {
            output[c * n + i] = if detection {
                (p[c] as f32 / 255. - [0.485, 0.456, 0.406][c]) / [0.229, 0.224, 0.225][c]
            } else {
                (p[c] as f32 / 255. - 0.5) / 0.5
            };
        }
    }
    output
}
fn decode(values: &[f32], classes: usize, dict: &[String]) -> String {
    let mut output = String::new();
    let mut previous = usize::MAX;
    for row in values.chunks_exact(classes) {
        let (index, _) = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        if index != 0 && index != previous {
            output.push_str(&dict[index]);
        }
        previous = index;
    }
    output
}
/// Connected DB regions are expanded before mapping back to original image pixels.
fn regions(map: &[f32], width: usize, height: usize) -> Vec<(usize, usize, usize, usize)> {
    let mut seen = vec![false; width * height];
    let mut output = Vec::new();
    for origin in 0..seen.len() {
        if seen[origin] || map[origin] < 0.3 {
            continue;
        }
        let mut queue = VecDeque::from([origin]);
        seen[origin] = true;
        let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
        let mut sum = 0.;
        let mut count = 0;
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i % width, i / width);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            sum += map[i];
            count += 1;
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < width && ny < height {
                    let next = ny * width + nx;
                    if !seen[next] && map[next] >= 0.3 {
                        seen[next] = true;
                        queue.push_back(next);
                    }
                }
            }
        }
        if count < 6 || sum / (count as f32) < 0.6 || x1 - x0 < 2 || y1 - y0 < 2 {
            continue;
        }
        let pad = (((x1 - x0 + 1) * (y1 - y0 + 1)) as f32 * 1.5
            / (2 * (x1 - x0 + y1 - y0 + 2)) as f32)
            .ceil() as usize;
        output.push((
            x0.saturating_sub(pad),
            y0.saturating_sub(pad),
            (x1 + pad + 1).min(width),
            (y1 + pad + 1).min(height),
        ));
    }
    output.sort_by_key(|b| (b.1, b.0));
    // Group a line against a fixed baseline so sorting remains transitive.
    let mut start = 0;
    while start < output.len() {
        let baseline = output[start].1;
        let tolerance = (output[start].3 - baseline) / 2;
        let mut end = start + 1;
        while end < output.len() && output[end].1.abs_diff(baseline) <= tolerance {
            end += 1;
        }
        output[start..end].sort_by_key(|b| b.0);
        start = end;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ctc_ignores_blank_and_collapses_repeated_characters() {
        let dict = vec!["".into(), "中".into(), "文".into()];
        assert_eq!(
            decode(
                &[0., 1., 0., 0., 1., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1.],
                3,
                &dict
            ),
            "中中文"
        );
    }
    #[test]
    #[ignore = "requires the integrity-checked OCR resource build; run with LEXIFT_OCR_TEST_RESOURCES"]
    fn real_paddle_models_recognize_supported_scripts() {
        let folder =
            PathBuf::from(std::env::var_os("LEXIFT_OCR_TEST_RESOURCES").expect("resource path"));
        let mut engine = Engine::new(&folder).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr");
        for (file, language, expected) in [
            ("common", "zh-CN", "你好世界"),
            ("common", "zh-TW", "繁體中文"),
            ("common", "en-US", "Hello world"),
            ("common", "en-GB", "Hello world"),
            ("common", "ja", "日本語"),
            ("latin", "fr", "Bonjour"),
            ("latin", "de", "Deutsch"),
            ("latin", "es", "Español"),
            ("latin", "it", "Italiano"),
            ("latin", "pt-PT", "Português"),
            ("latin", "pt-BR", "Português"),
            ("korean", "ko", "안녕하세요"),
        ] {
            let image = image::open(fixtures.join(format!("{file}.png")))
                .unwrap()
                .into_rgba8();
            let bounds = lexift_core::domain::geometry::Rect {
                left: 0,
                top: 0,
                right: image.width() as i32,
                bottom: image.height() as i32,
            };
            let result = engine
                .recognize(
                    CapturedRegion {
                        bounds,
                        pixels: image.into_raw(),
                    },
                    language,
                    &AtomicBool::new(false),
                )
                .unwrap();
            println!("{language}: {result}");
            assert!(
                result.contains(expected),
                "{result:?} must contain {expected:?}"
            );
        }
        let blank = CapturedRegion {
            bounds: lexift_core::domain::geometry::Rect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 480,
            },
            pixels: vec![255; 640 * 480 * 4],
        };
        assert_eq!(
            engine
                .recognize(blank, "en-US", &AtomicBool::new(false))
                .unwrap(),
            ""
        );
    }
}
