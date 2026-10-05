//! Draft construction and multi-click drawing state, without window or Registry ownership.
use super::{mosaic, parameters::endpoint};
use crate::AnnotationValues;
use lexift_core::domain::annotation::{Bounds, Extra, Kind, Object, Style};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct ArrowPending {
    start: (f32, f32),
    style: Style,
    start_scale: f32,
    values: AnnotationValues,
}

impl ArrowPending {
    pub fn new(
        start: (f32, f32),
        style: Style,
        start_scale: f32,
        values: AnnotationValues,
    ) -> Self {
        Self {
            start,
            style,
            start_scale,
            values,
        }
    }

    pub fn preview(&self, end: (f32, f32)) -> Object {
        arrow_draft(self.start, end, self.style, &self.values)
    }

    pub fn finish(&self, end: (f32, f32)) -> Option<Object> {
        arrow_endpoint_valid(self.start, end, self.start_scale).then(|| self.preview(end))
    }
}

/// Confirmed nodes and click timing belong to the same unfinished polyline.
pub(super) struct Polyline {
    object: Object,
    last_click: Option<(Instant, (f32, f32))>,
}

impl Polyline {
    pub fn new(point: (f32, f32), style: Style, values: &AnnotationValues) -> Self {
        Self {
            object: Object {
                id: Default::default(),
                bounds: Bounds::from_corners(point, point),
                kind: Kind::Polyline,
                style,
                points: Vec::new(),
                extra: Extra::Polyline {
                    curved: values.shape == 1,
                    head: values.head,
                    start: endpoint(values.line_start),
                    end: endpoint(values.line_end),
                },
            },
            last_click: None,
        }
    }

    pub fn is_double_click(&self, point: (f32, f32), scale: f32, now: Instant) -> bool {
        self.last_click.is_some_and(|(time, last)| {
            now.saturating_duration_since(time) <= Duration::from_millis(350)
                && (point.0 - last.0).hypot(point.1 - last.1) <= 6. * scale
        })
    }

    pub fn place(&mut self, point: (f32, f32), now: Instant) {
        self.object.points.push(point);
        self.object.refresh_bounds();
        self.last_click = Some((now, point));
    }

    pub fn confirmed(&self) -> Object {
        self.object.clone()
    }

    #[cfg(test)]
    pub fn preview(&self, point: (f32, f32), scale: f32) -> Object {
        polyline_preview(&self.object, point, scale)
    }

    /// Retire click timing along with a node so the next click is not a completion.
    pub fn backspace(&mut self) -> bool {
        self.object.points.pop();
        self.last_click = None;
        if self.object.points.is_empty() {
            return false;
        }
        self.object.refresh_bounds();
        true
    }

    pub fn finish(self) -> Option<Object> {
        (self.object.points.len() >= if self.object.style.fill { 3 } else { 2 })
            .then_some(self.object)
    }
}

/// An unfinished multi-click drawing has one kind and one set of frozen parameters.
enum Pending {
    Arrow(ArrowPending),
    Polyline(Polyline),
}

/// Multi-click input and its displayed preview retire together. The registry
/// cannot replace either independently or leave a preview after completion.
#[derive(Default)]
pub(super) struct PendingState {
    operation: Option<Pending>,
    preview: Option<Object>,
}

impl PendingState {
    pub fn is_none(&self) -> bool {
        self.operation.is_none()
    }
    pub fn arrow(&self) -> Option<&ArrowPending> {
        self.operation.as_ref().and_then(Pending::arrow)
    }
    pub fn polyline(&self) -> Option<&Polyline> {
        self.operation.as_ref().and_then(Pending::polyline)
    }
    pub fn await_arrow(&mut self, arrow: ArrowPending) {
        self.operation = Some(Pending::Arrow(arrow));
        self.preview = None;
    }
    pub fn cancel(&mut self) {
        Pending::cancel(&mut self.operation, &mut self.preview);
    }
    pub fn release_arrow(&mut self, point: (f32, f32)) -> Option<Object> {
        Pending::release_arrow(&mut self.operation, &mut self.preview, point)
    }
    pub fn finish_polyline(&mut self) -> Option<Progress> {
        Pending::finish_polyline(&mut self.operation, &mut self.preview)
    }
    pub fn backspace_polyline(&mut self) -> Option<Progress> {
        Pending::backspace_polyline(&mut self.operation, &mut self.preview)
    }
    pub fn start_polyline(&mut self, point: (f32, f32), style: Style, values: &AnnotationValues) {
        if self.operation.is_none() {
            self.operation = Some(Pending::Polyline(Polyline::new(point, style, values)));
        }
    }

    pub fn place_polyline(
        &mut self,
        point: (f32, f32),
        scale: f32,
        now: Instant,
        style: Style,
        values: &AnnotationValues,
    ) -> Option<Progress> {
        Pending::place_polyline(
            &mut self.operation,
            &mut self.preview,
            point,
            scale,
            now,
            style,
            values,
        )
    }
    pub fn pointer_moved(&mut self, point: (f32, f32), scale: f32, expected: Option<Kind>) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|pending| pending.pointer_moved(&mut self.preview, point, scale, expected))
    }
    pub fn pointer_left(&mut self) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|pending| pending.pointer_left(&mut self.preview))
    }
}

/// Borrow only the two drawing fields so scene compilation can independently
/// mutate its caches. A pending arrow with no preview must not reveal an older
/// captured draft while it waits for a valid next click.
pub(super) fn display_draft<'a>(
    captured: Option<&'a Press>,
    pending: &'a PendingState,
) -> Option<&'a Object> {
    if pending.is_none() {
        captured.and_then(Press::preview)
    } else {
        pending.preview.as_ref()
    }
}

pub(super) enum Progress {
    Retained,
    Retired(Option<Box<Object>>),
}

impl Pending {
    pub fn cancel(slot: &mut Option<Self>, draft: &mut Option<Object>) {
        *slot = None;
        *draft = None;
    }
    pub fn release_arrow(
        slot: &mut Option<Self>,
        draft: &mut Option<Object>,
        point: (f32, f32),
    ) -> Option<Object> {
        slot.as_ref()?.arrow()?;
        let result = Self::finish_arrow(slot, point);
        // Invalid endpoints keep the pending arrow but retire its click preview.
        *draft = None;
        result
    }
    pub fn finish_polyline(
        slot: &mut Option<Self>,
        draft: &mut Option<Object>,
    ) -> Option<Progress> {
        let polyline = Self::take_polyline(slot)?;
        *draft = None;
        Some(Progress::Retired(polyline.finish().map(Box::new)))
    }
    pub fn place_polyline(
        slot: &mut Option<Self>,
        draft: &mut Option<Object>,
        point: (f32, f32),
        scale: f32,
        now: Instant,
        style: Style,
        values: &AnnotationValues,
    ) -> Option<Progress> {
        if slot.is_none() {
            *slot = Some(Self::Polyline(Polyline::new(point, style, values)));
        }
        let polyline = slot.as_mut()?.polyline_mut()?;
        if polyline.is_double_click(point, scale, now) {
            return Self::finish_polyline(slot, draft);
        }
        polyline.place(point, now);
        *draft = Some(polyline.confirmed());
        Some(Progress::Retained)
    }
    pub fn backspace_polyline(
        slot: &mut Option<Self>,
        draft: &mut Option<Object>,
    ) -> Option<Progress> {
        let polyline = slot.as_mut()?.polyline_mut()?;
        if polyline.backspace() {
            *draft = Some(polyline.confirmed());
            Some(Progress::Retained)
        } else {
            Self::cancel(slot, draft);
            Some(Progress::Retired(None))
        }
    }

    pub fn arrow(&self) -> Option<&ArrowPending> {
        if let Self::Arrow(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn polyline(&self) -> Option<&Polyline> {
        if let Self::Polyline(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn polyline_mut(&mut self) -> Option<&mut Polyline> {
        if let Self::Polyline(value) = self {
            Some(value)
        } else {
            None
        }
    }
    #[cfg(test)]
    pub fn preview(&self, point: (f32, f32), scale: f32) -> Object {
        match self {
            Self::Arrow(value) => value.preview(point),
            Self::Polyline(value) => value.preview(point, scale),
        }
    }
    /// Captured requests must match the pending kind; hover can preview either.
    /// The viewport scale affects a polyline's tentative-node threshold, while
    /// arrow styling remains frozen at its initial press.
    pub fn pointer_moved(
        &self,
        draft: &mut Option<Object>,
        point: (f32, f32),
        scale: f32,
        expected: Option<Kind>,
    ) -> bool {
        let kind = match self {
            Self::Arrow(_) => Kind::Arrow,
            Self::Polyline(_) => Kind::Polyline,
        };
        if expected.is_some_and(|expected| expected != kind) {
            return false;
        }
        match self {
            Self::Arrow(value) => *draft = Some(value.preview(point)),
            Self::Polyline(value) => display_polyline(&value.object, draft, Some((point, scale))),
        }
        true
    }

    /// Outside the canvas a polyline displays confirmed nodes; an arrow keeps
    /// its last preview. Report changes so the coordinator refreshes even when
    /// no selected or hovered object needs an update.
    pub fn pointer_left(&self, draft: &mut Option<Object>) -> bool {
        if let Self::Polyline(value) = self
            && draft.as_ref() != Some(&value.object)
        {
            display_polyline(&value.object, draft, None);
            return true;
        }
        false
    }
    pub fn take_polyline(slot: &mut Option<Self>) -> Option<Polyline> {
        match slot.take()? {
            Self::Polyline(value) => Some(value),
            other => {
                *slot = Some(other);
                None
            }
        }
    }
    /// Invalid endpoints leave the frozen arrow available for the next click.
    pub fn finish_arrow(slot: &mut Option<Self>, point: (f32, f32)) -> Option<Object> {
        let result = slot.as_ref()?.arrow()?.finish(point)?;
        *slot = None;
        Some(result)
    }
}

#[derive(Clone, Copy)]
pub(super) struct PressInfo {
    pub start: (f32, f32),
    pub kind: Kind,
    pub style: Style,
    pub start_scale: f32,
    pub max_distance: f32,
}

/// Terminal result of one captured drawing. The coordinator applies document
/// or sampling effects without reinterpreting the press geometry.
pub(super) enum Release {
    Add(Object),
    StageMosaic(Object),
    AwaitArrow(ArrowPending),
    Magnifier {
        source: Bounds,
        style: Style,
        values: Box<AnnotationValues>,
    },
}

/// Immutable press parameters and accumulated movement for one drawing capture.
pub(super) struct Press {
    info: PressInfo,
    preview: Option<Object>,
    values: AnnotationValues,
}

impl Press {
    fn new(start: (f32, f32), kind: Kind, style: Style, scale: f32) -> Self {
        Self {
            info: PressInfo {
                start,
                kind,
                style,
                start_scale: scale,
                max_distance: 0.,
            },
            preview: None,
            values: AnnotationValues::default(),
        }
    }

    /// Start a capture with its own preview. Mosaic needs an initial sample for
    /// single-click brushes and must freeze its settings at the press DPI.
    pub fn begin(
        start: (f32, f32),
        kind: Kind,
        style: Style,
        scale: f32,
        values: &AnnotationValues,
    ) -> Self {
        let mut press = Self::new(start, kind, style, scale);
        press.values = values.clone();
        if matches!(kind, Kind::MosaicArea | Kind::MosaicBrush) {
            press.update(start, scale);
        }
        press
    }

    /// Consuming the capture releases its preview without touching the document.
    pub fn cancel(self) {}

    pub fn preview(&self) -> Option<&Object> {
        self.preview.as_ref()
    }

    /// Consume the capture and its displayed draft together. Mosaic sizing uses
    /// press DPI; other tools retain the existing release-viewport semantics.
    pub fn release(mut self, point: (f32, f32), viewport_scale: f32) -> Option<Release> {
        let info = self.info;
        let values = &self.values;
        if info.kind == Kind::Arrow {
            return Some(
                if arrow_drag_commits(info.start, point, info.max_distance, info.start_scale) {
                    Release::Add(arrow_draft(info.start, point, info.style, values))
                } else {
                    Release::AwaitArrow(ArrowPending::new(
                        info.start,
                        info.style,
                        info.start_scale,
                        values.clone(),
                    ))
                },
            );
        }
        if info.kind == Kind::Magnifier {
            return Some(Release::Magnifier {
                source: Bounds::from_corners(info.start, point),
                style: info.style,
                values: Box::new(self.values),
            });
        }
        self.update(point, viewport_scale);
        let object = self.preview.take()?;
        Some(if info.kind.tool() == 11 {
            Release::StageMosaic(object)
        } else {
            Release::Add(object)
        })
    }

    pub fn info(&self) -> PressInfo {
        self.info
    }

    pub fn update(&mut self, point: (f32, f32), viewport_scale: f32) {
        if self.info.kind == Kind::Arrow {
            self.info.max_distance = self
                .info
                .max_distance
                .max(arrow_distance(self.info.start, point));
        }
        let scale = if self.info.kind.tool() == 11 {
            self.info.start_scale
        } else {
            viewport_scale
        };
        update_draft(
            &mut self.preview,
            self.info.start,
            point,
            self.info.kind,
            self.info.style,
            scale,
            &self.values,
        );
    }
}

pub(super) fn update_draft(
    draft: &mut Option<Object>,
    start: (f32, f32),
    point: (f32, f32),
    kind: Kind,
    style: Style,
    scale: f32,
    values: &AnnotationValues,
) {
    if kind == Kind::MosaicBrush {
        let extra = mosaic::settings(values, scale);
        let object = draft.get_or_insert_with(|| {
            let mut object = Object {
                id: Default::default(),
                bounds: Bounds::from_corners(start, start),
                kind,
                style,
                points: vec![start],
                extra,
            };
            // A repeated initial sample is skipped by append_point. Initialize
            // radius padding here so a single click still has its full bounds.
            object.refresh_bounds();
            object
        });
        object.append_point(point, 0.5 * scale);
    } else if kind == Kind::Pencil {
        let object = draft.get_or_insert_with(|| Object {
            id: Default::default(),
            bounds: Bounds::from_corners(start, start),
            kind,
            style,
            points: vec![start],
            extra: Extra::None,
        });
        object.append_point(point, 0.5 * scale);
    } else if kind == Kind::Arrow {
        *draft = Some(arrow_draft(start, point, style, values));
    } else {
        let bounds = Bounds::from_corners(start, point);
        let extra = match kind {
            Kind::MosaicArea => draft
                .as_ref()
                .map(|o| o.extra.clone())
                .unwrap_or_else(|| mosaic::settings(values, scale)),
            // The drag previews only the source outline. Sampling starts on release.
            Kind::Magnifier => Extra::None,
            _ => Extra::None,
        };
        let points = match kind {
            Kind::HighlightLine => vec![start, point],
            _ => vec![],
        };
        let draft_kind = if kind == Kind::Magnifier {
            if values.shape == 1 {
                Kind::Ellipse
            } else {
                Kind::Rectangle
            }
        } else {
            kind
        };
        let mut draft_style = style;
        if kind == Kind::Magnifier {
            draft_style.rounding = 0.;
        }
        *draft = Some(Object {
            id: Default::default(),
            bounds,
            kind: draft_kind,
            style: draft_style,
            points,
            extra,
        });
    }
}

pub(super) fn arrow_draft(
    start: (f32, f32),
    end: (f32, f32),
    style: Style,
    values: &AnnotationValues,
) -> Object {
    Object {
        id: Default::default(),
        bounds: Bounds::from_corners(start, end),
        kind: Kind::Arrow,
        style,
        points: vec![start, end],
        extra: Extra::Arrow {
            curved: false,
            head: values.head,
            start: endpoint(values.line_start),
            end: endpoint(values.line_end),
        },
    }
}

pub(super) fn arrow_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn arrow_endpoint_valid(start: (f32, f32), end: (f32, f32), scale: f32) -> bool {
    arrow_distance(start, end) >= 3. * scale
}

pub(super) fn arrow_drag_commits(
    start: (f32, f32),
    end: (f32, f32),
    max_distance: f32,
    scale: f32,
) -> bool {
    max_distance.max(arrow_distance(start, end)) > 3. * scale
        && arrow_endpoint_valid(start, end, scale)
}

/// Reuses display storage while keeping confirmed geometry and parameters frozen.
/// Only one tentative endpoint can extend the already refreshed confirmed bounds.
fn display_polyline(
    polyline: &Object,
    draft: &mut Option<Object>,
    pointer: Option<((f32, f32), f32)>,
) {
    let mut points = draft.take().map_or_else(Vec::new, |draft| draft.points);
    points.clone_from(&polyline.points);
    let mut bounds = polyline.bounds;
    if let Some((point, scale)) = pointer
        && points
            .last()
            .is_some_and(|last| (point.0 - last.0).hypot(point.1 - last.1) > 2. * scale)
    {
        points.push(point);
        bounds.left = bounds.left.min(point.0);
        bounds.top = bounds.top.min(point.1);
        bounds.right = bounds.right.max(point.0);
        bounds.bottom = bounds.bottom.max(point.1);
    }
    *draft = Some(Object {
        id: polyline.id,
        kind: polyline.kind,
        style: polyline.style,
        extra: polyline.extra.clone(),
        bounds,
        points,
    });
}

/// Independent former production path for geometry and cost comparisons.
#[cfg(test)]
pub(super) fn polyline_preview(polyline: &Object, point: (f32, f32), scale: f32) -> Object {
    let mut draft = polyline.clone();
    if polyline
        .points
        .last()
        .is_some_and(|last| (point.0 - last.0).hypot(point.1 - last.1) > 2. * scale)
    {
        draft.points.push(point);
        draft.refresh_bounds();
    }
    draft
}

#[cfg(test)]
mod tests {
    use super::super::parameters::defaults;
    use super::*;

    fn reference_mosaic_update(
        draft: &mut Option<Object>,
        start: (f32, f32),
        point: (f32, f32),
        scale: f32,
        values: &AnnotationValues,
    ) {
        let extra = mosaic::settings(values, scale);
        let object = draft.get_or_insert_with(|| Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            style: Style::default(),
            bounds: Bounds::from_corners(start, start),
            points: vec![start],
            extra,
        });
        object.append_point(point, 0.5 * scale);
        object.refresh_bounds();
    }

    #[test]
    fn press_and_multi_click_drafts_freeze_all_parameters() {
        for scale in [1., 1.25] {
            let mut values = defaults(5);
            values.shape = 1;
            values.line_start = 2;
            values.line_end = 3;
            values.style = 1;
            values.zoom = 3;
            values.connector_style = 2;
            let saved = values.clone();
            let mut press = Press::begin(
                (-100., -50.),
                Kind::Magnifier,
                Style::default(),
                scale,
                &values,
            );
            values.shape = 0;
            values.zoom = 0;
            press.update((40., 60.), scale);
            assert_eq!(press.preview().unwrap().kind, Kind::Ellipse);
            let Some(Release::Magnifier { values, .. }) = press.release((40., 60.), scale) else {
                panic!()
            };
            assert_eq!(*values, saved);
            let press = Press::begin((-100., -50.), Kind::Arrow, Style::default(), scale, &saved);
            let Some(Release::AwaitArrow(pending)) = press.release((-100., -50.), scale) else {
                panic!()
            };
            let mut state = PendingState::default();
            state.await_arrow(pending);
            let arrow = state.release_arrow((40., 60.)).unwrap();
            assert_eq!(
                arrow,
                arrow_draft((-100., -50.), (40., 60.), Style::default(), &saved)
            );
            let mut state = PendingState::default();
            state.start_polyline((-100., -50.), Style::default(), &saved);
            state.place_polyline(
                (-100., -50.),
                scale,
                Instant::now(),
                Style::default(),
                &defaults(6),
            );
            state.place_polyline(
                (40., 60.),
                scale,
                Instant::now(),
                Style::default(),
                &defaults(6),
            );
            let mut reference = Polyline::new((-100., -50.), Style::default(), &saved);
            reference.place((-100., -50.), Instant::now());
            reference.place((40., 60.), Instant::now());
            assert_eq!(state.polyline().unwrap().confirmed(), reference.confirmed());
        }
    }

    #[test]
    fn reusable_previews_match_original_geometry_and_frozen_values() {
        for scale in [1., 1.25] {
            let mut line = Polyline::new((-50., -20.), Style::default(), &defaults(6));
            line.place((-50., -20.), Instant::now());
            line.place((-20., 30.), Instant::now());
            let confirmed = line.confirmed();
            let pending = Pending::Polyline(line);
            let mut draft = None;
            let mut allocation = None;
            for point in [
                (20., 10.),
                (-100., -40.),
                (0., 80.),
                (-20., 30.),
                (-20. + scale, 30.),
            ]
            .into_iter()
            .cycle()
            .take(100)
            {
                assert!(pending.pointer_moved(&mut draft, point, scale, None));
                assert_eq!(draft, Some(polyline_preview(&confirmed, point, scale)));
                let address = draft.as_ref().unwrap().points.as_ptr();
                if let Some(previous) = allocation {
                    assert_eq!(address, previous);
                }
                allocation = Some(address);
            }
            assert!(!pending.pointer_left(&mut draft));
            assert!(pending.pointer_moved(&mut draft, (-100., -40.), scale, None));
            assert!(pending.pointer_left(&mut draft));
            assert_eq!(draft, Some(confirmed));
            assert_eq!(draft.as_ref().unwrap().points.as_ptr(), allocation.unwrap());
            let mut actual = None;
            let mut reference = None;
            let values = defaults(11);
            let start = (-100., -50.);
            for point in [
                start,
                start,
                (-99.9, -50.),
                (-80., -50.),
                (-60., -50.),
                (-60., -10.),
                (-60., -10.),
                (-40., 0.),
                (-100., -80.),
            ] {
                update_draft(
                    &mut actual,
                    start,
                    point,
                    Kind::MosaicBrush,
                    Style::default(),
                    scale,
                    &values,
                );
                reference_mosaic_update(&mut reference, start, point, scale, &values);
                assert_eq!(actual, reference);
            }
        }
    }

    #[test]
    #[ignore = "run explicitly in Release without concurrent builds for CPU profiling"]
    fn long_drawing_preview_costs() {
        use std::hint::black_box;
        fn p95(mut samples: Vec<Duration>) -> f64 {
            samples.sort_unstable();
            samples[samples.len() * 95 / 100].as_secs_f64() * 1000.
        }
        for count in [1_000, 20_000, 100_000] {
            let points: Vec<_> = (0..count)
                .map(|i| (i as f32, if i % 2 == 0 { -10. } else { 10. }))
                .collect();
            let mut line = Polyline::new(points[0], Style::default(), &defaults(6));
            line.object.points = points.clone();
            line.object.refresh_bounds();
            let pending = Pending::Polyline(line);
            let mut preview = None;
            pending.pointer_moved(&mut preview, (count as f32, 20.), 1.25, None);
            let base = Object {
                id: Default::default(),
                kind: Kind::MosaicBrush,
                bounds: Bounds::from_corners((0., 0.), (0., 0.)),
                points,
                style: Style::default(),
                extra: mosaic::settings(&defaults(11), 1.25),
            };
            let mut base = base;
            base.refresh_bounds();
            let mut polyline = Vec::new();
            let mut old_polyline = Vec::new();
            let mut brush = Vec::new();
            let mut old_brush = Vec::new();
            let mut scan = Vec::new();
            for sample in 0..240 {
                let measure_brush = |legacy| {
                    // Prepare each input immediately before its own measurement;
                    // alternating order avoids favoring the second warmed copy.
                    let mut draft = Some(base.clone());
                    let began = Instant::now();
                    if legacy {
                        reference_mosaic_update(
                            &mut draft,
                            (0., -10.),
                            (count as f32, 20.),
                            1.25,
                            &defaults(11),
                        );
                    } else {
                        update_draft(
                            &mut draft,
                            (0., -10.),
                            (count as f32, 20.),
                            Kind::MosaicBrush,
                            Style::default(),
                            1.25,
                            &defaults(11),
                        );
                    }
                    black_box(&draft);
                    (began.elapsed(), draft)
                };
                let (new_result, old_result) = if sample % 2 == 0 {
                    let new_result = measure_brush(false);
                    (new_result, measure_brush(true))
                } else {
                    let old_result = measure_brush(true);
                    (measure_brush(false), old_result)
                };
                brush.push(new_result.0);
                old_brush.push(old_result.0);
                let mut draft = new_result.1;
                assert_eq!(draft, old_result.1);
                let mut measure_polyline = |legacy| {
                    let began = Instant::now();
                    if legacy {
                        black_box(pending.preview((count as f32, 20.), 1.25));
                    } else {
                        black_box(pending.pointer_moved(
                            black_box(&mut preview),
                            (count as f32, 20.),
                            1.25,
                            None,
                        ));
                    }
                    began.elapsed()
                };
                if sample % 2 == 0 {
                    polyline.push(measure_polyline(false));
                    old_polyline.push(measure_polyline(true));
                } else {
                    old_polyline.push(measure_polyline(true));
                    polyline.push(measure_polyline(false));
                }
                let began = Instant::now();
                draft.as_mut().unwrap().refresh_bounds();
                black_box(&draft);
                scan.push(began.elapsed());
            }
            eprintln!(
                "drawing_preview_scope=cpu_only points={count} samples=240 polyline_p95_ms={:.6} mosaic_append_p95_ms={:.6} bounds_scan_p95_ms={:.6} old_polyline_p95_ms={:.6} old_mosaic_append_p95_ms={:.6}",
                p95(polyline),
                p95(brush),
                p95(scan),
                p95(old_polyline),
                p95(old_brush)
            );
        }
    }

    #[test]
    fn begin_owns_initial_preview_and_freezes_mosaic_before_first_move() {
        use lexift_core::domain::annotation::Session;
        for scale in [1., 1.25] {
            let start = (-100., -50.);
            for kind in [Kind::MosaicArea, Kind::MosaicBrush] {
                let mut values = defaults(11);
                values.size = 27;
                values.strength = 73.;
                let press = Press::begin(start, kind, Style::default(), scale, &values);
                let initial = press.preview().expect("mosaic must appear on press");
                assert_eq!(initial.kind, kind);
                assert_eq!(
                    initial.points,
                    if kind == Kind::MosaicBrush {
                        vec![start]
                    } else {
                        vec![]
                    }
                );
                let settings = initial.extra.clone();
                if kind == Kind::MosaicBrush {
                    assert!(initial.bounds.width() > 0. && initial.bounds.height() > 0.);
                }
                values.size = 3;
                values.strength = 1.;
                let Some(Release::StageMosaic(object)) = press.release(start, 2.) else {
                    panic!("mosaic must enter capture validation");
                };
                assert_eq!(
                    object.extra, settings,
                    "release cannot replace press settings"
                );
                assert_eq!(
                    object.points,
                    if kind == Kind::MosaicBrush {
                        vec![start]
                    } else {
                        vec![]
                    }
                );
            }
            let press = Press::begin(start, Kind::Pencil, Style::default(), scale, &defaults(3));
            assert!(press.preview().is_none());
            let Some(Release::Add(object)) = press.release(start, scale) else {
                panic!("single click pencil must finish");
            };
            assert_eq!(object.kind, Kind::Pencil);
            assert_eq!(object.points, vec![start]);
            let mut session = Session::new();
            assert!(session.add(object));
            let baseline = session.objects.clone();
            let press = Press::begin(
                start,
                Kind::MosaicBrush,
                Style::default(),
                scale,
                &defaults(11),
            );
            press.cancel();
            assert_eq!(session.objects, baseline);
            assert!(session.undo());
            assert!(session.objects.is_empty() && !session.can_undo());
        }
    }

    #[test]
    fn release_consumes_single_point_and_long_strokes_without_extra_undo() {
        use lexift_core::domain::annotation::Session;
        for scale in [1., 1.25] {
            for moved in [false, true] {
                let start = (-100., -50.);
                let mut press =
                    Press::begin(start, Kind::Pencil, Style::default(), scale, &defaults(3));
                if moved {
                    press.update((-70., -25.), scale);
                }
                let end = if moved { (-40., 0.) } else { start };
                let Some(Release::Add(object)) = press.release(end, scale) else {
                    panic!("pencil did not finish");
                };
                assert_eq!(object.points.first(), Some(&start));
                assert_eq!(object.points.last(), Some(&end));
                let mut session = Session::new();
                assert!(session.add(object));
                assert!(session.undo());
                assert!(session.objects.is_empty() && !session.can_undo());
            }
        }
    }

    #[test]
    fn release_preserves_arrow_threshold_mosaic_dpi_and_magnifier_source() {
        for scale in [1., 1.25] {
            let start = (-80., -40.);
            let mut press = Press::begin(start, Kind::Arrow, Style::default(), scale, &defaults(5));
            press.update((start.0 + 20. * scale, start.1), scale);
            let Some(Release::AwaitArrow(arrow)) = press.release(start, scale) else {
                panic!("drag back should await a valid endpoint");
            };
            assert!(arrow.finish(start).is_none());
            assert!(arrow.finish((start.0 + 4. * scale, start.1)).is_some());
            let press = Press::begin(start, Kind::Arrow, Style::default(), scale, &defaults(5));
            assert!(matches!(
                press.release((start.0 + 4. * scale, start.1), scale),
                Some(Release::Add(_))
            ));
            for kind in [Kind::MosaicArea, Kind::MosaicBrush] {
                let press = Press::begin(start, kind, Style::default(), scale, &defaults(11));
                let Some(Release::StageMosaic(object)) = press.release((-20., 30.), 2.) else {
                    panic!("mosaic bypassed capture validation");
                };
                let Extra::Mosaic(settings) = object.extra else {
                    panic!("missing mosaic settings");
                };
                assert_eq!(settings.creation_scale, scale);
            }
            let press = Press::begin(
                start,
                Kind::Magnifier,
                Style::default(),
                scale,
                &defaults(7),
            );
            let Some(Release::Magnifier { source, .. }) = press.release((-20., 30.), 2.) else {
                panic!("magnifier source missing");
            };
            assert_eq!(source, Bounds::from_corners(start, (-20., 30.)));
        }
    }

    #[test]
    fn owned_pending_preview_retires_with_completion_backspace_and_cancel() {
        use lexift_core::domain::annotation::Session;
        for scale in [1., 1.25] {
            let start = (-120., -60.);
            let next = (-80., -20.);
            let hover = (-40., 20.);
            let now = Instant::now();
            let captured = None;
            let mut state = PendingState::default();
            assert!(matches!(
                state.place_polyline(start, scale, now, Style::default(), &defaults(6)),
                Some(Progress::Retained)
            ));
            assert!(state.pointer_moved(hover, scale, None));
            assert_eq!(
                display_draft(captured, &state).unwrap().points,
                [start, hover]
            );
            assert!(state.release_arrow(next).is_none());
            assert_eq!(
                display_draft(captured, &state).unwrap().points,
                [start, hover]
            );
            assert!(state.pointer_left());
            assert!(!state.pointer_left());
            assert_eq!(display_draft(captured, &state).unwrap().points, [start]);
            state.place_polyline(
                next,
                scale,
                now + Duration::from_secs(1),
                Style::default(),
                &defaults(6),
            );
            state.pointer_moved(hover, scale, None);
            let Some(Progress::Retired(Some(object))) = state.finish_polyline() else {
                panic!("confirmed line missing");
            };
            assert_eq!(
                object.points,
                [start, next],
                "hover is not a confirmed node"
            );
            assert!(state.is_none() && display_draft(captured, &state).is_none());
            assert!(!state.pointer_moved(hover, scale, None));
            let mut session = Session::new();
            assert!(session.add(*object));
            assert!(session.undo());
            assert!(!session.can_undo());

            state.place_polyline(start, scale, now, Style::default(), &defaults(6));
            state.pointer_moved(hover, scale, None);
            assert!(matches!(
                state.backspace_polyline(),
                Some(Progress::Retired(None))
            ));
            assert!(state.is_none() && display_draft(captured, &state).is_none());
            state.await_arrow(ArrowPending::new(
                start,
                Style::default(),
                scale,
                defaults(5),
            ));
            state.pointer_moved(hover, scale, None);
            assert!(state.release_arrow(start).is_none());
            assert!(!state.is_none());
            assert!(display_draft(captured, &state).is_none());
            assert!(state.finish_polyline().is_none());
            state.pointer_moved(hover, scale, Some(Kind::Arrow));
            state.cancel();
            assert!(state.is_none() && display_draft(captured, &state).is_none());
            assert!(state.release_arrow(next).is_none());
            assert!(!state.pointer_left());
            assert!(session.objects.is_empty() && !session.can_undo());
        }
    }

    #[test]
    fn pending_pointer_preview_is_kind_scoped_and_reports_exit_changes() {
        for scale in [1., 1.25] {
            let start = (-100., -50.);
            let next = (-80., -30.);
            let mut line = Polyline::new(start, Style::default(), &defaults(6));
            line.place(start, Instant::now());
            let confirmed = line.confirmed();
            let pending = Pending::Polyline(line);
            let mut draft = Some(confirmed.clone());
            assert!(!pending.pointer_moved(&mut draft, next, scale, Some(Kind::Arrow)));
            assert_eq!(draft, Some(confirmed.clone()));
            assert!(pending.pointer_moved(&mut draft, next, scale, Some(Kind::Polyline)));
            assert_eq!(draft.as_ref().unwrap().points, [start, next]);
            assert!(pending.pointer_left(&mut draft));
            assert_eq!(draft, Some(confirmed.clone()));
            assert!(!pending.pointer_left(&mut draft));
            assert!(pending.pointer_moved(&mut draft, next, scale, None));
            assert!(pending.pointer_left(&mut draft));
            assert!(pending.pointer_moved(&mut draft, (start.0 + scale, start.1), scale, None,));
            assert_eq!(draft, Some(confirmed));
            assert!(!pending.pointer_left(&mut draft));
            let Pending::Polyline(line) = pending else {
                unreachable!();
            };
            assert!(line.finish().is_none(), "hover must not confirm nodes");
            let pending = Pending::Arrow(ArrowPending::new(
                start,
                Style::default(),
                scale,
                defaults(5),
            ));
            assert!(pending.pointer_moved(&mut draft, next, 2., None));
            let preview = draft.clone();
            assert!(!pending.pointer_moved(&mut draft, (0., 0.), scale, Some(Kind::Polyline)));
            assert_eq!(draft, preview);
            assert!(!pending.pointer_left(&mut draft));
            assert_eq!(draft, preview);
            assert!(pending.pointer_moved(&mut draft, next, scale, Some(Kind::Arrow)));
            assert_eq!(
                draft, preview,
                "viewport DPI must not restyle a pending arrow"
            );
        }
    }

    #[test]
    fn pending_handoff_retires_display_and_preserves_wrong_kind_operations() {
        let start = (-100., -50.);
        let now = Instant::now();
        let mut slot = None;
        let mut draft = None;
        assert!(matches!(
            Pending::place_polyline(
                &mut slot,
                &mut draft,
                start,
                1.25,
                now,
                Style::default(),
                &defaults(6)
            ),
            Some(Progress::Retained)
        ));
        let confirmed = draft.clone();
        assert!(Pending::release_arrow(&mut slot, &mut draft, (0., 0.)).is_none());
        assert_eq!(draft, confirmed);
        let next = (0., 0.);
        assert!(matches!(
            Pending::place_polyline(
                &mut slot,
                &mut draft,
                next,
                1.25,
                now + Duration::from_secs(1),
                Style::default(),
                &defaults(6)
            ),
            Some(Progress::Retained)
        ));
        let Some(Progress::Retired(Some(object))) = Pending::place_polyline(
            &mut slot,
            &mut draft,
            next,
            1.25,
            now + Duration::from_millis(1100),
            Style::default(),
            &defaults(6),
        ) else {
            panic!("double click did not complete");
        };
        assert_eq!(object.points, [start, next]);
        assert!(slot.is_none() && draft.is_none());
        assert!(Pending::finish_polyline(&mut slot, &mut draft).is_none());
        Pending::place_polyline(
            &mut slot,
            &mut draft,
            start,
            1.,
            now,
            Style::default(),
            &defaults(6),
        );
        assert!(matches!(
            Pending::backspace_polyline(&mut slot, &mut draft),
            Some(Progress::Retired(None))
        ));
        assert!(slot.is_none() && draft.is_none());
        slot = Some(Pending::Arrow(ArrowPending::new(
            start,
            Style::default(),
            1.25,
            defaults(5),
        )));
        draft = Some(slot.as_ref().unwrap().preview(next, 1.25));
        let original = draft.clone();
        assert!(Pending::finish_polyline(&mut slot, &mut draft).is_none());
        assert!(Pending::backspace_polyline(&mut slot, &mut draft).is_none());
        assert!(
            Pending::place_polyline(
                &mut slot,
                &mut draft,
                next,
                1.25,
                now,
                Style::default(),
                &defaults(6)
            )
            .is_none()
        );
        assert_eq!(draft, original);
        assert!(Pending::release_arrow(&mut slot, &mut draft, start).is_none());
        assert!(slot.is_some() && draft.is_none());
        assert!(Pending::release_arrow(&mut slot, &mut draft, next).is_some());
        assert!(slot.is_none() && draft.is_none());
        Pending::cancel(&mut slot, &mut draft);
        assert!(slot.is_none() && draft.is_none());
    }

    #[test]
    fn pending_completion_preserves_other_kinds_and_invalid_endpoints() {
        for scale in [1., 1.25] {
            let start = (-100., -50.);
            let end = (-80., -30.);
            let mut slot = Some(Pending::Arrow(ArrowPending::new(
                start,
                Style::default(),
                scale,
                defaults(2),
            )));
            let expected = slot.as_ref().unwrap().preview(end, scale);
            assert!(Pending::take_polyline(&mut slot).is_none());
            assert!(Pending::finish_arrow(&mut slot, start).is_none());
            assert_eq!(slot.as_ref().unwrap().preview(end, scale), expected);
            let mut draft = Some(expected.clone());
            slot.as_ref().unwrap().pointer_left(&mut draft);
            assert_eq!(draft, Some(expected.clone()));
            assert_eq!(Pending::finish_arrow(&mut slot, end), Some(expected));
            assert!(slot.is_none());

            let mut line = Polyline::new(start, Style::default(), &defaults(6));
            line.place(start, Instant::now());
            line.place(end, Instant::now());
            let confirmed = line.confirmed();
            slot = Some(Pending::Polyline(line));
            assert!(Pending::finish_arrow(&mut slot, end).is_none());
            draft = Some(slot.as_ref().unwrap().preview((0., 0.), scale));
            assert_ne!(draft.as_ref(), Some(&confirmed));
            slot.as_ref().unwrap().pointer_left(&mut draft);
            assert_eq!(draft, Some(confirmed.clone()));
            assert_eq!(
                Pending::take_polyline(&mut slot).unwrap().finish(),
                Some(confirmed)
            );
            assert!(slot.is_none());
        }
    }

    #[test]
    fn polyline_completion_owns_click_timing_and_only_commits_confirmed_nodes() {
        for scale in [1., 1.25] {
            for filled in [false, true] {
                let style = Style {
                    fill: filled,
                    ..Style::default()
                };
                let now = Instant::now();
                let start = (-100., -50.);
                let mut polyline = Polyline::new(start, style, &defaults(6));
                polyline.place(start, now);
                assert!(polyline.is_double_click(start, scale, now + Duration::from_millis(350)));
                assert!(!polyline.is_double_click(start, scale, now + Duration::from_millis(351)));
                assert!(polyline.finish().is_none());
                let mut polyline = Polyline::new(start, style, &defaults(6));
                polyline.place(start, now);
                polyline.place((0., 0.), now);
                assert!(polyline.backspace());
                assert!(!polyline.is_double_click(start, scale, now));
                polyline.place((0., 0.), now);
                if filled {
                    polyline.place((10., -30.), now);
                }
                let confirmed = polyline.confirmed();
                assert!(polyline.preview((90., 80.), scale).points.len() > confirmed.points.len());
                let object = polyline.finish().unwrap();
                assert_eq!(object, confirmed);
                let mut session = lexift_core::domain::annotation::Session::new();
                assert!(session.add(object));
                assert!(session.undo());
                assert!(session.objects.is_empty() && !session.can_undo());
            }
        }
    }
}
