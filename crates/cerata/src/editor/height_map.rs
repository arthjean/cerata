//! Height map: one height per document line, in a two-level sum tree.
//!
//! Leaves are chunks of at most [`CHUNK`] line heights; a Fenwick tree over
//! the chunk sums and another over the chunk line counts give `prefix`,
//! `find` and `set` in O(log n + CHUNK). A splice that adds or removes lines
//! rewrites only the chunks it touches, then rebuilds the two Fenwick trees
//! over the chunks, O(n / CHUNK): 230 chunks for the 14 442 lines of the 1 MB
//! fixture.

const CHUNK: usize = 64;

#[derive(Clone, Debug)]
pub struct HeightMap {
    chunks: Vec<Vec<f64>>,
    sums: Fenwick<f64>,
    counts: Fenwick<usize>,
    len: usize,
}

impl HeightMap {
    pub fn new(heights: Vec<f64>) -> Self {
        let mut map = Self {
            chunks: Vec::new(),
            sums: Fenwick::new(Vec::new()),
            counts: Fenwick::new(Vec::new()),
            len: 0,
        };
        map.reset(heights);
        map
    }

    pub fn reset(&mut self, heights: Vec<f64>) {
        self.chunks = chunked(heights);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        if self.chunks.is_empty() {
            self.chunks.push(Vec::new());
        }
        self.sums = Fenwick::new(self.chunks.iter().map(|c| c.iter().sum()).collect());
        self.counts = Fenwick::new(self.chunks.iter().map(Vec::len).collect());
        self.len = self.chunks.iter().map(Vec::len).sum();
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// The chunk holding line `ix` and the index in it.
    fn locate(&self, ix: usize) -> (usize, usize) {
        let (chunk, before) = self.counts.find_count(ix);
        (chunk.min(self.chunks.len() - 1), ix - before)
    }

    pub fn get(&self, ix: usize) -> f64 {
        if ix >= self.len {
            return 0.0;
        }
        let (chunk, i) = self.locate(ix);
        self.chunks[chunk][i]
    }

    pub fn set(&mut self, ix: usize, height: f64) {
        if ix >= self.len {
            return;
        }
        let (chunk, i) = self.locate(ix);
        let delta = height - self.chunks[chunk][i];
        if delta != 0.0 {
            self.chunks[chunk][i] = height;
            self.sums.add(chunk, delta);
        }
    }

    /// Top of line `ix`: the sum of the heights of lines `0..ix`.
    pub fn prefix(&self, ix: usize) -> f64 {
        if ix >= self.len {
            return self.total();
        }
        let (chunk, i) = self.locate(ix);
        self.sums.prefix(chunk) + self.chunks[chunk][..i].iter().sum::<f64>()
    }

    pub fn total(&self) -> f64 {
        self.sums.prefix(self.chunks.len())
    }

    /// The line containing `y` and the offset of `y` inside it. `y` past the
    /// end lands in the last line; negative `y` at the top of the first.
    pub fn find(&self, y: f64) -> (usize, f64) {
        if self.len == 0 {
            return (0, 0.0);
        }
        let y = y.max(0.0);
        let (chunk, above) = self.sums.find_sum(y);
        if chunk >= self.chunks.len() {
            let last = self.len - 1;
            return (last, y - self.prefix(last));
        }
        let first = self.counts.prefix(chunk);
        let mut top = above;
        for (i, height) in self.chunks[chunk].iter().enumerate() {
            if y < top + height {
                return (first + i, y - top);
            }
            top += height;
        }
        let last = self.len - 1;
        (last, y - self.prefix(last))
    }

    /// Replaces lines `start..start + old` by `new_heights`.
    pub fn splice(&mut self, start: usize, old: usize, new_heights: Vec<f64>) {
        let start = start.min(self.len);
        let old = old.min(self.len - start);
        if old == new_heights.len() {
            for (i, height) in new_heights.into_iter().enumerate() {
                self.set(start + i, height);
            }
            return;
        }
        let (first_chunk, offset) = if start == self.len {
            let last = self.chunks.len() - 1;
            (last, self.chunks[last].len())
        } else {
            self.locate(start)
        };
        let last_chunk = if old == 0 {
            first_chunk
        } else {
            self.locate(start + old - 1).0
        };
        let mut items: Vec<f64> = self.chunks[first_chunk..=last_chunk]
            .iter()
            .flatten()
            .copied()
            .collect();
        items.splice(offset..offset + old, new_heights);
        self.chunks.splice(first_chunk..=last_chunk, chunked(items));
        self.rebuild();
    }
}

fn chunked(items: Vec<f64>) -> Vec<Vec<f64>> {
    items.chunks(CHUNK).map(<[f64]>::to_vec).collect()
}

/// A Fenwick tree (binary indexed tree) of partial sums.
#[derive(Clone, Debug)]
struct Fenwick<T> {
    tree: Vec<T>,
}

impl<T> Fenwick<T>
where
    T: Copy + Default + PartialOrd + std::ops::Add<Output = T> + std::ops::Sub<Output = T>,
{
    fn new(values: Vec<T>) -> Self {
        let n = values.len();
        let mut tree = vec![T::default(); n + 1];
        for (i, value) in values.into_iter().enumerate() {
            let node = i + 1;
            tree[node] = tree[node] + value;
            let parent = node + node.isolate_lowest_one();
            if parent <= n {
                tree[parent] = tree[parent] + tree[node];
            }
        }
        Self { tree }
    }

    fn add(&mut self, i: usize, delta: T) {
        let mut node = i + 1;
        while node < self.tree.len() {
            self.tree[node] = self.tree[node] + delta;
            node += node.isolate_lowest_one();
        }
    }

    /// Sum of items `0..i`.
    fn prefix(&self, i: usize) -> T {
        let mut node = i.min(self.tree.len() - 1);
        let mut sum = T::default();
        while node > 0 {
            sum = sum + self.tree[node];
            node -= node.isolate_lowest_one();
        }
        sum
    }

    /// The first item at which the running sum exceeds `target`, and the
    /// sum before it (`len` when the total does not exceed it).
    fn search(&self, target: T) -> (usize, T) {
        let n = self.tree.len() - 1;
        let mut pos = 0;
        let mut acc = T::default();
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && acc + self.tree[next] <= target {
                pos = next;
                acc = acc + self.tree[next];
            }
            step >>= 1;
        }
        (pos, acc)
    }
}

impl Fenwick<f64> {
    fn find_sum(&self, y: f64) -> (usize, f64) {
        self.search(y)
    }
}

impl Fenwick<usize> {
    fn find_count(&self, ix: usize) -> (usize, usize) {
        self.search(ix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_find(heights: &[f64], y: f64) -> (usize, f64) {
        let mut top = 0.0;
        for (ix, height) in heights.iter().enumerate() {
            if y < top + height {
                return (ix, y.max(0.0) - top);
            }
            top += height;
        }
        let last = heights.len() - 1;
        (last, y - (top - heights[last]))
    }

    fn check(map: &HeightMap, heights: &[f64]) {
        assert_eq!(map.len(), heights.len());
        let total: f64 = heights.iter().sum();
        assert!((map.total() - total).abs() < 1e-6);
        for ix in 0..heights.len() {
            assert_eq!(map.get(ix), heights[ix]);
            let expected: f64 = heights[..ix].iter().sum();
            assert!((map.prefix(ix) - expected).abs() < 1e-6, "prefix {ix}");
        }
        let mut y = -5.0;
        while y < total + 50.0 {
            let (ix, offset) = map.find(y);
            let (expected_ix, expected_offset) = naive_find(heights, y);
            assert_eq!(ix, expected_ix, "y = {y}");
            assert!((offset - expected_offset).abs() < 1e-6, "y = {y}");
            y += 13.7;
        }
    }

    #[test]
    fn prefix_find_and_splice_match_a_linear_scan() {
        let mut heights: Vec<f64> = (0..700)
            .map(|ix| [30.0, 43.264, 137.25, 0.5, 30.0, 286.0][ix % 6])
            .collect();
        let mut map = HeightMap::new(heights.clone());
        check(&map, &heights);
        map.set(3, 100.0);
        heights[3] = 100.0;
        check(&map, &heights);
        // Inserted, removed and replaced lines, inside and across chunks.
        for (start, old, new) in [
            (10usize, 0usize, vec![1.0, 2.0, 3.0]),
            (60, 10, vec![7.0]),
            (0, 1, vec![]),
            (120, 200, vec![9.0; 300]),
            (heights.len() - 3, 3, vec![]),
        ] {
            let start = start.min(heights.len());
            heights.splice(start..start + old, new.clone());
            map.splice(start, old, new);
            check(&map, &heights);
        }
        let end = heights.len();
        heights.extend([4.0, 5.0]);
        map.splice(end, 0, vec![4.0, 5.0]);
        check(&map, &heights);
    }

    #[test]
    fn a_single_line_document_works() {
        let mut map = HeightMap::new(vec![30.0]);
        assert_eq!(map.find(10.0), (0, 10.0));
        map.splice(0, 1, vec![30.0, 30.0]);
        assert_eq!(map.find(45.0), (1, 15.0));
        assert_eq!(map.total(), 60.0);
    }
}
