//! US-031 binary weight: Pele 1.13.0 in QuickJS, bundle embedded.
const PELE: &str = include_str!(concat!(
    env!("CERATA_SPIKE_JS"),
    "/pele/package/dist/pele.global.js"
));
fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    let rt = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&rt).unwrap();
    ctx.with(|ctx| {
        ctx.eval::<(), _>(PELE).unwrap();
        ctx.globals().set("__src", source).unwrap();
        let svg: rquickjs::Result<String> = ctx.eval("pele.render(__src).svg");
        println!("{:?}", svg.map(|s| s.len()));
    });
}
