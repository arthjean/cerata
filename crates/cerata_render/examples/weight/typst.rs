//! US-031 binary weight: mitex + Typst + typst-svg, with the bundled
//! fonts the formulas need (New Computer Modern Math and text).
use typst::LibraryExt;
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    let math = mitex::convert_math(&source, None).unwrap_or_default();
    let fonts: Vec<_> = typst_assets::fonts()
        .flat_map(|d| typst::text::Font::iter(typst::foundations::Bytes::new(d)))
        .collect();
    let library = typst::Library::default();
    let book = typst::text::FontBook::from_fonts(&fonts);
    let page: Option<&typst_layout::Page> = None;
    let svg = page.map(|p| typst_svg::svg(p, &Default::default()));
    println!(
        "{} {} {:?} {:?}",
        math.len(),
        fonts.len(),
        std::ptr::addr_of!(library),
        (book.families().count(), svg)
    );
}
