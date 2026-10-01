//! A badge, caption and connector form one movable, undoable annotation.
use super::*;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SequenceFormat {
    #[default]
    Decimal,
    Roman,
    LowerAlpha,
    UpperAlpha,
    Chinese,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SequenceShape {
    Circle,
    CircleWhiteBorder,
    Outline,
    Plain,
    #[default]
    Square,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceAnnotation {
    pub value: i32,
    pub format: SequenceFormat,
    pub shape: SequenceShape,
    pub size: f32,
    pub scale: f32,
    pub caption: TextAnnotation,
    pub caption_bounds: Bounds,
    pub caption_color: [u8; 3],
    pub caption_custom_color: bool,
    pub caption_ratio: usize,
    pub guide: bool,
    pub endpoint: Endpoint,
}
pub fn sequence_label(value: i32, format: SequenceFormat) -> String {
    let mut n = value.max(1) as u32;
    match format {
        SequenceFormat::Decimal => n.to_string(),
        SequenceFormat::LowerAlpha | SequenceFormat::UpperAlpha => {
            let mut bytes = Vec::new();
            while n > 0 {
                n -= 1;
                bytes.push(b'A' + (n % 26) as u8);
                n /= 26;
            }
            bytes.reverse();
            let s = String::from_utf8(bytes).unwrap();
            if format == SequenceFormat::LowerAlpha {
                s.to_ascii_lowercase()
            } else {
                s
            }
        }
        SequenceFormat::Roman => {
            let mut s = String::new();
            for (v, label) in [
                (1000, "M"),
                (900, "CM"),
                (500, "D"),
                (400, "CD"),
                (100, "C"),
                (90, "XC"),
                (50, "L"),
                (40, "XL"),
                (10, "X"),
                (9, "IX"),
                (5, "V"),
                (4, "IV"),
                (1, "I"),
            ] {
                while n >= v {
                    s.push_str(label);
                    n -= v;
                }
            }
            s
        }
        SequenceFormat::Chinese => chinese(n),
    }
}
fn chinese(n: u32) -> String {
    const DIGITS: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    fn small(n: u32) -> String {
        let mut s = String::new();
        let mut zero = false;
        for (p, unit) in [(1000, "千"), (100, "百"), (10, "十"), (1, "")] {
            let d = n / p % 10;
            if d != 0 {
                if zero {
                    s.push('零');
                    zero = false;
                }
                s.push_str(DIGITS[d as usize]);
                s.push_str(unit);
            } else if !s.is_empty() && !n.is_multiple_of(p) {
                zero = true;
            }
        }
        s
    }
    let mut s = String::new();
    for (p, unit) in [(100_000_000, "亿"), (10_000, "万"), (1, "")] {
        let block = n / p % 10_000;
        if block != 0 {
            if !s.is_empty() && (block < 1000 || (n / p / 10_000).is_multiple_of(10_000)) {
                s.push('零');
            }
            s.push_str(&small(block));
            s.push_str(unit);
        }
    }
    if s.starts_with("一十") {
        s.remove(0);
    }
    s
}
impl Object {
    pub fn sequence_caption(&self) -> Option<Object> {
        let Extra::Sequence(s) = &self.extra else {
            return None;
        };
        Some(Object {
            kind: Kind::Text,
            bounds: s.caption_bounds,
            style: Style {
                color: s.caption_color,
                ..self.style
            },
            points: Vec::new(),
            extra: Extra::Text(s.caption.clone()),
        })
    }
    pub fn sequence_badge_proxy(&self) -> Option<Object> {
        let Extra::Sequence(s) = &self.extra else {
            return None;
        };
        let mut text = s.caption.clone();
        text.rotation = 0.;
        Some(Object {
            kind: Kind::Text,
            extra: Extra::Text(text),
            ..self.clone()
        })
    }
    pub fn sequence_handles(&self, scale: f32) -> Vec<(f32, f32)> {
        let b = self.bounds;
        let g = 9. * scale;
        let mut h = vec![
            (b.left - g, b.top - g),
            (b.left - g, b.top + 11. * scale),
            (b.right + g, b.top - g),
            (b.left - g, b.bottom + g),
            (b.right + g, b.bottom + g),
        ];
        if let Some(a) = self.text_link_arrow() {
            h.push(a.points[1]);
            h.push(a.edit_handles(scale)[2]);
        }
        h
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn object(scale: f32) -> Object {
        Object {
            kind: Kind::Sequence,
            bounds: Bounds::from_corners((-100., -100.), (-60., -60.)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::Sequence(SequenceAnnotation {
                value: 1,
                format: SequenceFormat::Decimal,
                shape: SequenceShape::Square,
                size: 16.,
                scale,
                caption: TextAnnotation {
                    content: "说明".into(),
                    font_family: "Microsoft YaHei".into(),
                    font_size: 20.,
                    bold: false,
                    italic: false,
                    outline: None,
                    background: None,
                    rotation: 0.,
                    linked_arrow: None,
                    layout_scale: 1.,
                    linked_arrow_control: None,
                },
                caption_bounds: Bounds::from_corners((-55., -100.), (0., -60.)),
                caption_color: [217, 71, 43],
                caption_custom_color: false,
                caption_ratio: 2,
                guide: true,
                endpoint: Endpoint::FilledArrow,
            }),
        }
    }
    #[test]
    fn placement_creation_and_movement_share_one_undo_and_cancel_restores_selection() {
        for scale in [1., 1.25] {
            let mut session = Session::new();
            session.add(object(scale));
            let original = session.objects.clone();
            session.begin_drag();
            assert!(session.add(object(scale)));
            session.selected = None;
            session.objects[1].move_by(300., -100.);
            session.cancel_drag();
            assert_eq!(session.objects, original);
            assert_eq!(session.selected, Some(0));
            assert_eq!(session.next_sequence, 2);
            session.begin_drag();
            session.add(object(scale));
            session.objects[1].move_by(300., -100.);
            session.selected = Some(1);
            session.finish_drag();
            assert!(session.undo());
            assert_eq!(session.objects, original);
            assert_eq!(session.selected, Some(0));
            assert_eq!(session.next_sequence, 2);
            assert!(session.undo());
            assert!(session.objects.is_empty());
            assert!(!session.can_undo());
        }
    }

    #[test]
    fn creation_undo_rewinds_counter_but_edits_and_deletion_do_not_renumber() {
        let mut s = Session::new();
        s.add(object(1.));
        assert_eq!(s.next_sequence, 2);
        s.update_selected(|o| {
            if let Extra::Sequence(n) = &mut o.extra {
                n.value = 9;
            }
        });
        assert_eq!(s.next_sequence, 2);
        s.delete_selected();
        assert_eq!(s.next_sequence, 2);
        assert!(s.undo());
        assert_eq!(s.next_sequence, 2);
        assert!(s.undo());
        assert!(s.undo());
        assert_eq!(s.next_sequence, 1);
        assert!(s.objects.is_empty());
        assert!(!s.can_undo());
        s.next_sequence = i32::MAX;
        assert!(!s.add(object(1.)));
        assert!(!s.can_undo());
    }
    #[test]
    fn caption_hit_and_curved_connector_follow_badge_at_both_dpi_scales() {
        for scale in [1., 1.25] {
            let mut o = object(scale);
            o.toggle_text_arrow(scale);
            let tip = o.text_link_arrow().unwrap().points[1];
            o.edit_handle(6, (-20., -150.), scale);
            let arrow = o.text_link_arrow().unwrap();
            assert_eq!(arrow.points[1], tip);
            assert_eq!(arrow.edit_handles(scale)[2], (-20., -150.));
            let mut s = Session::new();
            s.add(o.clone());
            s.selected = None;
            assert_eq!(s.hit((-30., -80.), scale), Some(Hit::Object(0)));
            s.begin_drag();
            s.objects[0].move_by(40., 60.);
            s.finish_drag();
            assert_eq!(s.objects[0].text_link_arrow().unwrap().points[1], tip);
            assert!(s.undo());
            assert_eq!(s.objects[0], o);
        }
    }
    #[test]
    fn formats_cross_digit_and_alphabet_boundaries() {
        assert_eq!(sequence_label(27, SequenceFormat::UpperAlpha), "AA");
        assert_eq!(sequence_label(52, SequenceFormat::LowerAlpha), "az");
        assert_eq!(sequence_label(49, SequenceFormat::Roman), "XLIX");
        for (n, s) in [
            (10, "十"),
            (11, "十一"),
            (101, "一百零一"),
            (10001, "一万零一"),
            (100000001, "一亿零一"),
        ] {
            assert_eq!(sequence_label(n, SequenceFormat::Chinese), s);
        }
    }
}
