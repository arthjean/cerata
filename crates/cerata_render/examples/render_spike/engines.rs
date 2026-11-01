//! The four candidates of US-031, each `source -> Result<svg, error>`.

use std::path::Path;
use std::sync::OnceLock;

use rquickjs::{Context, Ctx, Runtime};

/// A QuickJS context with ECMAScript intrinsics only: rquickjs ships no
/// `std`/`os` module, no `fetch`, no `require`, and no module loader is
/// installed, so the script can reach neither the network nor the disk. The
/// only input it gets is the global `__src` string.
pub struct Sandbox {
    _runtime: Runtime,
    context: Context,
}

impl Sandbox {
    pub fn new(scripts: &[&str]) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(256 << 20);
        runtime.set_max_stack_size(4 << 20);
        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        context.with(|ctx| -> Result<(), String> {
            // Timers some bundles reach for: run on the next job drain.
            ctx.eval::<(), _>(
                "globalThis.setTimeout = (f, _ms, ...a) => { Promise.resolve().then(() => f(...a)); return 0; };\
                 globalThis.clearTimeout = () => {};",
            )
            .map_err(|e| js_error(&ctx, e))?;
            for script in scripts {
                ctx.eval::<(), _>(*script).map_err(|e| js_error(&ctx, e))?;
            }
            drain(&ctx);
            Ok(())
        })?;
        Ok(Self {
            _runtime: runtime,
            context,
        })
    }

    /// Evaluates `call` with `__src = source`; the script returns a string.
    pub fn call(&self, call: &str, source: &str) -> Result<String, String> {
        self.context.with(|ctx| {
            ctx.globals()
                .set("__src", source)
                .map_err(|e| e.to_string())?;
            let out = ctx.eval::<String, _>(call).map_err(|e| js_error(&ctx, e));
            drain(&ctx);
            out
        })
    }

    /// The globals a hostile script would use to escape, all `undefined`.
    pub fn escape_hatches(&self) -> String {
        self.context.with(|ctx| {
            ctx.eval::<String, _>(
                "['fetch','XMLHttpRequest','require','process','std','os','Deno','Bun','importScripts','WebSocket']\
                 .map(k => k + '=' + typeof globalThis[k]).join(' ')",
            )
            .unwrap_or_default()
        })
    }
}

fn drain(ctx: &Ctx<'_>) {
    while ctx.execute_pending_job() {}
}

fn js_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    if let rquickjs::Error::Exception = error {
        let value = ctx.catch();
        if let Some(object) = value.as_object() {
            let message: Option<String> = object.get("message").ok();
            if let Some(message) = message {
                let stack: Option<String> = object.get("stack").ok();
                return match stack {
                    Some(stack) if std::env::var_os("SPIKE_DEBUG").is_some() => {
                        format!(
                            "{message}
{stack}"
                        )
                    }
                    _ => message,
                };
            }
        }
        return format!("{value:?}");
    }
    error.to_string()
}

// --- Diagrams ---------------------------------------------------------------

pub fn mermaid_svg(source: &str) -> Result<String, String> {
    mermaid_svg::render(source).map_err(|e| e.to_string())
}

pub struct Pele(Sandbox);

impl Pele {
    pub fn new(js_dir: &Path) -> Result<Self, String> {
        let bundle = std::fs::read_to_string(js_dir.join("pele/package/dist/pele.global.js"))
            .map_err(|e| e.to_string())?;
        Ok(Self(Sandbox::new(&[&bundle])?))
    }

    pub fn render(&self, source: &str) -> Result<String, String> {
        self.0.call(
            "pele.render(__src, { responsive: false, links: false, outputLimit: 1000000 }).svg",
            source,
        )
    }

    pub fn sandbox(&self) -> &Sandbox {
        &self.0
    }
}

// --- Formulas ---------------------------------------------------------------

pub struct MathJax(Sandbox);

impl MathJax {
    /// MathJax 3.2.2's `tex-svg-full` component on its lite DOM adaptor
    /// (the headless setup of its Node examples), no browser globals.
    pub fn new(js_dir: &Path) -> Result<Self, String> {
        let es5 = js_dir.join("mathjax3/package/es5");
        let read = |p: &str| std::fs::read_to_string(es5.join(p)).map_err(|e| e.to_string());
        let lite = read("adaptors/liteDOM.js")?;
        let full = read("tex-svg-full.js")?;
        // SRE, the speech engine bundled in `-full`, asks Node's `require`
        // for an XML DOM when it loads: inert stubs, deleted once loaded.
        let config = "globalThis.process = { env: {} };\
            globalThis.require = () => ({ DOMImplementation: function () { this.createDocument = () => ({}); }, XMLSerializer: function () {}, DOMParser: function () {}, install: () => {} });\
            globalThis.MathJax = {\
            loader: { load: [], require: () => ({}) },\
            startup: { typeset: false, adaptor: 'liteAdaptor', document: '<html></html>' },\
            svg: { fontCache: 'none' },\
            tex: { packages: {'[+]': ['color', 'boldsymbol'], '[-]': ['noundefined']}, formatError: (_jax, err) => { throw err; } } };";
        let cleanup = "delete globalThis.process; delete globalThis.require;";
        let sandbox = Sandbox::new(&[config, &full, &lite, cleanup])?;
        Ok(Self(sandbox))
    }

    pub fn render(&self, tex: &str) -> Result<String, String> {
        let out = self.0.call(
            "(() => { const n = MathJax.tex2svg(__src, { display: true });\
              const a = MathJax.startup.adaptor; const h = a.outerHTML(n);\
              if (h.includes('data-mjx-error')) { const m = /data-mjx-error=\"([^\"]*)\"/.exec(h); throw new Error(m ? m[1] : 'error'); }\
              return h; })()",
            tex,
        )?;
        let start = out.find("<svg").ok_or("no svg")?;
        let end = out.rfind("</svg>").ok_or("no svg")? + "</svg>".len();
        Ok(ex_to_px(&out[start..end]))
    }
}

/// MathJax sizes its SVG in `ex`; resvg needs pixels (1ex = 8px at 16px).
fn ex_to_px(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let head_end = svg.find('>').unwrap_or(0);
    let (head, rest) = svg.split_at(head_end);
    let mut tail = head;
    while let Some(i) = tail.find("ex\"") {
        let num_start = tail[..i].rfind('"').map(|q| q + 1).unwrap_or(i);
        let value: f32 = tail[num_start..i].parse().unwrap_or(0.0);
        out.push_str(&tail[..num_start]);
        out.push_str(&format!("{}\"", value * 8.0));
        tail = &tail[i + 3..];
    }
    out.push_str(tail);
    out.push_str(rest);
    out
}

pub struct Typst {
    fonts: Vec<typst::text::Font>,
    book: typst::utils::LazyHash<typst::text::FontBook>,
    library: typst::utils::LazyHash<typst::Library>,
    /// The mitex Typst spec (`specs/**`) and its `xarrow` import, served
    /// from memory: the world reads no file.
    files: Vec<(typst::syntax::FileId, typst::syntax::Source)>,
}

impl Typst {
    pub fn new(js_dir: &Path) -> Result<Self, String> {
        use typst::LibraryExt;
        use typst::syntax::{RootedPath, VirtualPath, VirtualRoot, package::PackageSpec};
        let fonts: Vec<_> = typst_assets::fonts()
            .flat_map(|data| typst::text::Font::iter(typst::foundations::Bytes::new(data)))
            .collect();
        let book = typst::text::FontBook::from_fonts(&fonts);
        let mut files = Vec::new();
        let mut add =
            |root: VirtualRoot, vpath: &str, disk: std::path::PathBuf| -> Result<(), String> {
                // Typst 0.14 dropped `sym.kai`, which the 2024 spec's `\KaTeX`
                // logo still uses: the one line patched for Typst 0.15.
                let text = std::fs::read_to_string(disk)
                    .map_err(|e| e.to_string())?
                    .replace("$kai A T E X$", "\"KaTeX\"");
                let vpath = VirtualPath::new(vpath).map_err(|e| format!("{e:?}"))?;
                let id = RootedPath::new(root, vpath).intern();
                files.push((id, typst::syntax::Source::new(id, text)));
                Ok(())
            };
        let mitex = js_dir.join("mitex-typ");
        for f in [
            "specs/mod.typ",
            "specs/prelude.typ",
            "specs/latex/standard.typ",
        ] {
            add(VirtualRoot::Project, f, mitex.join(f))?;
        }
        let xarrow: PackageSpec = "@preview/xarrow:0.2.0"
            .parse()
            .map_err(|e| format!("{e:?}"))?;
        add(
            VirtualRoot::Package(xarrow.clone()),
            "lib.typ",
            js_dir.join("xarrow-typ/lib.typ"),
        )?;
        add(
            VirtualRoot::Package(xarrow),
            "typst.toml",
            js_dir.join("xarrow-typ/typst.toml"),
        )?;
        Ok(Self {
            fonts,
            book: typst::utils::LazyHash::new(book),
            library: typst::utils::LazyHash::new(typst::Library::default()),
            files,
        })
    }

    /// LaTeX -> Typst math (mitex) -> one auto-sized page -> SVG.
    pub fn render(&self, tex: &str) -> Result<String, String> {
        let math = mitex::convert_math(tex, None)?;
        let literal = math.replace('\\', "\\\\").replace('"', "\\\"");
        let doc = format!(
            "#import \"specs/mod.typ\": mitex-scope\n\
             #set page(width: auto, height: auto, margin: 2pt, fill: none)\n\
             #set text(size: 12pt)\n\
             #math.equation(block: true, eval(\"$\" + \"{literal}\" + \"$\", scope: mitex-scope))"
        );
        let world = World {
            typst: self,
            main: main_id(),
            source: typst::syntax::Source::new(main_id(), doc),
        };
        let compiled = typst::compile::<typst_layout::PagedDocument>(&world).output;
        let document = compiled.map_err(|errors| {
            errors
                .iter()
                .map(|e| e.message.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        let page = document.pages().first().ok_or("empty document")?;
        Ok(typst_svg::svg(page, &Default::default()))
    }
}

fn main_id() -> typst::syntax::FileId {
    static ID: OnceLock<typst::syntax::FileId> = OnceLock::new();
    *ID.get_or_init(|| {
        let vpath = typst::syntax::VirtualPath::new("main.typ").unwrap_or_else(|_| unreachable!());
        typst::syntax::RootedPath::new(typst::syntax::VirtualRoot::Project, vpath).intern()
    })
}

struct World<'a> {
    typst: &'a Typst,
    main: typst::syntax::FileId,
    source: typst::syntax::Source,
}

impl typst::World for World<'_> {
    fn library(&self) -> &typst::utils::LazyHash<typst::Library> {
        &self.typst.library
    }
    fn book(&self) -> &typst::utils::LazyHash<typst::text::FontBook> {
        &self.typst.book
    }
    fn main(&self) -> typst::syntax::FileId {
        self.main
    }
    fn source(&self, id: typst::syntax::FileId) -> typst::diag::FileResult<typst::syntax::Source> {
        if id == self.main {
            return Ok(self.source.clone());
        }
        self.typst
            .files
            .iter()
            .find(|(f, _)| *f == id)
            .map(|(_, s)| s.clone())
            .ok_or(typst::diag::FileError::AccessDenied)
    }
    fn file(
        &self,
        id: typst::syntax::FileId,
    ) -> typst::diag::FileResult<typst::foundations::Bytes> {
        self.typst
            .files
            .iter()
            .find(|(f, _)| *f == id)
            .map(|(_, s)| typst::foundations::Bytes::from_string(s.text().to_string()))
            .ok_or(typst::diag::FileError::AccessDenied)
    }
    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.typst.fonts.get(index).cloned()
    }
    fn today(
        &self,
        _offset: Option<typst::foundations::Duration>,
    ) -> Option<typst::foundations::Datetime> {
        None
    }
}
