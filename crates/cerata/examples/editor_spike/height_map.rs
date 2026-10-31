//! Height map: one item per buffer line, prefix sums in a Fenwick tree.
//!
//! The real engine will use a `sum_tree` (PLAN.md section 3.4) so structural
//! edits are O(log n) too. Here a structural edit (a line added or removed)
//! rebuilds the tree in O(n), which is fine at 1 MB and is measured by the
//! benchmark whenever it happens.

pub struct HeightMap {
    heights: Vec<f64>,
    tree: Vec<f64>,
}

impl HeightMap {
    pub fn new(heights: Vec<f64>) -> Self {
        let mut map = Self {
            heights,
            tree: Vec::new(),
        };
        map.rebuild();
        map
    }

    fn rebuild(&mut self) {
        let n = self.heights.len();
        self.tree = vec![0.0; n + 1];
        for ix in 0..n {
            let node = ix + 1;
            self.tree[node] += self.heights[ix];
            let parent = node + (node.isolate_lowest_one());
            if parent <= n {
                let value = self.tree[node];
                self.tree[parent] += value;
            }
        }
    }

    pub fn get(&self, ix: usize) -> f64 {
        self.heights[ix]
    }

    pub fn set(&mut self, ix: usize, height: f64) {
        let delta = height - self.heights[ix];
        if delta == 0.0 {
            return;
        }
        self.heights[ix] = height;
        let mut node = ix + 1;
        while node < self.tree.len() {
            self.tree[node] += delta;
            node += node.isolate_lowest_one();
        }
    }

    /// Sum of the heights of items `0..ix`, the top of item `ix`.
    pub fn prefix(&self, ix: usize) -> f64 {
        let mut node = ix.min(self.heights.len());
        let mut sum = 0.0;
        while node > 0 {
            sum += self.tree[node];
            node -= node.isolate_lowest_one();
        }
        sum
    }

    pub fn total(&self) -> f64 {
        self.prefix(self.heights.len())
    }

    /// The item containing `y` and the offset of `y` inside it. `y` past the
    /// end lands in the last item; negative `y` in the first.
    pub fn find(&self, y: f64) -> (usize, f64) {
        let n = self.heights.len();
        if n == 0 {
            return (0, 0.0);
        }
        let y = y.max(0.0);
        let mut pos = 0;
        let mut remaining = y;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= remaining {
                pos = next;
                remaining -= self.tree[next];
            }
            step >>= 1;
        }
        if pos >= n {
            (n - 1, remaining + self.heights[n - 1])
        } else {
            (pos, remaining)
        }
    }

    /// Replaces items `start..start + old_count` by `new_heights`.
    pub fn splice(&mut self, start: usize, old_count: usize, new_heights: Vec<f64>) {
        if old_count == new_heights.len() {
            for (ix, height) in new_heights.into_iter().enumerate() {
                self.set(start + ix, height);
            }
            return;
        }
        self.heights.splice(start..start + old_count, new_heights);
        self.rebuild();
    }

    pub fn reset(&mut self, heights: Vec<f64>) {
        self.heights = heights;
        self.rebuild();
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

    #[test]
    fn prefix_and_find_match_a_linear_scan() {
        let heights: Vec<f64> = (0..37)
            .map(|ix| [30.0, 43.264, 137.25, 0.5, 30.0][ix % 5])
            .collect();
        let mut map = HeightMap::new(heights.clone());
        let total: f64 = heights.iter().sum();
        assert!((map.total() - total).abs() < 1e-9);
        for ix in 0..heights.len() {
            let expected: f64 = heights[..ix].iter().sum();
            assert!((map.prefix(ix) - expected).abs() < 1e-9);
        }
        let mut y = -5.0;
        while y < total + 50.0 {
            let (ix, offset) = map.find(y);
            let (expected_ix, expected_offset) = naive_find(&heights, y);
            assert_eq!(ix, expected_ix, "y = {y}");
            assert!((offset - expected_offset).abs() < 1e-6, "y = {y}");
            y += 7.3;
        }
        map.set(3, 100.0);
        assert!((map.total() - (total - 0.5 + 100.0)).abs() < 1e-9);
    }

    #[test]
    fn splice_handles_inserted_and_removed_lines() {
        let mut map = HeightMap::new(vec![10.0, 20.0, 30.0]);
        map.splice(1, 1, vec![1.0, 2.0, 3.0]);
        assert_eq!(map.heights.len(), 5);
        assert_eq!(map.total(), 46.0);
        assert_eq!(map.find(11.5), (2, 0.5));
        map.splice(0, 4, vec![5.0]);
        assert_eq!(map.total(), 35.0);
        assert_eq!(map.find(6.0), (1, 1.0));
    }
}
