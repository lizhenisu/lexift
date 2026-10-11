//! CTC candidates retain spatial evidence; model-specific calibration makes scores comparable.
//! Calibration contains aggregate numbers only, never screenshots or recognized text.
use serde::Deserialize;

#[derive(Clone)]
pub(super) struct Token {
    pub text: String,
    pub probability: f32,
    pub center: f32,
}
pub(super) struct Recognition {
    pub tokens: Vec<Token>,
    pub model: &'static str,
}
impl Recognition {
    pub fn text(&self) -> String {
        self.tokens.iter().map(|t| t.text.as_str()).collect()
    }
}
/// Exported inference tensors must be normalized CTC probabilities, with blank at index zero.
pub(super) fn decode(
    values: &[f32],
    dict: &[String],
    model: &'static str,
) -> Result<Recognition, &'static str> {
    let classes = dict.len();
    if classes < 2 || values.is_empty() || !values.len().is_multiple_of(classes) {
        return Err("invalid CTC tensor");
    }
    let steps = values.len() / classes;
    let mut tokens: Vec<Token> = Vec::new();
    let mut previous = usize::MAX;
    for (step, row) in values.chunks_exact(classes).enumerate() {
        if row.iter().any(|v| !v.is_finite() || *v < 0. || *v > 1.)
            || (row.iter().sum::<f32>() - 1.).abs() > 0.01
        {
            return Err("CTC output is not a probability distribution");
        }
        let (index, probability) = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        if index != 0 {
            if index != previous {
                tokens.push(Token {
                    text: dict[index].clone(),
                    probability: *probability,
                    center: (step as f32 + 0.5) / steps as f32,
                });
            } else if let Some(last) = tokens.last_mut() {
                last.probability = last.probability.max(*probability);
            }
        }
        previous = index;
    }
    Ok(Recognition { tokens, model })
}
#[derive(Deserialize)]
struct Calibration {
    universal: Vec<[f32; 2]>,
    korean: Vec<[f32; 2]>,
    minimum: f32,
    margin: f32,
    #[cfg(test)]
    resource_sha256: String,
    #[allow(dead_code)]
    korean_resource_sha256: String,
}
fn calibration() -> &'static Calibration {
    static VALUE: std::sync::OnceLock<Calibration> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        serde_json::from_str(include_str!("calibration.json")).expect("OCR calibration")
    })
}
fn score(model: &str, probability: f32) -> f32 {
    let c = calibration();
    let points = if model == "korean" {
        &c.korean
    } else {
        &c.universal
    };
    for pair in points.windows(2) {
        if probability <= pair[1][0] {
            let weight = ((probability - pair[0][0]) / (pair[1][0] - pair[0][0])).clamp(0., 1.);
            return pair[0][1] + weight * (pair[1][1] - pair[0][1]);
        }
    }
    points.last().unwrap()[1]
}
fn hangul(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32, 0x1100..=0x11ff | 0x3130..=0x318f | 0xac00..=0xd7af))
}
/// Only Korean-script evidence may replace the universal candidate. ASCII, digits and
/// punctuation keep the universal result. Spatial replacement also preserves mixed lines.
pub(super) fn select(universal: &Recognition, korean: &Recognition) -> String {
    let c = calibration();
    let mut selected: Vec<(f32, &str)> = universal
        .tokens
        .iter()
        .map(|t| (t.center, t.text.as_str()))
        .collect();
    // Replace contiguous Hangul runs as a unit. Individual CTC character centers
    // are not aligned between models; per-character replacement can lose syllables.
    let mut accepted = vec![false; korean.tokens.len()];
    let mut index = 0;
    while index < korean.tokens.len() {
        if !hangul(&korean.tokens[index].text) {
            index += 1;
            continue;
        }
        let start = index;
        while index < korean.tokens.len() && hangul(&korean.tokens[index].text) {
            index += 1;
        }
        let run = &korean.tokens[start..index];
        let repeats = run.iter().filter(|t| t.text == run[0].text).count();
        if repeats > 3 && repeats * 2 > run.len() {
            continue;
        }
        let mut gaps: Vec<f32> = korean
            .tokens
            .windows(2)
            .map(|p| p[1].center - p[0].center)
            .filter(|g| *g > 0.)
            .collect();
        gaps.sort_by(f32::total_cmp);
        let half = gaps.get(gaps.len() / 2).copied().unwrap_or(1.) / 2.;
        // Unsupported glyphs may be omitted: the first/last Korean token must
        // not claim an entire image edge and erase supported neighboring scripts.
        let left = if start == 0 {
            (run[0].center - half).max(0.)
        } else {
            (korean.tokens[start - 1].center + run[0].center) / 2.
        };
        let right = korean
            .tokens
            .get(index)
            .map_or((run.last().unwrap().center + half).min(1.), |t| {
                (t.center + run.last().unwrap().center) / 2.
            });
        let competing: Vec<_> = universal
            .tokens
            .iter()
            .filter(|t| t.center >= left && t.center < right && !t.text.trim().is_empty())
            .collect();
        // Missing characters are not evidence for the universal candidate.
        let other = competing
            .iter()
            .map(|t| score(universal.model, t.probability))
            .sum::<f32>()
            / competing.len().max(run.len()) as f32;
        let confidence = run
            .iter()
            .map(|t| score(korean.model, t.probability))
            .sum::<f32>()
            / run.len() as f32;
        if confidence < c.minimum || confidence <= other + c.margin {
            continue;
        }
        selected.retain(|(position, text)| {
            *position < left || *position >= right || text.trim().is_empty()
        });
        selected.extend(run.iter().map(|t| (t.center, t.text.as_str())));
        accepted[start..index].fill(true);
    }
    // Preserve spacing supplied by the Korean recognizer between accepted runs;
    // the universal model often omits those words and their spaces together.
    for (i, t) in korean.tokens.iter().enumerate() {
        if !t.text.trim().is_empty()
            || i == 0
            || i + 1 == korean.tokens.len()
            || !accepted[i - 1]
            || !accepted[i + 1]
        {
            continue;
        }
        let left = korean.tokens[i - 1].center;
        let right = korean.tokens[i + 1].center;
        if !selected
            .iter()
            .any(|(p, text)| *p > left && *p < right && text.trim().is_empty())
        {
            selected.push((t.center, t.text.as_str()));
        }
    }
    selected.sort_by(|a, b| a.0.total_cmp(&b.0));
    selected.into_iter().map(|(_, text)| text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn korean_middle_run_preserves_neighboring_chinese() {
        let token = |text: &str, center: f32| Token {
            text: text.into(),
            probability: 0.999,
            center,
        };
        let u = Recognition {
            model: "universal",
            tokens: vec![token("中", 0.1), token("文", 0.9)],
        };
        let k = Recognition {
            model: "korean",
            tokens: vec![token("한", 0.4), token("글", 0.6)],
        };
        assert_eq!(select(&u, &k), "中한글文");
    }
    #[test]
    fn calibration_is_finite_monotonic_and_model_specific() {
        let c = calibration();
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("assets.json")).unwrap();
        assert_eq!(c.resource_sha256, manifest["sha256"].as_str().unwrap());
        let korean: serde_json::Value =
            serde_json::from_str(include_str!("korean-assets.json")).unwrap();
        assert_eq!(c.korean_resource_sha256, korean["sha256"].as_str().unwrap());
        for points in [&c.universal, &c.korean] {
            assert_eq!(points.first().unwrap()[0], 0.);
            assert_eq!(points.last().unwrap()[0], 1.);
            assert!(
                points
                    .iter()
                    .all(|p| p.iter().all(|v| v.is_finite() && (0. ..=1.).contains(v)))
            );
            assert!(
                points
                    .windows(2)
                    .all(|p| p[0][0] < p[1][0] && p[0][1] <= p[1][1])
            );
        }
        assert_ne!(c.universal, c.korean);
    }
    #[test]
    fn rejects_logits_nonfinite_and_bad_shape() {
        let dict = vec![String::new(), "a".into()];
        for values in [vec![2., -1.], vec![f32::NAN, 0.], vec![0.2, 0.2], vec![1.]] {
            assert!(decode(&values, &dict, "universal").is_err());
        }
    }
    #[test]
    fn uncertainty_and_non_korean_candidates_preserve_universal() {
        let u = Recognition {
            model: "universal",
            tokens: vec![Token {
                text: "文".into(),
                probability: 0.99,
                center: 0.5,
            }],
        };
        for text in ["a", "7", "한"] {
            let k = Recognition {
                model: "korean",
                tokens: vec![Token {
                    text: text.into(),
                    probability: 0.1,
                    center: 0.5,
                }],
            };
            assert_eq!(select(&u, &k), "文");
        }
    }
}
