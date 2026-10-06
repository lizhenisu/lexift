//! Gesture identity and guarded document updates, without window or registry ownership.
use lexift_core::domain::annotation::Kind;
use lexift_core::domain::annotation::{Object, ObjectId, Session};

#[derive(Clone, Copy)]
pub(super) enum Gesture {
    Erase,
    PlaceSequence,
    SequenceControl {
        target: ObjectId,
        handle: usize,
    },
    PlacePoint,
    FinishArrow,
    DeleteText,
    ToggleTextArrow {
        target: ObjectId,
    },
    CreateText {
        at: (f32, f32),
        scale: f32,
    },
    Draw {
        kind: Kind,
    },
    Move,
    Resize {
        handle: usize,
    },
    TextHandle {
        handle: usize,
        editing: bool,
        active: bool,
    },
    TextSelection {
        anchor: usize,
    },
}

/// Action-only captures cannot masquerade as document transaction owners.
pub(super) enum Action {
    SequenceControl {
        target: ObjectId,
        handle: usize,
    },
    PlacePoint,
    FinishArrow,
    DeleteText,
    ToggleTextArrow {
        target: ObjectId,
    },
    CreateText {
        at: (f32, f32),
        scale: f32,
        values: Box<crate::AnnotationValues>,
    },
    TextSelection {
        anchor: usize,
    },
}

impl Action {
    fn route(&self) -> Gesture {
        match *self {
            Self::SequenceControl { target, handle } => Gesture::SequenceControl { target, handle },
            Self::PlacePoint => Gesture::PlacePoint,
            Self::FinishArrow => Gesture::FinishArrow,
            Self::DeleteText => Gesture::DeleteText,
            Self::ToggleTextArrow { target } => Gesture::ToggleTextArrow { target },
            Self::CreateText { at, scale, .. } => Gesture::CreateText { at, scale },
            Self::TextSelection { anchor } => Gesture::TextSelection { anchor },
        }
    }
}

/// One pointer capture has exactly one owner. Routing is always derived from that owner.
pub(super) enum Active {
    Erasing(super::eraser::Press),
    Sequence(SequencePlacement),
    Geometry(GeometryDrag),
    Text(TextHandle),
    Drawing(super::drawing::Press),
    Action(Action),
}

/// Read the drawing owned by the capture or the multi-click operation while
/// borrowing only these fields, independently of scene compiler caches.
pub(super) fn display_draft<'a>(
    active: &'a Option<Active>,
    pending: &'a super::drawing::PendingState,
) -> Option<&'a Object> {
    super::drawing::display_draft(active.as_ref().and_then(Active::drawing), pending)
}

impl Active {
    pub fn route(&self) -> Gesture {
        match self {
            Self::Erasing(_) => Gesture::Erase,
            Self::Sequence(_) => Gesture::PlaceSequence,
            Self::Geometry(drag) => drag.gesture(),
            Self::Text(handle) => handle.gesture(),
            Self::Drawing(press) => Gesture::Draw {
                kind: press.info().kind,
            },
            Self::Action(action) => action.route(),
        }
    }

    pub fn is_manipulation(&self) -> bool {
        !matches!(self, Self::Action(_))
    }

    pub fn sequence(&self) -> Option<&SequencePlacement> {
        if let Self::Sequence(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn geometry(&self) -> Option<&GeometryDrag> {
        if let Self::Geometry(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn text(&self) -> Option<&TextHandle> {
        if let Self::Text(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn text_mut(&mut self) -> Option<&mut TextHandle> {
        if let Self::Text(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn drawing(&self) -> Option<&super::drawing::Press> {
        if let Self::Drawing(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn drawing_mut(&mut self) -> Option<&mut super::drawing::Press> {
        if let Self::Drawing(value) = self {
            Some(value)
        } else {
            None
        }
    }

    /// Retire the owner before returning editor geometry to restore or replaying parameters.
    pub fn cancel(self, session: &mut Session) -> Option<Object> {
        match self {
            Self::Erasing(_) => session.cancel_drag(),
            Self::Sequence(value) => value.cancel(session),
            Self::Geometry(value) => value.cancel(session),
            Self::Text(value) => return value.cancel(session),
            Self::Drawing(value) => value.cancel(),
            Self::Action(_) => {}
        }
        None
    }

    pub fn take_sequence(slot: &mut Option<Self>) -> Option<SequencePlacement> {
        match slot.take()? {
            Self::Sequence(value) => Some(value),
            other => {
                *slot = Some(other);
                None
            }
        }
    }
    pub fn take_geometry(slot: &mut Option<Self>) -> Option<GeometryDrag> {
        match slot.take()? {
            Self::Geometry(value) => Some(value),
            other => {
                *slot = Some(other);
                None
            }
        }
    }
    pub fn take_text(slot: &mut Option<Self>) -> Option<TextHandle> {
        match slot.take()? {
            Self::Text(value) => Some(value),
            other => {
                *slot = Some(other);
                None
            }
        }
    }
    pub fn take_drawing(slot: &mut Option<Self>) -> Option<super::drawing::Press> {
        match slot.take()? {
            Self::Drawing(value) => Some(value),
            other => {
                *slot = Some(other);
                None
            }
        }
    }
}

impl Gesture {
    /// Editor geometry also writes a captured object, even without a Core snapshot.
    pub fn blocks_parameter_edits(self) -> bool {
        self.owns_document_transaction() || matches!(self, Self::TextHandle { active: true, .. })
    }

    /// Drafts and editor-only geometry never own the Core snapshot used by parameter edits.
    pub fn owns_document_transaction(self) -> bool {
        matches!(
            self,
            Self::Erase
                | Self::PlaceSequence
                | Self::Move
                | Self::Resize { .. }
                | Self::TextHandle {
                    editing: false,
                    active: true,
                    ..
                }
        )
    }
}

/// Geometry and terminal operations for one newly inserted sequence badge.
/// The coordinator hands over an already-open Core transaction after settling parameters.
pub(super) struct SequencePlacement {
    start: (f32, f32),
    original: Object,
}

impl SequencePlacement {
    /// Insert inside the handed-over transaction; reject without leaving a snapshot alive.
    pub fn insert(session: &mut Session, start: (f32, f32), object: Object) -> Option<Self> {
        if !session.add(object) {
            session.cancel_drag();
            return None;
        }
        Some(Self {
            start,
            // Core assigns identity on insertion; every update retains that identity.
            original: session.objects.last().unwrap().clone(),
        })
    }

    pub fn update(&self, session: &mut Session, point: (f32, f32)) {
        let mut object = self.original.clone();
        object.move_by(point.0 - self.start.0, point.1 - self.start.1);
        write(session, self.original.id, object);
    }

    /// Consume the placement before replaying parameters or selecting the finished badge.
    pub fn finish(self, session: &mut Session, point: (f32, f32)) -> ObjectId {
        self.update(session, point);
        session.finish_drag();
        self.original.id
    }

    pub fn cancel(self, session: &mut Session) {
        session.cancel_drag();
    }
}

enum GeometryOperation {
    Move { start: (f32, f32) },
    Resize { handle: usize },
}

/// One object's press geometry, shared by input updates and effect previews.
/// Core history is handed over by the coordinator after parameter settlement.
pub(super) struct GeometryDrag {
    original: Object,
    operation: GeometryOperation,
}

impl GeometryDrag {
    pub fn moving(session: &Session, target: ObjectId, start: (f32, f32)) -> Option<Self> {
        Self::capture(session, target, GeometryOperation::Move { start })
    }

    pub fn resizing(session: &Session, target: ObjectId, handle: usize) -> Option<Self> {
        Self::capture(session, target, GeometryOperation::Resize { handle })
    }

    fn capture(session: &Session, target: ObjectId, operation: GeometryOperation) -> Option<Self> {
        let object = session.objects.get(session.object_index(target)?)?;
        session.can_edit(object).then(|| Self {
            original: object.clone(),
            operation,
        })
    }

    pub fn target(&self) -> ObjectId {
        self.original.id
    }

    pub fn original(&self) -> &Object {
        &self.original
    }

    pub fn move_start(&self) -> Option<(f32, f32)> {
        match self.operation {
            GeometryOperation::Move { start } => Some(start),
            GeometryOperation::Resize { .. } => None,
        }
    }

    pub fn is_resize(&self) -> bool {
        matches!(self.operation, GeometryOperation::Resize { .. })
    }

    /// Lightweight routing and cursor feedback, without a second geometry snapshot.
    pub fn gesture(&self) -> Gesture {
        match self.operation {
            GeometryOperation::Move { .. } => Gesture::Move,
            GeometryOperation::Resize { handle } => Gesture::Resize { handle },
        }
    }

    pub fn update(&self, session: &mut Session, point: (f32, f32), scale: f32) -> bool {
        let mut object = self.original.clone();
        match self.operation {
            GeometryOperation::Move { start } => {
                object.move_from(start, point.0 - start.0, point.1 - start.1);
            }
            GeometryOperation::Resize { handle } => {
                object.edit_handle(handle, point, scale);
                restore_complex_coverage(&self.original, &mut object);
            }
        }
        write(session, self.target(), object)
    }

    pub fn finish(self, session: &mut Session) {
        session.finish_drag();
    }

    pub fn cancel(self, session: &mut Session) {
        session.cancel_drag();
    }
}

#[derive(Clone, Copy)]
pub(super) struct TextHandleInfo {
    pub handle: usize,
    pub target: Option<ObjectId>,
    pub editing: bool,
    pub active: bool,
    pub scale: f32,
}

/// Text control press geometry; activation adopts parameters accepted below the threshold.
pub(super) struct TextHandle {
    start: (f32, f32),
    grab: (f32, f32),
    original: Object,
    info: TextHandleInfo,
}

impl TextHandle {
    pub fn new(
        original: Object,
        handle: usize,
        point: (f32, f32),
        scale: f32,
        editing: bool,
        target: Option<ObjectId>,
    ) -> Self {
        let center = original.edit_handles(scale)[handle];
        Self {
            start: point,
            grab: (point.0 - center.0, point.1 - center.1),
            original,
            info: TextHandleInfo {
                handle,
                target,
                editing,
                active: false,
                scale,
            },
        }
    }

    pub fn info(&self) -> TextHandleInfo {
        self.info
    }

    pub fn gesture(&self) -> Gesture {
        Gesture::TextHandle {
            handle: self.info.handle,
            editing: self.info.editing,
            active: self.info.active,
        }
    }

    /// Return a grab-corrected target only once geometry manipulation is requested.
    pub fn position(&self, point: (f32, f32)) -> Option<(f32, f32)> {
        if matches!(self.info.handle, 1 | 2) {
            return None;
        }
        text_drag_target(
            self.start,
            self.grab,
            point,
            self.info.scale,
            self.info.active,
        )
    }

    /// The coordinator settles parameters and hands over history before activation.
    pub fn activate(&mut self, current: Object) {
        self.original = current;
        self.info.active = true;
    }

    pub fn object_at(&self, position: (f32, f32)) -> Object {
        let mut object = self.original.clone();
        object.edit_handle(self.info.handle, position, self.info.scale);
        restore_complex_coverage(&self.original, &mut object);
        object
    }

    pub fn finish(self, session: &mut Session) {
        if self.info.active && !self.info.editing {
            session.finish_drag();
        }
    }

    /// An inactive press has changed no geometry; leave accepted editor parameters intact.
    pub fn cancel(self, session: &mut Session) -> Option<Object> {
        if self.info.active && !self.info.editing {
            session.cancel_drag();
        }
        (self.info.active && self.info.editing).then_some(self.original)
    }
}

pub(super) fn text_drag_target(
    start: (f32, f32),
    grab: (f32, f32),
    point: (f32, f32),
    scale: f32,
    active: bool,
) -> Option<(f32, f32)> {
    (active || (point.0 - start.0).hypot(point.1 - start.1) > 3. * scale)
        .then_some((point.0 - grab.0, point.1 - grab.1))
}

/// Decide complex-edit restoration from the press snapshot, never the previous frame.
fn restore_complex_coverage(original: &Object, object: &mut Object) {
    if object != original && object.erased == original.erased {
        object.erased.clear();
    }
}

/// Update inside the caller's active drag transaction; Core preserves object identity.
pub(super) fn write(session: &mut Session, target: ObjectId, object: Object) -> bool {
    let Some(index) = session.object_index(target) else {
        return false;
    };
    let current = &session.objects[index];
    if !session.can_edit(current) || object.id != target || object.kind != current.kind {
        return false;
    }
    let selected = session.selected;
    session.selected = Some(index);
    session.replace_selected_geometry(object);
    session.selected = selected;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{Bounds, Extra, Style};

    fn rectangle() -> Object {
        Object {
            erased: Vec::new(),
            id: ObjectId::DRAFT,
            bounds: Bounds::from_corners((10., 20.), (90., 80.)),
            kind: Kind::Rectangle,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        }
    }

    #[test]
    fn erased_magnifier_output_drag_keeps_hole_on_every_update() {
        use lexift_core::domain::annotation::MagnifierConnector;
        use lexift_core::domain::annotation_erase::{ErasePart, EraseRegion, EraseTarget};
        let mut lens = rectangle();
        lens.kind = Kind::Magnifier;
        lens.bounds = Bounds::from_corners((-100., 20.), (-60., 60.));
        lens.extra = Extra::Magnifier {
            output: Bounds::from_corners((10., 20.), (90., 100.)),
            zoom: 2.,
            ellipse: false,
            connector: MagnifierConnector::Plain,
            erase_annotations: true,
            antialias: true,
            shadow: false,
        };
        let mut session = Session::new();
        session.add(lens);
        let id = session.objects[0].id;
        session.erase(
            EraseRegion::Rectangle(Bounds::from_corners((10., 20.), (30., 40.))),
            &[EraseTarget::Object(id)],
        );
        session.begin_drag();
        let drag = GeometryDrag::moving(&session, id, (70., 80.)).unwrap();
        for dx in [10., 20., 30., 20., 10., 0.] {
            assert!(drag.update(&mut session, (70. + dx, 80.), 1.));
            assert!(
                session.objects[0]
                    .erased
                    .iter()
                    .any(|cut| cut.part == ErasePart::MagnifierOutput
                        && cut.contains((20. + dx, 30.))),
                "hole missing at delta {dx}"
            );
            assert!(
                session.objects[0]
                    .erased
                    .iter()
                    .any(|cut| cut.part == ErasePart::MagnifierSource && cut.contains((20., 30.)))
            );
            assert_eq!(session.objects[0].bounds, drag.original().bounds);
            if dx != 0. {
                assert!(
                    !session.objects[0]
                        .erased
                        .iter()
                        .any(|cut| cut.part == ErasePart::MagnifierConnector)
                );
            }
        }
        drag.finish(&mut session);
        // Returning to the press geometry adds no history, including connector cuts.
        assert!(session.undo());
        assert!(session.objects[0].erased.is_empty());
    }

    #[test]
    fn complex_magnifier_resize_restores_coverage_consistently_and_cancels() {
        use lexift_core::domain::annotation::MagnifierConnector;
        use lexift_core::domain::annotation_erase::{EraseRegion, EraseTarget};
        let mut lens = rectangle();
        lens.kind = Kind::Magnifier;
        lens.extra = Extra::Magnifier {
            output: Bounds::from_corners((110., 20.), (270., 140.)),
            zoom: 2.,
            ellipse: false,
            connector: MagnifierConnector::Plain,
            erase_annotations: true,
            antialias: true,
            shadow: true,
        };
        let mut session = Session::new();
        session.add(lens);
        let id = session.objects[0].id;
        session.erase(
            EraseRegion::Rectangle(Bounds::from_corners((110., 20.), (130., 40.))),
            &[EraseTarget::Object(id)],
        );
        let before = session.objects[0].clone();
        session.begin_drag();
        let drag = GeometryDrag::resizing(&session, id, 15).unwrap();
        for x in [280., 290., 300., 280.] {
            assert!(drag.update(&mut session, (x, 160.), 1.25));
            assert!(session.objects[0].erased.is_empty());
        }
        drag.cancel(&mut session);
        assert_eq!(session.objects[0], before);
        session.begin_drag();
        let drag = GeometryDrag::resizing(&session, id, 15).unwrap();
        drag.update(&mut session, (300., 160.), 1.25);
        drag.finish(&mut session);
        assert!(session.undo());
        assert_eq!(session.objects[0], before);
    }

    #[test]
    fn stale_geometry_operations_cannot_write_after_edit_scope_changes() {
        let mut session = Session::new();
        session.add(rectangle());
        let original = session.objects[0].clone();
        let drag = GeometryDrag::moving(&session, original.id, (0., 0.)).unwrap();
        session.set_edit_scope(lexift_core::domain::annotation::EditScope::Magnifier);
        assert!(GeometryDrag::moving(&session, original.id, (0., 0.)).is_none());
        assert!(GeometryDrag::resizing(&session, original.id, 7).is_none());
        let mut changed = original.clone();
        changed.move_by(50., 50.);
        assert!(!write(&mut session, original.id, changed));
        assert!(!drag.update(&mut session, (70., 70.), 1.));
        assert_eq!(session.objects[0], original);
    }

    #[test]
    fn mismatched_completion_retains_the_owner_and_action_cancellation_leaves_parameter_history() {
        let mut session = Session::new();
        assert!(session.add(rectangle()));
        let original = session.objects[0].clone();
        session.begin_drag();
        session.update_selected(|object| object.style.width = 25.);
        let mut slot = Some(Active::Drawing(super::super::drawing::Press::begin(
            (-100., -50.),
            Kind::Rectangle,
            Style::default(),
            1.25,
            &super::super::parameters::defaults(0),
        )));
        assert!(Active::take_sequence(&mut slot).is_none());
        assert!(Active::take_geometry(&mut slot).is_none());
        assert!(Active::take_text(&mut slot).is_none());
        assert!(matches!(
            slot.as_ref().map(Active::route),
            Some(Gesture::Draw {
                kind: Kind::Rectangle
            })
        ));
        assert!(slot.take().unwrap().cancel(&mut session).is_none());
        let action = Active::Action(Action::PlacePoint);
        assert!(action.cancel(&mut session).is_none());
        assert_eq!(session.objects[0].style.width, 25.);
        session.update_selected(|object| object.style.width = 30.);
        session.finish_drag();
        assert!(session.undo());
        assert_eq!(session.objects, vec![original.clone()]);
        // Cancellation of action-only captures must not split the continuous parameter operation.
        assert!(session.undo());
        assert!(session.objects.is_empty() && !session.can_undo());

        assert!(session.add(rectangle()));
        let original = session.objects[0].clone();
        session.begin_drag();
        let drag = GeometryDrag::moving(&session, original.id, (0., 0.)).unwrap();
        let mut slot = Some(Active::Geometry(drag));
        assert!(Active::take_drawing(&mut slot).is_none());
        assert!(
            slot.as_ref()
                .unwrap()
                .geometry()
                .unwrap()
                .update(&mut session, (40., 20.), 1.)
        );
        assert_ne!(session.objects[0].bounds, original.bounds);
        slot.take().unwrap().cancel(&mut session);
        assert_eq!(session.objects, vec![original]);
        assert!(Active::take_geometry(&mut slot).is_none());
        assert!(session.undo());
        assert!(session.objects.is_empty() && !session.can_undo());
    }

    #[test]
    fn captured_preview_follows_only_its_live_owner() {
        use super::super::{drawing, parameters::defaults};
        for scale in [1., 1.25] {
            for kind in [
                Kind::Rectangle,
                Kind::Pencil,
                Kind::MosaicBrush,
                Kind::Magnifier,
            ] {
                let mut session = Session::new();
                assert!(session.add(rectangle()));
                let baseline = session.objects.clone();
                let values = defaults(kind.tool() as i32);
                let mut press =
                    drawing::Press::begin((-150., -80.), kind, Style::default(), scale, &values);
                press.update((-100., -40.), scale);
                let mut active = Some(Active::Drawing(press));
                let pending = drawing::PendingState::default();
                assert!(display_draft(&active, &pending).is_some());
                assert!(Active::take_geometry(&mut active).is_none());
                assert!(display_draft(&active, &pending).is_some());
                let press = Active::take_drawing(&mut active).unwrap();
                assert!(display_draft(&active, &pending).is_none());
                // The retired capture remains inspectable until consumed, but
                // the registry view can no longer expose its drawing.
                assert!(press.preview().is_some());
                press.cancel();
                assert_eq!(session.objects, baseline);
                assert!(session.undo());
                assert!(session.objects.is_empty() && !session.can_undo());
            }
        }
    }

    #[test]
    fn placement_terminal_operations_preserve_numbering_selection_and_one_undo() {
        for cancel in [false, true] {
            let mut session = Session::new();
            assert!(session.add(rectangle()));
            let previous = session.objects[0].clone();
            let mut badge = rectangle();
            badge.kind = Kind::Sequence;
            let bounds = badge.bounds;
            session.begin_drag();
            let placement = SequencePlacement::insert(&mut session, (-200., -100.), badge).unwrap();
            session.selected = None;
            placement.update(&mut session, (-100., 40.));
            placement.update(&mut session, (-200., -100.));
            assert_eq!(
                session.objects[1].bounds, bounds,
                "movement uses the press snapshot"
            );
            if cancel {
                placement.cancel(&mut session);
                assert_eq!(session.objects, vec![previous.clone()]);
                assert_eq!(session.selected, Some(0));
                assert_eq!(session.next_sequence, 1);
            } else {
                let id = placement.finish(&mut session, (-150., -50.));
                assert_eq!(session.objects[1].id, id);
                assert_eq!(session.next_sequence, 2);
                assert!(session.undo());
                assert_eq!(session.objects, vec![previous.clone()]);
                assert_eq!(session.selected, Some(0));
                assert_eq!(session.next_sequence, 1);
            }
            assert!(session.undo());
            assert!(
                session.objects.is_empty(),
                "placement adds no extra history"
            );
            assert!(!session.can_undo());
        }
        let mut session = Session::new();
        session.next_sequence = i32::MAX;
        let mut badge = rectangle();
        badge.kind = Kind::Sequence;
        session.begin_drag();
        assert!(SequencePlacement::insert(&mut session, (0., 0.), badge).is_none());
        assert!(session.objects.is_empty() && !session.can_undo());
        // A rejected insertion releases the snapshot so the next normal operation can settle.
        session.add(rectangle());
        assert!(session.undo());
        assert!(session.objects.is_empty());
    }

    #[test]
    fn editing_geometry_blocks_parameters_without_owning_core_history() {
        for editing in [false, true] {
            for active in [false, true] {
                let gesture = Gesture::TextHandle {
                    handle: 3,
                    editing,
                    active,
                };
                assert_eq!(gesture.blocks_parameter_edits(), active);
                assert_eq!(gesture.owns_document_transaction(), active && !editing);
            }
        }
    }

    #[test]
    fn resize_uses_press_geometry_and_cancel_restores_the_document_without_history() {
        for scale in [1., 1.25] {
            let mut session = Session::new();
            assert!(session.add(rectangle()));
            let original = session.objects[0].clone();
            session.begin_drag();
            let drag = GeometryDrag::resizing(&session, original.id, 7).unwrap();
            assert!(drag.update(&mut session, (150., 140.), scale));
            assert_ne!(session.objects[0].bounds, original.bounds);
            assert_eq!(
                drag.original(),
                &original,
                "preview retains the press snapshot"
            );
            assert!(drag.update(&mut session, (90., 80.), scale));
            assert_eq!(session.objects[0], original);
            assert!(drag.update(&mut session, (180., 160.), scale));
            drag.cancel(&mut session);
            assert_eq!(session.objects, vec![original]);
            assert_eq!(session.selected, Some(0));
            assert!(session.undo());
            assert!(session.objects.is_empty(), "cancellation adds no history");
            assert!(!session.can_undo());
        }
    }

    #[test]
    fn gesture_updates_follow_identity_after_deletion_and_reject_stale_snapshots() {
        let mut session = Session::new();
        for _ in 0..3 {
            assert!(session.add(rectangle()));
        }
        let target = session.objects[1].id;
        let mut snapshot = session.objects[1].clone();
        let drag = GeometryDrag::moving(&session, target, (0., 0.)).unwrap();
        let neighbor = session.objects[2].clone();
        session.selected = Some(0);
        session.delete_selected();
        session.selected = Some(1);
        session.begin_drag();
        snapshot.move_by(25., -15.);
        assert!(drag.update(&mut session, (25., -15.), 1.));
        assert_eq!(session.objects[0], snapshot);
        assert_eq!(session.objects[1], neighbor);
        assert_eq!(
            session.selected,
            Some(1),
            "updates preserve the user's selection"
        );
        let before = session.objects.clone();
        assert!(!write(&mut session, neighbor.id, snapshot.clone()));
        let mut changed_kind = snapshot.clone();
        changed_kind.kind = Kind::Ellipse;
        assert!(!write(&mut session, target, changed_kind));
        assert_eq!(session.objects, before);
        session.finish_drag();
        session.selected = Some(0);
        session.delete_selected();
        assert!(!write(&mut session, target, snapshot));
        assert_eq!(session.objects, vec![neighbor]);
    }

    #[test]
    fn gesture_updates_merge_with_core_transaction_and_do_not_edit_pencil() {
        let mut session = Session::new();
        assert!(session.add(rectangle()));
        let original = session.objects[0].clone();
        let target = original.id;
        session.begin_drag();
        let drag = GeometryDrag::moving(&session, target, (0., 0.)).unwrap();
        session.selected = None;
        for distance in [10., 20., 30.] {
            assert!(drag.update(&mut session, (distance, -distance), 1.));
            assert!(session.selected.is_none());
        }
        drag.finish(&mut session);
        assert!(session.undo());
        assert_eq!(session.objects, vec![original]);
        assert!(session.undo());
        assert!(
            session.objects.is_empty(),
            "a completed gesture creates one undo record"
        );
        let mut pencil = rectangle();
        pencil.kind = Kind::Pencil;
        pencil.points = vec![(10., 20.), (80., 80.)];
        assert!(session.add(pencil));
        let original = session.objects[0].clone();
        assert!(GeometryDrag::moving(&session, original.id, (0., 0.)).is_none());
        assert!(GeometryDrag::resizing(&session, original.id, 7).is_none());
        let mut changed = original.clone();
        changed.move_by(20., 20.);
        assert!(!write(&mut session, original.id, changed));
        assert_eq!(session.objects, vec![original]);
    }
}
