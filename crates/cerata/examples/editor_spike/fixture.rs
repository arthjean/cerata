//! Deterministic Markdown fixture: H1 to H6, wrapped paragraphs with strong
//! text and links, and block images whose heights are not multiples of the
//! line height. The generator is committed instead of a 1 MB file.

use std::path::Path;

/// Natural sizes of the generated figures (width, height). After scaling to
/// the 720 px column none of them is a multiple of the 30 px body row.
pub const FIGURES: [(u32, u32); 4] = [(640, 251), (480, 173), (800, 333), (1000, 397)];

const WORDS: [&str; 48] = [
    "the",
    "editor",
    "keeps",
    "markdown",
    "plain",
    "while",
    "rendering",
    "headings",
    "at",
    "their",
    "own",
    "size",
    "and",
    "a",
    "height",
    "map",
    "tracks",
    "every",
    "line",
    "so",
    "scrolling",
    "stays",
    "stable",
    "across",
    "wrapped",
    "paragraphs",
    "with",
    "inline",
    "marks",
    "that",
    "hide",
    "when",
    "selection",
    "leaves",
    "node",
    "reveal",
    "as",
    "soon",
    "it",
    "touches",
    "images",
    "measure",
    "exact",
    "pixels",
    "never",
    "rounded",
    "rows",
    "notes",
];

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn sentence(rng: &mut Lcg, out: &mut String, section: usize) {
    let words = 8 + rng.below(14);
    for ix in 0..words {
        if ix > 0 {
            out.push(' ');
        }
        let word = WORDS[rng.below(WORDS.len())];
        match rng.below(14) {
            0 => {
                out.push_str("**");
                out.push_str(word);
                out.push_str("**");
            }
            1 => {
                out.push('[');
                out.push_str(word);
                out.push_str("](https://example.com/notes/");
                out.push_str(&section.to_string());
                out.push('/');
                out.push_str(word);
                out.push(')');
            }
            _ => out.push_str(word),
        }
    }
    out.push('.');
}

fn paragraph(rng: &mut Lcg, out: &mut String, section: usize, sentences: usize) {
    for ix in 0..sentences {
        if ix > 0 {
            out.push(' ');
        }
        sentence(rng, out, section);
    }
    out.push_str("\n\n");
}

/// Generates at least `min_bytes` of Markdown. Same input, same output.
pub fn generate(min_bytes: usize) -> String {
    let mut rng = Lcg(0x5eed_cafe);
    let mut out = String::with_capacity(min_bytes + 4096);
    let mut section = 0;
    while out.len() < min_bytes {
        if section % 10 == 0 {
            out.push_str(&format!("# Chapter {}\n\n", section / 10 + 1));
        }
        out.push_str(&format!("## Section {section} with **strong** title\n\n"));
        let count = 3 + rng.below(4);
        paragraph(&mut rng, &mut out, section, count);
        out.push_str(&format!(
            "### Notes on [item {section}](https://example.com/{section})\n\n"
        ));
        let count = 1 + rng.below(3);
        paragraph(&mut rng, &mut out, section, count);
        if section % 5 == 2 {
            let figure = section / 5 % FIGURES.len();
            out.push_str(&format!(
                "![Figure {section}](images/figure-{figure}.svg)\n\n"
            ));
        }
        out.push_str(&format!("#### Detail {section}\n\n"));
        let count = 1 + rng.below(2);
        paragraph(&mut rng, &mut out, section, count);
        out.push_str(&format!("##### Aside {section}\n\n"));
        paragraph(&mut rng, &mut out, section, 1);
        out.push_str(&format!("###### Footnote {section}\n\n"));
        paragraph(&mut rng, &mut out, section, 1);
        section += 1;
    }
    out
}

pub fn figure_svg(index: usize) -> String {
    let (w, h) = FIGURES[index % FIGURES.len()];
    let colors = ["#7aa2f7", "#9ece6a", "#e0af68", "#bb9af7"];
    let color = colors[index % colors.len()];
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="#f4f1ea"/>
<rect x="8" y="8" width="{iw}" height="{ih}" fill="none" stroke="{color}" stroke-width="4"/>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="{color}"/>
<line x1="0" y1="{h}" x2="{w}" y2="0" stroke="#565f89" stroke-width="3"/>
</svg>
"##,
        iw = w - 16,
        ih = h - 16,
        cx = w / 2,
        cy = h / 2,
        r = h / 4,
    )
}

/// Writes `spike-1mb.md` and its figures into `dir`, returns the note path.
pub fn write(dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let images = dir.join("images");
    std::fs::create_dir_all(&images)?;
    for index in 0..FIGURES.len() {
        std::fs::write(
            images.join(format!("figure-{index}.svg")),
            figure_svg(index),
        )?;
    }
    let path = dir.join("spike-1mb.md");
    std::fs::write(&path, generate(1 << 20))?;
    Ok(path)
}

/// Natural size of an SVG (from its `width`/`height` attributes) or a PNG
/// (from its IHDR chunk), read without decoding the image.
pub fn image_natural_size(bytes: &[u8]) -> Option<(f32, f32)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((w as f32, h as f32));
    }
    let text = std::str::from_utf8(bytes.get(..bytes.len().min(2048))?).ok()?;
    let tag_start = text.find("<svg")?;
    let tag = &text[tag_start..tag_start + text[tag_start..].find('>')?];
    let attr = |name: &str| -> Option<f32> {
        let key = format!(" {name}=\"");
        let start = tag.find(&key)? + key.len();
        let end = start + tag[start..].find('"')?;
        tag[start..end].trim_end_matches("px").parse().ok()
    };
    Some((attr("width")?, attr("height")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{LineKind, scan_line};

    #[test]
    fn fixture_is_deterministic_and_covers_every_heading_level() {
        let a = generate(1 << 20);
        assert!(a.len() >= 1 << 20);
        assert!(a.len() < (1 << 20) + 8192);
        assert_eq!(a, generate(1 << 20));
        let mut levels = [0usize; 6];
        let mut images = 0;
        for line in a.lines() {
            match scan_line(line).kind {
                LineKind::Heading { level } => levels[level as usize - 1] += 1,
                LineKind::ImageBlock { .. } => images += 1,
                LineKind::Paragraph => {}
            }
        }
        assert!(levels.iter().all(|&count| count > 10), "{levels:?}");
        assert!(images > 10);
    }

    #[test]
    fn natural_size_is_read_from_the_svg_header() {
        for (index, &(w, h)) in FIGURES.iter().enumerate() {
            assert_eq!(
                image_natural_size(figure_svg(index).as_bytes()),
                Some((w as f32, h as f32))
            );
        }
    }
}
