//! US-031 throwaway spike: diagram and formula rendering without a browser.
//! Compares `mermaid-svg` with Pele in QuickJS for diagrams, and mitex +
//! Typst with MathJax in QuickJS for formulas: share rendered without error,
//! visual gap, p95 render time. Binary weight comes from the `weight_*`
//! examples. Results: `docs/adr/0005-diagram-and-math-rendering.md`.
//!
//! `CERATA_SPIKE_JS` points at a folder holding the unpacked npm and Typst
//! packages (`pele/package`, `mathjax3/package`, `mitex-typ`, `xarrow-typ`).
//! Writes SVGs, PNGs and `report.json` under `target/render_spike/`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "render_spike/corpus.rs"]
mod corpus;
#[path = "render_spike/engines.rs"]
mod engines;

use std::path::{Path, PathBuf};
use std::time::Instant;

const RUNS: usize = 20;

struct Outcome {
    name: &'static str,
    ok: usize,
    total: usize,
    p95_ms: f64,
    errors: Vec<(String, String)>,
    rasters: Vec<Option<Raster>>,
}

#[derive(Clone)]
struct Raster {
    /// 128x128 grayscale, the image fitted and centered.
    thumb: Vec<u8>,
    png: Vec<u8>,
}

fn main() {
    let js = PathBuf::from(std::env::var("CERATA_SPIKE_JS").expect("CERATA_SPIKE_JS"));
    let out =
        PathBuf::from(std::env::var("CERATA_SPIKE_OUT").unwrap_or_else(|_| "render_spike".into()));
    std::fs::create_dir_all(&out).unwrap();
    assert_eq!(corpus::FORMULAS.len(), 100, "100 formulas");

    let t = Instant::now();
    let pele = engines::Pele::new(&js).expect("pele");
    println!("pele init {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    println!("pele sandbox: {}", pele.sandbox().escape_hatches());
    let t = Instant::now();
    let mathjax = engines::MathJax::new(&js);
    println!("mathjax init {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let typst = engines::Typst::new(&js).expect("typst");
    let _ = typst.render("x");
    println!("typst init {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    for bad in [r"\frac{a", r"\undefinedmacro x", r"x^"] {
        println!(
            "invalid {bad:?}: mathjax {:?} | typst {:?}",
            mathjax.as_ref().map(|m| m.render(bad).err()),
            typst.render(bad).err()
        );
    }
    let bad = "flowchart LR\n A -->";
    println!(
        "invalid diagram: pele {:?} | mermaid-svg {:?}",
        pele.render(bad).err(),
        engines::mermaid_svg(bad).err()
    );

    let diagrams: Vec<&str> = corpus::DIAGRAMS.iter().map(|(_, s)| *s).collect();
    let names: Vec<String> = corpus::DIAGRAMS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    let pele_out = run("pele", &diagrams, &names, &out, |s| {
        pele.render(s).map(|svg| resolve_vars(&svg))
    });
    let mermaid_out = run("mermaid-svg", &diagrams, &names, &out, engines::mermaid_svg);

    let formula_names: Vec<String> = (0..corpus::FORMULAS.len())
        .map(|i| format!("f{i:03}"))
        .collect();
    let typst_out = run("mitex-typst", corpus::FORMULAS, &formula_names, &out, |s| {
        typst.render(s)
    });
    let mathjax_out = match &mathjax {
        Ok(mj) => run("mathjax", corpus::FORMULAS, &formula_names, &out, |s| {
            mj.render(s)
        }),
        Err(e) => {
            println!("mathjax: setup failed: {e}");
            Outcome {
                name: "mathjax",
                ok: 0,
                total: corpus::FORMULAS.len(),
                p95_ms: f64::NAN,
                errors: vec![("setup".into(), e.clone())],
                rasters: vec![None; corpus::FORMULAS.len()],
            }
        }
    };

    let mut report = serde_json::Map::new();
    for (outcome, reference) in [
        (&pele_out, &pele_out),
        (&mermaid_out, &pele_out),
        (&mathjax_out, &typst_out),
        (&typst_out, &mathjax_out),
    ] {
        let gap = visual_gap(outcome, reference);
        println!(
            "{:12} {:3}/{:3} rendered ({:5.1} %), p95 {:7.2} ms, gap vs {:12} {}",
            outcome.name,
            outcome.ok,
            outcome.total,
            100.0 * outcome.ok as f64 / outcome.total as f64,
            outcome.p95_ms,
            reference.name,
            gap.map(|g| format!("{g:.1} %")).unwrap_or("n/a".into()),
        );
        for (name, error) in &outcome.errors {
            println!(
                "    {name}: {}",
                error.chars().take(140).collect::<String>()
            );
        }
        report.insert(
            outcome.name.into(),
            serde_json::json!({
                "rendered": outcome.ok, "total": outcome.total, "p95_ms": outcome.p95_ms,
                "gap_percent": gap, "gap_reference": reference.name,
                "errors": outcome.errors,
            }),
        );
    }
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
}

fn run(
    name: &'static str,
    sources: &[&str],
    names: &[String],
    out: &Path,
    render: impl Fn(&str) -> Result<String, String>,
) -> Outcome {
    let dir = out.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut times = Vec::new();
    let mut ok = 0;
    let mut errors = Vec::new();
    let mut rasters = Vec::new();
    for (source, label) in sources.iter().zip(names) {
        let first = render(source);
        match first {
            Ok(svg) => {
                for _ in 0..RUNS {
                    // Cold renders: Typst memoizes whole compilations, and
                    // the app caches results itself.
                    typst::comemo::evict(0);
                    let start = Instant::now();
                    let _ = render(source);
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                std::fs::write(dir.join(format!("{label}.svg")), &svg).unwrap();
                match rasterize(&svg) {
                    Ok(raster) => {
                        std::fs::write(dir.join(format!("{label}.png")), &raster.png).unwrap();
                        ok += 1;
                        rasters.push(Some(raster));
                    }
                    Err(e) => {
                        errors.push((label.clone(), format!("rasterize: {e}")));
                        rasters.push(None);
                    }
                }
            }
            Err(e) => {
                errors.push((label.clone(), e));
                rasters.push(None);
            }
        }
    }
    times.sort_by(f64::total_cmp);
    let p95 = times
        .get(((times.len() as f64) * 0.95) as usize)
        .or(times.last())
        .copied()
        .unwrap_or(f64::NAN);
    Outcome {
        name,
        ok,
        total: sources.len(),
        p95_ms: p95,
        errors,
        rasters,
    }
}

/// resvg resolves no CSS custom property. Pele declares its palette as
/// custom properties on the root `style` (`--_bg:var(--pele-bg,#fff)`) and
/// uses `var(--_bg)` everywhere: substitute the declared values, falling
/// back to the `var()` fallback.
fn resolve_vars(svg: &str) -> String {
    let mut vars: std::collections::HashMap<String, String> = Default::default();
    let mut s = svg.to_string();
    for _ in 0..4 {
        // Declarations, in `style` attributes only.
        let mut rest = s.as_str();
        while let Some(i) = rest.find("--") {
            let tail = &rest[i..];
            let name_end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .unwrap_or(tail.len());
            if tail[name_end..].starts_with(':') {
                let value_end = tail[name_end + 1..]
                    .find([';', '"'])
                    .map(|e| e + name_end + 1)
                    .unwrap_or(tail.len());
                let value = tail[name_end + 1..value_end].trim();
                if !value.contains("var(") {
                    vars.insert(tail[..name_end].to_string(), value.to_string());
                }
            }
            rest = &tail[name_end.max(2)..];
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(i) = rest.find("var(--") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 4..];
            let mut depth = 1;
            let mut end = tail.len();
            let mut comma = None;
            for (j, c) in tail.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = j;
                            break;
                        }
                    }
                    ',' if depth == 1 && comma.is_none() => comma = Some(j),
                    _ => {}
                }
            }
            let name = tail[..comma.unwrap_or(end)].trim();
            match (vars.get(name), comma) {
                (Some(v), _) => out.push_str(v),
                (None, Some(c)) => out.push_str(tail[c + 1..end].trim()),
                (None, None) => out.push_str(&rest[i..i + 4 + end + 1]),
            }
            rest = tail.get(end + 1..).unwrap_or("");
        }
        out.push_str(rest);
        s = out;
    }
    s
}

fn rasterize(svg: &str) -> Result<Raster, String> {
    static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    let fontdb = FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            std::sync::Arc::new(db)
        })
        .clone();
    let options = resvg::usvg::Options {
        fontdb,
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
    if w == 0 || h == 0 {
        return Err("empty".into());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("pixmap")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(&tree, Default::default(), &mut pixmap.as_mut());
    let png = pixmap.encode_png().map_err(|e| e.to_string())?;
    let blank = pixmap
        .pixels()
        .iter()
        .all(|p| p.red() > 250 && p.green() > 250 && p.blue() > 250);
    if blank {
        return Err("blank raster".into());
    }
    let n = 128u32;
    let scale = (n as f32 / w.max(h) as f32).min(f32::MAX);
    let thumb_px = (w as f32 * scale, h as f32 * scale);
    let mut thumb = resvg::tiny_skia::Pixmap::new(n, n).ok_or("thumb")?;
    thumb.fill(resvg::tiny_skia::Color::WHITE);
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale)
        .post_translate((n as f32 - thumb_px.0) / 2.0, (n as f32 - thumb_px.1) / 2.0);
    resvg::render(&tree, transform, &mut thumb.as_mut());
    let gray = thumb
        .pixels()
        .iter()
        .map(|p| ((p.red() as u16 + p.green() as u16 + p.blue() as u16) / 3) as u8)
        .collect();
    Ok(Raster { thumb: gray, png })
}

/// Mean absolute gray difference of the fitted thumbnails, in percent of
/// full scale, over the items both engines rendered.
fn visual_gap(a: &Outcome, b: &Outcome) -> Option<f64> {
    if std::ptr::eq(a, b) {
        return Some(0.0);
    }
    let mut sum = 0.0;
    let mut n = 0;
    for (x, y) in a.rasters.iter().zip(&b.rasters) {
        if let (Some(x), Some(y)) = (x, y) {
            let d: f64 = x
                .thumb
                .iter()
                .zip(&y.thumb)
                .map(|(p, q)| (*p as f64 - *q as f64).abs())
                .sum::<f64>()
                / (x.thumb.len() as f64 * 255.0);
            sum += d;
            n += 1;
        }
    }
    (n > 0).then(|| 100.0 * sum / n as f64)
}
