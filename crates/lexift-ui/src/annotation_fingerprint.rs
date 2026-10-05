//! Allocation-free content fingerprints for render invalidation, not persistence.
//! Exhaustive field destructuring makes new render-affecting fields a compile error.
use lexift_core::domain::annotation::{
    Bounds, Extra, Style, TextAnnotation, TextBackground, TextOutline, Watermark,
};
use std::hash::{Hash, Hasher};

fn bounds(value: &Bounds, hash: &mut impl Hasher) {
    let Bounds {
        left,
        top,
        right,
        bottom,
    } = value;
    for value in [left, top, right, bottom] {
        value.to_bits().hash(hash);
    }
}
fn text(value: &TextAnnotation, hash: &mut impl Hasher) {
    let TextAnnotation {
        content,
        font_family,
        font_size,
        bold,
        italic,
        outline,
        background,
        rotation,
        linked_arrow,
        layout_scale,
        linked_arrow_control,
    } = value;
    content.hash(hash);
    font_family.hash(hash);
    font_size.to_bits().hash(hash);
    bold.hash(hash);
    italic.hash(hash);
    outline.is_some().hash(hash);
    if let Some(TextOutline { color, width }) = outline {
        color.hash(hash);
        width.to_bits().hash(hash);
    }
    background.is_some().hash(hash);
    if let Some(TextBackground {
        color,
        opacity,
        rounding,
        padding,
    }) = background
    {
        color.hash(hash);
        opacity.hash(hash);
        rounding.to_bits().hash(hash);
        padding.to_bits().hash(hash);
    }
    rotation.to_bits().hash(hash);
    layout_scale.to_bits().hash(hash);
    linked_arrow
        .map(|(x, y)| (x.to_bits(), y.to_bits()))
        .hash(hash);
    linked_arrow_control
        .map(|(x, y)| (x.to_bits(), y.to_bits()))
        .hash(hash);
}
pub(crate) fn style(value: &Style, hash: &mut impl Hasher) {
    let Style {
        color,
        width,
        rounding,
        fill,
        dash,
        outline,
    } = value;
    color.hash(hash);
    width.to_bits().hash(hash);
    rounding.to_bits().hash(hash);
    fill.hash(hash);
    dash.hash(hash);
    outline.hash(hash);
}
pub(crate) fn watermark(value: Option<&Watermark>, hash: &mut impl Hasher) {
    value.is_some().hash(hash);
    if let Some(Watermark {
        template,
        content,
        font_family,
        font_size,
        opacity,
        color,
        position,
    }) = value
    {
        template.hash(hash);
        content.hash(hash);
        font_family.hash(hash);
        font_size.to_bits().hash(hash);
        opacity.hash(hash);
        color.hash(hash);
        position.hash(hash);
    }
}
pub(crate) fn extra(value: &Extra, hash: &mut impl Hasher) {
    std::mem::discriminant(value).hash(hash);
    match value {
        Extra::None => {}
        Extra::Arrow {
            curved,
            head,
            start,
            end,
        }
        | Extra::Polyline {
            curved,
            head,
            start,
            end,
        } => {
            curved.hash(hash);
            head.hash(hash);
            std::mem::discriminant(start).hash(hash);
            std::mem::discriminant(end).hash(hash);
        }
        Extra::Magnifier {
            output,
            zoom,
            ellipse,
            connector,
            erase_annotations,
            antialias,
            shadow,
        } => {
            bounds(output, hash);
            zoom.to_bits().hash(hash);
            ellipse.hash(hash);
            std::mem::discriminant(connector).hash(hash);
            erase_annotations.hash(hash);
            antialias.hash(hash);
            shadow.hash(hash);
        }
        Extra::Text(value) => text(value, hash),
        Extra::Sequence(value) => {
            use lexift_core::domain::annotation::SequenceAnnotation;
            let SequenceAnnotation {
                value,
                format,
                shape,
                size,
                scale,
                caption,
                caption_bounds,
                caption_color,
                caption_custom_color,
                caption_ratio,
                guide,
                endpoint,
            } = value;
            value.hash(hash);
            std::mem::discriminant(format).hash(hash);
            std::mem::discriminant(shape).hash(hash);
            size.to_bits().hash(hash);
            scale.to_bits().hash(hash);
            text(caption, hash);
            bounds(caption_bounds, hash);
            caption_color.hash(hash);
            caption_custom_color.hash(hash);
            caption_ratio.hash(hash);
            guide.hash(hash);
            std::mem::discriminant(endpoint).hash(hash);
        }
        Extra::Mosaic(value) => {
            use lexift_core::domain::annotation::MosaicAnnotation;
            let MosaicAnnotation {
                id,
                effect,
                strength,
                diameter,
                creation_scale,
            } = value;
            id.hash(hash);
            effect.hash(hash);
            strength.hash(hash);
            diameter.to_bits().hash(hash);
            creation_scale.to_bits().hash(hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fingerprint(value: &Extra) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        extra(value, &mut hash);
        hash.finish()
    }
    #[test]
    fn text_connectors_effects_and_variant_identity_invalidate_content() {
        use lexift_core::domain::annotation::Endpoint;
        let original = Extra::Text(TextAnnotation {
            content: "中文abc".into(),
            font_family: "Arial".into(),
            font_size: 32.,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            rotation: 0.,
            linked_arrow: None,
            layout_scale: 1.25,
            linked_arrow_control: None,
        });
        let key = fingerprint(&original);
        let changes: [fn(&mut TextAnnotation); 8] = [
            |text| text.linked_arrow = Some((-200., 50.)),
            |text| text.linked_arrow_control = Some((-150., 70.)),
            |text| text.rotation = 0.5,
            |text| text.layout_scale = 2.,
            |text| {
                text.outline = Some(TextOutline {
                    color: [1, 2, 3],
                    width: 3.,
                })
            },
            |text| {
                text.background = Some(TextBackground {
                    color: [4, 5, 6],
                    opacity: 128,
                    rounding: 5.,
                    padding: 7.,
                })
            },
            |text| text.font_family = "local font".into(),
            |text| text.content.push('字'),
        ];
        for change in changes {
            let mut changed = original.clone();
            if let Extra::Text(text) = &mut changed {
                change(text);
            }
            assert_ne!(key, fingerprint(&changed));
            assert_eq!(fingerprint(&changed), fingerprint(&changed.clone()));
        }
        assert_ne!(
            fingerprint(&Extra::Arrow {
                curved: false,
                head: true,
                start: Endpoint::None,
                end: Endpoint::Arrow
            }),
            fingerprint(&Extra::Polyline {
                curved: false,
                head: true,
                start: Endpoint::None,
                end: Endpoint::Arrow
            })
        );
    }
}
