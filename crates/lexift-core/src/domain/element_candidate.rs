//! Bounded accessibility geometry and pure selection hierarchy; no native objects or content.
use super::{
    geometry::{Point, Rect},
    window_candidate::WindowCandidate,
};
use crate::ports::magnifier::WindowToken;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementCandidate {
    pub window: WindowToken,
    /// Opaque identity, valid only within its capture. It never contains UI text.
    pub id: Vec<i32>,
    pub parent: Option<Vec<i32>>,
    pub bounds: Rect,
    pub order: u32,
}
fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}
fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}
struct Node {
    candidate: ElementCandidate,
    chain: Vec<Rect>,
    depth: usize,
}
#[derive(Default)]
pub struct Graph {
    nodes: Vec<Node>,
}
impl Graph {
    /// Geometry of the deepest normalized hit, independent of the chosen wheel level.
    pub fn leaf_bounds(
        &self,
        windows: &[WindowCandidate],
        desktop: Rect,
        point: Point,
    ) -> Option<Rect> {
        self.at(windows, desktop, point)?.2.first().copied()
    }
    /// Normalize once, clipping each node against its complete valid ancestor chain.
    pub fn new(elements: Vec<ElementCandidate>, windows: &[WindowCandidate]) -> Self {
        let map: HashMap<_, _> = elements
            .iter()
            .enumerate()
            .map(|(i, e)| ((e.window, &e.id), i))
            .collect();
        let mut nodes = vec![];
        for e in &elements {
            let Some(w) = windows.iter().find(|w| w.window == e.window) else {
                continue;
            };
            let mut lineage = vec![e];
            let mut current = e;
            let mut seen = HashSet::new();
            seen.insert(&e.id);
            let mut valid = true;
            while let Some(parent) = current.parent.as_ref() {
                if lineage.len() >= 17 || !seen.insert(parent) {
                    valid = false;
                    break;
                }
                let Some(&index) = map.get(&(e.window, parent)) else {
                    valid = false;
                    break;
                };
                current = &elements[index];
                lineage.push(current);
            }
            if !valid {
                continue;
            }
            let mut clipped = w.bounds;
            let mut chain = vec![clipped];
            for item in lineage.iter().rev() {
                let Some(b) = intersection(clipped, item.bounds) else {
                    valid = false;
                    break;
                };
                clipped = b;
                if chain.last() != Some(&b) {
                    chain.push(b);
                }
            }
            if valid {
                chain.reverse();
                nodes.push(Node {
                    candidate: e.clone(),
                    chain,
                    depth: lineage.len(),
                });
            }
        }
        Self { nodes }
    }
    fn at(
        &self,
        windows: &[WindowCandidate],
        desktop: Rect,
        p: Point,
    ) -> Option<(WindowToken, Vec<i32>, Vec<Rect>)> {
        let window = windows
            .iter()
            .filter(|w| intersection(w.bounds, desktop).is_some_and(|r| contains(r, p)))
            .min_by_key(|w| w.order)?;
        let node = self
            .nodes
            .iter()
            .filter(|n| {
                n.candidate.window == window.window
                    && n.chain.first().is_some_and(|r| contains(*r, p))
            })
            .max_by(|a, b| {
                a.depth
                    .cmp(&b.depth)
                    .then_with(|| {
                        let area = |n: &Node| {
                            let r = n.chain[0];
                            (i128::from(r.right) - i128::from(r.left))
                                * (i128::from(r.bottom) - i128::from(r.top))
                        };
                        area(b).cmp(&area(a))
                    })
                    .then_with(|| b.candidate.order.cmp(&a.candidate.order))
            });
        let (id, chain) = node.map_or_else(
            || (vec![], vec![window.bounds]),
            |n| (n.candidate.id.clone(), n.chain.clone()),
        );
        let mut result = vec![];
        for r in chain {
            if let Some(b) = intersection(r, desktop)
                && result.last() != Some(&b)
            {
                result.push(b);
            }
        }
        Some((window.window, id, result))
    }
}
/// UI-owned hover state; pure rules preserve a chosen level inside the same leaf.
#[derive(Default)]
pub struct Navigation {
    identity: Option<(WindowToken, Vec<i32>)>,
    chain: Vec<Rect>,
    index: usize,
    remainder: f32,
}
impl Navigation {
    pub fn point(
        &mut self,
        graph: &Graph,
        windows: &[WindowCandidate],
        desktop: Rect,
        p: Point,
    ) -> Option<Rect> {
        let Some((window, id, chain)) = graph.at(windows, desktop, p) else {
            *self = Self::default();
            return None;
        };
        if self.identity.as_ref() != Some(&(window, id.clone())) || self.chain != chain {
            self.index = 0;
            self.remainder = 0.;
        }
        self.identity = Some((window, id));
        self.chain = chain;
        self.index = self.index.min(self.chain.len().saturating_sub(1));
        self.current()
    }
    pub fn current(&self) -> Option<Rect> {
        self.chain.get(self.index).copied()
    }
    /// Input is normalized wheel detents, positive towards larger parent regions.
    pub fn scroll(&mut self, delta: f32) -> Option<Rect> {
        if !delta.is_finite() || self.chain.is_empty() {
            return self.current();
        }
        if self.remainder.signum() != delta.signum() {
            self.remainder = 0.;
        }
        self.remainder = (self.remainder + delta).clamp(-64., 64.);
        let steps = self.remainder.trunc() as i32;
        self.remainder -= steps as f32;
        self.index = (self.index as i32 + steps).clamp(0, self.chain.len() as i32 - 1) as usize;
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }
    fn window() -> WindowCandidate {
        WindowCandidate {
            process_id: 0,
            window: WindowToken(1),
            bounds: rect(-100, -50, 300, 250),
            order: 0,
        }
    }
    fn item(id: i32, parent: Option<i32>, bounds: Rect) -> ElementCandidate {
        ElementCandidate {
            window: WindowToken(1),
            id: vec![id],
            parent: parent.map(|p| vec![p]),
            bounds,
            order: id as u32,
        }
    }
    fn elements() -> Vec<ElementCandidate> {
        vec![
            item(1, None, window().bounds),
            item(2, Some(1), rect(-80, -20, 200, 200)),
            item(3, Some(2), rect(-60, 0, 60, 60)),
            item(4, Some(3), rect(-60, 0, 60, 60)),
            item(5, Some(2), rect(100, 0, 160, 60)),
        ]
    }
    #[test]
    fn hierarchy_clips_deduplicates_and_preserves_scroll_only_inside_same_leaf() {
        let w = [window()];
        let desktop = rect(-200, -100, 400, 300);
        let g = Graph::new(elements(), &w);
        let mut n = Navigation::default();
        assert_eq!(
            n.point(&g, &w, desktop, Point { x: 0, y: 10 }),
            Some(rect(-60, 0, 60, 60))
        );
        assert_eq!(n.scroll(0.5), Some(rect(-60, 0, 60, 60)));
        assert_eq!(n.scroll(0.5), Some(rect(-80, -20, 200, 200)));
        assert_eq!(
            n.point(&g, &w, desktop, Point { x: 1, y: 10 }),
            Some(rect(-80, -20, 200, 200))
        );
        assert_eq!(n.scroll(8.), Some(window().bounds));
        assert_eq!(n.scroll(1.), Some(window().bounds));
        assert_eq!(n.scroll(-1.), Some(rect(-80, -20, 200, 200)));
        assert_eq!(
            n.point(&g, &w, desktop, Point { x: 110, y: 10 }),
            Some(rect(100, 0, 160, 60))
        );
        assert_eq!(n.scroll(-64.), Some(rect(100, 0, 160, 60)));
        assert_eq!(n.point(&g, &w, desktop, Point { x: 301, y: 10 }), None);
        assert_eq!(n.scroll(f32::NAN), None);
    }
    #[test]
    fn invalid_ancestors_cycles_and_hidden_back_window_do_not_win() {
        let mut list = elements();
        list.push(item(6, Some(99), rect(-1, 1, 1, 2)));
        list.push(item(7, Some(8), rect(-1, 1, 1, 2)));
        list.push(item(8, Some(7), rect(-1, 1, 1, 2)));
        let mut front = window();
        front.window = WindowToken(2);
        front.order = 0;
        let mut back = window();
        back.order = 1;
        let windows = [front, back];
        let g = Graph::new(list, &windows);
        let mut n = Navigation::default();
        assert_eq!(
            n.point(
                &g,
                &windows,
                rect(-40, -40, 100, 100),
                Point { x: 0, y: 10 }
            ),
            Some(rect(-40, -40, 100, 100))
        );
        let windows = [window()];
        let g = Graph::new(
            vec![
                item(1, None, rect(-100, -50, 100, 100)),
                item(2, Some(1), rect(-200, -200, 200, 200)),
            ],
            &windows,
        );
        assert_eq!(
            n.point(&g, &windows, rect(-30, -20, 60, 80), Point { x: 0, y: 10 }),
            Some(rect(-30, -20, 60, 80))
        );
    }
    #[test]
    fn same_depth_prefers_smaller_then_stable_order_and_deep_trees_are_bounded() {
        let w = [window()];
        let mut list = vec![
            item(1, None, w[0].bounds),
            item(2, Some(1), rect(-50, 0, 50, 70)),
            item(3, Some(1), rect(-10, 0, 10, 30)),
        ];
        let mut equal = item(4, Some(1), rect(-10, 0, 10, 30));
        equal.order = 2;
        list.push(equal);
        let g = Graph::new(list, &w);
        let (token, id, chain) = g.at(&w, w[0].bounds, Point { x: 0, y: 10 }).unwrap();
        assert_eq!(token, WindowToken(1));
        assert_eq!(id, vec![4]);
        assert_eq!(chain, vec![rect(-10, 0, 10, 30), w[0].bounds]);
        let list = (1..=32)
            .map(|id| item(id, (id > 1).then_some(id - 1), w[0].bounds))
            .collect();
        let g = Graph::new(list, &w);
        assert_eq!(g.nodes.len(), 17);
        assert_eq!(
            g.at(&w, w[0].bounds, Point { x: 0, y: 10 }).unwrap().2,
            vec![w[0].bounds]
        );
    }
}

/// Point query identity also binds the result to its original selection host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementQuery {
    pub session: u64,
    pub request: u64,
    pub host: u64,
    pub point: Point,
    pub window: WindowCandidate,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementQueryResult {
    pub query: ElementQuery,
    pub elements: Vec<ElementCandidate>,
    /// Only a fully examined leaf is safe for short-lived containment caching.
    pub leaf: bool,
}
