//! Content similarity of two notes, to recognize a note renamed and edited
//! outside Cerata (US-085).
//!
//! The estimate follows git's rename score (`diffcore-delta.c`): both texts
//! are cut into lines, a line longer than 64 bytes into 64-byte chunks;
//! the bytes of the chunks the two texts share (counted with their
//! multiplicity) are divided by the size of the larger text. A note is kept
//! as a [`Sketch`], so the store can compare it once the file is gone.

/// Chunks longer than this are cut, as git does.
const CHUNK_BYTES: usize = 64;

/// Line chunks of a text, as sorted `(hash, bytes)` pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sketch {
    chunks: Vec<(u32, u32)>,
}

impl Sketch {
    pub fn of(text: &[u8]) -> Self {
        let mut chunks: Vec<(u32, u32)> = Vec::new();
        let mut start = 0;
        for (index, byte) in text.iter().enumerate() {
            if *byte == b'\n' || index + 1 - start == CHUNK_BYTES {
                chunks.push(chunk(&text[start..=index]));
                start = index + 1;
            }
        }
        if start < text.len() {
            chunks.push(chunk(&text[start..]));
        }
        chunks.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(chunks.len());
        for (hash, bytes) in chunks {
            match merged.last_mut() {
                Some((last, total)) if *last == hash => *total = total.saturating_add(bytes),
                _ => merged.push((hash, bytes)),
            }
        }
        Self { chunks: merged }
    }

    /// Little-endian `(hash, bytes)` pairs, for the store.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.chunks.len() * 8);
        for (hash, count) in &self.chunks {
            bytes.extend_from_slice(&hash.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
        }
        bytes
    }

    /// The inverse of [`Self::to_bytes`]; trailing bytes are ignored.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let (pairs, _) = bytes.as_chunks::<8>();
        let chunks = pairs
            .iter()
            .map(|[a, b, c, d, e, f, g, h]| {
                (
                    u32::from_le_bytes([*a, *b, *c, *d]),
                    u32::from_le_bytes([*e, *f, *g, *h]),
                )
            })
            .collect();
        Self { chunks }
    }

    /// Total bytes of the text.
    pub fn size(&self) -> u64 {
        self.chunks.iter().map(|(_, bytes)| u64::from(*bytes)).sum()
    }

    /// Share of content in common, from 0 (nothing) to 1 (same chunks).
    pub fn similarity(&self, other: &Sketch) -> f64 {
        let largest = self.size().max(other.size());
        if largest == 0 {
            return 1.0;
        }
        let (mut left, mut right) = (
            self.chunks.iter().peekable(),
            other.chunks.iter().peekable(),
        );
        let mut shared: u64 = 0;
        while let (Some((a, a_bytes)), Some((b, b_bytes))) = (left.peek(), right.peek()) {
            match a.cmp(b) {
                std::cmp::Ordering::Less => {
                    left.next();
                }
                std::cmp::Ordering::Greater => {
                    right.next();
                }
                std::cmp::Ordering::Equal => {
                    shared += u64::from((*a_bytes).min(*b_bytes));
                    left.next();
                    right.next();
                }
            }
        }
        shared as f64 / largest as f64
    }
}

/// 32-bit FNV-1a: stable across builds, since sketches are stored.
fn chunk(bytes: &[u8]) -> (u32, u32) {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash, u32::try_from(bytes.len()).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(count: usize, tag: &str) -> String {
        (0..count).map(|i| format!("{tag} line {i}\n")).collect()
    }

    #[test]
    fn identical_texts_are_fully_similar_and_disjoint_ones_not_at_all() {
        let text = lines(10, "a");
        assert_eq!(
            Sketch::of(text.as_bytes()).similarity(&Sketch::of(text.as_bytes())),
            1.0
        );
        let other = lines(10, "b");
        assert_eq!(
            Sketch::of(text.as_bytes()).similarity(&Sketch::of(other.as_bytes())),
            0.0
        );
    }

    #[test]
    fn similarity_is_the_share_of_shared_lines() {
        // 6 lines of 10 kept, 4 replaced by lines of the same length.
        let old = lines(10, "x");
        let new: String = old
            .lines()
            .enumerate()
            .map(|(i, line)| {
                if i < 6 {
                    format!("{line}\n")
                } else {
                    format!("{}\n", line.replace('x', "y"))
                }
            })
            .collect();
        let score = Sketch::of(old.as_bytes()).similarity(&Sketch::of(new.as_bytes()));
        assert!((score - 0.6).abs() < 0.01, "{score}");
    }

    #[test]
    fn long_lines_are_cut_and_sketches_round_trip() {
        let long = "z".repeat(200);
        let sketch = Sketch::of(long.as_bytes());
        assert_eq!(sketch.size(), 200);
        assert_eq!(Sketch::from_bytes(&sketch.to_bytes()), sketch);
        assert_eq!(Sketch::of(b"").similarity(&Sketch::of(b"")), 1.0);
    }
}
