//! System-font glyph rasterization for the transparent annotation surface.

use chrono::{DateTime, Local};
use std::{cell::RefCell, collections::HashMap};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use fontdb::{Database, Family, ID, Query, Style as FontStyle, Weight};
use lexift_core::domain::{
    annotation::{Bounds, Extra, Object, TextAnnotation, Watermark, WatermarkPosition},
    geometry::Rect,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Transform};

thread_local! { static FONTS: RefCell<FontCatalog> = RefCell::new(FontCatalog::new()); }

struct FontCatalog {
    db: Database,
    bytes: HashMap<ID, (Vec<u8>, u32)>,
    fallback: HashMap<char, ID>,
    text_fallback: HashMap<(char, bool, bool), ID>,
}

impl FontCatalog {
    fn new() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        Self {
            db,
            bytes: HashMap::new(),
            fallback: HashMap::new(),
            text_fallback: HashMap::new(),
        }
    }

    fn font(&mut self, id: ID) -> Option<FontRef<'_>> {
        if !self.bytes.contains_key(&id) {
            let index = self.db.face(id)?.index;
            let data = self.db.with_face_data(id, |bytes, _| bytes.to_vec())?;
            self.bytes.insert(id, (data, index));
        }
        let (bytes, index) = self.bytes.get(&id)?;
        FontRef::try_from_slice_and_index(bytes, *index).ok()
    }

    /// Preserve requested styles when a character needs another font family.
    fn find_text(&mut self, family: &str, bold: bool, italic: bool, ch: char) -> Option<ID> {
        let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        let style = if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let key = (ch, bold, italic);
        let primary = Query {
            families: &[Family::Name(family), Family::SansSerif],
            weight,
            style,
            ..Query::default()
        };
        if let Some(id) = self.db.query(&primary)
            && self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0)
        {
            return Some(id);
        }
        if let Some(&id) = self.text_fallback.get(&key) {
            return Some(id);
        }
        for preferred in [
            "Microsoft YaHei",
            "Noto Sans CJK SC",
            "SimSun",
            "Segoe UI Symbol",
        ] {
            let query = Query {
                families: &[Family::Name(preferred)],
                weight,
                style,
                ..Query::default()
            };
            if let Some(id) = self.db.query(&query)
                && self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0)
            {
                self.text_fallback.insert(key, id);
                return Some(id);
            }
        }
        let mut ids = self
            .db
            .faces()
            .map(|face| (face.id, face.style != style, face.weight != weight))
            .collect::<Vec<_>>();
        ids.sort_by_key(|&(_, style_mismatch, weight_mismatch)| (style_mismatch, weight_mismatch));
        let id = ids
            .into_iter()
            .map(|(id, _, _)| id)
            .find(|&id| self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0))?;
        self.text_fallback.insert(key, id);
        Some(id)
    }

    fn text_shear(&self, id: ID, italic: bool) -> f32 {
        if italic
            && self
                .db
                .face(id)
                .is_some_and(|face| face.style == FontStyle::Normal)
        {
            12_f32.to_radians().tan()
        } else {
            0.
        }
    }

    fn find(&mut self, family: &str, bold: bool, italic: bool, ch: char) -> Option<ID> {
        let name = Family::Name(family);
        let query = Query {
            families: &[name, Family::SansSerif],
            weight: if bold { Weight::BOLD } else { Weight::NORMAL },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..Query::default()
        };
        if let Some(id) = self.db.query(&query)
            && self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0)
        {
            return Some(id);
        }
        if let Some(&id) = self.fallback.get(&ch) {
            return Some(id);
        }
        for preferred in [
            "Microsoft YaHei",
            "Noto Sans CJK SC",
            "SimSun",
            "Segoe UI Symbol",
        ] {
            let query = Query {
                families: &[Family::Name(preferred)],
                ..Query::default()
            };
            if let Some(id) = self.db.query(&query)
                && self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0)
            {
                self.fallback.insert(ch, id);
                return Some(id);
            }
        }
        let ids = self.db.faces().map(|face| face.id).collect::<Vec<_>>();
        let found = ids
            .into_iter()
            .find(|&id| self.font(id).is_some_and(|font| font.glyph_id(ch).0 != 0))?;
        self.fallback.insert(ch, found);
        Some(found)
    }
}

pub(crate) fn font_families() -> Vec<String> {
    FONTS.with(|slot| {
        let fonts = slot.borrow();
        let mut names = fonts
            .db
            .faces()
            .flat_map(|face| face.families.iter().map(|f| f.0.clone()))
            .collect::<Vec<_>>();
        normalize_family_names(&mut names);
        names
    })
}

fn normalize_family_names(names: &mut Vec<String>) {
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
}

/// Return an installed family that fontdb can resolve for its generic sans-serif face.
pub(crate) fn system_sans_family() -> Option<String> {
    FONTS.with(|slot| {
        let fonts = slot.borrow();
        let id = fonts
            .db
            .query(&Query {
                families: &[Family::SansSerif],
                ..Query::default()
            })
            .or_else(|| {
                [
                    "Noto Sans",
                    "DejaVu Sans",
                    "Liberation Sans",
                    "Helvetica Neue",
                    "Helvetica",
                    "Arial",
                ]
                .into_iter()
                .find_map(|family| {
                    fonts.db.query(&Query {
                        families: &[Family::Name(family)],
                        ..Query::default()
                    })
                })
            })?;
        fonts
            .db
            .face(id)?
            .families
            .first()
            .map(|family| family.0.clone())
    })
}

fn glyph_advance(
    fonts: &mut FontCatalog,
    family: &str,
    size: f32,
    bold: bool,
    italic: bool,
    ch: char,
) -> f32 {
    let Some(id) = fonts.find_text(family, bold, italic, ch) else {
        return size * 0.6;
    };
    let Some(font) = fonts.font(id) else {
        return size * 0.6;
    };
    font.as_scaled(PxScale::from(size))
        .h_advance(font.glyph_id(ch))
}

/// The identical outline path determines visual bounds and paints the glyph.
fn glyph_path(
    outline: ab_glyph::Outline,
    x: f32,
    y: f32,
    hscale: f32,
    vscale: f32,
    shear: f32,
) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    let mut last = None;
    let map = |p: ab_glyph::Point| (x + p.x * hscale + p.y * vscale * shear, y - p.y * vscale);
    for curve in outline.curves {
        use ab_glyph::OutlineCurve;
        let first = match &curve {
            OutlineCurve::Line(a, _)
            | OutlineCurve::Quad(a, _, _)
            | OutlineCurve::Cubic(a, _, _, _) => *a,
        };
        if last != Some(first) {
            if last.is_some() {
                builder.close();
            }
            let (px, py) = map(first);
            builder.move_to(px, py);
        }
        last = Some(match curve {
            OutlineCurve::Line(_, b) => {
                let (px, py) = map(b);
                builder.line_to(px, py);
                b
            }
            OutlineCurve::Quad(_, b, c) => {
                let (bx, by) = map(b);
                let (cx, cy) = map(c);
                builder.quad_to(bx, by, cx, cy);
                c
            }
            OutlineCurve::Cubic(_, b, c, d) => {
                let (bx, by) = map(b);
                let (cx, cy) = map(c);
                let (dx, dy) = map(d);
                builder.cubic_to(bx, by, cx, cy, dx, dy);
                d
            }
        });
    }
    builder.close();
    builder.finish()
}

/// Physical line geometry shared by glyphs, caret, selection and pointer hit testing.
struct TextLayout {
    ink_bounds: Option<Bounds>,
    baseline: f32,
    line_height: f32,
    ascent: f32,
    descent: f32,
    padding: f32,
    height: f32,
    width: f32,
    lines: Vec<Vec<(usize, f32)>>,
}

fn text_layout(text: &TextAnnotation, scale: f32) -> TextLayout {
    let size = text.font_size * scale;
    let padding = text_padding(text, scale);
    let line_height = size * 1.25;
    FONTS.with(|slot| {
        let mut fonts = slot.borrow_mut();
        let mut ascent = size * 0.8;
        let mut descent = -size * 0.2;
        if let Some(id) = fonts.find_text(&text.font_family, text.bold, text.italic, 'M')
            && let Some(font) = fonts.font(id)
        {
            let scaled = font.as_scaled(size);
            ascent = scaled.ascent();
            descent = scaled.descent();
        }
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        let mut left = 0_f32;
        let mut right = 0_f32;
        let mut lines = Vec::new();
        let mut ink_bounds: Option<Bounds> = None;
        let mut offset = 0;
        for (row, line) in text.content.split('\n').enumerate() {
            let mut positions = vec![(offset, padding)];
            let mut x = padding;
            let mut has_ink = false;
            for ch in line.chars() {
                if let Some(id) = fonts.find_text(&text.font_family, text.bold, text.italic, ch) {
                    let shear = fonts.text_shear(id, text.italic);
                    if let Some(font) = fonts.font(id)
                        && let Some(outline) = font.outline(font.glyph_id(ch))
                    {
                        let scaled = font.as_scaled(size);
                        if let Some(bounds) = glyph_path(
                            outline,
                            0.,
                            0.,
                            scaled.h_scale_factor(),
                            scaled.v_scale_factor(),
                            shear,
                        )
                        .and_then(|path| path.compute_tight_bounds())
                        {
                            let ink = Bounds::from_corners(
                                (x + bounds.left(), row as f32 * line_height + bounds.top()),
                                (
                                    x + bounds.right(),
                                    row as f32 * line_height + bounds.bottom(),
                                ),
                            );
                            ink_bounds = Some(ink_bounds.map_or(ink, |previous| Bounds {
                                left: previous.left.min(ink.left),
                                top: previous.top.min(ink.top),
                                right: previous.right.max(ink.right),
                                bottom: previous.bottom.max(ink.bottom),
                            }));
                            top = top.min(row as f32 * line_height + bounds.top());
                            bottom = bottom.max(row as f32 * line_height + bounds.bottom());
                            left = left.min(x - padding + bounds.left());
                            right = right.max(x - padding + bounds.right());
                            has_ink = true;
                        }
                    }
                }
                x += glyph_advance(
                    &mut fonts,
                    &text.font_family,
                    size,
                    text.bold,
                    text.italic,
                    ch,
                );
                offset += ch.len_utf8();
                positions.push((offset, x));
            }
            if !has_ink {
                top = top.min(row as f32 * line_height - ascent);
                bottom = bottom.max(row as f32 * line_height - descent);
            }
            right = right.max(x - padding);
            lines.push(positions);
            offset += 1; // Explicit newline, absent after the final line.
        }
        for positions in &mut lines {
            for (_, x) in positions {
                *x -= left;
            }
        }
        let height = lines.len() as f32 * line_height;
        let baseline = padding + (height - (bottom - top)) * 0.5 - top;
        TextLayout {
            ink_bounds: ink_bounds.map(|bounds| bounds.moved(-left, baseline)),
            baseline,
            line_height,
            ascent,
            descent,
            padding,
            height: height + padding * 2.,
            width: right - left + padding * 2.,
            lines,
        }
    })
}

/// Visible fill extents in local physical coordinates, from the same paths as rendering.
/// Input width, caret reserve and font side bearings are deliberately excluded.
pub(crate) fn text_ink_bounds(text: &TextAnnotation, scale: f32) -> Option<Bounds> {
    text_layout(text, scale).ink_bounds
}

fn layout_with_reference(text: &TextAnnotation, scale: f32, reference: Option<&str>) -> TextLayout {
    let mut layout = text_layout(text, scale);
    if let Some(content) = reference {
        let mut confirmed = text.clone();
        confirmed.content = content.to_owned();
        let stable = text_layout(&confirmed, scale);
        layout.baseline = stable.baseline;
        layout.ascent = stable.ascent;
        layout.descent = stable.descent;
    }
    layout
}

/// Returns a UTF-8 insertion offset in the exact geometry used on the canvas.
pub(crate) fn caret_at(object: &Object, point: (f32, f32)) -> usize {
    let Extra::Text(text) = &object.extra else {
        return 0;
    };
    let point =
        lexift_core::domain::annotation::rotate_text_point(point, object.bounds, -text.rotation);
    let layout = text_layout(text, object_scale(object));
    let y = point.1 - object.bounds.top;
    let row = ((y - layout.baseline + (layout.ascent + layout.descent) * 0.5) / layout.line_height)
        .round()
        .clamp(0., layout.lines.len() as f32 - 1.) as usize;
    let x = point.0 - object.bounds.left;
    layout.lines[row]
        .iter()
        .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
        .map_or(0, |p| p.0)
}

pub(crate) fn draw_selection(
    target: &mut Pixmap,
    object: &Object,
    range: std::ops::Range<usize>,
    monitor: Rect,
) {
    let Extra::Text(text) = &object.extra else {
        return;
    };
    if range.is_empty() {
        return;
    }
    let layout = text_layout(text, object_scale(object));
    let center = object.bounds.center();
    let transform = Transform::from_translate(
        object.bounds.left - monitor.left as f32,
        object.bounds.top - monitor.top as f32,
    )
    .post_concat(Transform::from_rotate_at(
        text.rotation.to_degrees(),
        center.0 - monitor.left as f32,
        center.1 - monitor.top as f32,
    ));
    let mut paint = Paint::default();
    paint.set_color_rgba8(55, 130, 255, 75);
    for (row, positions) in layout.lines.iter().enumerate() {
        let left = positions.iter().find(|p| p.0 >= range.start).map(|p| p.1);
        let right = positions
            .iter()
            .rev()
            .find(|p| p.0 <= range.end)
            .map(|p| p.1);
        if let (Some(left), Some(right)) = (left, right) {
            let baseline = layout.baseline + row as f32 * layout.line_height;
            let top = (baseline - layout.ascent).max(layout.padding);
            let bottom = (baseline - layout.descent).min(layout.height - layout.padding);
            if let Some(rect) = SkRect::from_xywh(left, top, right - left, bottom - top) {
                target.fill_rect(rect, &paint, transform, None);
            }
        }
    }
}

/// Measures explicit lines. The input box expands as the user types; wrapping is deliberate only.
pub(crate) fn measure(
    content: &str,
    family: &str,
    size: f32,
    bold: bool,
    italic: bool,
    padding: f32,
) -> (f32, f32) {
    FONTS.with(|slot| {
        let mut fonts = slot.borrow_mut();
        let width = content
            .split('\n')
            .map(|line| {
                line.chars()
                    .map(|ch| {
                        let Some(id) = fonts.find(family, bold, italic, ch) else {
                            return size * 0.6;
                        };
                        fonts.font(id).map_or(size * 0.6, |font| {
                            font.as_scaled(size).h_advance(font.glyph_id(ch))
                        })
                    })
                    .sum::<f32>()
            })
            .fold(0f32, f32::max);
        let lines = content.split('\n').count() as f32;
        // Slint's TextInput and ab_glyph can resolve different faces for the
        // same family. Leave room for the caret and wider fallback glyphs so
        // the native editor never clips text while the user is composing.
        let width = width * 1.25 + size * 0.4;
        (
            (width + padding * 2.).max(size + padding * 2.),
            lines * size * 1.25 + padding * 2.,
        )
    })
}

/// Measures the same advances and baselines used by `raster`, with only a
/// small trailing allowance for the insertion caret.
fn measure_text(content: &str, text: &TextAnnotation, scale: f32, padding: f32) -> (f32, f32) {
    let size = text.font_size * scale;
    let mut measured = text.clone();
    measured.content = content.to_owned();
    let layout = text_layout(&measured, scale);
    (
        (layout.width + 3. * scale * text.layout_scale).max(size + padding * 2.),
        layout.height,
    )
}

pub(crate) fn text_padding(text: &TextAnnotation, scale: f32) -> f32 {
    let base = text
        .background
        .as_ref()
        .map_or(2. * scale * text.layout_scale, |bg| bg.padding * scale);
    base + text
        .outline
        .as_ref()
        .map_or(0., |outline| outline.width * scale)
}

/// Physical object bounds retain their layout DPI when moved across monitors.
pub(crate) fn object_scale(object: &Object) -> f32 {
    let Extra::Text(text) = &object.extra else {
        return 1.;
    };
    let logical_height = text.content.split('\n').count() as f32 * text.font_size * 1.25
        + 2. * text_padding(text, 1.);
    let scale = object.bounds.height() / logical_height;
    if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    }
}

/// IME text is temporary; byte offsets are clamped to valid UTF-8 boundaries.
pub(crate) fn composition_preview(
    confirmed: &str,
    preedit: &str,
    offset: usize,
) -> (String, std::ops::Range<usize>) {
    let start = confirmed.floor_char_boundary(offset.min(confirmed.len()));
    let mut display = confirmed.to_owned();
    display.insert_str(start, preedit);
    (display, start..start + preedit.len())
}

/// Uses the same advances, line spacing and rotation as the editing caret.
pub(crate) fn draw_composition_underline(
    target: &mut Pixmap,
    object: &Object,
    range: std::ops::Range<usize>,
    monitor: Rect,
    _scale: f32,
) {
    let scale = object_scale(object);
    if range.is_empty() {
        return;
    }
    let Extra::Text(text) = &object.extra else {
        return;
    };
    let size = text.font_size * scale;
    let mut confirmed = text.content.clone();
    confirmed.replace_range(range.clone(), "");
    let layout = layout_with_reference(text, scale, Some(&confirmed));
    let padding = layout.padding;
    let mut path = PathBuilder::new();
    let mut x = layout.lines[0][0].1;
    let mut row = 0.;
    FONTS.with(|slot| {
        let mut fonts = slot.borrow_mut();
        for (offset, ch) in text.content.char_indices() {
            if ch == '\n' {
                row += 1.;
                x = layout.lines[row as usize][0].1;
                continue;
            }
            let advance = glyph_advance(
                &mut fonts,
                &text.font_family,
                size,
                text.bold,
                text.italic,
                ch,
            );
            if range.contains(&offset) {
                let y = object.bounds.top - monitor.top as f32
                    + (layout.baseline + row * layout.line_height - layout.descent)
                        .min(layout.height - padding);
                let left = object.bounds.left - monitor.left as f32 + x;
                path.move_to(left, y);
                path.line_to(left + advance, y);
            }
            x += advance;
        }
    });
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(
            object.style.color[0],
            object.style.color[1],
            object.style.color[2],
            255,
        );
        let center = object.bounds.center();
        target.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: scale.max(1.),
                ..Default::default()
            },
            Transform::from_rotate_at(
                text.rotation.to_degrees(),
                center.0 - monitor.left as f32,
                center.1 - monitor.top as f32,
            ),
            None,
        );
    }
}

pub(crate) fn draw_caret(
    target: &mut Pixmap,
    object: &Object,
    offset: usize,
    monitor: Rect,
    _scale: f32,
    reference: Option<&str>,
) {
    let scale = object_scale(object);
    let Extra::Text(text) = &object.extra else {
        return;
    };
    let end = text
        .content
        .floor_char_boundary(offset.min(text.content.len()));
    let layout = layout_with_reference(text, scale, reference);
    let row = text.content[..end].bytes().filter(|&b| b == b'\n').count();
    let advance = layout.lines[row]
        .iter()
        .find(|p| p.0 == end)
        .map_or(layout.padding, |p| p.1);
    let baseline = layout.baseline + row as f32 * layout.line_height;
    let x = object.bounds.left - monitor.left as f32 + advance;
    let top =
        object.bounds.top - monitor.top as f32 + (baseline - layout.ascent).max(layout.padding);
    let bottom = object.bounds.top - monitor.top as f32
        + (baseline - layout.descent).min(layout.height - layout.padding);
    let mut path = PathBuilder::new();
    path.move_to(x, top);
    path.line_to(x, bottom);
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(
            object.style.color[0],
            object.style.color[1],
            object.style.color[2],
            255,
        );
        let center = object.bounds.center();
        target.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: scale.max(1.),
                ..Default::default()
            },
            Transform::from_rotate_at(
                text.rotation.to_degrees(),
                center.0 - monitor.left as f32,
                center.1 - monitor.top as f32,
            ),
            None,
        );
    }
}

fn blend_pixel(
    data: &mut [u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    color: [u8; 3],
    alpha: f32,
) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        return;
    }
    let at = (y as usize * width + x as usize) * 4;
    let sa = alpha.clamp(0., 1.);
    let remain = 1. - sa;
    for channel in 0..3 {
        data[at + channel] = ((color[channel] as f32 * sa + data[at + channel] as f32 * remain)
            .round() as u32)
            .min(255) as u8;
    }
    data[at + 3] = ((255. * sa + data[at + 3] as f32 * remain).round() as u32).min(255) as u8;
}

fn glyph(
    pixmap: &mut Pixmap,
    font: &FontRef<'_>,
    ch: char,
    size: f32,
    position: (f32, f32),
    color: [u8; 3],
    outline: Option<([u8; 3], f32)>,
) {
    let glyph = font
        .glyph_id(ch)
        .with_scale_and_position(PxScale::from(size), point(position.0, position.1));
    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    let data = pixmap.data_mut();
    if let Some((stroke_color, stroke_width)) = outline {
        let radius = stroke_width.round().clamp(1., 12.) as i32;
        outlined.draw(|gx, gy, coverage| {
            if coverage <= 0. {
                return;
            }
            let x = bounds.min.x as i32 + gx as i32;
            let y = bounds.min.y as i32 + gy as i32;
            for oy in -radius..=radius {
                for ox in -radius..=radius {
                    if ox * ox + oy * oy <= radius * radius {
                        blend_pixel(data, w, h, x + ox, y + oy, stroke_color, coverage);
                    }
                }
            }
        });
    }
    outlined.draw(|gx, gy, coverage| {
        blend_pixel(
            data,
            w,
            h,
            bounds.min.x as i32 + gx as i32,
            bounds.min.y as i32 + gy as i32,
            color,
            coverage,
        );
    });
}

fn background_path(w: f32, h: f32, rounding: f32) -> Option<tiny_skia::Path> {
    let r = rounding.clamp(0., w.min(h) / 2.);
    let rect = SkRect::from_xywh(0., 0., w, h)?;
    let path = if r < 0.5 {
        PathBuilder::from_rect(rect)
    } else {
        let k = r * 0.552_284_8;
        let mut p = PathBuilder::new();
        p.move_to(r, 0.);
        p.line_to(w - r, 0.);
        p.cubic_to(w - r + k, 0., w, r - k, w, r);
        p.line_to(w, h - r);
        p.cubic_to(w, h - r + k, w - r + k, h, w - r, h);
        p.line_to(r, h);
        p.cubic_to(r - k, h, 0., h - r + k, 0., h - r);
        p.line_to(0., r);
        p.cubic_to(0., r - k, r - k, 0., r, 0.);
        p.close();
        p.finish()?
    };
    Some(path)
}

fn background(pixmap: &mut Pixmap, color: [u8; 3], opacity: u8, rounding: f32) {
    if opacity == 0 {
        return;
    }
    let Some(path) = background_path(pixmap.width() as f32, pixmap.height() as f32, rounding)
    else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], opacity);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

struct RasterStyle<'a> {
    content: &'a str,
    family: &'a str,
    size: f32,
    bold: bool,
    italic: bool,
    padding: f32,
    color: [u8; 3],
    outline: Option<([u8; 3], f32)>,
    background: Option<([u8; 3], u8, f32)>,
}

fn raster(style: &RasterStyle<'_>, canvas_size: (u32, u32)) -> Option<Pixmap> {
    if canvas_size.0 == 0
        || canvas_size.1 == 0
        || canvas_size.0 as u64 * canvas_size.1 as u64 > 8_000_000
    {
        return None;
    }
    let mut pixmap = Pixmap::new(canvas_size.0, canvas_size.1)?;
    if let Some((color, opacity, rounding)) = style.background {
        background(&mut pixmap, color, opacity, rounding);
    }
    FONTS.with(|slot| {
        let mut fonts = slot.borrow_mut();
        for (line_no, line) in style.content.split('\n').enumerate() {
            let mut x = style.padding;
            let y = style.padding + style.size + line_no as f32 * style.size * 1.25;
            for ch in line.chars() {
                if let Some(id) = fonts.find(style.family, style.bold, style.italic, ch)
                    && let Some(font) = fonts.font(id)
                {
                    glyph(
                        &mut pixmap,
                        &font,
                        ch,
                        style.size,
                        (x, y),
                        style.color,
                        style.outline,
                    );
                    x += font
                        .as_scaled(PxScale::from(style.size))
                        .h_advance(font.glyph_id(ch));
                } else {
                    x += style.size * 0.6;
                }
            }
        }
    });
    Some(pixmap)
}

fn composite(
    target: &mut Pixmap,
    source: &Pixmap,
    bounds: Bounds,
    angle: f32,
    monitor: Rect,
    opacity: u8,
) {
    let (sin, cos) = angle.sin_cos();
    let (cx, cy) = (bounds.width() / 2., bounds.height() / 2.);
    let tx = bounds.left - monitor.left as f32 + cx - cos * cx + sin * cy;
    let ty = bounds.top - monitor.top as f32 + cy - sin * cx - cos * cy;
    let paint = PixmapPaint {
        opacity: opacity as f32 / 100.,
        ..PixmapPaint::default()
    };
    target.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &paint,
        Transform::from_row(cos, sin, -sin, cos, tx, ty),
        None,
    );
}

/// Paints glyph contours directly into the monitor surface. Memory stays bounded by
/// the visible canvas, even when the logical text frame is much larger than a screen.
pub(crate) fn draw_text(target: &mut Pixmap, object: &Object, monitor: Rect, _scale: f32) {
    draw_text_with_reference(target, object, monitor, _scale, None);
}

pub(crate) fn draw_text_with_reference(
    target: &mut Pixmap,
    object: &Object,
    monitor: Rect,
    _scale: f32,
    reference: Option<&str>,
) {
    let scale = object_scale(object);
    let Extra::Text(text) = &object.extra else {
        return;
    };
    let size = text.font_size * scale;
    if !size.is_finite() || size <= 0. {
        return;
    }
    let center = object.bounds.center();
    let transform = Transform::from_translate(
        object.bounds.left - monitor.left as f32,
        object.bounds.top - monitor.top as f32,
    )
    .post_concat(Transform::from_rotate_at(
        text.rotation.to_degrees(),
        center.0 - monitor.left as f32,
        center.1 - monitor.top as f32,
    ));
    if let Some(bg) = &text.background
        && let Some(path) = background_path(
            object.bounds.width(),
            object.bounds.height(),
            bg.rounding * scale,
        )
    {
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg.color[0], bg.color[1], bg.color[2], bg.opacity);
        target.fill_path(&path, &paint, FillRule::Winding, transform, None);
    }
    let layout = layout_with_reference(text, scale, reference);
    let paths = FONTS.with(|slot| {
        let mut fonts = slot.borrow_mut();
        let mut paths = Vec::new();
        for (row, line) in text.content.split('\n').enumerate() {
            let mut x = layout.lines[row][0].1;
            let y = layout.baseline + row as f32 * layout.line_height;
            for ch in line.chars() {
                if let Some(id) = fonts.find_text(&text.font_family, text.bold, text.italic, ch) {
                    let shear = fonts.text_shear(id, text.italic);
                    if let Some(font) = fonts.font(id) {
                        let scaled = font.as_scaled(PxScale::from(size));
                        let glyph_id = font.glyph_id(ch);
                        if let Some(outline) = font.outline(glyph_id)
                            && let Some(path) = glyph_path(
                                outline,
                                x,
                                y,
                                scaled.h_scale_factor(),
                                scaled.v_scale_factor(),
                                shear,
                            )
                        {
                            paths.push(path);
                        }
                        x += scaled.h_advance(glyph_id);
                        continue;
                    }
                }
                x += size * 0.6;
            }
        }
        paths
    });
    // All strokes must be behind every fill, including neighboring glyphs.
    let mut paint = Paint::default();
    if let Some(outline) = &text.outline {
        paint.set_color_rgba8(outline.color[0], outline.color[1], outline.color[2], 255);
        let stroke = tiny_skia::Stroke {
            width: 2. * outline.width * scale,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        for path in &paths {
            target.stroke_path(path, &paint, &stroke, transform, None);
        }
    }
    paint.set_color_rgba8(
        object.style.color[0],
        object.style.color[1],
        object.style.color[2],
        255,
    );
    for path in &paths {
        target.fill_path(path, &paint, FillRule::Winding, transform, None);
    }
}

pub(crate) fn draw_watermark(
    target: &mut Pixmap,
    watermark: &Watermark,
    region: Rect,
    layout_monitor: Rect,
    scale: f32,
) {
    if watermark.content.is_empty() || watermark.opacity == 0 {
        return;
    }
    let size = (watermark.font_size * scale).clamp(5., 144.);
    let (w, h) = measure(
        &watermark.content,
        &watermark.font_family,
        size,
        false,
        false,
        0.,
    );
    let (width, height) = (w.ceil() as u32, h.ceil() as u32);
    let style = RasterStyle {
        content: &watermark.content,
        family: &watermark.font_family,
        size,
        bold: false,
        italic: false,
        padding: 0.,
        color: watermark.color,
        outline: None,
        background: None,
    };
    let Some(image) = raster(&style, (width, height)) else {
        return;
    };
    let left = layout_monitor.left as f32;
    let top = layout_monitor.top as f32;
    let mw = (layout_monitor.right - layout_monitor.left) as f32;
    let mh = (layout_monitor.bottom - layout_monitor.top) as f32;
    let margin = 16. * scale;
    if watermark.position == WatermarkPosition::Tile {
        let step_x = (w * 1.75).max(160. * scale);
        let step_y = (h * 3.5).max(90. * scale);
        let mut y = top - w;
        while y < top + mh + w {
            let mut x = left - w;
            while x < left + mw + w {
                composite(
                    target,
                    &image,
                    Bounds {
                        left: x,
                        top: y,
                        right: x + w,
                        bottom: y + h,
                    },
                    -std::f32::consts::FRAC_PI_4,
                    region,
                    watermark.opacity,
                );
                x += step_x;
            }
            y += step_y;
        }
        return;
    }
    let x = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::BottomLeft => left + margin,
        WatermarkPosition::TopRight | WatermarkPosition::BottomRight => left + mw - w - margin,
        _ => left + (mw - w) / 2.,
    };
    let y = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::TopRight | WatermarkPosition::TopCenter => {
            top + margin
        }
        WatermarkPosition::BottomLeft
        | WatermarkPosition::BottomRight
        | WatermarkPosition::BottomCenter => top + mh - h - margin,
        _ => top + (mh - h) / 2.,
    };
    composite(
        target,
        &image,
        Bounds {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        0.,
        region,
        watermark.opacity,
    );
}

pub(crate) fn text_bounds(at: (f32, f32), text: &TextAnnotation, scale: f32) -> Bounds {
    let padding = text_padding(text, scale);
    let (w, h) = measure_text(&text.content, text, scale, padding);
    Bounds {
        left: at.0,
        top: at.1,
        right: at.0 + w,
        bottom: at.1 + h,
    }
}

/// Expands the reference application's `$...$` local-time fragments once on Apply.
pub(crate) fn resolve_watermark_template(template: &str, at: DateTime<Local>) -> String {
    let mut result = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('$') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + 1..];
        if remaining.starts_with('$') {
            result.push('$');
            remaining = &remaining[1..];
        } else if let Some(end) = remaining.find('$') {
            let format = &remaining[..end];
            let mut converted = String::new();
            let mut input = format;
            while !input.is_empty() {
                let matched = [
                    ("yyyy", "%Y"),
                    ("EEEE", "%A"),
                    ("SSS", "%3f"),
                    ("MM", "%m"),
                    ("dd", "%d"),
                    ("HH", "%H"),
                    ("hh", "%I"),
                    ("mm", "%M"),
                    ("ss", "%S"),
                    ("a", "%p"),
                ]
                .into_iter()
                .find(|(token, _)| input.starts_with(token));
                if let Some((token, output)) = matched {
                    converted.push_str(output);
                    input = &input[token.len()..];
                } else {
                    let ch = input.chars().next().unwrap();
                    if ch == '%' {
                        converted.push('%');
                    }
                    converted.push(ch);
                    input = &input[ch.len_utf8()..];
                }
            }
            result.push_str(&at.format(&converted).to_string());
            remaining = &remaining[end + 1..];
        } else {
            result.push('$');
            result.push_str(remaining);
            return result;
        }
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn family_names_sort_stably_and_deduplicate_case_variants() {
        let mut names = [
            "微软雅黑",
            "zeta",
            "arial",
            "Arial",
            "École",
            "école",
            "Beta",
            "Arial",
        ]
        .map(String::from)
        .to_vec();
        normalize_family_names(&mut names);
        assert_eq!(names, ["Arial", "Beta", "zeta", "École", "微软雅黑"]);
    }
    use super::*;
    use chrono::TimeZone;
    use lexift_core::domain::annotation::{Kind, Style, TextOutline};

    fn layout_test_object(content: &str, size: f32, dpi: f32) -> Object {
        let text = TextAnnotation {
            content: content.into(),
            font_family: "Microsoft YaHei".into(),
            font_size: size,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            rotation: 0.,
            layout_scale: 1.,
            linked_arrow: None,
            linked_arrow_control: None,
        };
        Object {
            bounds: text_bounds((-300., -40.), &text, dpi),
            kind: Kind::Text,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Text(text),
        }
    }

    #[test]
    fn thick_outlines_never_cover_neighboring_glyph_fills() {
        for dpi in [1., 1.25] {
            let mut object = layout_test_object("中国 AV\n实例 gj", 38., dpi);
            let Extra::Text(text) = &mut object.extra else {
                unreachable!()
            };
            text.outline = Some(TextOutline {
                color: [30, 170, 40],
                width: 20.,
            });
            object.bounds = text_bounds((-300., -40.), text, dpi);
            object.style.color = [220, 40, 20];
            let monitor = Rect {
                left: -320,
                top: -60,
                right: 320,
                bottom: 260,
            };
            let mut outlined = Pixmap::new(640, 320).unwrap();
            draw_text(&mut outlined, &object, monitor, dpi);
            // Keep identical padding, positions and baselines for a fill-only reference.
            if let Extra::Text(text) = &mut object.extra {
                text.outline.as_mut().unwrap().width = 0.;
            }
            // Zero-width outline must retain original geometry; background padding substitutes it.
            if let Extra::Text(text) = &mut object.extra {
                text.background = Some(lexift_core::domain::annotation::TextBackground {
                    color: [0, 0, 0],
                    opacity: 0,
                    rounding: 0.,
                    padding: 22.,
                });
            }
            let mut filled = Pixmap::new(640, 320).unwrap();
            draw_text(&mut filled, &object, monitor, dpi);
            let mut checked = 0;
            for (a, b) in filled.pixels().iter().zip(outlined.pixels()) {
                if a.alpha() == 255 && a.red() == 220 && a.green() == 40 && a.blue() == 20 {
                    assert_eq!(a, b, "neighbor stroke overwrote a fully covered fill");
                    checked += 1;
                }
            }
            assert!(checked > 100);
        }
    }

    #[test]
    fn italic_faces_and_fallback_keep_style_and_shared_caret_geometry() {
        let mut fonts = FontCatalog::new();
        if fonts.db.faces().next().is_none() {
            return;
        }
        let normal = fonts
            .find_text("Microsoft YaHei", false, false, '中')
            .unwrap();
        assert_eq!(fonts.text_shear(normal, false), 0.);
        let synthetic = fonts
            .find_text("Microsoft YaHei", false, true, '中')
            .unwrap();
        if fonts.db.face(synthetic).unwrap().style == FontStyle::Normal {
            assert!((fonts.text_shear(synthetic, true) - 12_f32.to_radians().tan()).abs() < 0.0001);
        }
        let real = fonts.find_text("Arial", false, true, 'A').unwrap();
        if fonts.db.face(real).unwrap().style != FontStyle::Normal {
            assert_eq!(fonts.text_shear(real, true), 0.);
        }
        for bold in [false, true] {
            fonts.find_text("Arial", bold, false, '中').unwrap();
            let id = fonts.find_text("Arial", bold, true, '中').unwrap();
            assert_eq!(fonts.text_fallback.get(&('中', bold, true)), Some(&id));
        }
        for dpi in [1., 1.25] {
            let mut object = layout_test_object("中国 gj\nAV文字", 200., dpi);
            let Extra::Text(text) = &mut object.extra else {
                unreachable!()
            };
            text.italic = true;
            text.rotation = 0.4;
            object.bounds = text_bounds((-300., -40.), text, dpi);
            let layout = text_layout(text, dpi);
            let rotation = text.rotation;
            assert!(layout.width <= object.bounds.width());
            for (row, positions) in layout.lines.iter().enumerate() {
                for &(offset, x) in positions {
                    let point = lexift_core::domain::annotation::rotate_text_point(
                        (
                            object.bounds.left + x,
                            object.bounds.top + layout.baseline + row as f32 * layout.line_height
                                - (layout.ascent + layout.descent) * 0.5,
                        ),
                        object.bounds,
                        rotation,
                    );
                    assert_eq!(caret_at(&object, point), offset);
                }
            }
        }
    }

    #[test]
    fn pointer_uses_shared_multiline_utf8_positions_after_rotation() {
        for dpi in [1., 1.25] {
            let mut object = layout_test_object("实例 gj\n文字 A", 32., dpi);
            if let Extra::Text(text) = &mut object.extra {
                text.rotation = 0.65;
            }
            let Extra::Text(text) = &object.extra else {
                unreachable!()
            };
            let layout = text_layout(text, dpi);
            for (row, positions) in layout.lines.iter().enumerate() {
                for &(offset, x) in positions {
                    let point = lexift_core::domain::annotation::rotate_text_point(
                        (
                            object.bounds.left + x,
                            object.bounds.top + layout.baseline + row as f32 * layout.line_height,
                        ),
                        object.bounds,
                        text.rotation,
                    );
                    assert_eq!(caret_at(&object, point), offset);
                }
            }
        }
    }

    #[test]
    fn empty_caption_caret_renders_at_both_dpis_and_large_sizes() {
        let monitor = Rect {
            left: -400,
            top: -300,
            right: 600,
            bottom: 500,
        };
        for dpi in [1., 1.25] {
            for size in [22., 220.] {
                let object = layout_test_object("", size, dpi);
                let mut frame = Pixmap::new(1000, 800).unwrap();
                draw_caret(&mut frame, &object, 0, monitor, dpi, None);
                assert!(frame.pixels().iter().any(|pixel| pixel.alpha() > 0));
            }
        }
    }

    #[test]
    fn empty_draft_geometry_survives_scaling_and_accepts_text() {
        for dpi in [1., 1.25] {
            let mut object = layout_test_object("", 22., dpi);
            let original = object.bounds;
            let handle = object.edit_handles(dpi)[3];
            object.edit_handle(
                3,
                (handle.0 + original.width(), handle.1 + original.height()),
                dpi,
            );
            assert!((object.bounds.width() / original.width() - 2.).abs() < 0.001);
            let Extra::Text(text) = &mut object.extra else {
                unreachable!()
            };
            assert_eq!(text.content, "");
            assert!((text.font_size - 44.).abs() < 0.001);
            text.content = "实例文本".into();
            let filled = text_bounds((object.bounds.left, object.bounds.top), text, dpi);
            assert_eq!(filled.height(), object.bounds.height());
        }
    }

    #[test]
    fn rendered_ink_has_equal_vertical_margins_at_small_and_large_sizes() {
        for dpi in [1., 1.25] {
            for size in [22., 220.] {
                let mut object = layout_test_object("实例文本", size, dpi);
                object.bounds = text_bounds(
                    (20., 20.),
                    match &object.extra {
                        Extra::Text(text) => text,
                        _ => unreachable!(),
                    },
                    dpi,
                );
                let mut frame = Pixmap::new(1400, 500).unwrap();
                draw_text(
                    &mut frame,
                    &object,
                    Rect {
                        left: 0,
                        top: 0,
                        right: 1400,
                        bottom: 500,
                    },
                    dpi,
                );
                let rows = (0..500)
                    .filter(|&y| {
                        frame.pixels()[y * 1400..(y + 1) * 1400]
                            .iter()
                            .any(|p| p.alpha() > 20)
                    })
                    .collect::<Vec<_>>();
                let top = *rows.first().unwrap() as f32 - object.bounds.top;
                let bottom = object.bounds.bottom - *rows.last().unwrap() as f32 - 1.;
                assert!(
                    (top - bottom).abs() <= 2.,
                    "dpi={dpi} size={size} margins={top}/{bottom}"
                );
            }
        }
    }

    #[test]
    fn very_large_text_paints_visible_pixels_without_a_full_frame_bitmap() {
        for dpi in [1., 1.25] {
            let mut text = TextAnnotation {
                content: "示例文本".into(),
                font_family: "Microsoft YaHei".into(),
                font_size: 22.,
                bold: false,
                italic: false,
                outline: None,
                background: None,
                rotation: 0.,
                layout_scale: 1.,
                linked_arrow: None,
                linked_arrow_control: None,
            };
            let before = text_bounds((-10., -10.), &text, dpi);
            let pad = text_padding(&text, dpi);
            text.font_size *= 100.;
            text.layout_scale *= 100.;
            let after = text_bounds((-10., -10.), &text, dpi);
            assert!((after.width() / before.width() - 100.).abs() < 0.01);
            assert!((after.height() / before.height() - 100.).abs() < 0.01);
            assert!((text_padding(&text, dpi) / pad - 100.).abs() < 0.01);
            let object = Object {
                bounds: after,
                kind: Kind::Text,
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text),
            };
            assert!(after.width() * after.height() > 8_000_000.);
            let mut frame = Pixmap::new(1600, 1600).unwrap();
            draw_text(
                &mut frame,
                &object,
                Rect {
                    left: 0,
                    top: 0,
                    right: 1600,
                    bottom: 1600,
                },
                dpi,
            );
            assert!(frame.pixels().iter().any(|pixel| pixel.alpha() > 0));
        }
    }

    #[test]
    fn composition_preserves_confirmed_text_and_utf8_offsets() {
        let confirmed = "实例文本\n下一行";
        let (display, range) = composition_preview(confirmed, "wen'zi", 6);
        assert_eq!(display, "实例wen'zi文本\n下一行");
        assert_eq!(&display[range], "wen'zi");
        assert_eq!(composition_preview(confirmed, "", 6).0, confirmed);
        assert_eq!(composition_preview("文字", "pin", 1).0, "pin文字");
        assert_eq!(composition_preview("文字", "pin", usize::MAX).0, "文字pin");
    }

    #[test]
    fn composition_bounds_expand_and_restore_at_both_dpi_scales() {
        let text = TextAnnotation {
            content: "实例文本".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 22.,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            rotation: 0.,
            layout_scale: 1.,
            linked_arrow_control: None,
            linked_arrow: None,
        };
        for scale in [1., 1.25] {
            let original = text_bounds((-300., -40.), &text, scale);
            let mut preview = text.clone();
            preview.content =
                composition_preview(&text.content, "changpinyin", text.content.len()).0;
            let expanded = text_bounds((-300., -40.), &preview, scale);
            assert!(expanded.width() > original.width());
            assert_eq!(expanded.left, original.left);
            assert_eq!(expanded.top, original.top);
            preview.content = composition_preview(&text.content, "", text.content.len()).0;
            assert_eq!(text_bounds((-300., -40.), &preview, scale), original);
            assert_eq!(text.content, "实例文本");
        }
    }

    #[test]
    fn composition_raster_keeps_confirmed_prefix_and_contains_underline() {
        let base = TextAnnotation {
            content: "实例文本".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 22.,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            rotation: 0.,
            layout_scale: 1.,
            linked_arrow_control: None,
            linked_arrow: None,
        };
        for scale in [1., 1.25] {
            let monitor = Rect {
                left: 0,
                top: 0,
                right: 600,
                bottom: 160,
            };
            let make_object = |text: TextAnnotation| Object {
                bounds: text_bounds((10., 10.), &text, scale),
                kind: Kind::Text,
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text),
            };
            let before = make_object(base.clone());
            let mut text = base.clone();
            let (display, range) = composition_preview(&text.content, "wen'zi", text.content.len());
            text.content = display;
            let during = make_object(text);
            let mut original = Pixmap::new(600, 160).unwrap();
            let mut composed = original.clone();
            draw_text(&mut original, &before, monitor, scale);
            draw_text_with_reference(&mut composed, &during, monitor, scale, Some(&base.content));
            // Appending preedit must not change the already confirmed glyph pixels.
            for y in 0..160 {
                for x in 0..(before.bounds.width() as usize - 4) {
                    let offset = (y * 600 + x) * 4;
                    assert_eq!(
                        &original.data()[offset..offset + 4],
                        &composed.data()[offset..offset + 4]
                    );
                }
            }
            let before_underline = composed.clone();
            draw_composition_underline(&mut composed, &during, range, monitor, scale);
            assert_ne!(composed.data(), before_underline.data());
            if scale == 1.25
                && let Ok(path) = std::env::var("LEXIFT_IME_RENDER_RGBA")
            {
                std::fs::write(path, composed.data()).unwrap();
            }
        }
    }

    #[test]
    fn text_bounds_leave_room_for_multiline_glyphs_and_outline() {
        let text = TextAnnotation {
            content: "WIDE 文字\n第二行 gj".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 28.,
            bold: true,
            italic: true,
            outline: Some(TextOutline {
                color: [0, 0, 0],
                width: 3.,
            }),
            background: None,
            rotation: 0.,
            layout_scale: 1.,
            linked_arrow_control: None,
            linked_arrow: None,
        };
        for scale in [1., 1.25] {
            let bounds = text_bounds((0., 0.), &text, scale);
            let object = Object {
                bounds,
                kind: Kind::Text,
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text.clone()),
            };
            let monitor = Rect {
                left: 0,
                top: 0,
                right: bounds.width().ceil() as i32,
                bottom: bounds.height().ceil() as i32,
            };
            let mut image = Pixmap::new(monitor.right as u32, monitor.bottom as u32).unwrap();
            draw_text(&mut image, &object, monitor, scale);
            let pixels = image.data().as_chunks::<4>().0;
            let width = image.width() as usize;
            let height = image.height() as usize;
            assert!(pixels.iter().any(|pixel| pixel[3] != 0));
            assert!(
                pixels
                    .iter()
                    .skip(width * (height - 1))
                    .all(|pixel| pixel[3] == 0)
            );
            assert!((0..height).all(|row| pixels[row * width + width - 1][3] == 0));
        }
    }

    #[test]
    fn watermark_time_is_resolved_only_when_applied() {
        let at = Local
            .with_ymd_and_hms(2026, 9, 30, 12, 34, 56)
            .single()
            .unwrap();
        assert_eq!(
            resolve_watermark_template("水印$yyyy/MM/dd HH:mm:ss$ $$", at),
            "水印2026/09/30 12:34:56 $"
        );
        assert_eq!(resolve_watermark_template("price $", at), "price $");
    }

    #[test]
    fn tiled_watermark_produces_pixels_across_monitor() {
        use lexift_core::domain::annotation::Session;
        let mut session = Session::new();
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 500,
        };
        session.displays.push(monitor);
        session.set_watermark(Some(Watermark {
            template: "TEST".into(),
            content: "TEST".into(),
            font_family: "Segoe UI".into(),
            font_size: 24.,
            opacity: 100,
            color: [217, 71, 43],
            position: WatermarkPosition::Tile,
        }));
        let image = crate::annotation_render::render(&session, monitor, 1., None, None).unwrap();
        let (left, right) = image
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0)
            .fold((0, 0), |(left, right), (index, _)| {
                if index % 800 < 400 {
                    (left + 1, right)
                } else {
                    (left, right + 1)
                }
            });
        assert!(
            left > 100 && right > 100,
            "tile pixels: left={left}, right={right}"
        );
    }

    #[test]
    fn single_watermark_positions_use_each_monitor_origin() {
        let monitor = Rect {
            left: -500,
            top: -200,
            right: 300,
            bottom: 300,
        };
        let positions = [
            (WatermarkPosition::TopLeft, (0, 0)),
            (WatermarkPosition::TopCenter, (1, 0)),
            (WatermarkPosition::TopRight, (2, 0)),
            (WatermarkPosition::Center, (1, 1)),
            (WatermarkPosition::BottomLeft, (0, 2)),
            (WatermarkPosition::BottomCenter, (1, 2)),
            (WatermarkPosition::BottomRight, (2, 2)),
        ];
        for (position, expected) in positions {
            let watermark = Watermark {
                template: "TEST".into(),
                content: "TEST".into(),
                font_family: "Segoe UI".into(),
                font_size: 24.,
                opacity: 100,
                color: [217, 71, 43],
                position,
            };
            let mut image = Pixmap::new(800, 500).unwrap();
            draw_watermark(&mut image, &watermark, monitor, monitor, 1.);
            let pixels = image.data().as_chunks::<4>().0;
            let (count, x_sum, y_sum) = pixels
                .iter()
                .enumerate()
                .filter(|(_, pixel)| pixel[3] > 0)
                .fold((0usize, 0usize, 0usize), |(count, x, y), (index, _)| {
                    (count + 1, x + index % 800, y + index / 800)
                });
            assert!(count > 20, "{position:?}");
            let section = |value: usize, extent: usize| -> usize { (value * 3 / extent).min(2) };
            assert_eq!(
                (section(x_sum / count, 800), section(y_sum / count, 500)),
                expected,
                "{position:?}"
            );
        }
    }

    #[test]
    fn chinese_text_and_watermark_render_on_negative_monitor() {
        use lexift_core::domain::annotation::{Session, Style};
        let mut session = Session::new();
        let text = TextAnnotation {
            content: "文字 PixPin 123".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 24.,
            bold: true,
            italic: false,
            outline: None,
            background: Some(lexift_core::domain::annotation::TextBackground {
                color: [229, 191, 85],
                opacity: 220,
                rounding: 12.,
                padding: 9.,
            }),
            rotation: 0.3,
            layout_scale: 1.,
            linked_arrow_control: None,
            linked_arrow: Some((-40., 100.)),
        };
        session.add(Object {
            bounds: text_bounds((-190., -60.), &text, 1.),
            kind: lexift_core::domain::annotation::Kind::Text,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Text(text),
        });
        session.set_watermark(Some(Watermark {
            template: "水印".into(),
            content: "水印".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 30.,
            opacity: 55,
            color: [217, 71, 43],
            position: WatermarkPosition::Tile,
        }));
        let monitor = Rect {
            left: -220,
            top: -90,
            right: 280,
            bottom: 230,
        };
        let image = crate::annotation_render::render(&session, monitor, 1., None, None).unwrap();
        assert!(
            image
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0)
        );
    }
}
