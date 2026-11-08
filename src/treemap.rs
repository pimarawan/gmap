use crate::analysis::Analysis;
use crate::history::History;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The squarified treemap of Bruls, Huizing and van Wijk (2000): one rectangle
/// per size, in the order given. Sizes should be sorted largest first; that is
/// what keeps the aspect ratios low.
pub fn squarify(sizes: &[f64], area: Rect) -> Vec<Rect> {
    let mut out = vec![Rect { x: area.x, y: area.y, w: 0.0, h: 0.0 }; sizes.len()];
    let total: f64 = sizes.iter().sum();
    if total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return out;
    }
    let scale = area.w * area.h / total;
    let mut r = area;
    let mut start = 0;
    while start < sizes.len() {
        let side = r.w.min(r.h);
        let mut end = start + 1;
        let mut sum = sizes[start] * scale;
        let mut worst = worst_ratio(sizes[start] * scale, sizes[start] * scale, sum, side);
        while end < sizes.len() {
            let a = sizes[end] * scale;
            let s = sum + a;
            let w = worst_ratio(sizes[start] * scale, a, s, side);
            if w > worst {
                break;
            }
            worst = w;
            sum = s;
            end += 1;
        }
        // Lay the row along the shorter side, then cut it off the remaining area.
        if r.w >= r.h {
            let w = if r.h > 0.0 { sum / r.h } else { 0.0 };
            let mut y = r.y;
            for i in start..end {
                let h = if w > 0.0 { sizes[i] * scale / w } else { 0.0 };
                out[i] = Rect { x: r.x, y, w, h };
                y += h;
            }
            r = Rect { x: r.x + w, y: r.y, w: (r.w - w).max(0.0), h: r.h };
        } else {
            let h = if r.w > 0.0 { sum / r.w } else { 0.0 };
            let mut x = r.x;
            for i in start..end {
                let w = if h > 0.0 { sizes[i] * scale / h } else { 0.0 };
                out[i] = Rect { x, y: r.y, w, h };
                x += w;
            }
            r = Rect { x: r.x, y: r.y + h, w: r.w, h: (r.h - h).max(0.0) };
        }
        start = end;
    }
    out
}

/// The worst aspect ratio in a row with the given largest and smallest areas
/// and total, laid along a side of length `side`.
fn worst_ratio(largest: f64, smallest: f64, sum: f64, side: f64) -> f64 {
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * largest / s2).max(s2 / (w2 * smallest))
}

pub struct Node {
    pub name: String,
    pub path: String,
    pub parent: Option<usize>,
    /// Largest first.
    pub children: Vec<usize>,
    /// Index into `Analysis::files` for a file.
    pub file: Option<usize>,
    pub lines: u64,
    pub files: u32,
    pub weight: f64,
    pub revs: u64,
    /// Heat in [0, 1]: for a file, where its weighted changes rank among all
    /// files (0 if it never changed); for a directory, the line-weighted mean.
    pub heat: f64,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.file.is_none()
    }
}

pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn build(h: &History, a: &Analysis) -> Self {
        let mut nodes = vec![Node {
            name: h.name.clone(),
            path: String::new(),
            parent: None,
            children: Vec::new(),
            file: None,
            lines: 0,
            files: 0,
            weight: 0.0,
            revs: 0,
            heat: 0.0,
        }];
        // Color by rank, not by value: change counts are so skewed that any
        // scale on the values paints a few files bright and the rest the same.
        // Ranks are weighted by lines, so each shade covers the same area of
        // the map, then squared, so most of the map stays calm and the top
        // tenth of the code by recent change is what glows.
        let mut order: Vec<usize> = (0..a.files.len()).filter(|&i| a.files[i].weight > 0.0).collect();
        order.sort_by(|&x, &y| a.files[x].weight.total_cmp(&a.files[y].weight));
        let total: f64 = order.iter().map(|&i| a.files[i].lines.max(1) as f64).sum();
        let mut rank = vec![0.0; a.files.len()];
        let mut below = 0.0;
        for &i in &order {
            let own = a.files[i].lines.max(1) as f64;
            let t = (below + own / 2.0) / total.max(1.0);
            rank[i] = 0.06 + 0.94 * t * t;
            below += own;
        }
        for (i, f) in a.files.iter().enumerate() {
            let path = &h.paths[f.path as usize];
            let mut at = 0;
            let mut parts = path.split('/').peekable();
            let mut sofar = String::new();
            while let Some(part) = parts.next() {
                if !sofar.is_empty() {
                    sofar.push('/');
                }
                sofar.push_str(part);
                let leaf = parts.peek().is_none();
                let found =
                    nodes[at].children.iter().copied().find(|&c| nodes[c].name == part && nodes[c].is_dir() != leaf);
                at = match found {
                    Some(c) => c,
                    None => {
                        nodes.push(Node {
                            name: part.to_string(),
                            path: sofar.clone(),
                            parent: Some(at),
                            children: Vec::new(),
                            file: leaf.then_some(i),
                            lines: 0,
                            files: 0,
                            weight: 0.0,
                            revs: 0,
                            heat: 0.0,
                        });
                        let n = nodes.len() - 1;
                        nodes[at].children.push(n);
                        n
                    }
                };
            }
            let lines = f.lines.max(1) as u64;
            let heat = rank[i];
            let mut up = Some(at);
            while let Some(n) = up {
                let node = &mut nodes[n];
                node.lines += lines;
                node.files += 1;
                node.weight += f.weight;
                node.revs += f.revs as u64;
                node.heat += heat * lines as f64;
                up = node.parent;
            }
        }
        for n in &mut nodes {
            n.heat = if n.lines > 0 { n.heat / n.lines as f64 } else { 0.0 };
        }
        let mut t = Tree { nodes };
        t.collapse(0);
        for n in 0..t.nodes.len() {
            let mut c = std::mem::take(&mut t.nodes[n].children);
            c.sort_by(|&x, &y| t.nodes[y].lines.cmp(&t.nodes[x].lines).then(t.nodes[x].name.cmp(&t.nodes[y].name)));
            t.nodes[n].children = c;
        }
        t
    }

    /// Fold chains of directories with a single child directory into one
    /// node named `a/b/c`, the way GitHub shows `src/main/java/com`.
    fn collapse(&mut self, n: usize) {
        let children = self.nodes[n].children.clone();
        for c in children {
            loop {
                let only = &self.nodes[c].children;
                if !self.nodes[c].is_dir() || only.len() != 1 || !self.nodes[only[0]].is_dir() {
                    break;
                }
                let g = only[0];
                let name = format!("{}/{}", self.nodes[c].name, self.nodes[g].name);
                let grandchildren = std::mem::take(&mut self.nodes[g].children);
                for &gc in &grandchildren {
                    self.nodes[gc].parent = Some(c);
                }
                let path = std::mem::take(&mut self.nodes[g].path);
                let node = &mut self.nodes[c];
                node.name = name;
                node.path = path;
                node.children = grandchildren;
            }
            self.collapse(c);
        }
    }

    /// The deepest node whose path is `path` or contains it.
    pub fn find(&self, path: &str) -> usize {
        let mut at = 0;
        'down: loop {
            for &c in &self.nodes[at].children {
                let p = &self.nodes[c].path;
                if p == path || (path.starts_with(p.as_str()) && path.as_bytes().get(p.len()) == Some(&b'/')) {
                    at = c;
                    continue 'down;
                }
            }
            return at;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_exactly() {
        let sizes = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let area = Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 };
        let r = squarify(&sizes, area);
        let total: f64 = sizes.iter().sum();
        for (s, r) in sizes.iter().zip(&r) {
            assert!((r.w * r.h - s * 24.0 / total).abs() < 1e-9);
            assert!(r.x >= -1e-9 && r.y >= -1e-9 && r.x + r.w <= 6.0 + 1e-9 && r.y + r.h <= 4.0 + 1e-9);
        }
        for i in 0..r.len() {
            for j in i + 1..r.len() {
                let (a, b) = (r[i], r[j]);
                let ox = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let oy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(ox <= 1e-9 || oy <= 1e-9, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn paper_example_is_squarish() {
        // The example from Bruls et al.: no rectangle worse than 3:1.
        let r = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 });
        for r in r {
            assert!(r.w.max(r.h) / r.w.min(r.h) <= 3.0 + 1e-9, "{r:?}");
        }
    }

    #[test]
    fn degenerate() {
        assert!(squarify(&[], Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }).is_empty());
        let r = squarify(&[1.0], Rect { x: 2.0, y: 3.0, w: 4.0, h: 5.0 });
        assert_eq!(r[0], Rect { x: 2.0, y: 3.0, w: 4.0, h: 5.0 });
    }
}
