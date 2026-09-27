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
        if object.bounds.width() < 2. || object.bounds.height() < 2. {
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
            for (handle, (x, y)) in self.objects[index].bounds.handles().iter().enumerate() {
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
                let b = object.bounds;
                if point.0 < b.left - 6. * scale
                    || point.0 > b.right + 6. * scale
                    || point.1 < b.top - 6. * scale
                    || point.1 > b.bottom + 6. * scale
                {
                    return None;
                }
                if object.style.fill || object.kind.is_spotlight() {
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
    #[test]
    fn drawing_selection_and_undo() {
        let mut s = Session::new();
        let obj = Object {
            bounds: Bounds::from_corners((120., 80.), (-30., -20.)),
            kind: Kind::Rectangle,
            style: Style::default(),
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
