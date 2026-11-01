//! Diagrams (Pele) and formulas (MathJax) rendered to SVG in an isolated
//! QuickJS runtime on a thread of its own (`docs/adr/0005`).
//!
//! The contexts hold the ECMAScript intrinsics only: rquickjs ships no
//! `std` or `os` module, no `fetch` and no `require`, and no module loader
//! is installed, so a script reaches neither the network nor the disk. Its
//! one input is the global `__src` string, its one output an SVG string.
//! A render running past [`DEADLINE`] is interrupted and reported as an
//! error (US-036); the engine it ran in is then rebuilt.
//!
//! Results, errors included, are kept in two LRU caches shared by every
//! editor: 200 formulas (`math-renderer.ts`) and 50 diagrams
//! (`mermaid-renderer.ts:22`).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Runtime};

const PELE: &str = include_str!("../assets/js/pele.global.js");
const MATHJAX: &str = include_str!("../assets/js/mathjax-tex-svg-full.js");
const MATHJAX_LITE_DOM: &str = include_str!("../assets/js/mathjax-liteDOM.js");

/// A render past this is abandoned (US-036).
pub const DEADLINE: Duration = Duration::from_millis(500);
pub const MATH_CACHE: usize = 200;
pub const DIAGRAM_CACHE: usize = 50;

/// A rendered SVG and its size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Svg {
    pub data: Arc<str>,
    pub width: f32,
    pub height: f32,
    /// How far below the text baseline the image's bottom sits (MathJax's
    /// `vertical-align`), in pixels; 0 for diagrams.
    pub descent: f32,
}

/// A render: its SVG, or the engine's error message.
pub type Rendered = Result<Arc<Svg>, Arc<str>>;

/// A formula to render.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MathRequest {
    pub tex: String,
    pub display: bool,
    /// The body font size: MathJax sizes in `ex`, half of it.
    pub font_px: u32,
    /// The text color, `#rrggbb`.
    pub color: String,
}

/// The theme a diagram is drawn in: Pele's CSS variables
/// (`mermaid-canvas.css:22-53`), each `(name without --pele-, #rrggbb)`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DiagramTheme {
    pub vars: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DiagramRequest {
    pub source: String,
    pub theme: DiagramTheme,
    /// CSS font lists for labels and code spans; empty leaves Pele's.
    pub font: String,
    pub mono: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Math(MathRequest),
    Diagram(DiagramRequest),
}

type Reply = Box<dyn FnOnce(Rendered) + Send>;

struct Job {
    key: Key,
    reply: Reply,
}

/// A small LRU: the least recently used entry goes first.
struct Lru {
    capacity: usize,
    tick: u64,
    entries: HashMap<Key, (Rendered, u64)>,
}

impl Lru {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            tick: 0,
            entries: HashMap::new(),
        }
    }

    fn get(&mut self, key: &Key) -> Option<Rendered> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|(value, used)| {
            *used = tick;
            value.clone()
        })
    }

    fn put(&mut self, key: Key, value: Rendered) {
        self.tick += 1;
        if self.entries.len() >= self.capacity
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (value, self.tick));
    }
}

struct Caches {
    math: Lru,
    diagrams: Lru,
}

impl Caches {
    fn of(&mut self, key: &Key) -> &mut Lru {
        match key {
            Key::Math(_) => &mut self.math,
            Key::Diagram(_) => &mut self.diagrams,
        }
    }
}

/// The renderer every editor shares.
pub struct Renderer {
    jobs: Mutex<Sender<Job>>,
    caches: Arc<Mutex<Caches>>,
}

impl Renderer {
    /// The process-wide renderer; its thread starts at the first call.
    pub fn global() -> &'static Renderer {
        static RENDERER: OnceLock<Renderer> = OnceLock::new();
        RENDERER.get_or_init(Renderer::spawn)
    }

    fn spawn() -> Renderer {
        let (tx, rx) = mpsc::channel();
        let caches = Arc::new(Mutex::new(Caches {
            math: Lru::new(MATH_CACHE),
            diagrams: Lru::new(DIAGRAM_CACHE),
        }));
        let thread_caches = caches.clone();
        let spawned = std::thread::Builder::new()
            .name("cerata-js".into())
            // MathJax recurses deeply; QuickJS's own limit is 4 MB.
            .stack_size(16 << 20)
            .spawn(move || run(rx, thread_caches));
        if let Err(error) = spawned {
            log::error!("the diagram and formula thread did not start: {error}");
        }
        Renderer {
            jobs: Mutex::new(tx),
            caches,
        }
    }

    fn cached(&self, key: &Key) -> Option<Rendered> {
        self.caches.lock().ok()?.of(key).get(key)
    }

    pub fn cached_math(&self, request: &MathRequest) -> Option<Rendered> {
        self.cached(&Key::Math(request.clone()))
    }

    pub fn cached_diagram(&self, request: &DiagramRequest) -> Option<Rendered> {
        self.cached(&Key::Diagram(request.clone()))
    }

    fn request(&self, key: Key, reply: Reply) {
        if let Some(hit) = self.cached(&key) {
            reply(hit);
            return;
        }
        let sent = self
            .jobs
            .lock()
            .ok()
            .and_then(|jobs| jobs.send(Job { key, reply }).err());
        if let Some(mpsc::SendError(job)) = sent {
            (job.reply)(Err("The renderer is not running".into()));
        }
    }

    /// Renders `request` off the calling thread; `reply` runs on the
    /// renderer's thread (or at once on a cache hit).
    pub fn math(&self, request: MathRequest, reply: impl FnOnce(Rendered) + Send + 'static) {
        self.request(Key::Math(request), Box::new(reply));
    }

    pub fn diagram(&self, request: DiagramRequest, reply: impl FnOnce(Rendered) + Send + 'static) {
        self.request(Key::Diagram(request), Box::new(reply));
    }

    /// [`Self::math`], waiting for the result.
    pub fn math_blocking(&self, request: MathRequest) -> Rendered {
        let (tx, rx) = mpsc::channel();
        self.math(request, move |r| {
            let _ = tx.send(r);
        });
        rx.recv()
            .unwrap_or_else(|_| Err("The renderer stopped".into()))
    }

    /// [`Self::diagram`], waiting for the result.
    pub fn diagram_blocking(&self, request: DiagramRequest) -> Rendered {
        let (tx, rx) = mpsc::channel();
        self.diagram(request, move |r| {
            let _ = tx.send(r);
        });
        rx.recv()
            .unwrap_or_else(|_| Err("The renderer stopped".into()))
    }
}

// --- The renderer's thread ---------------------------------------------------

/// The runtime both engines share, and the deadline its interrupt handler
/// enforces (nanoseconds since `epoch`, 0 for none).
struct Engines {
    runtime: Runtime,
    epoch: Instant,
    deadline: Arc<AtomicU64>,
    pele: Option<Context>,
    mathjax: Option<Context>,
}

fn run(rx: Receiver<Job>, caches: Arc<Mutex<Caches>>) {
    let mut engines = match Engines::new() {
        Ok(engines) => engines,
        Err(error) => {
            log::error!("the JavaScript runtime did not start: {error}");
            for job in rx {
                (job.reply)(Err(error.clone().into()));
            }
            return;
        }
    };
    for job in rx {
        // Asked twice before the first answer: the cache has it now.
        let cached = caches
            .lock()
            .ok()
            .and_then(|mut c| c.of(&job.key).get(&job.key));
        let result = match cached {
            Some(hit) => hit,
            None => {
                let result = engines.render(&job.key);
                if let Ok(mut c) = caches.lock() {
                    c.of(&job.key).put(job.key.clone(), result.clone());
                }
                result
            }
        };
        (job.reply)(result);
    }
}

impl Engines {
    fn new() -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(256 << 20);
        runtime.set_max_stack_size(4 << 20);
        let epoch = Instant::now();
        let deadline = Arc::new(AtomicU64::new(0));
        let watched = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let at = watched.load(Ordering::Relaxed);
            at != 0 && epoch.elapsed().as_nanos() as u64 > at
        })));
        Ok(Self {
            runtime,
            epoch,
            deadline,
            pele: None,
            mathjax: None,
        })
    }

    fn context(&self, scripts: &[&str]) -> Result<Context, String> {
        let context = Context::full(&self.runtime).map_err(|e| e.to_string())?;
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
        Ok(context)
    }

    fn pele(&mut self) -> Result<&Context, String> {
        if self.pele.is_none() {
            self.pele = Some(self.context(&[PELE])?);
        }
        self.pele
            .as_ref()
            .ok_or_else(|| "Pele is not loaded".to_owned())
    }

    /// MathJax 3.2.2's `tex-svg-full` on its lite DOM adaptor (its Node
    /// setup), no browser globals. SRE, its speech engine, asks Node's
    /// `require` for an XML DOM when it loads: inert stubs, deleted once
    /// loaded. Invalid TeX throws instead of rendering in red.
    fn mathjax(&mut self) -> Result<&Context, String> {
        if self.mathjax.is_none() {
            let config = "globalThis.process = { env: {} };\
                globalThis.require = () => ({ DOMImplementation: function () { this.createDocument = () => ({}); }, XMLSerializer: function () {}, DOMParser: function () {}, install: () => {} });\
                globalThis.MathJax = {\
                loader: { load: [], require: () => ({}) },\
                startup: { typeset: false, adaptor: 'liteAdaptor', document: '<html></html>' },\
                svg: { fontCache: 'none' },\
                tex: { packages: {'[+]': ['color', 'boldsymbol'], '[-]': ['noundefined']}, formatError: (_jax, err) => { throw err; } } };";
            let cleanup = "delete globalThis.process; delete globalThis.require;";
            self.mathjax = Some(self.context(&[config, MATHJAX, MATHJAX_LITE_DOM, cleanup])?);
        }
        self.mathjax
            .as_ref()
            .ok_or_else(|| "MathJax is not loaded".to_owned())
    }

    /// Evaluates `call` with `__src = source` before the deadline.
    /// Evaluates `call` with each `(name, value)` set as a string global.
    fn call(
        &self,
        context: &Context,
        call: &str,
        globals: &[(&str, &str)],
    ) -> Result<String, String> {
        let at = self.epoch.elapsed() + DEADLINE;
        self.deadline.store(at.as_nanos() as u64, Ordering::Relaxed);
        let out = context.with(|ctx| {
            for (name, value) in globals {
                ctx.globals()
                    .set(*name, *value)
                    .map_err(|e| e.to_string())?;
            }
            let out = ctx.eval::<String, _>(call).map_err(|e| js_error(&ctx, e));
            drain(&ctx);
            out
        });
        let late = self.epoch.elapsed() > at;
        self.deadline.store(0, Ordering::Relaxed);
        if late && out.is_err() {
            return Err(TIMED_OUT.to_owned());
        }
        out
    }

    fn render(&mut self, key: &Key) -> Rendered {
        let result = match key {
            Key::Math(request) => self.render_math(request),
            Key::Diagram(request) => self.render_diagram(request),
        };
        if result.as_ref().is_err_and(|e| e == TIMED_OUT) {
            // An interrupted engine may be left half-way: built again.
            match key {
                Key::Math(_) => self.mathjax = None,
                Key::Diagram(_) => self.pele = None,
            }
        }
        result.map(Arc::new).map_err(Arc::from)
    }

    fn render_math(&mut self, request: &MathRequest) -> Result<Svg, String> {
        let call = if request.display {
            "(() => { const n = MathJax.tex2svg(__src, { display: true });"
        } else {
            "(() => { const n = MathJax.tex2svg(__src, { display: false });"
        };
        let call = format!(
            "{call} const a = MathJax.startup.adaptor; const h = a.outerHTML(n);\
             if (h.includes('data-mjx-error')) {{ const m = /data-mjx-error=\"([^\"]*)\"/.exec(h); throw new Error(m ? m[1] : 'error'); }}\
             return h; }})()"
        );
        let context = self.mathjax()?.clone();
        let out = self.call(&context, &call, &[("__src", &request.tex)])?;
        let start = out.find("<svg").ok_or("MathJax returned no SVG")?;
        let end = out.rfind("</svg>").ok_or("MathJax returned no SVG")? + "</svg>".len();
        let ex = request.font_px as f32 / 2.0;
        let svg = math_svg(&out[start..end], ex, &request.color);
        inert(&svg.data)?;
        Ok(svg)
    }

    fn render_diagram(&mut self, request: &DiagramRequest) -> Result<Svg, String> {
        let context = self.pele()?.clone();
        let id = format!("pele-{:x}", hash_of(&request.source) as u32);
        // `images: false`: an `<image href>` would be read by GPUI's SVG
        // renderer from any path, outside the workspace too. A throw that
        // is not Pele's own error is a renderer bug (`mermaid-renderer.ts`).
        let svg = self.call(
            &context,
            "(() => { try { return pele.render(__src, { responsive: false, links: false, images: false,              outputLimit: 1000000, idPrefix: __id,              fontFamily: __font || undefined, fontFamilyMono: __mono || undefined }).svg; }              catch (e) { if (e instanceof pele.PeleError) throw e;              throw new Error('Unexpected error: ' + (e instanceof Error ? e.message : String(e))); } })()",
            &[
                ("__src", &request.source),
                ("__font", &request.font),
                ("__mono", &request.mono),
                ("__id", &id),
            ],
        )
        .inspect_err(|e| {
            if e.starts_with(UNEXPECTED) {
                log::error!("a Mermaid diagram failed to render: {e}");
            }
        })?;
        let themed = root_font(&theme_svg(&svg, &request.theme), &request.font);
        inert(&themed)?;
        let (width, height) = root_size(&themed).ok_or("Pele returned no size")?;
        Ok(Svg {
            data: themed.into(),
            width,
            height,
            descent: 0.0,
        })
    }
}

const TIMED_OUT: &str = "The render took longer than 500 ms";
const UNEXPECTED: &str = "Unexpected error: ";

/// Pele's color variables (`--pele-*` without the prefix) its diagrams
/// read: a theme sets each, or that part keeps Pele's light default
/// (`mermaid.test.ts:243-275`). Fonts, weights, radius and the error color
/// are left to Pele.
pub const PELE_COLOR_VARS: &[&str] = &[
    "bg",
    "fg",
    "muted",
    "line",
    "surface",
    "surface-alt",
    "border",
    "accent",
    "critical",
    "series-1",
    "series-2",
    "series-3",
    "series-4",
    "series-5",
    "series-6",
    "series-7",
    "series-8",
];

/// An engine's SVG goes to GPUI (resvg) only when nothing in it can act or
/// reach outside it: no script, no `foreignObject`, no event handler, no
/// `javascript:`, and every `href` a fragment or an inline raster image
/// (`mermaid.test.ts:215`, DOMPurify's role in Writer). Pele and MathJax
/// emit none; a change in a bundle that did would show an error, not run.
pub fn inert(svg: &str) -> Result<(), String> {
    const REFUSED: &str = "The diagram holds content Cerata does not draw";
    let lower = svg.to_ascii_lowercase();
    if ["<script", "<foreignobject", "javascript:", "<!entity"]
        .iter()
        .any(|t| lower.contains(t))
    {
        return Err(REFUSED.to_owned());
    }
    let mut rest = lower.as_str();
    while let Some(open) = rest.find('<') {
        let tag = &rest[open + 1..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag_text = &tag[..end];
        rest = &tag[end..];
        let mut attrs = tag_text;
        while let Some(eq) = attrs.find('=') {
            let name = attrs[..eq]
                .trim_end()
                .rsplit(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("");
            let value = attrs[eq + 1..].trim_start();
            let (value, after) = match value.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &value[1..];
                    let close = body.find(q).unwrap_or(body.len());
                    (&body[..close], &body[(close + 1).min(body.len())..])
                }
                _ => {
                    let stop = value.find(char::is_whitespace).unwrap_or(value.len());
                    (&value[..stop], &value[stop..])
                }
            };
            if name.starts_with("on") && name.len() > 2 {
                return Err(REFUSED.to_owned());
            }
            if (name == "href" || name.ends_with(":href"))
                && !value.starts_with('#')
                && !value.starts_with("data:image/")
            {
                return Err(REFUSED.to_owned());
            }
            attrs = after;
        }
    }
    Ok(())
}

fn drain(ctx: &Ctx<'_>) {
    while ctx.execute_pending_job() {}
}

fn js_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    if let rquickjs::Error::Exception = error {
        let value = ctx.catch();
        if let Some(message) = value
            .as_object()
            .and_then(|o| o.get::<_, Option<String>>("message").ok().flatten())
        {
            return message;
        }
        return value
            .as_string()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| "The script failed".to_owned());
    }
    error.to_string()
}

// --- SVG post-processing -------------------------------------------------------

/// The value of attribute `name` in the `<svg ...>` head.
fn root_attr<'a>(svg: &'a str, name: &str) -> Option<&'a str> {
    let head = &svg[..svg.find('>')?];
    let needle = format!(" {name}=\"");
    let start = head.find(&needle)? + needle.len();
    let len = head[start..].find('"')?;
    Some(&head[start..start + len])
}

/// Pele sets the labels' face in the root's `style`, beside custom
/// properties and the `font` shorthand, which resvg does not apply: the
/// face is repeated as a presentation attribute the labels inherit.
fn root_font(svg: &str, font: &str) -> String {
    match svg.strip_prefix("<svg") {
        Some(rest) if !font.is_empty() => {
            let font = font
                .replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('<', "&lt;");
            format!("<svg font-family=\"{font}\"{rest}")
        }
        _ => svg.to_owned(),
    }
}

fn root_size(svg: &str) -> Option<(f32, f32)> {
    let px = |v: &str| v.trim_end_matches("px").parse::<f32>().ok();
    Some((
        px(root_attr(svg, "width")?)?,
        px(root_attr(svg, "height")?)?,
    ))
}

/// MathJax sizes in `ex` and paints in `currentColor`: pixels and the text
/// color, which resvg understands.
fn math_svg(svg: &str, ex: f32, color: &str) -> Svg {
    let ex_value = |v: Option<&str>| {
        v.and_then(|v| v.trim_end_matches("ex").parse::<f32>().ok())
            .unwrap_or(0.0)
    };
    let width = ex_value(root_attr(svg, "width")) * ex;
    let height = ex_value(root_attr(svg, "height")) * ex;
    let descent = root_attr(svg, "style")
        .and_then(|s| s.split("vertical-align:").nth(1))
        .map(|v| -ex_value(Some(v.trim().trim_end_matches(';').trim())) * ex)
        .unwrap_or(0.0);
    let head_end = svg.find('>').unwrap_or(0);
    let (head, rest) = svg.split_at(head_end);
    let head = replace_attr(head, "width", &format!("{width}"));
    let head = replace_attr(&head, "height", &format!("{height}"));
    let data = format!("{head}{rest}").replace("currentColor", color);
    Svg {
        data: data.into(),
        width,
        height,
        descent,
    }
}

fn replace_attr(head: &str, name: &str, value: &str) -> String {
    let needle = format!(" {name}=\"");
    let Some(start) = head.find(&needle).map(|i| i + needle.len()) else {
        return head.to_owned();
    };
    let Some(len) = head[start..].find('"') else {
        return head.to_owned();
    };
    format!("{}{value}{}", &head[..start], &head[start + len..])
}

/// Pele paints with CSS variables, which resvg does not resolve: the theme's
/// `--pele-*` values are declared on the root, then every `var()` is
/// replaced by its value (or its fallback).
pub fn theme_svg(svg: &str, theme: &DiagramTheme) -> String {
    let declared: String = theme
        .vars
        .iter()
        .map(|(name, value)| format!("--pele-{name}:{value};"))
        .collect();
    let mut svg = match svg.find(" style=\"") {
        Some(i) if i < svg.find('>').unwrap_or(0) => {
            let at = i + " style=\"".len();
            format!("{}{declared}{}", &svg[..at], &svg[at..])
        }
        _ => svg.replacen("<svg", &format!("<svg style=\"{declared}\""), 1),
    };
    let vars = declarations(&svg);
    for _ in 0..8 {
        let next = substitute(&svg, &vars);
        if next == svg {
            break;
        }
        svg = next;
    }
    svg
}

/// `--name:value` declarations; the first one of a name wins (the theme's,
/// declared first).
fn declarations(svg: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let mut rest = svg;
    while let Some(i) = rest.find("--") {
        let tail = &rest[i + 2..];
        let name_end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        let name = &tail[..name_end];
        if let Some(after) = tail[name_end..].strip_prefix(':') {
            let end = value_end(after);
            vars.entry(name.to_owned())
                .or_insert_with(|| after[..end].trim().to_owned());
            rest = &after[end..];
        } else {
            rest = &tail[name_end..];
        }
    }
    vars
}

/// The end of a declared value: its `;` or the attribute's `"`, outside
/// parentheses.
fn value_end(text: &str) -> usize {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ';' | '"' | '}' if depth == 0 => return i,
            _ => {}
        }
    }
    text.len()
}

/// One pass replacing each `var(--name[, fallback])` by its value.
fn substitute(svg: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find("var(--") {
        out.push_str(&rest[..i]);
        let inner_start = i + "var(".len();
        let mut depth = 1usize;
        let mut close = None;
        for (j, c) in rest[inner_start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(inner_start + j);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &rest[inner_start..close];
        let (name, fallback) = match inner.find(',') {
            Some(comma) => (&inner[2..comma], Some(inner[comma + 1..].trim())),
            None => (&inner[2..], None),
        };
        let value = vars
            .get(name.trim())
            .map(String::as_str)
            .or(fallback)
            .unwrap_or("currentColor");
        out.push_str(value);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A stable hash, for callers that key their own state by a request.
pub fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn math(tex: &str) -> MathRequest {
        MathRequest {
            tex: tex.into(),
            display: false,
            font_px: 16,
            color: "#112233".into(),
        }
    }

    #[test]
    fn formulas_render_with_the_text_color_and_errors_are_messages() {
        let renderer = Renderer::global();
        let svg = renderer.math_blocking(math("\\frac{a}{b}")).unwrap();
        assert!(svg.data.starts_with("<svg"));
        assert!(svg.width > 0.0 && svg.height > 0.0);
        assert!(svg.data.contains("#112233") && !svg.data.contains("currentColor"));
        let error = renderer.math_blocking(math("\\frac{a")).unwrap_err();
        assert!(error.contains("Missing close brace"), "{error}");
        // Both are cached.
        assert!(renderer.cached_math(&math("\\frac{a")).is_some());
    }

    /// `math.test.ts:86`: one formula inline and as a block are two renders,
    /// each kept in the cache.
    #[test]
    fn inline_and_display_renders_are_cached_apart() {
        let renderer = Renderer::global();
        let inline = math("\\sum_{i=1}^n i");
        let display = MathRequest {
            display: true,
            ..inline.clone()
        };
        let a = renderer.math_blocking(inline.clone()).unwrap();
        let b = renderer.math_blocking(display.clone()).unwrap();
        assert_ne!(a.data, b.data);
        let cached = |r: &MathRequest| match renderer.cached_math(r) {
            Some(Ok(svg)) => svg.data.clone(),
            other => panic!("not cached: {other:?}"),
        };
        assert_eq!(cached(&inline), a.data);
        assert_eq!(cached(&display), b.data);
    }

    #[test]
    fn diagrams_render_in_the_theme_colors_without_variables() {
        let renderer = Renderer::global();
        let request = DiagramRequest {
            source: "flowchart LR\n  A --> B".into(),
            theme: DiagramTheme {
                vars: vec![
                    ("bg".into(), "#010203".into()),
                    ("fg".into(), "#a1a2a3".into()),
                ],
            },
            font: "Test Sans".into(),
            mono: String::new(),
        };
        let svg = renderer.diagram_blocking(request).unwrap();
        assert!(
            svg.data.starts_with("<svg font-family=\"Test Sans\""),
            "the labels use the given font"
        );
        assert!(
            !svg.data.contains("var("),
            "{}",
            &svg.data[..400.min(svg.data.len())]
        );
        assert!(svg.data.contains("#a1a2a3"));
        assert!(svg.width > 0.0);
        let error = renderer
            .diagram_blocking(DiagramRequest {
                source: "flowchart LR\n  A -->".into(),
                theme: DiagramTheme::default(),
                ..DiagramRequest::default()
            })
            .unwrap_err();
        assert!(!error.is_empty());
    }

    /// A diagram never names a file or a URL for GPUI's SVG renderer to
    /// load: usvg's default resolver reads any `href`, outside the
    /// workspace and on network shares too.
    #[test]
    fn diagram_images_are_never_emitted() {
        let renderer = Renderer::global();
        for img in [
            "../../secret.png",
            "C:/secret.png",
            "https://example.com/a.png",
        ] {
            let svg = renderer
                .diagram_blocking(DiagramRequest {
                    source: format!("flowchart LR\n  A@{{ img: \"{img}\", h: 60 }} --> B"),
                    ..DiagramRequest::default()
                })
                .unwrap();
            assert!(!svg.data.contains("<image"), "{img}: {}", svg.data);
            assert!(!svg.data.contains("secret") && !svg.data.contains("example.com"));
        }
    }

    #[test]
    fn a_runaway_render_is_interrupted_at_the_deadline() {
        let mut engines = Engines::new().unwrap();
        let context = engines.context(&[]).unwrap();
        let started = Instant::now();
        let out = engines.call(&context, "while (true) {}", &[]);
        assert_eq!(out, Err(TIMED_OUT.to_owned()));
        assert!(started.elapsed() < DEADLINE * 3);
        // The runtime still serves the next render.
        assert_eq!(
            engines.call(&context, "__src + '!'", &[("__src", "ok")]),
            Ok("ok!".into())
        );
        let _ = &mut engines.pele;
    }

    /// `mermaid.test.ts:21-44`: one sample per diagram type Writer's notes
    /// hold, keyed by the class Pele puts on the root.
    const DIAGRAMS: &[(&str, &str)] = &[
        (
            "flowchart",
            "flowchart LR\n  A[Draft] --> B{Ready?}\n  B -- Yes --> C[Publish]\n  subgraph S\n    C\n  end",
        ),
        (
            "sequence",
            "sequenceDiagram\n  A->>B: Save\n  B-->>A: OK\n  Note over A,B: local",
        ),
        (
            "class",
            "classDiagram\n  class Note {\n    +String title\n    +save() bool\n  }\n  Note <|-- Daily",
        ),
        (
            "state",
            "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Editing : key\n  Editing --> [*]",
        ),
        (
            "er",
            "erDiagram\n  NOTE ||--o{ TAG : has\n  NOTE {\n    string path PK\n  }",
        ),
        (
            "gantt",
            "gantt\n  dateFormat YYYY-MM-DD\n  section A\n    Task :done, a1, 2026-10-01, 3d\n    Next :crit, after a1, 1d",
        ),
        ("pie", "pie\n  \"A\" : 3\n  \"B\" : 5"),
        (
            "journey",
            "journey\n  title J\n  section S\n    Step: 5: Me",
        ),
        (
            "gitGraph",
            "gitGraph\n  commit\n  branch f\n  commit\n  checkout main\n  merge f",
        ),
        ("mindmap", "mindmap\n  root((W))\n    A\n    B"),
        ("timeline", "timeline\n  2026 : One\n  2027 : Two"),
        (
            "quadrantChart",
            "quadrantChart\n  x-axis Low --> High\n  y-axis Low --> High\n  P: [0.3, 0.8]",
        ),
        (
            "xychart",
            "xychart-beta\n  x-axis [a, b]\n  bar [1, 2]\n  line [2, 1]",
        ),
        ("sankey", "sankey-beta\nA,B,5\nA,C,3"),
        ("block", "block-beta\n  columns 2\n  A B"),
        (
            "architecture",
            "architecture-beta\n  service a(server)[A]\n  service b(disk)[B]\n  a:R --> L:b",
        ),
        ("kanban", "kanban\n  todo[To do]\n    t1[One]"),
        (
            "requirement",
            "requirementDiagram\n  requirement r {\n    id: 1\n    text: t\n  }\n  element e {\n    type: lib\n  }\n  e - satisfies -> r",
        ),
        (
            "c4",
            "C4Context\n  Person(p, \"P\")\n  System(s, \"S\")\n  Rel(p, s, \"uses\")",
        ),
        (
            "packet",
            "packet-beta\n  0-15: \"Source\"\n  16-31: \"Dest\"",
        ),
        (
            "radar",
            "radar-beta\n  axis a, b, c\n  curve x[\"X\"]{1, 2, 3}",
        ),
        ("treemap", "treemap-beta\n\"Root\"\n  \"A\": 1\n  \"B\": 2"),
    ];

    fn diagram(source: &str) -> Rendered {
        Renderer::global().diagram_blocking(DiagramRequest {
            source: source.into(),
            ..DiagramRequest::default()
        })
    }

    fn head(svg: &str) -> &str {
        &svg[..svg.find('>').unwrap()]
    }

    /// `mermaid.test.ts:72-85`: every type renders through Pele to a fixed
    /// size SVG the canvas scales, and passes the inert check.
    #[test]
    fn every_diagram_type_renders_at_a_fixed_size() {
        for (kind, source) in DIAGRAMS {
            let svg = diagram(source).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(
                head(&svg.data).contains(&format!("class=\"pele pele-{kind}\"")),
                "{kind}: {}",
                head(&svg.data)
            );
            assert!(svg.width > 0.0 && svg.height > 0.0, "{kind}");
            assert!(!svg.data.contains("max-width:100%"), "{kind}");
            assert!(inert(&svg.data).is_ok(), "{kind}");
        }
        let svg = diagram(DIAGRAMS[0].1).unwrap();
        assert_eq!(
            root_attr(&svg.data, "width"),
            Some(&*format!("{}", svg.width))
        );
        assert_eq!(
            root_attr(&svg.data, "viewBox"),
            Some(&*format!("0 0 {} {}", svg.width, svg.height))
        );
    }

    fn same(a: &Rendered, b: &Rendered) -> bool {
        match (a, b) {
            (Ok(a), Ok(b)) => Arc::ptr_eq(a, b),
            (Err(a), Err(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// `mermaid.test.ts:87-92` and `:139-148`: a source is rendered once;
    /// its diagram or its error is then served from the cache.
    #[test]
    fn diagrams_and_errors_are_cached() {
        let renderer = Renderer::global();
        for source in [
            "flowchart LR\n  cached --> once",
            "flowchart LR\n  cached -->",
        ] {
            let first = diagram(source);
            let request = DiagramRequest {
                source: source.into(),
                ..DiagramRequest::default()
            };
            let hit = renderer.cached_diagram(&request).expect("cached");
            assert!(same(&first, &hit), "{source}");
            assert!(same(&first, &diagram(source)), "{source}");
        }
    }

    /// `mermaid.test.ts:94-104`: another UI font is another render, in it.
    #[test]
    fn a_new_font_renders_again_in_that_font() {
        let renderer = Renderer::global();
        let render = |font: &str| {
            renderer
                .diagram_blocking(DiagramRequest {
                    source: DIAGRAMS[0].1.into(),
                    font: font.into(),
                    ..DiagramRequest::default()
                })
                .unwrap()
        };
        let (avenir, georgia) = (render("Avenir, sans-serif"), render("Georgia, serif"));
        assert!(!Arc::ptr_eq(&avenir, &georgia));
        assert!(head(&avenir.data).contains("font-family=\"Avenir, sans-serif\""));
        assert!(head(&georgia.data).contains("font-family=\"Georgia, serif\""));
    }

    /// `mermaid.test.ts:106-118` and `:150-153`: Pele's own messages.
    #[test]
    fn pele_errors_name_the_problem() {
        let error = diagram("flowchart TD\n  A --> B -->\n  C[[[ broken").unwrap_err();
        assert!(error.contains("Parse error on line 3"), "{error}");
        assert!(error.contains("C[[[ broken"), "{error}");
        assert_eq!(
            &*diagram("zenuml\n  Alice->Bob: hi").unwrap_err(),
            "No diagram type detected for the given text."
        );
        let huge = format!("flowchart TD\n{}", "  A --> B\n".repeat(6000));
        let error = diagram(&huge).unwrap_err();
        assert!(error.contains("limit"), "{error}");
    }

    /// `mermaid.test.ts:120-137`: a throw that is not Pele's error is a
    /// renderer bug, reported as unexpected with its message or value.
    #[test]
    fn a_renderer_bug_is_reported_as_unexpected() {
        for (throw, message) in [
            (
                "throw new TypeError('x is undefined')",
                "Unexpected error: x is undefined",
            ),
            ("throw 'string error'", "Unexpected error: string error"),
        ] {
            let mut engines = Engines::new().unwrap();
            let context = engines.pele().unwrap().clone();
            engines
                .call(
                    &context,
                    &format!("pele = {{ ...pele, render: () => {{ {throw}; }} }}; ''"),
                    &[],
                )
                .unwrap();
            let request = DiagramRequest {
                source: "flowchart TD\n  A --> B".into(),
                ..DiagramRequest::default()
            };
            assert_eq!(engines.render_diagram(&request), Err(message.to_owned()));
        }
    }

    /// `mermaid.test.ts:155-185`: links stay inert text, HTML never reaches
    /// a label, and two diagrams never share an `id`.
    #[test]
    fn diagram_links_html_and_ids_are_inert() {
        let svg = diagram(
            "flowchart LR\n  A --> B\n  click A \"https://example.com\" \"Open\"\n  click B \"javascript:alert(1)\"",
        )
        .unwrap();
        for absent in ["<a", "example.com", "javascript:"] {
            assert!(!svg.data.contains(absent), "{absent}");
        }
        let svg = diagram("flowchart LR\n  A[\"<img src=x onerror=alert(1)>\"] --> B").unwrap();
        assert!(!svg.data.contains("<img") && !svg.data.contains("onerror"));
        let ids = |svg: &str| -> Vec<String> {
            svg.split(" id=\"")
                .skip(1)
                .filter_map(|s| s.split('"').next().map(str::to_owned))
                .collect()
        };
        let a = ids(&diagram("flowchart LR\n  accTitle: First\n  A --> B")
            .unwrap()
            .data);
        let b = ids(&diagram("flowchart LR\n  accTitle: Second\n  A --> B")
            .unwrap()
            .data);
        assert!(!a.is_empty());
        assert!(a.iter().all(|id| !b.contains(id)), "{a:?} {b:?}");
    }

    /// `mermaid.test.ts:215-226`: scripts, event handlers, `javascript:`,
    /// `foreignObject` and outside references never reach the renderer.
    #[test]
    fn only_inert_svg_is_drawn() {
        assert!(
            inert(
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"1\" height=\"1\"/><use href=\"#g\"/></svg>"
            )
            .is_ok()
        );
        for hostile in [
            "<svg><script>alert(1)</script></svg>",
            "<svg><rect onclick=\"alert(2)\" width=\"1\"/></svg>",
            "<svg><rect ONLOAD='x'/></svg>",
            "<svg><a href=\"javascript:alert(3)\"><text>x</text></a></svg>",
            "<svg><foreignObject><div>y</div></foreignObject></svg>",
            "<svg><image href=\"../secret.png\"/></svg>",
            "<svg><image xlink:href='file:///etc/passwd'/></svg>",
            "<svg><use href=\"https://example.com/a.svg#x\"/></svg>",
        ] {
            assert!(inert(hostile).is_err(), "{hostile}");
        }
    }

    /// `mermaid.test.ts:243-275`: every color variable Pele's diagrams read
    /// is in [`PELE_COLOR_VARS`], which the app's theme sets.
    #[test]
    fn every_color_variable_pele_reads_is_listed() {
        const UNMAPPED: &[&str] = &[
            "font",
            "font-mono",
            "title-weight",
            "heading-weight",
            "bold-weight",
            "radius",
            "error",
        ];
        let mut engines = Engines::new().unwrap();
        let context = engines.pele().unwrap().clone();
        let mut used = std::collections::BTreeSet::new();
        for (_, source) in DIAGRAMS {
            let svg = engines
                .call(
                    &context,
                    "pele.render(__src, { responsive: false, links: false }).svg",
                    &[("__src", source)],
                )
                .unwrap();
            for part in svg.split("var(--pele-").skip(1) {
                let name: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                used.insert(name);
            }
        }
        assert!(used.len() > 8, "{used:?}");
        let missing: Vec<_> = used
            .iter()
            .filter(|v| !UNMAPPED.contains(&v.as_str()) && !PELE_COLOR_VARS.contains(&v.as_str()))
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn the_sandbox_has_no_way_out() {
        let engines = Engines::new().unwrap();
        let context = engines.context(&[]).unwrap();
        let out = engines
            .call(
                &context,
                "['fetch','XMLHttpRequest','require','process','std','os','Deno','Bun','importScripts','WebSocket']\
                 .map(k => typeof globalThis[k]).join(' ')",
                &[],
            )
            .unwrap();
        assert!(out.split(' ').all(|t| t == "undefined"), "{out}");
    }

    #[test]
    fn the_lru_drops_the_least_recently_used() {
        let mut lru = Lru::new(2);
        let key = |s: &str| Key::Math(math(s));
        let value: Rendered = Err("x".into());
        lru.put(key("a"), value.clone());
        lru.put(key("b"), value.clone());
        assert!(lru.get(&key("a")).is_some());
        lru.put(key("c"), value);
        assert!(lru.get(&key("b")).is_none());
        assert!(lru.get(&key("a")).is_some());
    }

    #[test]
    fn variables_resolve_to_the_theme_then_their_fallbacks() {
        let svg = "<svg width=\"10\" height=\"5\" style=\"--_bg:var(--pele-bg,#fff);--_fg:var(--pele-fg, var(--x, #000))\"><rect fill=\"var(--_bg)\" stroke=\"var(--_fg)\"/></svg>";
        let theme = DiagramTheme {
            vars: vec![("bg".into(), "#123456".into())],
        };
        let out = theme_svg(svg, &theme);
        assert!(out.contains("fill=\"#123456\""), "{out}");
        assert!(out.contains("stroke=\"#000\""), "{out}");
        assert_eq!(root_size(&out), Some((10.0, 5.0)));
    }
}
