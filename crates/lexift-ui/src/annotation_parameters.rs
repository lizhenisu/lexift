//! Annotation defaults, projection and accepted requests, without window ownership.
use super::color;
use crate::AnnotationValues;
use lexift_core::domain::annotation::{
    Endpoint, Extra, Kind, MagnifierConnector, Object, ObjectId, SequenceFormat, SequenceShape,
    Style, TextAnnotation, TextBackground, TextOutline, Watermark, WatermarkPosition,
};

/// A panel notification updates only fields changed since its previous notification.
/// Applying it to current accepted values preserves independent menu edits.
pub(super) struct Patch {
    before: AnnotationValues,
    after: AnnotationValues,
}
impl Patch {
    pub fn between(before: AnnotationValues, after: AnnotationValues) -> Option<Self> {
        (before != after).then_some(Self { before, after })
    }
    pub fn apply(&self, values: &mut AnnotationValues) {
        macro_rules! changed_fields {
            ($($field:ident),+ $(,)?) => {
                // Exhaustive destructuring forces newly exposed fields to join the protocol.
                let AnnotationValues { $($field),+ } = &self.after;
                $(if self.before.$field != *$field { values.$field.clone_from($field); })+
            };
        }
        changed_fields!(
            size,
            rounding,
            text_size,
            start,
            strength,
            shape,
            style,
            mode,
            font,
            format,
            zoom,
            fill,
            erase,
            antialias,
            shadow,
            text,
            color_index,
            custom_color,
            line_start,
            line_end,
            connector_style,
            head,
            text_bold,
            text_italic,
            text_outline,
            text_background,
            text_outline_width,
            text_outline_color_index,
            text_outline_custom_color,
            text_bg_color_index,
            text_bg_custom_color,
            text_bg_opacity,
            text_bg_rounding,
            text_bg_padding,
            sequence_ratio,
            sequence_guide,
            sequence_expanded,
            sequence_text_color,
            sequence_label
        );
    }
}

/// Owns a panel notification baseline. Projection advances it without generating
/// a user request, so the next change is relative to the values actually displayed.
pub(super) struct Notifications(std::cell::RefCell<AnnotationValues>);
impl Notifications {
    pub fn new(values: AnnotationValues) -> Self {
        Self(std::cell::RefCell::new(values))
    }
    pub fn observe(&self, values: AnnotationValues, user_input: bool) -> Option<Patch> {
        let before = self.0.replace(values.clone());
        if user_input {
            Patch::between(before, values)
        } else {
            None
        }
    }
}

/// Accepted inputs waiting for geometry to release its transaction. Targets are
/// never rebound to a later selection; replacing one target preserves FIFO order.
#[derive(Default)]
pub(super) struct Pending {
    requests: Vec<(super::parameter_edit::Target, AnnotationValues)>,
}
impl Pending {
    pub fn accept(&mut self, target: super::parameter_edit::Target, values: AnnotationValues) {
        if let Some((_, pending)) = self
            .requests
            .iter_mut()
            .find(|(pending, _)| *pending == target)
        {
            *pending = values;
        } else {
            self.requests.push((target, values));
        }
    }
    pub fn values(&self, target: super::parameter_edit::Target) -> Option<&AnnotationValues> {
        self.requests
            .iter()
            .find(|(pending, _)| *pending == target)
            .map(|(_, values)| values)
    }
    /// Detach the batch before replay: applying it may retire a panel or accept
    /// another request. New requests belong to the next batch, not this iterator.
    pub fn take(&mut self) -> Vec<(super::parameter_edit::Target, AnnotationValues)> {
        std::mem::take(&mut self.requests)
    }
    pub fn clear(&mut self) {
        self.requests.clear();
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.requests.len()
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

/// Replay keeps the accepted destination, including defaults accepted during a
/// cancelled sequence placement. A restored selection is never a replacement target.
pub(super) enum Replay {
    Current,
    SequenceDefaults,
    Discard,
}
pub(super) struct ReplayContext {
    pub current: super::parameter_edit::Target,
    pub panel_tool: Option<i32>,
    pub bound_object: Option<ObjectId>,
    pub cancelled_sequence: bool,
}
impl ReplayContext {
    pub fn resolve(self, target: super::parameter_edit::Target) -> Replay {
        if self.panel_tool != Some(target.tool) {
            return Replay::Discard;
        }
        if self.current == target {
            return Replay::Current;
        }
        let mut defaults = self.current;
        defaults.object = None;
        if self.cancelled_sequence
            && target.tool == 10
            && target.object.is_none()
            && target.editor.is_none()
            && self.bound_object.is_none()
            && defaults == target
        {
            Replay::SequenceDefaults
        } else {
            Replay::Discard
        }
    }
}

/// Rectangle and ellipse expose separate toolbar slots for the same Core tool.
pub(super) fn compatible_tools(left: usize, right: usize) -> bool {
    left == right || matches!((left, right), (0, 1) | (1, 0))
}

/// Drawing defaults never receive an object's projection. Reads validate stable
/// object identity, so deselection/undo cannot expose a retired object's values.
pub(super) struct State {
    defaults: Vec<AnnotationValues>,
    binding: std::rc::Rc<std::cell::Cell<Option<ObjectId>>>,
    projection: Option<Projection>,
    projection_revision: u64,
    watermark: Option<AnnotationValues>,
    watermark_revision: u64,
    mosaic_mode: Option<i32>,
}
struct Projection {
    tool: usize,
    object: ObjectId,
    values: AnnotationValues,
}
impl State {
    pub fn new() -> Self {
        Self {
            defaults: (0..14).map(defaults).collect(),
            binding: Default::default(),
            projection: None,
            projection_revision: Default::default(),
            watermark: None,
            watermark_revision: Default::default(),
            mosaic_mode: None,
        }
    }
    pub fn binding(&self) -> std::rc::Rc<std::cell::Cell<Option<ObjectId>>> {
        self.binding.clone()
    }
    pub fn bound_object(&self) -> Option<ObjectId> {
        self.binding.get()
    }
    pub fn bind(&self, object: Option<ObjectId>) {
        self.binding.set(object);
    }
    pub fn deselect(&mut self) {
        self.advance_projection();
        self.projection = None;
        self.binding.set(None);
    }
    pub fn values(&self, tool: usize, object: Option<ObjectId>) -> &AnnotationValues {
        if tool == 9
            && let Some(values) = &self.watermark
        {
            return values;
        }
        self.projection
            .as_ref()
            .filter(|projection| {
                compatible_tools(projection.tool, tool) && Some(projection.object) == object
            })
            .map_or(&self.defaults[tool], |projection| &projection.values)
    }
    pub fn defaults(&self, tool: usize) -> &AnnotationValues {
        &self.defaults[tool]
    }
    pub fn defaults_mut(&mut self, tool: usize) -> &mut AnnotationValues {
        &mut self.defaults[tool]
    }
    /// User changes target the selected object; without a target they configure
    /// subsequent drawing. Never make object refill look like user defaults.
    pub fn edit(&mut self, tool: usize, object: Option<ObjectId>) -> &mut AnnotationValues {
        if tool == 9
            && let Some(values) = &mut self.watermark
        {
            return values;
        }
        let Some(object) = object else {
            return &mut self.defaults[tool];
        };
        if !self.projection.as_ref().is_some_and(|projection| {
            compatible_tools(projection.tool, tool) && projection.object == object
        }) {
            self.projection = Some(Projection {
                tool,
                object,
                values: self.defaults[tool].clone(),
            });
        }
        &mut self.projection.as_mut().unwrap().values
    }
    pub fn replace(&mut self, tool: usize, object: Option<ObjectId>, values: AnnotationValues) {
        // Geometry mode chooses the next gesture, never converts a selected
        // mosaic's trajectory. Effect parameters still belong to its projection.
        if tool == 11 && self.values(tool, object).shape != values.shape {
            self.mosaic_mode = Some(values.shape);
            self.defaults[tool].shape = values.shape;
        }
        self.remember_creation(tool, &values);
        *self.edit(tool, object) = values;
    }
    /// Remember only settings: object contents and sequence identity are not templates.
    pub fn remember_creation(&mut self, tool: usize, values: &AnnotationValues) {
        if tool == 9 {
            return;
        }
        let mut creation = values.clone();
        creation.text = Default::default();
        creation.sequence_label = Default::default();
        self.defaults[tool] = creation.clone();
        if matches!(tool, 0 | 1) {
            self.defaults[1 - tool] = creation;
        }
    }
    pub fn project(
        &mut self,
        object: &Object,
        font_index: i32,
        next_sequence: i32,
        spotlight_opacity: f32,
    ) {
        self.advance_projection();
        let tool = object.kind.tool();
        self.binding.set(Some(object.id));
        let mut values = self.defaults[tool].clone();
        refill_object(
            &mut values,
            object,
            font_index,
            next_sequence,
            spotlight_opacity,
        );
        if tool == 11
            && let Some(mode) = self.mosaic_mode
        {
            values.shape = mode;
        }
        self.projection = Some(Projection {
            tool,
            object: object.id,
            values,
        });
    }
    pub fn sequence_counter(&mut self, next: i32) {
        fn update(values: &mut AnnotationValues, next: i32) {
            values.start = next.max(1);
            let format = match values.format {
                1 => SequenceFormat::Roman,
                2 => SequenceFormat::LowerAlpha,
                3 => SequenceFormat::UpperAlpha,
                4 => SequenceFormat::Chinese,
                _ => SequenceFormat::Decimal,
            };
            values.sequence_label =
                lexift_core::domain::annotation::sequence_label(values.start, format).into();
        }
        update(&mut self.defaults[10], next);
        if let Some(projection) = self
            .projection
            .as_mut()
            .filter(|projection| projection.tool == 10)
        {
            update(&mut projection.values, next);
        }
    }

    /// Applied watermark refill is independent of the next watermark's defaults.
    /// A new projection invalidates queued events; continuous edits retain it.
    pub fn project_watermark(&mut self, watermark: Option<&Watermark>, font_index: i32) {
        self.watermark_revision = self
            .watermark_revision
            .checked_add(1)
            .expect("watermark projection identity exhausted");
        self.watermark = watermark.map(|watermark| {
            let mut values = self.defaults[9].clone();
            refill_watermark(&mut values, Some(watermark), font_index);
            values
        });
    }
    pub fn watermark_revision(&self) -> u64 {
        self.watermark_revision
    }
    fn advance_projection(&mut self) {
        self.projection_revision = self
            .projection_revision
            .checked_add(1)
            .expect("parameter projection identity exhausted");
    }
    pub fn projection_revision(&self) -> u64 {
        self.projection_revision
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChoiceTarget {
    Shape,
    Style,
    Mode,
    Zoom,
    Font,
    Format,
    LineStart,
    LineEnd,
    ConnectorStyle,
    SequenceRatio,
}

impl ChoiceTarget {
    fn from_field(field: i32) -> Option<Self> {
        Some(match field {
            1 | 17 | 23 => Self::Shape,
            2 => Self::Style,
            6 | 14 | 18 => Self::Mode,
            7 => Self::Zoom,
            12 => Self::Font,
            16 => Self::Format,
            19 => Self::LineStart,
            20 => Self::LineEnd,
            21 => Self::ConnectorStyle,
            24 => Self::SequenceRatio,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum TextStyleTarget {
    Outline,
    Background,
}
impl TextStyleTarget {
    pub fn from_field(field: i32) -> Option<Self> {
        match field {
            -3 => Some(Self::Outline),
            -4 => Some(Self::Background),
            _ => None,
        }
    }
    pub fn mode(self) -> i32 {
        match self {
            Self::Outline => 1,
            Self::Background => 2,
        }
    }
    pub fn edit(self, source: &AnnotationValues) -> Edit {
        match self {
            Self::Outline => Edit::Outline {
                width: source.text_outline_width,
                color_index: source.text_outline_color_index,
                custom_color: source.text_outline_custom_color.clone(),
            },
            Self::Background => Edit::Background {
                opacity: source.text_bg_opacity,
                rounding: source.text_bg_rounding,
                padding: source.text_bg_padding,
                color_index: source.text_bg_color_index,
                custom_color: source.text_bg_custom_color.clone(),
            },
        }
    }
}

/// A popup returns only the parameters it owns, never a stale whole toolbar
/// snapshot. Apply against the latest values so ongoing text edits survive.
pub(super) enum Edit {
    Choice(ChoiceTarget, i32),
    Color(ColorTarget, [u8; 3]),
    Outline {
        width: i32,
        color_index: i32,
        custom_color: slint::SharedString,
    },
    Background {
        opacity: i32,
        rounding: i32,
        padding: i32,
        color_index: i32,
        custom_color: slint::SharedString,
    },
}

impl Edit {
    pub fn choice(field: i32, index: i32) -> Option<Self> {
        ChoiceTarget::from_field(field).map(|target| Self::Choice(target, index))
    }
    pub fn apply(&self, values: &mut AnnotationValues) {
        match self {
            Self::Choice(target, index) => {
                let value = match target {
                    ChoiceTarget::Shape => &mut values.shape,
                    ChoiceTarget::Style => &mut values.style,
                    ChoiceTarget::Mode => &mut values.mode,
                    ChoiceTarget::Zoom => &mut values.zoom,
                    ChoiceTarget::Font => &mut values.font,
                    ChoiceTarget::Format => &mut values.format,
                    ChoiceTarget::LineStart => &mut values.line_start,
                    ChoiceTarget::LineEnd => &mut values.line_end,
                    ChoiceTarget::ConnectorStyle => &mut values.connector_style,
                    ChoiceTarget::SequenceRatio => &mut values.sequence_ratio,
                };
                *value = *index;
            }
            Self::Color(target, rgb) => target.set(values, *rgb),
            Self::Outline {
                width,
                color_index,
                custom_color,
            } => {
                values.text_outline_width = *width;
                values.text_outline_color_index = *color_index;
                values.text_outline_custom_color = custom_color.clone();
            }
            Self::Background {
                opacity,
                rounding,
                padding,
                color_index,
                custom_color,
            } => {
                values.text_bg_opacity = *opacity;
                values.text_bg_rounding = *rounding;
                values.text_bg_padding = *padding;
                values.text_bg_color_index = *color_index;
                values.text_bg_custom_color = custom_color.clone();
            }
        }
    }
}

pub(super) enum Selection {
    KeepOpen,
    Close,
    Apply(Edit),
}

impl Selection {
    pub fn from_menu(tool: i32, field: i32, index: Option<i32>, option_count: usize) -> Self {
        let Some(index) = index.filter(|index| *index >= 0 && (*index as usize) < option_count)
        else {
            return Self::Close;
        };
        if tool == 4 && field == 6 && index == 1 {
            return Self::KeepOpen;
        }
        Edit::choice(field, index).map_or(Self::Close, Self::Apply)
    }
}

/// Converts the Slint color-menu protocol once, before applying a parameter edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColorTarget {
    Body,
    TextOutline,
    TextBackground,
    SequenceText,
}

impl ColorTarget {
    pub fn from_field(field: i32) -> Option<Self> {
        match field {
            -2 => Some(Self::Body),
            -5 => Some(Self::TextOutline),
            -6 => Some(Self::TextBackground),
            -8 => Some(Self::SequenceText),
            _ => None,
        }
    }

    pub fn color(self, values: &AnnotationValues, palette: [[u8; 3]; 8]) -> [u8; 3] {
        match self {
            Self::Body => palette_color(palette, values.color_index, &values.custom_color),
            Self::TextOutline => palette_color(
                palette,
                values.text_outline_color_index,
                &values.text_outline_custom_color,
            ),
            Self::TextBackground => palette_color(
                palette,
                values.text_bg_color_index,
                &values.text_bg_custom_color,
            ),
            Self::SequenceText => color::parse_hex(&values.sequence_text_color)
                .unwrap_or_else(|| Self::Body.color(values, palette)),
        }
    }

    pub fn set(self, values: &mut AnnotationValues, rgb: [u8; 3]) {
        let hex = color::hex(rgb).into();
        match self {
            Self::Body => {
                values.color_index = 8;
                values.custom_color = hex;
            }
            Self::TextOutline => {
                values.text_outline_color_index = 8;
                values.text_outline_custom_color = hex;
            }
            Self::TextBackground => {
                values.text_bg_color_index = 8;
                values.text_bg_custom_color = hex;
            }
            Self::SequenceText => values.sequence_text_color = hex,
        }
    }
}

pub(super) fn palette(tool: usize) -> [[u8; 3]; 8] {
    if tool == 4 {
        [
            [255, 244, 0],
            [255, 165, 0],
            [70, 235, 235],
            [165, 155, 255],
            [255, 160, 245],
            [255, 255, 255],
            [50, 236, 102],
            [33, 33, 33],
        ]
    } else {
        [
            [217, 71, 43],
            [229, 191, 85],
            [105, 173, 97],
            [89, 143, 223],
            [33, 33, 33],
            [255, 255, 255],
            [50, 236, 102],
            [255, 244, 0],
        ]
    }
}

pub(super) fn palette_color(palette: [[u8; 3]; 8], index: i32, custom: &str) -> [u8; 3] {
    if index == 8 {
        color::parse_hex(custom).unwrap_or(palette[0])
    } else {
        palette[index.clamp(0, 7) as usize]
    }
}

pub(super) fn color_to_values(palette: [[u8; 3]; 8], rgb: [u8; 3]) -> (i32, slint::SharedString) {
    (
        palette
            .iter()
            .position(|c| *c == rgb)
            .map_or(8, |i| i as i32),
        color::hex(rgb).into(),
    )
}

/// Parameter projections only; callers resolve preferred fonts and own layout.
pub(super) fn style(v: &AnnotationValues, tool: i32) -> Style {
    let palette = palette(tool as usize);
    let color = if v.color_index == 8 {
        color::parse_hex(&v.custom_color).unwrap_or(palette[0])
    } else {
        palette[v.color_index.clamp(0, 7) as usize]
    };
    Style {
        color,
        width: v.size.max(1) as f32,
        rounding: v.rounding.max(0) as f32,
        fill: v.fill,
        dash: v.style.clamp(0, 4) as u8,
        outline: tool != 2 || v.erase,
    }
}

pub(super) fn text(v: &AnnotationValues, font_family: String, content: String) -> TextAnnotation {
    let colors = palette(8);
    TextAnnotation {
        content,
        font_family,
        font_size: v.text_size.max(5) as f32,
        bold: v.text_bold,
        italic: v.text_italic,
        outline: v.text_outline.then(|| TextOutline {
            color: palette_color(
                colors,
                v.text_outline_color_index,
                &v.text_outline_custom_color,
            ),
            width: v.text_outline_width.clamp(1, 20) as f32,
        }),
        background: v.text_background.then(|| TextBackground {
            color: palette_color(colors, v.text_bg_color_index, &v.text_bg_custom_color),
            opacity: (v.text_bg_opacity.clamp(0, 100) as f32 * 2.55).round() as u8,
            rounding: v.text_bg_rounding.max(0) as f32,
            padding: v.text_bg_padding.max(0) as f32,
        }),
        rotation: 0.,
        layout_scale: 1.,
        linked_arrow_control: None,
        linked_arrow: None,
    }
}

pub(super) fn watermark(
    v: &AnnotationValues,
    font_family: String,
    template: String,
    content: String,
) -> Watermark {
    Watermark {
        template,
        content,
        font_family,
        font_size: v.text_size.clamp(5, 144) as f32,
        opacity: v.strength.round().clamp(0., 100.) as u8,
        color: style(v, 9).color,
        position: match v.mode {
            1 => WatermarkPosition::BottomRight,
            2 => WatermarkPosition::BottomLeft,
            3 => WatermarkPosition::TopRight,
            4 => WatermarkPosition::TopLeft,
            5 => WatermarkPosition::TopCenter,
            6 => WatermarkPosition::BottomCenter,
            7 => WatermarkPosition::Center,
            _ => WatermarkPosition::Tile,
        },
    }
}

pub(super) fn endpoint(index: i32) -> Endpoint {
    match index {
        1 => Endpoint::OpenArrow,
        2 => Endpoint::Arrow,
        3 => Endpoint::FilledArrow,
        4 => Endpoint::Circle,
        5 => Endpoint::OpenCircle,
        6 => Endpoint::Diamond,
        7 => Endpoint::OpenDiamond,
        8 => Endpoint::Bar,
        _ => Endpoint::None,
    }
}

pub(super) fn sequence_style(
    object: &mut Object,
    v: &AnnotationValues,
    fonts: &[String],
    style: Style,
) {
    let Extra::Sequence(s) = &mut object.extra else {
        return;
    };
    s.size = v.text_size.max(5) as f32;
    s.shape = match v.shape {
        0 => SequenceShape::Circle,
        1 => SequenceShape::CircleWhiteBorder,
        2 => SequenceShape::Outline,
        3 => SequenceShape::Plain,
        _ => SequenceShape::Square,
    };
    s.format = match v.format {
        1 => SequenceFormat::Roman,
        2 => SequenceFormat::LowerAlpha,
        3 => SequenceFormat::UpperAlpha,
        4 => SequenceFormat::Chinese,
        _ => SequenceFormat::Decimal,
    };
    s.caption.font_family = fonts
        .get(v.font.max(0) as usize)
        .cloned()
        .unwrap_or_else(|| "Microsoft YaHei".into());
    s.caption.outline = v.text_outline.then(|| TextOutline {
        width: v.text_outline_width.max(1) as f32,
        color: palette_color(
            palette(10),
            v.text_outline_color_index,
            &v.text_outline_custom_color,
        ),
    });
    s.caption_ratio = v.sequence_ratio.clamp(0, 4) as usize;
    s.guide = v.sequence_guide;
    s.caption_color = color::parse_hex(&v.sequence_text_color).unwrap_or(style.color);
    s.caption_custom_color = color::parse_hex(&v.sequence_text_color).is_some();
    s.endpoint = endpoint(v.line_end);
    object.style = style;
}
pub(super) fn defaults(tool: i32) -> AnnotationValues {
    AnnotationValues {
        size: if tool == 7 {
            2
        } else if matches!(tool, 3 | 4 | 11..=13) {
            12
        } else {
            4
        },
        rounding: if tool == 4 { 0 } else { 21 },
        text_size: if tool == 10 { 16 } else { 22 },
        text_outline_width: 1,
        text_bg_opacity: 100,
        text_bg_rounding: 0,
        text_bg_padding: 4,
        strength: if tool == 9 {
            100.
        } else if tool == 2 {
            10.
        } else {
            50.
        },
        start: 1,
        sequence_ratio: 2,
        sequence_guide: true,
        sequence_text_color: "".into(),
        sequence_label: "1".into(),
        shape: if tool == 10 {
            4
        } else {
            i32::from(matches!(tool, 1 | 2))
        },
        zoom: if tool == 7 { 150 } else { 0 },
        head: tool == 5,
        line_end: if matches!(tool, 5 | 10) { 3 } else { 0 },
        mode: 0,
        erase: tool != 2,
        antialias: true,
        color_index: if tool == 4 {
            6
        } else if tool == 2 {
            1
        } else {
            0
        },
        ..Default::default()
    }
}

fn endpoint_index(value: Endpoint) -> i32 {
    match value {
        Endpoint::None => 0,
        Endpoint::OpenArrow => 1,
        Endpoint::Arrow => 2,
        Endpoint::FilledArrow => 3,
        Endpoint::Circle => 4,
        Endpoint::OpenCircle => 5,
        Endpoint::Diamond => 6,
        Endpoint::OpenDiamond => 7,
        Endpoint::Bar => 8,
    }
}

fn connector_index(value: MagnifierConnector) -> i32 {
    match value {
        MagnifierConnector::Plain => 0,
        MagnifierConnector::Dot => 1,
        MagnifierConnector::Frame => 2,
        MagnifierConnector::None => 3,
    }
}

pub(super) fn text_dimensions_to_values(values: &mut AnnotationValues, text: &TextAnnotation) {
    values.text_size = text.font_size.round() as i32;
    if let Some(background) = &text.background {
        values.text_bg_padding = background.padding.round() as i32;
        values.text_bg_rounding = background.rounding.round() as i32;
    }
    if let Some(outline) = &text.outline {
        values.text_outline_width = outline.width.round() as i32;
    }
}

/// Projects an object into panel values without copying its trajectory or changing
/// font preferences. The coordinator supplies document and directory context.
pub(super) fn refill_object(
    values: &mut AnnotationValues,
    object: &Object,
    font_index: i32,
    next_sequence: i32,
    spotlight_opacity: f32,
) {
    let tool = object.kind.tool();
    let palette = palette(tool);
    values.size = object.style.width.round() as i32;
    values.rounding = object.style.rounding.round() as i32;
    values.shape = i32::from(matches!(
        object.kind,
        Kind::Ellipse | Kind::SpotlightEllipse | Kind::HighlightRectangle
    ));
    match &object.extra {
        Extra::Mosaic(mosaic) => {
            values.shape = i32::from(object.kind == Kind::MosaicBrush);
            values.mode =
                i32::from(mosaic.effect == lexift_core::domain::annotation::MosaicEffect::Blur);
            values.strength = mosaic.strength as f32;
            values.size = (mosaic.diameter / mosaic.creation_scale).round() as i32;
        }
        Extra::Arrow {
            curved,
            head,
            start,
            end,
        } => {
            values.shape = i32::from(*curved);
            values.head = *head;
            values.line_start = endpoint_index(*start);
            values.line_end = endpoint_index(*end);
        }
        Extra::Polyline {
            curved,
            head,
            start,
            end,
        } => {
            values.shape = i32::from(*curved);
            values.head = *head;
            values.line_start = endpoint_index(*start);
            values.line_end = endpoint_index(*end);
        }
        Extra::Magnifier {
            zoom,
            ellipse,
            connector,
            erase_annotations,
            antialias,
            shadow,
            ..
        } => {
            values.shape = i32::from(*ellipse);
            values.zoom = (zoom * 100.).round() as i32;
            values.connector_style = connector_index(*connector);
            values.erase = *erase_annotations;
            values.antialias = *antialias;
            values.shadow = *shadow;
        }
        Extra::Text(text) => {
            values.text = text.content.clone().into();
            text_dimensions_to_values(values, text);
            values.font = font_index;
            values.text_bold = text.bold;
            values.text_italic = text.italic;
            values.text_outline = text.outline.is_some();
            values.text_background = text.background.is_some();
            if let Some(outline) = &text.outline {
                (
                    values.text_outline_color_index,
                    values.text_outline_custom_color,
                ) = color_to_values(palette, outline.color);
            }
            if let Some(background) = &text.background {
                values.text_bg_opacity = (background.opacity as f32 / 2.55).round() as i32;
                (values.text_bg_color_index, values.text_bg_custom_color) =
                    color_to_values(palette, background.color);
            }
        }
        Extra::Sequence(s) => {
            values.text_size = s.size.round() as i32;
            values.shape = s.shape as i32;
            values.format = s.format as i32;
            values.start = next_sequence;
            values.sequence_ratio = s.caption_ratio as i32;
            values.sequence_guide = s.guide;
            values.sequence_text_color = if s.caption_custom_color {
                color::hex(s.caption_color).into()
            } else {
                "".into()
            };
            values.text_outline = s.caption.outline.is_some();
            if let Some(outline) = &s.caption.outline {
                values.text_outline_width = outline.width.round() as i32;
                let (i, c) = color_to_values(palette, outline.color);
                values.text_outline_color_index = i;
                values.text_outline_custom_color = c;
            }
            values.line_end = endpoint_index(s.endpoint);
            values.font = font_index;
        }
        Extra::None => {}
    }
    values.fill = object.style.fill;
    values.style = object.style.dash as i32;
    if tool != 7 {
        values.erase = object.style.outline;
    }
    if tool != 11 {
        values.strength = spotlight_opacity * 100.;
    }
    (values.color_index, values.custom_color) = color_to_values(palette, object.style.color);
}

/// Absence clears applied content while retaining subsequent drawing defaults.
pub(super) fn refill_watermark(
    values: &mut AnnotationValues,
    watermark: Option<&Watermark>,
    font_index: i32,
) {
    let Some(watermark) = watermark else {
        values.text = "".into();
        return;
    };
    values.text = watermark.template.clone().into();
    values.text_size = watermark.font_size.round() as i32;
    values.strength = watermark.opacity as f32;
    values.mode = match watermark.position {
        WatermarkPosition::Tile => 0,
        WatermarkPosition::BottomRight => 1,
        WatermarkPosition::BottomLeft => 2,
        WatermarkPosition::TopRight => 3,
        WatermarkPosition::TopLeft => 4,
        WatermarkPosition::TopCenter => 5,
        WatermarkPosition::BottomCenter => 6,
        WatermarkPosition::Center => 7,
    };
    values.font = font_index;
    (values.color_index, values.custom_color) = color_to_values(palette(9), watermark.color);
}

/// Integer toolbar values must not quantize scaled geometry on unrelated edits.
pub(super) fn preserve_text_dimensions(
    updated: &mut TextAnnotation,
    original: &TextAnnotation,
    before: &AnnotationValues,
    after: &AnnotationValues,
) {
    if before.text_size == after.text_size {
        updated.font_size = original.font_size;
    }
    if let (Some(next), Some(old)) = (&mut updated.background, &original.background) {
        if before.text_bg_padding == after.text_bg_padding {
            next.padding = old.padding;
        }
        if before.text_bg_rounding == after.text_bg_rounding {
            next.rounding = old.rounding;
        }
    }
    if let (Some(next), Some(old)) = (&mut updated.outline, &original.outline)
        && before.text_outline_width == after.text_outline_width
    {
        next.width = old.width;
    }
}

/// Merge typography while preserving the object's content and geometric metadata.
/// Use the same projection for the live editor and the committed object.
pub(super) fn updated_text_style(
    original: &TextAnnotation,
    next: &TextAnnotation,
    before: &AnnotationValues,
    after: &AnnotationValues,
) -> TextAnnotation {
    let mut updated = next.clone();
    updated.content = original.content.clone();
    updated.rotation = original.rotation;
    updated.linked_arrow = original.linked_arrow;
    updated.linked_arrow_control = original.linked_arrow_control;
    updated.layout_scale = original.layout_scale;
    preserve_text_dimensions(&mut updated, original, before, after);
    updated
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mosaic_refill_and_explicit_gesture_mode_have_separate_ownership() {
        use lexift_core::domain::annotation::{Bounds, MosaicAnnotation, MosaicEffect, Session};
        let mut session = Session::new();
        session.add(Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-100., 20.), (100., 100.)),
            points: vec![(-80., 40.), (80., 80.)],
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 73, 80., 1.25)),
        });
        let object = &session.objects[0];
        let mut state = State::new();
        state.project(object, 0, 1, 0.5);
        let mut values = state.values(11, Some(object.id)).clone();
        assert_eq!(
            values.shape, 1,
            "selected brush must expose its diameter control"
        );
        values.strength = 60.;
        state.replace(11, Some(object.id), values);
        assert_eq!(
            state.defaults(11).shape,
            1,
            "accepted parameters also define subsequent drawing"
        );
        assert_eq!(state.defaults(11).strength, 60.);
        let mut values = state.values(11, Some(object.id)).clone();
        values.shape = 0;
        state.replace(11, Some(object.id), values);
        state.project(object, 0, 1, 0.5);
        assert_eq!(state.values(11, Some(object.id)).shape, 0);
        let mut values = state.values(11, Some(object.id)).clone();
        values.shape = 1;
        state.replace(11, Some(object.id), values);
        state.deselect();
        assert_eq!(state.values(11, None).shape, 1);
        assert_eq!(state.values(11, None).strength, 73.);
        state.project(object, 0, 1, 0.5);
        assert_eq!(state.values(11, Some(object.id)).shape, 1);
        assert_eq!(state.values(11, Some(object.id)).strength, 73.);
    }

    #[test]
    fn projection_and_user_edits_do_not_replace_defaults_or_follow_another_object() {
        use lexift_core::domain::annotation::{Bounds, Session};
        let mut session = Session::new();
        for color in [[1, 2, 3], [4, 5, 6]] {
            session.add(Object {
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((0., 0.), (20., 20.)),
                points: Vec::new(),
                style: Style {
                    color,
                    ..Style::default()
                },
                extra: Extra::None,
            });
        }
        let first = &session.objects[0];
        let second = &session.objects[1];
        let mut state = State::new();
        state.defaults_mut(0).size = 19;
        let defaults = state.values(0, None).clone();
        state.project(first, 0, 1, 0.5);
        let edit_revision = state.projection_revision();
        assert_eq!(state.values(0, Some(first.id)).custom_color, "#010203");
        state.edit(0, Some(first.id)).size = 37;
        assert_eq!(
            state.projection_revision(),
            edit_revision,
            "continuous edits must share one target"
        );
        assert_eq!(state.values(0, Some(first.id)).size, 37);
        assert_eq!(
            state.values(1, Some(first.id)).size,
            37,
            "ellipse toolbar lost the shared shape projection"
        );
        assert_eq!(state.values(0, None), &defaults);
        state.defaults_mut(0).font = 7;
        assert_eq!(
            state.values(0, Some(first.id)).font,
            0,
            "default preference replaced the object's font"
        );
        state.defaults_mut(0).font = defaults.font;
        assert_eq!(state.values(0, Some(second.id)), &defaults);
        let emitted_for = state.binding().get();
        state.project(second, 0, 1, 0.5);
        assert_ne!(state.projection_revision(), edit_revision);
        assert_ne!(
            emitted_for,
            state.binding().get(),
            "a queued callback must retain its original target"
        );
        state.deselect();
        assert_eq!(state.values(0, Some(first.id)), &defaults);
        assert!(state.binding().get().is_none());
        state.edit(0, None).size = 23;
        assert_eq!(state.values(0, None).size, 23);
    }

    #[test]
    fn object_projection_preserves_custom_text_effects_and_creation_scale() {
        use lexift_core::domain::annotation::{
            Bounds, MosaicAnnotation, MosaicEffect, Style, TextBackground, TextOutline,
        };
        let text = TextAnnotation {
            content: "caption".into(),
            font_family: "local font".into(),
            font_size: 27.6,
            bold: true,
            italic: true,
            outline: Some(TextOutline {
                color: [11, 22, 33],
                width: 3.6,
            }),
            background: Some(TextBackground {
                color: [44, 55, 66],
                opacity: 128,
                rounding: 5.6,
                padding: 7.6,
            }),
            rotation: 0.,
            linked_arrow: None,
            layout_scale: 1.,
            linked_arrow_control: None,
        };
        let mut object = Object {
            id: Default::default(),
            kind: Kind::Text,
            bounds: Bounds::from_corners((0., 0.), (40., 40.)),
            style: Style::default(),
            points: vec![],
            extra: Extra::Text(text),
        };
        object.style.color = [77, 88, 99];
        let mut values = defaults(8);
        refill_object(&mut values, &object, 17, 42, 0.25);
        assert_eq!(values.font, 17);
        assert_eq!(values.text.as_str(), "caption");
        assert_eq!(values.text_size, 28);
        assert!(
            values.text_bold && values.text_italic && values.text_outline && values.text_background
        );
        assert_eq!(
            (
                values.text_outline_width,
                values.text_bg_rounding,
                values.text_bg_padding
            ),
            (4, 6, 8)
        );
        assert_eq!(values.text_bg_opacity, 50);
        assert_eq!(ColorTarget::Body.color(&values, palette(8)), [77, 88, 99]);
        assert_eq!(
            ColorTarget::TextOutline.color(&values, palette(8)),
            [11, 22, 33]
        );
        assert_eq!(
            ColorTarget::TextBackground.color(&values, palette(8)),
            [44, 55, 66]
        );
        object.kind = Kind::MosaicBrush;
        object.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 73, 80., 1.25));
        refill_object(&mut values, &object, 0, 42, 0.25);
        assert_eq!((values.shape, values.mode, values.size), (1, 1, 80));
        assert_eq!(values.strength, 73.);
    }

    #[test]
    fn watermark_projection_and_clear_retain_unapplied_defaults() {
        let watermark = Watermark {
            template: "{time}".into(),
            content: "resolved".into(),
            font_family: "local".into(),
            font_size: 30.6,
            opacity: 73,
            color: [11, 22, 33],
            position: WatermarkPosition::BottomCenter,
        };
        let mut values = defaults(9);
        refill_watermark(&mut values, Some(&watermark), 19);
        assert_eq!(values.text.as_str(), "{time}");
        assert_eq!((values.font, values.text_size, values.mode), (19, 31, 6));
        assert_eq!(values.strength, 73.);
        assert_eq!(
            ColorTarget::Body.color(&values, palette(9)),
            watermark.color
        );
        let mut expected = values.clone();
        expected.text = "".into();
        refill_watermark(&mut values, None, 0);
        assert_eq!(values, expected);
        let mut state = State::new();
        let defaults = state.defaults(9).clone();
        let absent = state.watermark_revision();
        state.project_watermark(Some(&watermark), 19);
        let applied = state.watermark_revision();
        assert_ne!(applied, absent);
        assert_eq!(state.values(9, None).font, 19);
        assert_eq!(state.values(9, None).text_size, 31);
        state.edit(9, None).text_size = 44;
        assert_eq!(state.watermark_revision(), applied);
        assert_eq!(state.defaults(9), &defaults);
        state.deselect();
        assert_eq!(state.values(9, None).text_size, 44);
        state.project_watermark(None, 0);
        assert_ne!(state.watermark_revision(), applied);
        assert_eq!(state.values(9, None), &defaults);
        assert_eq!(state.defaults(9), &defaults);
    }

    #[test]
    fn delayed_style_patch_preserves_intervening_input_and_other_effect_updates() {
        let source = AnnotationValues {
            text: "old text".into(),
            text_outline_width: 7,
            text_outline_color_index: 8,
            text_outline_custom_color: "#123456".into(),
            text_bg_opacity: 80,
            text_bg_rounding: 9,
            text_bg_padding: 13,
            text_bg_color_index: 8,
            text_bg_custom_color: "#654321".into(),
            ..Default::default()
        };
        for field in [-3, -4] {
            let edit = TextStyleTarget::from_field(field).unwrap().edit(&source);
            let mut current = source.clone();
            current.text = "新输入的说明".into();
            current.font = 17;
            current.text_size = 200;
            current.text_outline = true;
            current.text_background = true;
            current.text_outline_width = 15;
            current.text_bg_opacity = 20;
            let mut expected = current.clone();
            if field == -3 {
                expected.text_outline_width = 7;
            } else {
                expected.text_bg_opacity = 80;
            }
            edit.apply(&mut current);
            assert_eq!(current, expected);
        }
    }

    #[test]
    fn selection_validates_cancel_indices_disabled_options_and_unknown_fields() {
        for index in [None, Some(-1), Some(2), Some(i32::MAX)] {
            assert!(matches!(
                Selection::from_menu(0, 1, index, 2),
                Selection::Close
            ));
        }
        assert!(matches!(
            Selection::from_menu(0, 1, Some(0), 0),
            Selection::Close
        ));
        assert!(matches!(
            Selection::from_menu(4, 6, Some(1), 2),
            Selection::KeepOpen
        ));
        assert!(matches!(
            Selection::from_menu(0, 6, Some(1), 2),
            Selection::Apply(Edit::Choice(ChoiceTarget::Mode, 1))
        ));
        assert!(matches!(
            Selection::from_menu(0, 12, Some(1), 2),
            Selection::Apply(Edit::Choice(ChoiceTarget::Font, 1))
        ));
        assert!(matches!(
            Selection::from_menu(0, 999, Some(1), 2),
            Selection::Close
        ));
        assert!(TextStyleTarget::from_field(999).is_none());
    }

    #[test]
    fn custom_color_updates_only_its_target_and_survives_parameter_changes() {
        let targets = [
            ColorTarget::Body,
            ColorTarget::TextOutline,
            ColorTarget::TextBackground,
            ColorTarget::SequenceText,
        ];
        let palette = palette(8);
        for target in targets {
            let mut values = AnnotationValues::default();
            for (index, other) in targets.into_iter().enumerate() {
                other.set(&mut values, [index as u8 + 1, 20, 30]);
            }
            let before = values.clone();
            target.set(&mut values, [91, 132, 173]);
            values.text_size = 100;
            values.strength = 75.;
            for other in targets {
                assert_eq!(
                    other.color(&values, palette),
                    if target == other {
                        [91, 132, 173]
                    } else {
                        other.color(&before, palette)
                    }
                );
            }
        }
        assert_eq!(ColorTarget::from_field(-7), None);
    }
}
