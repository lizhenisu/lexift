use super::{
    geometry::Rect,
    screenshot_polygon::bounds,
    screenshot_windows::{intersection, subtract},
};
use crate::{Error, Result};
use std::sync::Arc;

const MAX_PARTS: usize = 4096;
const MAX_UNDO: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Add,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Mode(Mode),
    Apply(Rect),
    Undo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub mode: Mode,
    pub mask: Arc<Vec<Rect>>,
    undo: Vec<Arc<Vec<Rect>>>,
}

impl Selection {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn edit(&mut self, edit: Edit, displays: &[Rect]) -> Result<()> {
        match edit {
            Edit::Mode(mode) => self.mode = mode,
            Edit::Undo => {
                if let Some(mask) = self.undo.pop() {
                    self.mask = mask;
                }
            }
            Edit::Apply(rect) => self.apply(rect, displays)?,
        }
        Ok(())
    }

    fn apply(&mut self, rect: Rect, displays: &[Rect]) -> Result<()> {
        let mut next = Vec::new();
        match self.mode {
            Mode::Add => {
                next.extend_from_slice(&self.mask);
                for part in displays
                    .iter()
                    .filter_map(|&display| intersection(rect, display))
                {
                    let mut pending = vec![part];
                    for &covered in &next {
                        let mut rest = Vec::new();
                        for piece in pending {
                            subtract(piece, covered, &mut rest);
                        }
                        check_parts(next.len() + rest.len())?;
                        pending = rest;
                        if pending.is_empty() {
                            break;
                        }
                    }
                    next.extend(pending);
                    check_parts(next.len())?;
                }
            }
            Mode::Remove => {
                if rect.left >= rect.right || rect.top >= rect.bottom {
                    return Ok(());
                }
                for &part in self.mask.iter() {
                    subtract(part, rect, &mut next);
                    check_parts(next.len())?;
                }
            }
        }
        if next == *self.mask {
            return Ok(());
        }
        if let Some(b) = bounds(&next) {
            let w = i64::from(b.right) - i64::from(b.left);
            let h = i64::from(b.bottom) - i64::from(b.top);
            if w.checked_mul(h).is_none_or(|n| n > 40_000_000) {
                return Err(Error::new("Polygon selection is too large"));
            }
        }
        if self.undo.len() == MAX_UNDO {
            self.undo.remove(0);
        }
        self.undo
            .push(std::mem::replace(&mut self.mask, Arc::new(next)));
        Ok(())
    }
}

fn check_parts(parts: usize) -> Result<()> {
    if parts > MAX_PARTS {
        Err(Error::new("Selection is too complex"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::screen_capture::CapturedRegion;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn ordered_edits_keep_gaps_and_holes_transparent() {
        let displays = [rect(-3, 0, 0, 4), rect(1, 0, 5, 4)];
        let mut selection = Selection::default();
        for b in [rect(-3, 0, 3, 4), rect(2, 0, 5, 4), rect(2, 0, 5, 4)] {
            selection.edit(Edit::Apply(b), &displays).unwrap();
        }
        assert_eq!(selection.undo.len(), 2);
        selection.edit(Edit::Mode(Mode::Remove), &displays).unwrap();
        selection
            .edit(Edit::Apply(rect(-1, 1, 3, 3)), &displays)
            .unwrap();
        selection
            .edit(Edit::Apply(rect(0, 0, 0, 3)), &displays)
            .unwrap();
        assert_eq!(selection.undo.len(), 3);
        let frames = displays.map(|bounds| CapturedRegion {
            bounds,
            pixels: (0..4)
                .flat_map(|y| {
                    (bounds.left..bounds.right).flat_map(move |x| [(x + 10) as u8, y, 90, 255])
                })
                .collect(),
        });
        let image = super::super::screenshot_windows::compose(&frames, &selection.mask).unwrap();
        let expected: Vec<_> = (0..4)
            .flat_map(|y| {
                (-3..5).flat_map(move |x| {
                    if x == 0 || ((1..3).contains(&y) && (-1..3).contains(&x)) {
                        [0; 4]
                    } else {
                        [(x + 10) as u8, y, 90, 255]
                    }
                })
            })
            .collect();
        assert_eq!(image.bounds, rect(-3, 0, 5, 4));
        assert_eq!(image.pixels, expected);
        let cut = selection.mask.clone();
        selection.edit(Edit::Mode(Mode::Add), &displays).unwrap();
        selection
            .edit(Edit::Apply(rect(-1, 1, 3, 3)), &displays)
            .unwrap();
        assert_ne!(selection.mask, cut);
        selection.edit(Edit::Undo, &displays).unwrap();
        assert_eq!(selection.mask, cut);
        selection.edit(Edit::Mode(Mode::Remove), &displays).unwrap();
        selection
            .edit(Edit::Apply(rect(-3, 0, 5, 4)), &displays)
            .unwrap();
        assert!(selection.mask.is_empty());
        selection.edit(Edit::Undo, &displays).unwrap();
        assert_eq!(selection.mask, cut);
    }

    #[test]
    fn oversized_addition_does_not_change_mask_or_history() {
        let display = [rect(0, 0, 10000, 10000)];
        let mut selection = Selection::default();
        selection
            .edit(Edit::Apply(rect(0, 0, 1, 1)), &display)
            .unwrap();
        let prior = selection.clone();
        assert!(selection.edit(Edit::Apply(display[0]), &display).is_err());
        assert_eq!(selection, prior);
    }
}
