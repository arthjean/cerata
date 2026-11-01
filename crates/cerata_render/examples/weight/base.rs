//! US-031 binary weight baseline: the other `weight_*` examples add one
//! engine each, so their size minus this one is what the engine costs.
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    println!("{}", source.len());
}
