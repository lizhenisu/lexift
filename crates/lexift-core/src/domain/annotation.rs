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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Rectangle,
    Ellipse,
    SpotlightRectangle,
    SpotlightEllipse,
    Pencil,
    HighlightLine,
    HighlightRectangle,
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
}

impl Kind {
    pub fn tool(self) -> usize {
        match self {
            Self::Pencil => 3,
            Self::HighlightLine | Self::HighlightRectangle => 4,
            Self::SpotlightRectangle | Self::SpotlightEllipse => 2,
            _ => 0,
        }
    }
}

impl Object {
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
        self.points
            .first()
            .is_some_and(|p| distance(*p, point) <= tolerance)
            || self
                .points
                .windows(2)
                .any(|p| segment_distance(point, p[0], p[1]) <= tolerance)
    }
    pub fn edit_handles(&self, scale: f32) -> Vec<(f32, f32)> {
        if self.kind == Kind::HighlightLine {
            return self.points.clone();
        }
        let mut handles = self.bounds.handles().to_vec();
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
        if self.kind == Kind::HighlightLine {
            if let Some(p) = self.points.get_mut(handle) {
                *p = point;
            }
            self.refresh_bounds();
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
        if !matches!(object.kind, Kind::Pencil | Kind::HighlightLine)
            && (object.bounds.width() < 2. || object.bounds.height() < 2.)
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
                if (point.0 - x).abs() <= 6. * scale && (point.1 - y).abs() <= 6. * scale {
                    return Some(Hit::Handle(handle));
                }
            }
        }
        self.objects
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, object)| {
                if matches!(object.kind, Kind::Pencil | Kind::HighlightLine) {
                    return object.hit_path(point, scale).then_some(Hit::Object(index));
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
}
