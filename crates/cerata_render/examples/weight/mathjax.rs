//! US-031 binary weight: MathJax 3.2.2 `tex-svg-full` + lite DOM in QuickJS.
const FULL: &str = include_str!(concat!(
    env!("CERATA_SPIKE_JS"),
    "/mathjax3/package/es5/tex-svg-full.js"
));
const LITE: &str = include_str!(concat!(
    env!("CERATA_SPIKE_JS"),
    "/mathjax3/package/es5/adaptors/liteDOM.js"
));
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    let rt = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&rt).unwrap();
    ctx.with(|ctx| {
        ctx.eval::<(), _>(FULL).ok();
        ctx.eval::<(), _>(LITE).ok();
        ctx.globals().set("__src", source).unwrap();
        let svg: rquickjs::Result<String> = ctx.eval("String(MathJax.tex2svg(__src))");
        println!("{:?}", svg.map(|s| s.len()));
    });
}
