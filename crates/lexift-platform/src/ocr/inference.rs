//! Shared DB detection and versioned CTC recognition on the CPU.
use image::{
    RgbImage,
    imageops::{self, FilterType},
};
use lexift_core::{Error, Result, ports::screen_capture::CapturedRegion};
use ort::{
    session::Session,
    value::{Tensor, TensorRef},
};
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

pub(super) static RUNTIME_LOADED: AtomicBool = AtomicBool::new(false);

pub(super) struct Engine {
    folder: PathBuf,
    pub(super) korean_folder: PathBuf,
    batch_limit: usize,
    input_scratch: Vec<f32>,
    #[cfg(test)]
    stages_us: [u128; 4],
    detector: Session,
    pub(super) readers: HashMap<&'static str, (Session, Vec<String>)>,
}
impl Engine {
    pub fn new(folder: &Path) -> Result<Self> {
        let dll = folder
            .join("onnxruntime.dll")
            .canonicalize()
            .map_err(error)?;
        RUNTIME_LOADED.store(true, Ordering::Release);
        ort::init_from(dll.to_string_lossy())
            .with_name("Lexift OCR")
            .commit()
            .map_err(error)?;
        Ok(Self {
            folder: folder.into(),
            korean_folder: if folder.join("korean.onnx").exists() {
                folder.into()
            } else {
                folder.parent().unwrap_or(folder).join("korean-v1")
            },
            batch_limit: 4,
            input_scratch: Vec::new(),
            #[cfg(test)]
            stages_us: [0; 4],
            detector: session(&folder.join("det.onnx"))?,
            readers: HashMap::new(),
        })
    }
    pub fn recognize(
        &mut self,
        input: CapturedRegion,
        strategy: &lexift_core::domain::ocr::OcrRecognitionStrategy,
        cancel: &AtomicBool,
    ) -> Result<String> {
        cancelled(cancel)?;
        #[cfg(test)]
        {
            self.stages_us[3] = 0;
        }
        #[cfg(test)]
        let stage_start = std::time::Instant::now();
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
        drop(input);
        let resized = imageops::resize(&image, dw, dh, FilterType::Triangle);
        let tensor = Tensor::from_array((
            [1usize, 3, dh as usize, dw as usize],
            normalize(&resized, true),
        ))
        .map_err(error)?;
        drop(resized);
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
        #[cfg(test)]
        {
            self.stages_us[0] = stage_start.elapsed().as_micros();
        }
        #[cfg(test)]
        let stage_start = std::time::Instant::now();
        drop(outputs);
        let mut regions = Vec::new();
        for b in boxes {
            let left = (b.0 as f32 / mw as f32 * w as f32).floor().max(0.) as u32;
            let top = (b.1 as f32 / mh as f32 * h as f32).floor().max(0.) as u32;
            let right = (b.2 as f32 / mw as f32 * w as f32).ceil().min(w as f32) as u32;
            let bottom = (b.3 as f32 / mh as f32 * h as f32).ceil().min(h as f32) as u32;
            if right > left && bottom > top {
                regions.push((left, top, right, bottom));
            }
        }
        let mut order: Vec<usize> = (0..regions.len()).collect();
        order.sort_by_key(|&i| {
            let (l, t, r, b) = regions[i];
            normalized_width(r - l, b - t)
        });
        let mut texts = vec![String::new(); regions.len()];
        let mut offset = 0;
        while offset < order.len() {
            cancelled(cancel)?;
            let mut count = 1;
            let width_at = |i: usize| {
                let (l, t, r, b) = regions[order[i]];
                normalized_width(r - l, b - t) as usize
            };
            let mut sum = width_at(offset);
            while count < self.batch_limit && offset + count < order.len() {
                let max_width = width_at(offset + count);
                let next_sum = sum + max_width;
                // Bound padding waste and the worst-case CTC probability output (f32).
                if max_width != width_at(offset)
                    || max_width * (count + 1) * 2 > next_sum * 3
                    || (max_width / 4 + 4) * 18710 * 4 * (count + 1) > 8 * 1024 * 1024
                {
                    break;
                }
                sum = next_sum;
                count += 1;
            }
            let crops: Vec<_> = order[offset..offset + count]
                .iter()
                .map(|&i| {
                    let (l, t, r, b) = regions[i];
                    imageops::crop_imm(&image, l, t, r - l, b - t).to_image()
                })
                .collect();
            let refs: Vec<_> = crops.iter().collect();
            let key = strategy.manual_model().unwrap_or("universal");
            let primary = self.read_batch(&refs, key, cancel)?;
            let korean = if matches!(
                strategy,
                lexift_core::domain::ocr::OcrRecognitionStrategy::AutomaticKorean
            ) {
                cancelled(cancel)?;
                Some(self.read_batch(&refs, "korean", cancel)?)
            } else {
                None
            };
            for (j, result) in primary.iter().enumerate() {
                texts[order[offset + j]] = korean.as_ref().map_or_else(
                    || result.text(),
                    |k| super::recognition::select(result, &k[j]),
                );
            }
            offset += count;
        }
        #[cfg(test)]
        {
            self.stages_us[1] = stage_start.elapsed().as_micros();
        }
        #[cfg(test)]
        let stage_start = std::time::Instant::now();
        let lines: Vec<_> = regions
            .into_iter()
            .zip(texts)
            .filter_map(|((l, t, r, b), text)| {
                (!text.trim().is_empty()).then_some((l, t, r, b, text))
            })
            .collect();
        cancelled(cancel)?;
        let result = compose_lines(&lines);
        #[cfg(test)]
        {
            self.stages_us[2] = stage_start.elapsed().as_micros();
        }
        Ok(result)
    }
    pub(super) fn release_korean(&mut self) {
        self.readers.remove("korean");
    }
    #[cfg(test)]
    fn read_line(
        &mut self,
        crop: &RgbImage,
        key: &'static str,
        cancel: &AtomicBool,
    ) -> Result<super::recognition::Recognition> {
        Ok(self.read_batch(&[crop], key, cancel)?.remove(0))
    }
    fn read_batch(
        &mut self,
        crops: &[&RgbImage],
        key: &'static str,
        cancel: &AtomicBool,
    ) -> Result<Vec<super::recognition::Recognition>> {
        cancelled(cancel)?;
        if !self.readers.contains_key(key) {
            #[cfg(test)]
            let loading = std::time::Instant::now();
            let folder = if key == "korean" {
                &self.korean_folder
            } else {
                &self.folder
            };
            let mut dict = vec![String::new()];
            dict.extend(
                std::fs::read_to_string(folder.join(format!("{key}.txt")))
                    .map_err(error)?
                    .lines()
                    .map(str::to_owned),
            );
            if dict.last().is_none_or(|s| s != " ") {
                dict.push(" ".into());
            }
            self.readers
                .insert(key, (session(&folder.join(format!("{key}.onnx")))?, dict));
            #[cfg(test)]
            {
                self.stages_us[3] += loading.elapsed().as_micros();
            }
        }
        let (reader, dict) = self.readers.get_mut(key).unwrap();
        let widths: Vec<_> = crops
            .iter()
            .map(|crop| normalized_width(crop.width(), crop.height()))
            .collect();
        let width = *widths
            .iter()
            .max()
            .ok_or_else(|| error("empty recognition batch"))?;
        if widths.iter().any(|&w| w != width) {
            return Err(error("mixed-width batch is not validated"));
        }
        let plane = 48 * width as usize;
        self.input_scratch.resize(crops.len() * plane * 3, 0.);
        self.input_scratch.fill(0.);
        let input = &mut self.input_scratch;
        for (batch, (crop, &valid)) in crops.iter().zip(&widths).enumerate() {
            cancelled(cancel)?;
            let resized = imageops::resize(*crop, valid, 48, FilterType::Triangle);
            for (i, pixel) in resized.pixels().enumerate() {
                let y = i / valid as usize;
                let x = i % valid as usize;
                for c in 0..3 {
                    input[batch * plane * 3 + c * plane + y * width as usize + x] =
                        (pixel[2 - c] as f32 / 255. - 0.5) / 0.5;
                }
            }
        }
        let tensor = TensorRef::from_array_view(([crops.len(), 3, 48, width as usize], &input[..]))
            .map_err(error)?;
        let outputs = reader.run(ort::inputs![tensor]).map_err(error)?;
        let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
        if shape.len() != 3
            || shape[0] != crops.len() as i64
            || shape[1] <= 0
            || shape[2] != dict.len() as i64
            || shape[0]
                .checked_mul(shape[1])
                .and_then(|n| n.checked_mul(shape[2]))
                != Some(values.len() as i64)
        {
            return Err(error(
                "invalid recognition output or model dictionary mismatch",
            ));
        }
        cancelled(cancel)?;
        let steps = shape[1] as usize;
        let classes = dict.len();
        widths
            .iter()
            .enumerate()
            .map(|(i, &valid)| {
                let count = (steps * valid as usize).div_ceil(width as usize).min(steps);
                super::recognition::decode(
                    &values[i * steps * classes..i * steps * classes + count * classes],
                    dict,
                    key,
                )
                .map_err(error)
            })
            .collect()
    }
}
/// Detection may split one visual line into separate script/word regions. Preserve
/// physical line breaks, rather than inventing a newline at every detected region.
fn compose_lines(regions: &[(u32, u32, u32, u32, String)]) -> String {
    let mut output = String::new();
    let mut baseline: Option<(u32, u32)> = None;
    let mut previous_right = 0;
    for (left, top, right, bottom, text) in regions {
        if let Some((line_top, line_bottom)) = baseline {
            let overlap = line_bottom.min(*bottom).saturating_sub(line_top.max(*top));
            let height = (line_bottom - line_top).min(bottom - top);
            if overlap * 2 >= height {
                if left.saturating_sub(previous_right) > height / 5 {
                    output.push(' ');
                }
            } else {
                output.push('\n');
                baseline = Some((*top, *bottom));
            }
        } else {
            baseline = Some((*top, *bottom));
        }
        output.push_str(text);
        previous_right = *right;
    }
    output
}

fn normalized_width(width: u32, height: u32) -> u32 {
    ((width as f32 / height as f32 * 48.).ceil() as u32).clamp(8, 3200)
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
                // Recognition decodes BGR; detection keeps the existing RGB contract.
                (p[2 - c] as f32 / 255. - 0.5) / 0.5
            };
        }
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
    fn visual_rows_join_mixed_regions_and_preserve_actual_newlines() {
        assert_eq!(
            compose_lines(&[
                (0, 0, 40, 20, "你好".into()),
                (48, 1, 100, 21, "한글".into()),
                (0, 30, 40, 50, "next".into())
            ]),
            "你好 한글\nnext"
        );
    }
    #[test]
    fn ctc_ignores_blank_and_collapses_repeated_characters() {
        let dict = vec!["".into(), "中".into(), "文".into()];
        assert_eq!(
            super::super::recognition::decode(
                &[0., 1., 0., 0., 1., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1.],
                &dict,
                "universal"
            )
            .unwrap()
            .text(),
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
                    &lexift_core::domain::ocr::OcrRecognitionStrategy::Language(language.into()),
                    &AtomicBool::new(false),
                )
                .unwrap();
            println!("{language}: {result}");
            assert!(
                result.contains(expected),
                "{result:?} must contain {expected:?}"
            );
        }
        let cancelled_image = CapturedRegion {
            bounds: lexift_core::domain::geometry::Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            pixels: vec![255; 4],
        };
        assert!(
            engine
                .recognize(
                    cancelled_image,
                    &lexift_core::domain::ocr::OcrRecognitionStrategy::Automatic,
                    &AtomicBool::new(true)
                )
                .is_err()
        );
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
                .recognize(
                    blank,
                    &lexift_core::domain::ocr::OcrRecognitionStrategy::Automatic,
                    &AtomicBool::new(false)
                )
                .unwrap(),
            ""
        );
    }
    /// Development-only corpus runner. Inputs and candidate text stay under ignored target/.
    #[test]
    #[ignore = "requires a generated corpus and verified v2 resources"]
    fn real_automatic_corpus() {
        use lexift_core::domain::{geometry::Rect, ocr::OcrRecognitionStrategy};
        let folder = PathBuf::from(std::env::var_os("LEXIFT_OCR_TEST_RESOURCES").unwrap());
        let corpus = PathBuf::from(std::env::var_os("LEXIFT_OCR_CORPUS").unwrap());
        let entries: serde_json::Value =
            serde_json::from_slice(&std::fs::read(corpus.join("manifest.json")).unwrap()).unwrap();
        let mut engine = Engine::new(&folder).unwrap();
        let mut records = Vec::new();
        for entry in entries.as_array().unwrap() {
            let image = image::open(corpus.join(entry["file"].as_str().unwrap()))
                .unwrap()
                .into_rgb8();
            let cancel = AtomicBool::new(false);
            let u = engine.read_line(&image, "universal", &cancel).unwrap();
            let k = engine.read_line(&image, "korean", &cancel).unwrap();
            let tokens = |r: &super::super::recognition::Recognition| {
                r.tokens.iter().map(|t|
                serde_json::json!({"text":t.text,"probability":t.probability,"center":t.center})).collect::<Vec<_>>()
            };
            let start = std::time::Instant::now();
            let rgba = image::DynamicImage::ImageRgb8(image).into_rgba8();
            let result = engine
                .recognize(
                    CapturedRegion {
                        bounds: Rect {
                            left: 0,
                            top: 0,
                            right: rgba.width() as i32,
                            bottom: rgba.height() as i32,
                        },
                        pixels: rgba.into_raw(),
                    },
                    &OcrRecognitionStrategy::AutomaticKorean,
                    &cancel,
                )
                .unwrap();
            records.push(serde_json::json!({"id":entry["file"],"split":entry["split"],"language":entry["language"],"expected":entry["text"],"universal":tokens(&u),"korean":tokens(&k),"selected":super::super::recognition::select(&u,&k),"detected":result,"milliseconds":start.elapsed().as_millis()}));
        }
        for record in &records {
            let expected = record["expected"].as_str().unwrap();
            if !expected
                .chars()
                .any(|c| matches!(c as u32, 0xac00..=0xd7af))
            {
                assert!(
                    !record["detected"]
                        .as_str()
                        .unwrap()
                        .chars()
                        .any(|c| matches!(c as u32, 0xac00..=0xd7af)),
                    "unexpected Korean routing in {}",
                    record["id"]
                );
            }
        }
        std::fs::write(
            corpus.join("results.json"),
            serde_json::to_vec(&records).unwrap(),
        )
        .unwrap();
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "fresh-process CPU benchmark with verified v2 resources"]
    fn real_automatic_performance() {
        use lexift_core::domain::{geometry::Rect, ocr::OcrRecognitionStrategy};
        let corpus = PathBuf::from(std::env::var_os("LEXIFT_OCR_CORPUS").unwrap());
        let folder = PathBuf::from(std::env::var_os("LEXIFT_OCR_TEST_RESOURCES").unwrap());
        let image = image::open(corpus.join("validation-14-34.png"))
            .unwrap()
            .into_rgba8();
        let input = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: image.width() as i32,
                bottom: image.height() as i32,
            },
            pixels: image.into_raw(),
        };
        let cancel = AtomicBool::new(false);
        let start = std::time::Instant::now();
        let mut engine = Engine::new(&folder).unwrap();
        engine
            .recognize(
                input.clone(),
                &OcrRecognitionStrategy::AutomaticKorean,
                &cancel,
            )
            .unwrap();
        let cold = start.elapsed().as_millis();
        let mut samples = Vec::new();
        for strategy in [
            OcrRecognitionStrategy::Automatic,
            OcrRecognitionStrategy::Language("zh-CN".into()),
        ] {
            let start = std::time::Instant::now();
            for _ in 0..10 {
                engine.recognize(input.clone(), &strategy, &cancel).unwrap();
            }
            samples.push(start.elapsed().as_micros() as f64 / 10000.);
        }
        use windows::Win32::System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
            Threading::GetCurrentProcess,
        };
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
        let mut result = serde_json::json!({"cold_auto_ms":cold,"warm_auto_mean_ms":samples[0],"warm_manual_v2_mean_ms":samples[1],"peak_working_set_bytes":counters.PeakWorkingSetSize,"private_bytes":counters.PrivateUsage});
        if let Some(old) = std::env::var_os("LEXIFT_OCR_BASELINE_RESOURCES") {
            let old = PathBuf::from(old);
            let stage = corpus.join("baseline-v1");
            std::fs::create_dir_all(&stage).unwrap();
            for (source, destination) in [
                ("det.onnx", "det.onnx"),
                ("common.onnx", "universal.onnx"),
                ("common.txt", "universal.txt"),
                ("onnxruntime.dll", "onnxruntime.dll"),
            ] {
                std::fs::copy(old.join(source), stage.join(destination)).unwrap();
            }
            let mut baseline = Engine::new(&stage).unwrap();
            let manual = OcrRecognitionStrategy::Language("zh-CN".into());
            baseline.recognize(input.clone(), &manual, &cancel).unwrap();
            let start = std::time::Instant::now();
            for _ in 0..10 {
                baseline.recognize(input.clone(), &manual, &cancel).unwrap();
            }
            result["warm_manual_v1_mean_ms"] =
                serde_json::json!(start.elapsed().as_micros() as f64 / 10000.);
        }
        std::fs::write(
            corpus.join("performance.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }
    /// Real model gate: padded batches must preserve decoded text and spatial ordering.
    #[test]
    #[ignore = "requires verified models and generated corpus"]
    fn real_batch_equivalence_and_default_uses_no_korean() {
        use lexift_core::domain::{geometry::Rect, ocr::OcrRecognitionStrategy};
        let folder = PathBuf::from(std::env::var_os("LEXIFT_OCR_TEST_RESOURCES").unwrap());
        let corpus = PathBuf::from(std::env::var_os("LEXIFT_OCR_CORPUS").unwrap());
        let entries: serde_json::Value =
            serde_json::from_slice(&std::fs::read(corpus.join("manifest.json")).unwrap()).unwrap();
        let mut engine = Engine::new(&folder).unwrap();
        let cancel = AtomicBool::new(false);
        for key in ["universal", "korean"] {
            let images: Vec<_> = entries
                .as_array()
                .unwrap()
                .iter()
                .take(16)
                .map(|entry| {
                    image::open(corpus.join(entry["file"].as_str().unwrap()))
                        .unwrap()
                        .into_rgb8()
                })
                .collect();
            for image in &images {
                let single = engine.read_line(image, key, &cancel).unwrap().text();
                let batch = engine
                    .read_batch(&[image, image, image, image], key, &cancel)
                    .unwrap();
                assert!(
                    batch.iter().all(|r| r.text() == single),
                    "equal-width batch changed {key} text"
                );
            }
        }
        engine.release_korean();
        let rgba = image::open(corpus.join("benchmark-ten.png"))
            .unwrap()
            .into_rgba8();
        let input = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: rgba.width() as i32,
                bottom: rgba.height() as i32,
            },
            pixels: rgba.into_raw(),
        };
        engine.batch_limit = 1;
        let single = engine
            .recognize(input.clone(), &OcrRecognitionStrategy::Automatic, &cancel)
            .unwrap();
        engine.batch_limit = 4;
        let batch = engine
            .recognize(input, &OcrRecognitionStrategy::Automatic, &cancel)
            .unwrap();
        assert_eq!(single, batch);
        assert!(!engine.readers.contains_key("korean"));
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "fresh process benchmark with generated images"]
    fn real_extension_performance() {
        use lexift_core::domain::{geometry::Rect, ocr::OcrRecognitionStrategy};
        use windows::Win32::{
            Foundation::FILETIME,
            System::{
                ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
                Threading::{GetCurrentProcess, GetProcessTimes},
            },
        };
        fn metrics() -> (u64, usize, usize) {
            unsafe {
                let mut creation = FILETIME::default();
                let mut exit = FILETIME::default();
                let mut kernel = FILETIME::default();
                let mut user = FILETIME::default();
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
                .unwrap();
                let time =
                    |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
                let mut m = PROCESS_MEMORY_COUNTERS_EX {
                    cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                    ..Default::default()
                };
                GetProcessMemoryInfo(
                    GetCurrentProcess(),
                    (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                    m.cb,
                )
                .unwrap();
                (
                    time(kernel) + time(user),
                    m.PrivateUsage,
                    m.PeakWorkingSetSize,
                )
            }
        }
        let folder = PathBuf::from(std::env::var_os("LEXIFT_OCR_TEST_RESOURCES").unwrap());
        let corpus = PathBuf::from(std::env::var_os("LEXIFT_OCR_CORPUS").unwrap());
        let mode = std::env::var("LEXIFT_OCR_BENCH_MODE").unwrap_or_else(|_| "base".into());
        let selected_case = std::env::var("LEXIFT_OCR_BENCH_CASE").ok();
        let strategy = if mode == "dual" {
            OcrRecognitionStrategy::AutomaticKorean
        } else {
            OcrRecognitionStrategy::Automatic
        };
        let mut records = Vec::new();
        let load_start = std::time::Instant::now();
        let mut engine = Engine::new(&folder).unwrap();
        let detector_load_us = load_start.elapsed().as_micros();
        if mode == "single" {
            engine.batch_limit = 1;
        }
        let cancel = AtomicBool::new(false);
        for file in [
            "validation-14-34.png",
            "benchmark-ten.png",
            "benchmark-many.png",
            "benchmark-varied.png",
        ] {
            if selected_case.as_deref().is_some_and(|name| name != file) {
                continue;
            }
            let rgba = image::open(corpus.join(file)).unwrap().into_rgba8();
            let input = CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: rgba.width() as i32,
                    bottom: rgba.height() as i32,
                },
                pixels: rgba.into_raw(),
            };
            let start = std::time::Instant::now();
            let text = engine.recognize(input.clone(), &strategy, &cancel).unwrap();
            let cold = start.elapsed().as_micros();
            let cold_phases = engine.stages_us;
            let before = metrics();
            let start = std::time::Instant::now();
            let mut phases = [0u128; 4];
            for _ in 0..10 {
                engine.recognize(input.clone(), &strategy, &cancel).unwrap();
                for (total, value) in phases.iter_mut().zip(engine.stages_us) {
                    *total += value;
                }
            }
            let elapsed = start.elapsed().as_micros();
            let after = metrics();
            records.push(serde_json::json!({"file":file,"first_us":cold,"first_phases_us":cold_phases,"warm_mean_us":elapsed/10,"cpu_mean_us":(after.0-before.0)/100,"phases_mean_us":phases.map(|p|p/10),"private_bytes":after.1,"peak_working_set_bytes":after.2,"characters":text.chars().count()}));
        }
        drop(engine);
        let after = metrics();
        std::fs::write(corpus.join(format!("performance-{mode}.json")),serde_json::to_vec_pretty(&serde_json::json!({"samples":records,"released_private_bytes":after.1,"detector_load_us":detector_load_us})).unwrap()).unwrap();
    }
}
