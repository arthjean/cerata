fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    println!("{}", source.len());
}
