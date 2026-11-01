//! US-031 binary weight: Pele and MathJax sharing one QuickJS.
const PELE: &str = include_str!(concat!(
    env!("CERATA_SPIKE_JS"),
    "/pele/package/dist/pele.global.js"
));
const FULL: &str = include_str!(concat!(
    env!("CERATA_SPIKE_JS"),
    "/mathjax3/package/es5/tex-svg-full.js"
));
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    let rt = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&rt).unwrap();
    ctx.with(|ctx| {
        ctx.eval::<(), _>(PELE).ok();
        ctx.eval::<(), _>(FULL).ok();
        ctx.globals().set("__src", source).unwrap();
        let svg: rquickjs::Result<String> =
            ctx.eval("pele.render(__src).svg + MathJax.tex2svg(__src)");
        println!("{:?}", svg.map(|s| s.len()));
    });
}
