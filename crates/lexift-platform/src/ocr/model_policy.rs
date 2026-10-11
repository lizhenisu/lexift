//! Provider-specific model routing; Core strategies describe recognition intent only.
use lexift_core::domain::ocr::OcrRecognitionStrategy;
pub(super) fn recognizer(strategy: &OcrRecognitionStrategy) -> &'static str {
    match strategy {
        OcrRecognitionStrategy::Language(language) if language == "ko" => "korean",
        _ => "universal",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_languages_and_automatic_modes_route_without_resource_versions() {
        for language in ["ja", "en-GB", "pt-PT", "zh-CN"] {
            assert_eq!(
                recognizer(&OcrRecognitionStrategy::Language(language.into())),
                "universal"
            );
        }
        assert_eq!(
            recognizer(&OcrRecognitionStrategy::Language("ko".into())),
            "korean"
        );
        assert_eq!(recognizer(&OcrRecognitionStrategy::Automatic), "universal");
        assert_eq!(
            recognizer(&OcrRecognitionStrategy::AutomaticKorean),
            "universal"
        );
    }
}
