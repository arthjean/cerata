//! US-031 binary weight: `mermaid-svg`.
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    println!("{:?}", mermaid_svg::render(&source).map(|s| s.len()));
}
