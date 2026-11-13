use super::{geometry::Rect, screenshot::LastRegion};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: u64,
    pub name: String,
    pub region: LastRegion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Create { name: String, region: LastRegion },
    Rename { id: u64, name: String },
    Update { id: u64, region: LastRegion },
    Delete { id: u64 },
}

pub fn matches_layout(a: &[Rect], b: &[Rect]) -> bool {
    !a.is_empty() && a.iter().all(|r| b.contains(r)) && b.iter().all(|r| a.contains(r))
}

/// Check that every pixel in the selection is covered, including across monitor seams.
pub fn covers(bounds: Rect, layout: &[Rect]) -> bool {
    if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
        return false;
    }
    let mut edges = vec![bounds.left, bounds.right];
    for r in layout {
        edges.extend([
            r.left.clamp(bounds.left, bounds.right),
            r.right.clamp(bounds.left, bounds.right),
        ]);
    }
    edges.sort_unstable();
    edges.dedup();
    edges.windows(2).all(|edge| {
        let mut ranges = layout
            .iter()
            .filter(|r| r.left <= edge[0] && r.right >= edge[1])
            .map(|r| (r.top, r.bottom))
            .collect::<Vec<_>>();
        ranges.sort_unstable();
        let mut covered = bounds.top;
        for (top, bottom) in ranges {
            if top > covered {
                break;
            }
            covered = covered.max(bottom);
        }
        covered >= bounds.bottom
    })
}

pub fn apply(presets: &mut Vec<Preset>, edit: &Edit) -> Result<(), &'static str> {
    if let Edit::Create { region, .. } | Edit::Update { region, .. } = edit
        && !covers(region.bounds, &region.layout)
    {
        return Err("Display layout changed. Reselect the screenshot region.");
    }
    let (id, name) = match edit {
        Edit::Create { name, .. } => (None, Some(name.trim())),
        Edit::Rename { id, name } => (Some(*id), Some(name.trim())),
        Edit::Update { id, .. } | Edit::Delete { id } => (Some(*id), None),
    };
    let index = id
        .map(|id| {
            presets
                .iter()
                .position(|p| p.id == id)
                .ok_or("Screenshot preset no longer exists")
        })
        .transpose()?;
    if let Some(name) = name {
        if name.is_empty() || name.chars().count() > 64 {
            return Err("Enter a name with 1–64 characters");
        }
        if presets
            .iter()
            .any(|p| Some(p.id) != id && p.name.to_lowercase() == name.to_lowercase())
        {
            return Err("A screenshot preset with this name already exists");
        }
    }
    match edit {
        Edit::Create { region, .. } => {
            let id = presets
                .iter()
                .map(|p| p.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Screenshot preset identity exhausted")?;
            presets.push(Preset {
                id,
                name: name.unwrap().into(),
                region: region.clone(),
            });
        }
        Edit::Rename { .. } => presets[index.unwrap()].name = name.unwrap().into(),
        Edit::Update { region, .. } => presets[index.unwrap()].region = region.clone(),
        Edit::Delete { .. } => {
            presets.remove(index.unwrap());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_keep_identity_and_layout_is_a_set() {
        let left = Rect {
            left: -100,
            top: 0,
            right: 0,
            bottom: 100,
        };
        let right = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        assert!(matches_layout(&[left, right], &[right, left, left]));
        assert!(!matches_layout(&[left, right], &[right]));
        let region = LastRegion {
            bounds: left,
            layout: vec![left, right],
        };
        let mut presets = vec![];
        let create = Edit::Create {
            name: " Report ".into(),
            region: region.clone(),
        };
        apply(&mut presets, &create).unwrap();
        assert!(apply(&mut presets, &create).is_err());
        let id = presets[0].id;
        apply(
            &mut presets,
            &Edit::Update {
                id,
                region: LastRegion {
                    bounds: right,
                    ..region
                },
            },
        )
        .unwrap();
        assert_eq!(
            (
                presets[0].id,
                presets[0].name.as_str(),
                presets[0].region.bounds
            ),
            (id, "Report", right)
        );
    }
}
