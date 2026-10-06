//! One mapping from toolbar entries to creation and editing permissions.
use lexift_core::domain::annotation::{EditScope, Kind};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Drawing {
    Object(Kind),
    Watermark,
    Erase,
    Unavailable,
}
pub(super) fn drawing(tool: usize, shape: i32, capture: bool) -> Drawing {
    let kind = match tool {
        0 | 1 => {
            if shape == 0 {
                Kind::Rectangle
            } else {
                Kind::Ellipse
            }
        }
        2 => {
            if shape == 0 {
                Kind::SpotlightRectangle
            } else {
                Kind::SpotlightEllipse
            }
        }
        3 => Kind::Pencil,
        4 => {
            if shape == 0 {
                Kind::HighlightLine
            } else {
                Kind::HighlightRectangle
            }
        }
        5 => Kind::Arrow,
        6 => Kind::Polyline,
        7 => Kind::Magnifier,
        8 => Kind::Text,
        9 => return Drawing::Watermark,
        10 => Kind::Sequence,
        11 if capture => {
            if shape == 1 {
                Kind::MosaicBrush
            } else {
                Kind::MosaicArea
            }
        }
        12 => {
            if shape == 1 {
                Kind::StickerBrush
            } else {
                Kind::StickerArea
            }
        }
        13 => return Drawing::Erase,
        _ => return Drawing::Unavailable,
    };
    Drawing::Object(kind)
}
pub(super) fn scope(tool: usize) -> EditScope {
    match drawing(tool, 0, true) {
        Drawing::Object(kind) => kind.edit_scope(),
        _ => EditScope::None,
    }
}
