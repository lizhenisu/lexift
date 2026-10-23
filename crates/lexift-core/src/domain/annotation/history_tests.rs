use super::*;

fn object(kind: Kind) -> Object {
    Object {
        image_basis: Default::default(),
        id: ObjectId::DRAFT,
        kind,
        bounds: Bounds::from_corners((-100., 10.), (100., 100.)),
        points: Vec::new(),
        style: Style::default(),
        extra: Extra::None,
        erased: Vec::new(),
    }
}

#[test]
fn history_redo_round_trip_all_commits_and_identity() {
    let mut s = Session::new();
    assert!(!s.undo() && !s.redo());
    let mut states = vec![s.snapshot()];
    s.add(object(Kind::Rectangle));
    let identity = s.objects[0].id;
    states.push(s.snapshot());
    s.begin_drag();
    for delta in [1., 10., 20.] {
        s.update_selected(|o| o.bounds = o.bounds.moved(delta, -delta));
    }
    s.finish_drag();
    states.push(s.snapshot());
    s.update_selected(|o| o.style.width = 17.);
    states.push(s.snapshot());
    s.set_spotlight_opacity(0.7);
    states.push(s.snapshot());
    s.delete_selected();
    states.push(s.snapshot());
    for state in states[..states.len() - 1].iter().rev() {
        assert!(s.undo());
        assert!(s.snapshot().same_content(state));
    }
    assert!(!s.undo());
    for state in &states[1..] {
        assert!(s.redo());
        assert!(s.snapshot().same_content(state));
    }
    assert!(!s.redo());
    assert!(s.undo());
    assert_eq!(s.objects[0].id, identity);
    s.add(object(Kind::Rectangle));
    assert!(s.objects[1].id.value() > identity.value());
    assert!(!s.can_redo());
}

#[test]
fn history_redo_survives_cancel_noop_selection_and_tool_changes() {
    let mut s = Session::new();
    s.add(object(Kind::Rectangle));
    s.update_selected(|o| o.style.width = 20.);
    assert!(s.undo());
    let original = s.snapshot();
    s.begin_drag();
    s.update_selected(|o| o.bounds = o.bounds.moved(35., 40.));
    assert!(!s.redo()); // Never traverse a live transaction.
    s.cancel_drag();
    assert!(s.snapshot().same_content(&original));
    s.begin_drag();
    s.selected = None;
    s.finish_drag(); // Selection alone must not fork history.
    s.set_edit_scope(EditScope::None);
    s.set_spotlight_opacity(s.spotlight_opacity);
    assert!(!s.add(Object {
        bounds: Bounds::from_corners((0., 0.), (0., 0.)),
        ..object(Kind::Rectangle)
    }));
    assert!(s.redo());
    assert_eq!(s.objects[0].style.width, 20.);
    assert!(s.selected.is_none());
}

#[test]
fn history_redo_branch_only_disappears_on_gesture_commit() {
    let mut s = Session::new();
    s.add(object(Kind::Rectangle));
    s.add(object(Kind::Ellipse));
    s.undo();
    s.begin_drag();
    s.add(object(Kind::Rectangle));
    assert!(s.can_redo());
    s.finish_drag();
    assert!(!s.can_redo());
    assert!(s.undo());
    assert_eq!(s.objects.len(), 1);
    assert!(s.redo());
    assert_eq!(s.objects[1].kind, Kind::Rectangle);
}

#[test]
fn history_redo_empty_history_does_not_retire_blank_erase_handles() {
    use crate::domain::annotation_erase::RoundedRectangle;
    let mut s = Session::new();
    let geometry = RoundedRectangle {
        bounds: Bounds::from_corners((0., 0.), (100., 100.)),
        rounding: 12.,
        creation_scale: 1.,
    };
    s.begin_rectangle_erase(geometry);
    s.finish_drag();
    assert!(s.rectangle_erase_show_handles());
    assert!(!s.undo() && !s.redo());
    assert_eq!(s.rectangle_erase_geometry(), Some(geometry));
    assert!(s.rectangle_erase_show_handles());
}

#[test]
fn history_redo_sequence_watermark_erase_clear_and_teardown() {
    use crate::domain::annotation_erase::{EraseRegion, EraseTarget};
    let mut s = Session::new();
    s.add(object(Kind::Sequence));
    let id = s.objects[0].id;
    assert_eq!(s.next_sequence, 2);
    s.set_watermark(Some(Watermark {
        template: "mark".into(),
        content: "mark".into(),
        font_family: "Arial".into(),
        font_size: 20.,
        opacity: 60,
        color: [50, 60, 70],
        position: WatermarkPosition::Center,
    }));
    s.erase(
        EraseRegion::Rectangle(Bounds::from_corners((-50., 20.), (20., 50.))),
        &[EraseTarget::Object(id), EraseTarget::Watermark],
    );
    let erased = s.snapshot();
    s.clear_all();
    assert!(s.undo());
    assert!(s.snapshot().same_content(&erased));
    assert!(s.redo());
    assert!(!s.has_content());
    assert!(s.undo() && s.undo());
    assert!(s.objects[0].erased.is_empty());
    assert!(s.redo());
    assert!(s.snapshot().same_content(&erased));
    while s.undo() {}
    assert_eq!(s.next_sequence, 1);
    assert!(s.redo());
    assert_eq!(s.next_sequence, 2);
    s.clear();
    assert!(!s.can_undo() && !s.can_redo());
}

#[test]
fn history_redo_freeze_visible_preserves_or_forks_independently() {
    let mut s = Session::new();
    s.add(object(Kind::Rectangle));
    s.add(object(Kind::Ellipse));
    s.undo();
    let mut unchanged = s.freeze_visible(&s);
    assert!(unchanged.redo());
    assert_eq!(s.objects.len(), 1);
    let mut visible = s.clone();
    visible.objects.push(object(Kind::Text));
    let mut frozen = s.freeze_visible(&visible);
    assert!(!frozen.can_redo());
    assert!(frozen.undo());
    assert_eq!(frozen.objects.len(), 1);
    assert!(frozen.redo());
    assert_eq!(frozen.objects[1].kind, Kind::Text);
    assert_ne!(frozen.objects[1].id, ObjectId::DRAFT);
    assert!(s.can_redo());
}

#[test]
fn history_redo_failed_mosaic_is_removed_without_losing_other_edits() {
    let mut s = Session::new();
    let mut mask = object(Kind::MosaicBrush);
    mask.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 5, 20., 1.));
    s.add(mask);
    let id = s.objects[0].id;
    s.add(object(Kind::Rectangle));
    s.undo();
    s.undo();
    s.discard_mosaics(&[id.value()]);
    assert!(s.redo());
    assert_eq!(s.objects.len(), 1);
    assert_eq!(s.objects[0].kind, Kind::Rectangle);
    assert!(!s.redo());
}

#[test]
fn image_crop_shares_annotation_chronology_and_preserves_hidden_objects() {
    let full = Rect {
        left: 0,
        top: 0,
        right: 600,
        bottom: 2000,
    };
    let crop = Rect {
        left: 100,
        top: 200,
        right: 500,
        bottom: 1800,
    };
    let mut s = Session::new();
    s.add(object(Kind::Rectangle));
    let id = s.objects[0].id;
    assert!(s.set_image_crop(crop, full));
    s.update_selected(|o| o.style.width = 17.);
    assert!(s.undo());
    assert_eq!(s.image_crop, Some(crop));
    assert!(s.undo());
    assert_eq!(s.image_crop, None);
    assert_eq!(s.objects[0].id, id);
    assert!(!s.set_image_crop(full, full));
    assert!(s.can_redo());
    assert!(s.redo());
    assert_eq!(s.image_crop, Some(crop));
    assert!(!s.set_image_crop(crop, full));
    assert!(s.can_redo());
    assert!(s.redo());
    assert_eq!(s.objects[0].style.width, 17.);
    let frozen = s.freeze_visible(&s);
    assert_eq!(frozen.image_crop, Some(crop));
    assert!(s.set_image_crop(full, full));
    assert!(s.undo());
    assert_eq!(s.image_crop, Some(crop));
    assert!(!s.set_image_crop(Rect { right: 601, ..full }, full));
}
