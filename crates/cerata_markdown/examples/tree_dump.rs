//! Prints the MarkdownTree of a file (debug aid).
fn main() {
    let Some(path) = std::env::args().nth(1) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    print!("{}", cerata_markdown::MarkdownTree::parse(&text).dump());
}
