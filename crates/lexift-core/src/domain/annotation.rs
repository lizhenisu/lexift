//! Transient screen annotation state in physical virtual-desktop coordinates.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            left: a.0.min(b.0),
            top: a.1.min(b.1),
            right: a.0.max(b.0),
            bottom: a.1.max(b.1),
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }
    pub fn height(self) -> f32 {
        self.bottom - self.top
    }
    pub fn moved(self, dx: f32, dy: f32) -> Self {
        Self {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }
    pub fn resized(self, handle: usize, point: (f32, f32)) -> Self {
        let (mut l, mut t, mut r, mut b) = (self.left, self.top, self.right, self.bottom);
        if matches!(handle, 0 | 3 | 5) {
            l = point.0.min(r - 2.);
        }
        if matches!(handle, 2 | 4 | 7) {
            r = point.0.max(l + 2.);
        }
        if matches!(handle, 0..=2) {
            t = point.1.min(b - 2.);
        }
        if matches!(handle, 5..=7) {
            b = point.1.max(t + 2.);
        }
        Self {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }
    pub fn handles(self) -> [(f32, f32); 8] {
        let x = (self.left + self.right) / 2.;
        let y = (self.top + self.bottom) / 2.;
        [
            (self.left, self.top),
            (x, self.top),
            (self.right, self.top),
            (self.left, y),
            (self.right, y),
            (self.left, self.bottom),
            (x, self.bottom),
            (self.right, self.bottom),
        ]
    }
}

/// Centers a magnifier output frame around its sampled desktop region.
pub fn magnifier_output_for_source(source: Bounds, zoom: f32) -> Bounds {
    let zoom = zoom.clamp(1., 8.);
    let width = source.width() * zoom;
    let height = source.height() * zoom;
    let left = source.left - (width - source.width()) / 2.;
    let top = source.top - (height - source.height()) / 2.;
    Bounds {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Rectangle,
    Ellipse,
    SpotlightRectangle,
    SpotlightEllipse,
    Pencil,
    HighlightLine,
    HighlightRectangle,
    Arrow,
    Polyline,
    Magnifier,
}
impl Kind {
    pub fn is_spotlight(self) -> bool {
        matches!(self, Self::SpotlightRectangle | Self::SpotlightEllipse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: [u8; 3],
    pub width: f32,
    pub rounding: f32,
    pub fill: bool,
    pub dash: u8,
    pub outline: bool,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            color: [217, 71, 43],
            width: 4.,
            rounding: 21.,
            fill: false,
            dash: 0,
            outline: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub bounds: Bounds,
    pub kind: Kind,
    pub style: Style,
    /// Physical desktop points for pencil strokes and line endpoints.
    pub points: Vec<(f32, f32)>,
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Endpoint {
    #[default]
    None,
    OpenArrow,
    Arrow,
    FilledArrow,
    Circle,
    OpenCircle,
    Diamond,
    OpenDiamond,
    Bar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MagnifierConnector {
    #[default]
    Plain,
    Dot,
    Frame,
    None,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Extra {
    #[default]
    None,
    Arrow {
        curved: bool,
        head: bool,
        start: Endpoint,
        end: Endpoint,
    },
    Polyline {
        curved: bool,
        head: bool,
        start: Endpoint,
        end: Endpoint,
    },
    Magnifier {
        output: Bounds,
        zoom: f32,
        ellipse: bool,
        connector: MagnifierConnector,
        erase_annotations: bool,
        antialias: bool,
        shadow: bool,
    },
}

impl Kind {
    pub fn tool(self) -> usize {
        match self {
            Self::Pencil => 3,
            Self::HighlightLine | Self::HighlightRectangle => 4,
            Self::Arrow => 5,
            Self::Polyline => 6,
            Self::Magnifier => 7,
            Self::SpotlightRectangle | Self::SpotlightEllipse => 2,
            _ => 0,
        }
    }
}

impl Object {
    /// Changes magnification while keeping the output frame at its current center.
    pub fn set_magnifier_zoom_centered(&mut self, requested_zoom: f32) {
        let Extra::Magnifier { output, zoom, .. } = &mut self.extra else {
            return;
        };
        let next_zoom = requested_zoom.clamp(1., 8.);
        if *zoom == next_zoom {
            return;
        }
        let center_x = (output.left + output.right) / 2.;
        let center_y = (output.top + output.bottom) / 2.;
        let half_width = self.bounds.width() * next_zoom / 2.;
        let half_height = self.bounds.height() * next_zoom / 2.;
        *output = Bounds {
            left: center_x - half_width,
            top: center_y - half_height,
            right: center_x + half_width,
            bottom: center_y + half_height,
        };
        *zoom = next_zoom;
    }

    /// Returns the displayed centerline, including interpolated curve segments.
    pub fn stroke_points(&self) -> Vec<(f32, f32)> {
        if self.kind == Kind::Arrow {
            if let Extra::Arrow { curved: true, .. } = self.extra
                && self.points.len() >= 3
            {
                let [a, b, c] = [self.points[0], self.points[1], self.points[2]];
                return (0..=32)
                    .map(|i| {
                        let t = i as f32 / 32.;
                        let u = 1. - t;
                        (
                            u * u * a.0 + 2. * u * t * c.0 + t * t * b.0,
                            u * u * a.1 + 2. * u * t * c.1 + t * t * b.1,
                        )
                    })
                    .collect();
            }
            return self.points.iter().take(2).copied().collect();
        }
        if self.kind != Kind::Polyline
            || !matches!(self.extra, Extra::Polyline { curved: true, .. })
            || self.points.len() < 3
        {
            return self.points.clone();
        }
        let mut result = Vec::with_capacity((self.points.len() - 1) * 12 + 1);
        for i in 0..self.points.len() - 1 {
            let p0 = self.points[i.saturating_sub(1)];
            let p1 = self.points[i];
            let p2 = self.points[i + 1];
            let p3 = self.points[(i + 2).min(self.points.len() - 1)];
            for step in 0..12 {
                let t = step as f32 / 12.;
                let axis = |a: f32, b: f32, c: f32, d: f32| {
                    0.5 * ((2. * b)
                        + (-a + c) * t
                        + (2. * a - 5. * b + 4. * c - d) * t * t
                        + (-a + 3. * b - 3. * c + d) * t * t * t)
                };
                result.push((axis(p0.0, p1.0, p2.0, p3.0), axis(p0.1, p1.1, p2.1, p3.1)));
            }
        }
        result.push(*self.points.last().unwrap());
        result
    }
    pub fn refresh_bounds(&mut self) {
        if let Some(&(x, y)) = self.points.first() {
            self.bounds = Bounds::from_corners((x, y), (x, y));
            for &(x, y) in &self.points {
                self.bounds.left = self.bounds.left.min(x);
                self.bounds.top = self.bounds.top.min(y);
                self.bounds.right = self.bounds.right.max(x);
                self.bounds.bottom = self.bounds.bottom.max(y);
            }
        }
    }
    /// Drops redundant samples while retaining corners and the newest endpoint.
    pub fn append_point(&mut self, point: (f32, f32), tolerance: f32) {
        if self
            .points
            .last()
            .is_some_and(|p| distance(*p, point) < tolerance)
        {
            return;
        }
        let n = self.points.len();
        if n >= 2
            && segment_distance(self.points[n - 1], self.points[n - 2], point) < tolerance * 0.25
        {
            self.points.pop();
        }
        self.points.push(point);
        self.refresh_bounds();
    }
    pub fn hit_path(&self, point: (f32, f32), scale: f32) -> bool {
        let tolerance = (self.style.width / 2. + 6.) * scale;
        let rendered = self.stroke_points();
        let points = rendered.as_slice();
        if self.kind == Kind::Polyline
            && self.style.fill
            && rendered.len() >= 3
            && polygon_contains(point, &rendered)
        {
            return true;
        }
        points
            .first()
            .is_some_and(|p| distance(*p, point) <= tolerance)
            || points
                .windows(2)
                .any(|p| segment_distance(point, p[0], p[1]) <= tolerance)
    }
    pub fn edit_handles(&self, scale: f32) -> Vec<(f32, f32)> {
        if self.kind == Kind::Arrow && self.points.len() >= 2 {
            let a = self.points[0];
            let b = self.points[1];
            let midpoint = if matches!(self.extra, Extra::Arrow { curved: true, .. })
                && self.points.len() >= 3
            {
                let control = self.points[2];
                (
                    (a.0 + 2. * control.0 + b.0) / 4.,
                    (a.1 + 2. * control.1 + b.1) / 4.,
                )
            } else {
                ((a.0 + b.0) / 2., (a.1 + b.1) / 2.)
            };
            return vec![a, b, midpoint];
        }
        if matches!(
            self.kind,
            Kind::Arrow | Kind::Polyline | Kind::HighlightLine
        ) {
            return self.points.clone();
        }
        let mut handles = self.bounds.handles().to_vec();
        if let Extra::Magnifier { output, .. } = self.extra {
            handles.extend(output.handles());
        }
        if self.kind == Kind::HighlightRectangle {
            let b = self.bounds;
            let r = (self.style.rounding * scale)
                .min(b.width() / 2.)
                .min(b.height() / 2.);
            let inset = (r * 0.5 + 10. * scale)
                .min(b.width() / 2.)
                .min(b.height() / 2.);
            handles.extend([
                (b.left + inset, b.top + inset),
                (b.right - inset, b.top + inset),
                (b.left + inset, b.bottom - inset),
                (b.right - inset, b.bottom - inset),
            ]);
        }
        handles
    }
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.bounds = self.bounds.moved(dx, dy);
        for p in &mut self.points {
            p.0 += dx;
            p.1 += dy;
        }
        if let Extra::Magnifier { output, .. } = &mut self.extra {
            *output = output.moved(dx, dy);
        }
    }
    pub fn move_from(&mut self, origin: (f32, f32), dx: f32, dy: f32) {
        if let Extra::Magnifier { output, .. } = &mut self.extra {
            // The new object starts with the source inside the output frame.
            // Give the source precedence so its body still moves both regions.
            if inside(self.bounds, origin, 0.) {
                self.move_by(dx, dy);
            } else if inside(*output, origin, 0.) {
                *output = output.moved(dx, dy);
            } else {
                self.move_by(dx, dy);
            }
        } else {
            self.move_by(dx, dy);
        }
    }
    pub fn resize_to(&mut self, target: Bounds) {
        let b = self.bounds;
        for p in &mut self.points {
            p.0 = target.left + (p.0 - b.left) / b.width().max(1.) * target.width();
            p.1 = target.top + (p.1 - b.top) / b.height().max(1.) * target.height();
        }
        self.bounds = target;
    }
    pub fn edit_handle(&mut self, handle: usize, point: (f32, f32), scale: f32) {
        if self.kind == Kind::Arrow && handle == 2 && self.points.len() >= 2 {
            let a = self.points[0];
            let b = self.points[1];
            let midpoint = ((a.0 + b.0) / 2., (a.1 + b.1) / 2.);
            if distance(point, midpoint) <= 2. * scale {
                self.points.truncate(2);
                if let Extra::Arrow { curved, .. } = &mut self.extra {
                    *curved = false;
                }
            } else {
                let control = (2. * point.0 - midpoint.0, 2. * point.1 - midpoint.1);
                if self.points.len() == 2 {
                    self.points.push(control);
                } else {
                    self.points[2] = control;
                }
                if let Extra::Arrow { curved, .. } = &mut self.extra {
                    *curved = true;
                }
            }
            self.refresh_bounds();
        } else if matches!(
            self.kind,
            Kind::Arrow | Kind::Polyline | Kind::HighlightLine
        ) {
            if let Some(p) = self.points.get_mut(handle) {
                *p = point;
            }
            self.refresh_bounds();
        } else if let Extra::Magnifier { output, zoom, .. } = &mut self.extra {
            if handle >= 8 {
                let h = handle - 8;
                let resized = output.resized(h, point);
                *zoom = (if matches!(h, 1 | 6) {
                    resized.height() / self.bounds.height().max(1.)
                } else {
                    resized.width() / self.bounds.width().max(1.)
                })
                .clamp(1., 8.);
                let (width, height) = (self.bounds.width() * *zoom, self.bounds.height() * *zoom);
                if matches!(h, 0 | 3 | 5) {
                    output.left = output.right - width;
                } else {
                    output.right = output.left + width;
                }
                if matches!(h, 0..=2) {
                    output.top = output.bottom - height;
                } else {
                    output.bottom = output.top + height;
                }
            } else {
                self.bounds = self.bounds.resized(handle, point);
                // The source handle must not derive a new zoom from the old output size.
                output.right = output.left + self.bounds.width() * *zoom;
                output.bottom = output.top + self.bounds.height() * *zoom;
            }
        } else if handle >= 8 && self.kind == Kind::HighlightRectangle {
            let b = self.bounds;
            let x = if handle == 8 || handle == 10 {
                point.0 - b.left
            } else {
                b.right - point.0
            };
            let y = if handle < 10 {
                point.1 - b.top
            } else {
                b.bottom - point.1
            };
            self.style.rounding = ((x.min(y) - 10. * scale) * 2. / scale)
                .clamp(0., b.width().min(b.height()) / 2. / scale);
        } else {
            self.resize_to(self.bounds.resized(handle, point));
        }
    }
}

fn polygon_contains(point: (f32, f32), polygon: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let (xi, yi) = polygon[current];
        let (xj, yj) = polygon[previous];
        if (yi > point.1) != (yj > point.1) && point.0 < (xj - xi) * (point.1 - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn inside(bounds: Bounds, point: (f32, f32), margin: f32) -> bool {
    point.0 >= bounds.left - margin
        && point.0 <= bounds.right + margin
        && point.1 >= bounds.top - margin
        && point.1 <= bounds.bottom + margin
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy).max(f32::EPSILON))
        .clamp(0., 1.);
    distance(p, (a.0 + t * dx, a.1 + t * dy))
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    objects: Vec<Object>,
    selected: Option<usize>,
    opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Handle(usize),
    Object(usize),
}

/// Owns the undoable document; a drag is committed as one undo step on release.
#[derive(Default)]
pub struct Session {
    pub objects: Vec<Object>,
    pub selected: Option<usize>,
    pub spotlight_opacity: f32,
    undo: Vec<Snapshot>,
    drag_before: Option<Snapshot>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            spotlight_opacity: 0.1,
            ..Self::default()
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            objects: self.objects.clone(),
            selected: self.selected,
            opacity: self.spotlight_opacity,
        }
    }
    pub fn set_spotlight_opacity(&mut self, opacity: f32) {
        let opacity = opacity.clamp(0., 1.);
        if (self.spotlight_opacity - opacity).abs() > 0.0001 {
            if self.drag_before.is_none() {
                self.undo.push(self.snapshot());
            }
            self.spotlight_opacity = opacity;
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn add(&mut self, object: Object) -> bool {
        if !matches!(
            object.kind,
            Kind::Pencil | Kind::HighlightLine | Kind::Arrow | Kind::Polyline
        ) && (object.bounds.width() < 2. || object.bounds.height() < 2.)
        {
            return false;
        }
        self.undo.push(self.snapshot());
        self.objects.push(object);
        self.selected = Some(self.objects.len() - 1);
        true
    }
    pub fn begin_drag(&mut self) {
        self.drag_before = Some(self.snapshot());
    }
    pub fn finish_drag(&mut self) {
        if let Some(before) = self.drag_before.take()
            && before != self.snapshot()
        {
            self.undo.push(before);
        }
    }
    pub fn cancel_drag(&mut self) {
        if let Some(before) = self.drag_before.take() {
            self.objects = before.objects;
            self.selected = before.selected;
            self.spotlight_opacity = before.opacity;
        }
    }
    pub fn update_selected(&mut self, change: impl FnOnce(&mut Object)) {
        let Some(index) = self.selected else { return };
        let before = self.snapshot();
        change(&mut self.objects[index]);
        if before != self.snapshot() && self.drag_before.is_none() {
            self.undo.push(before);
        }
    }
    pub fn delete_selected(&mut self) {
        if let Some(index) = self.selected {
            self.undo.push(self.snapshot());
            self.selected = None;
            self.objects.remove(index);
        }
    }
    pub fn undo(&mut self) -> bool {
        let Some(before) = self.undo.pop() else {
            return false;
        };
        self.drag_before = None;
        self.objects = before.objects;
        self.selected = before.selected;
        self.spotlight_opacity = before.opacity;
        true
    }
    pub fn clear(&mut self) {
        *self = Self::new();
    }
    pub fn hit(&self, point: (f32, f32), scale: f32) -> Option<Hit> {
        if let Some(index) = self.selected {
            for (handle, (x, y)) in self.objects[index].edit_handles(scale).iter().enumerate() {
                let tolerance = if self.objects[index].kind == Kind::Arrow && handle == 2 {
                    9. * scale
                } else {
                    6. * scale
                };
                if (point.0 - x).abs() <= tolerance && (point.1 - y).abs() <= tolerance {
                    return Some(Hit::Handle(handle));
                }
            }
        }
        self.objects
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, object)| {
                if matches!(
                    object.kind,
                    Kind::Pencil | Kind::HighlightLine | Kind::Arrow | Kind::Polyline
                ) {
                    return object.hit_path(point, scale).then_some(Hit::Object(index));
                }
                if let Extra::Magnifier { output, .. } = object.extra {
                    return (inside(output, point, 6. * scale)
                        || inside(object.bounds, point, 6. * scale))
                    .then_some(Hit::Object(index));
                }
                let b = object.bounds;
                if point.0 < b.left - 6. * scale
                    || point.0 > b.right + 6. * scale
                    || point.1 < b.top - 6. * scale
                    || point.1 > b.bottom + 6. * scale
                {
                    return None;
                }
                if object.style.fill
                    || object.kind.is_spotlight()
                    || object.kind == Kind::HighlightRectangle
                {
                    return Some(Hit::Object(index));
                }
                let x = ((point.0 - b.left) / b.width().max(1.) * 2. - 1.).abs();
                let y = ((point.1 - b.top) / b.height().max(1.) * 2. - 1.).abs();
                let edge = if matches!(object.kind, Kind::Ellipse) {
                    (x * x + y * y).sqrt()
                } else {
                    x.max(y)
                };
                let tolerance = (object.style.width * scale / 2. + 6. * scale)
                    / (b.width().min(b.height()) / 2.).max(1.);
                (edge >= 1. - tolerance && edge <= 1. + tolerance).then_some(Hit::Object(index))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn brush(kind: Kind, points: Vec<(f32, f32)>) -> Object {
        let mut o = Object {
            kind,
            points,
            style: Style {
                width: 12.,
                ..Style::default()
            },
            bounds: Bounds::from_corners((0., 0.), (0., 0.)),
            extra: Extra::None,
        };
        o.refresh_bounds();
        o
    }
    #[test]
    fn pencil_sampling_hit_transform_and_undo() {
        let mut o = brush(Kind::Pencil, vec![(-100., -100.)]);
        o.append_point((-100., -100.), 0.5);
        o.append_point((-50., -100.), 0.5);
        o.append_point((0., -100.), 0.5);
        o.append_point((0., 0.), 0.5);
        assert_eq!(o.points.len(), 3);
        assert!(o.hit_path((-50., -100.), 1.));
        assert!(!o.hit_path((-50., -50.), 1.));
        let mut s = Session::new();
        assert!(s.add(o.clone()));
        s.begin_drag();
        s.objects[0].move_by(10., 20.);
        s.finish_drag();
        assert!(s.undo());
        assert_eq!(s.objects[0], o);
        o.resize_to(Bounds::from_corners((0., 0.), (200., 200.)));
        assert_eq!(o.points, vec![(0., 0.), (200., 0.), (200., 200.)]);
        assert_eq!(o.style.width, 12.);
        assert!(s.add(brush(Kind::Pencil, vec![(20., 20.)])));
    }
    #[test]
    fn highlight_endpoints_rounding_and_cancel() {
        let mut line = brush(Kind::HighlightLine, vec![(0., 0.), (100., 0.)]);
        assert_eq!(line.edit_handles(1.).len(), 2);
        line.edit_handle(0, (-50., 20.), 1.);
        assert_eq!(line.points[0], (-50., 20.));
        let mut s = Session::new();
        assert!(s.add(line.clone()));
        s.begin_drag();
        s.objects[0].edit_handle(1, (200., 100.), 1.);
        s.cancel_drag();
        assert_eq!(s.objects[0], line);
        let mut rect = Object {
            kind: Kind::HighlightRectangle,
            points: vec![],
            style: Style::default(),
            bounds: Bounds::from_corners((0., 0.), (100., 80.)),
            extra: Extra::None,
        };
        rect.edit_handle(8, (200., 200.), 2.);
        assert_eq!(rect.style.rounding, 20.);
        assert_eq!(rect.edit_handles(2.).len(), 12);
        rect.edit_handle(8, (-50., -50.), 2.);
        assert_eq!(rect.style.rounding, 0.);
    }
    #[test]
    fn drawing_selection_and_undo() {
        let mut s = Session::new();
        let obj = Object {
            bounds: Bounds::from_corners((120., 80.), (-30., -20.)),
            kind: Kind::Rectangle,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        };
        assert!(s.add(obj));
        assert_eq!(s.objects[0].bounds.left, -30.);
        assert_eq!(s.hit((-30., -20.), 1.), Some(Hit::Handle(0)));
        s.begin_drag();
        s.objects[0].bounds = s.objects[0].bounds.moved(20., 10.);
        s.finish_drag();
        assert!(s.undo());
        assert_eq!(s.objects[0].bounds.left, -30.);
        s.delete_selected();
        assert!(s.objects.is_empty());
        assert!(s.undo());
        assert_eq!(s.objects.len(), 1);
    }

    #[test]
    fn continuous_style_adjustment_is_one_undo_step() {
        let mut s = Session::new();
        s.add(Object {
            bounds: Bounds::from_corners((0., 0.), (100., 100.)),
            kind: Kind::SpotlightRectangle,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        });
        s.begin_drag();
        s.set_spotlight_opacity(0.6);
        s.set_spotlight_opacity(0.7);
        s.update_selected(|object| object.style.width = 8.);
        s.finish_drag();
        assert!(s.undo());
        assert_eq!(s.spotlight_opacity, 0.1);
        assert_eq!(s.objects[0].style.width, 4.);
        assert!(s.undo());
        assert!(s.objects.is_empty());
    }

    #[test]
    fn curved_arrow_uses_its_visible_midpoint_for_hit_and_edit() {
        let mut session = Session::new();
        let mut arrow = Object {
            bounds: Bounds::from_corners((0., 0.), (100., 0.)),
            kind: Kind::Arrow,
            style: Style::default(),
            points: vec![(0., 0.), (100., 0.), (50., 100.)],
            extra: Extra::Arrow {
                curved: true,
                head: true,
                start: Endpoint::None,
                end: Endpoint::FilledArrow,
            },
        };
        arrow.refresh_bounds();
        assert!(session.add(arrow));
        assert_eq!(session.objects[0].edit_handles(1.)[2], (50., 50.));
        assert_eq!(session.hit((50., 50.), 1.), Some(Hit::Handle(2)));
        assert_eq!(session.hit((58., 50.), 1.), Some(Hit::Handle(2)));
        assert_ne!(session.hit((50., 100.), 1.), Some(Hit::Handle(2)));
        session.begin_drag();
        session.objects[0].edit_handle(2, (50., 75.), 1.);
        assert_eq!(session.objects[0].points[2], (50., 150.));
        assert_eq!(session.objects[0].edit_handles(1.)[2], (50., 75.));
        session.finish_drag();
        assert!(session.undo());
        assert_eq!(session.objects[0].points[2], (50., 100.));
    }

    #[test]
    fn straight_arrow_midpoint_bends_and_snaps_back_to_straight() {
        let mut arrow = Object {
            bounds: Bounds::from_corners((-100., -50.), (0., -50.)),
            kind: Kind::Arrow,
            style: Style::default(),
            points: vec![(-100., -50.), (0., -50.)],
            extra: Extra::Arrow {
                curved: false,
                head: true,
                start: Endpoint::None,
                end: Endpoint::FilledArrow,
            },
        };
        assert_eq!(arrow.edit_handles(1.)[2], (-50., -50.));
        arrow.edit_handle(2, (-50., -10.), 1.);
        assert_eq!(arrow.points[2], (-50., 30.));
        assert_eq!(arrow.edit_handles(1.)[2], (-50., -10.));
        assert!(matches!(arrow.extra, Extra::Arrow { curved: true, .. }));
        arrow.edit_handle(2, (-50., -50.), 1.);
        assert_eq!(arrow.points.len(), 2);
        assert!(matches!(arrow.extra, Extra::Arrow { curved: false, .. }));
    }

    #[test]
    fn smooth_filled_polyline_hits_interior_and_keeps_editable_nodes() {
        let mut shape = Object {
            bounds: Bounds::from_corners((0., 0.), (0., 0.)),
            kind: Kind::Polyline,
            style: Style {
                fill: true,
                ..Style::default()
            },
            points: vec![(0., 0.), (100., 0.), (50., 100.)],
            extra: Extra::Polyline {
                curved: true,
                head: false,
                start: Endpoint::None,
                end: Endpoint::None,
            },
        };
        shape.refresh_bounds();
        assert!(shape.stroke_points().len() > shape.points.len());
        assert!(shape.hit_path((50., 40.), 1.));
        assert_eq!(shape.edit_handles(1.).len(), 3);
        shape.edit_handle(1, (120., 0.), 1.);
        assert_eq!(shape.points[1], (120., 0.));
    }

    #[test]
    fn magnifier_source_and_output_edit_independently_with_undo() {
        let source = Bounds::from_corners((-200., 50.), (-100., 100.));
        let output = Bounds::from_corners((0., 50.), (150., 125.));
        let mut session = Session::new();
        session.add(Object {
            bounds: source,
            kind: Kind::Magnifier,
            style: Style::default(),
            points: vec![],
            extra: Extra::Magnifier {
                output,
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false,
            },
        });
        assert_eq!(session.objects[0].edit_handles(1.).len(), 16);
        assert!(matches!(session.hit((40., 80.), 1.), Some(Hit::Object(0))));
        session.begin_drag();
        session.objects[0].move_from((40., 80.), 20., 0.);
        session.finish_drag();
        assert_eq!(session.objects[0].bounds, source);
        assert!(session.undo());
        assert_eq!(
            session.objects[0].extra,
            Extra::Magnifier {
                output,
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false
            }
        );
        session.begin_drag();
        session.objects[0].move_from((-150., 75.), 30., -10.);
        session.finish_drag();
        assert_eq!(session.objects[0].bounds, source.moved(30., -10.));
        assert!(
            matches!(session.objects[0].extra, Extra::Magnifier { output: moved, .. } if moved == output.moved(30., -10.))
        );
        assert!(session.undo());
        assert_eq!(session.objects[0].bounds, source);
    }

    #[test]
    fn magnifier_source_handles_preserve_zoom_and_output_aspect_ratio() {
        let source = Bounds::from_corners((0., 0.), (100., 60.));
        let output = Bounds::from_corners((200., 10.), (350., 100.));
        let object = Object {
            bounds: source,
            kind: Kind::Magnifier,
            style: Style::default(),
            points: vec![],
            extra: Extra::Magnifier {
                output,
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false,
            },
        };
        let targets = [
            (-30., -20.),
            (50., -20.),
            (130., -20.),
            (-30., 30.),
            (130., 30.),
            (-30., 90.),
            (50., 90.),
            (130., 90.),
        ];
        for (handle, target) in targets.into_iter().enumerate() {
            let mut edited = object.clone();
            for point in [target, source.handles()[handle], (99., 59.)] {
                edited.edit_handle(handle, point, 1.);
                let Extra::Magnifier {
                    output: resized,
                    zoom,
                    ..
                } = edited.extra
                else {
                    unreachable!();
                };
                assert_eq!(zoom, 1.5, "handle {handle}");
                assert_eq!(resized.left, output.left, "handle {handle}");
                assert_eq!(resized.top, output.top, "handle {handle}");
                assert_eq!(resized.width(), edited.bounds.width() * zoom);
                assert_eq!(resized.height(), edited.bounds.height() * zoom);
            }
        }
    }

    #[test]
    fn centered_magnifier_output_keeps_source_body_draggable() {
        let source = Bounds::from_corners((-180., -120.), (-30., -30.));
        assert_eq!(
            source,
            Bounds::from_corners((-30., -30.), (-180., -120.)),
            "reverse dragging keeps the same sampled area"
        );
        let output = magnifier_output_for_source(source, 1.5);
        assert_eq!(output, Bounds::from_corners((-217.5, -142.5), (7.5, -7.5)));
        assert_eq!(magnifier_output_for_source(source, 1.), source);
        assert_eq!(magnifier_output_for_source(source, 8.).width(), 1200.);
        let mut object = Object {
            bounds: source,
            kind: Kind::Magnifier,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Magnifier {
                output,
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false,
            },
        };
        object.move_from((-100., -75.), 20., 10.);
        assert_eq!(object.bounds, source.moved(20., 10.));
        assert!(
            matches!(object.extra, Extra::Magnifier { output: moved, .. } if moved == output.moved(20., 10.))
        );
    }

    #[test]
    fn toolbar_zoom_keeps_moved_output_center_and_is_undoable() {
        let source = Bounds::from_corners((-180., -120.), (-80., -60.));
        let output = Bounds::from_corners((200., 100.), (350., 190.));
        let object = Object {
            bounds: source,
            kind: Kind::Magnifier,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Magnifier {
                output,
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false,
            },
        };
        let mut session = Session::new();
        assert!(session.add(object.clone()));
        session.begin_drag();
        session.update_selected(|selected| selected.set_magnifier_zoom_centered(2.));
        session.update_selected(|selected| selected.set_magnifier_zoom_centered(8.));
        session.finish_drag();
        assert_eq!(session.objects[0].bounds, source);
        assert!(
            matches!(session.objects[0].extra, Extra::Magnifier { output: resized, zoom: 8., .. }
            if resized == Bounds::from_corners((-125., -95.), (675., 385.)))
        );
        assert!(session.undo());
        assert_eq!(session.objects[0], object);

        session.update_selected(|selected| selected.set_magnifier_zoom_centered(1.));
        assert!(
            matches!(session.objects[0].extra, Extra::Magnifier { output: resized, zoom: 1., .. }
            if resized == Bounds::from_corners((225., 115.), (325., 175.)))
        );
        let unchanged = session.objects[0].clone();
        session.update_selected(|selected| selected.set_magnifier_zoom_centered(1.));
        assert_eq!(session.objects[0], unchanged);
    }
}
