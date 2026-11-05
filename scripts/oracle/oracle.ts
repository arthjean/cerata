import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const WRITER_DIR = process.env.WRITER_DIR ?? "C:/dev/writer-computer";
const WRITER_SRC = join(WRITER_DIR, "apps/desktop/src");
const LOCAL = join(import.meta.dir, ".writer");

function syncWriterSources() {
  if (!existsSync(WRITER_SRC)) throw new Error(`writer-computer not found at ${WRITER_DIR}`);
  mkdirSync(join(LOCAL, "lib"), { recursive: true });
  cpSync(join(WRITER_SRC, "lib/prosemark-core"), join(LOCAL, "lib/prosemark-core"), {
    recursive: true,
  });
  cpSync(join(WRITER_SRC, "lib/paths.ts"), join(LOCAL, "lib/paths.ts"));
}

syncWriterSources();

const { EditorSelection, EditorState } = await import("@codemirror/state");
const { markdown } = await import("@codemirror/lang-markdown");
const { ensureSyntaxTree, syntaxTree } = await import("@codemirror/language");
const { GFM } = await import("@lezer/markdown");
const core = await import("./.writer/lib/prosemark-core/main");

const parserExtensions = () => markdown({ extensions: [GFM, core.prosemarkMarkdownSyntaxExtensions] });

function fullParse(state: InstanceType<typeof EditorState>) {
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state.update({}).state;
}

function makeState(doc: string, ranges: Array<[number, number]>) {
  return fullParse(
    EditorState.create({
      doc,
      selection: EditorSelection.create(ranges.map(([a, h]) => EditorSelection.range(a, h))),
      extensions: [
        EditorState.allowMultipleSelections.of(true),
        parserExtensions(),
        core.defaultHideExtensions,
        core.defaultFoldableSyntaxExtensions,
      ],
    }),
  );
}

function byteOffsets(doc: string): (pos: number) => number {
  const table = new Array<number>(doc.length + 1);
  let bytes = 0;
  for (let i = 0; i < doc.length; i++) {
    table[i] = bytes;
    const code = doc.charCodeAt(i);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code < 0xdc00) {
      table[i + 1] = bytes;
      bytes += 4;
      i++;
      continue;
    } else bytes += 3;
  }
  table[doc.length] = bytes;
  return (pos) => table[pos] ?? bytes;
}

function dumpTree(doc: string): string[] {
  const state = fullParse(EditorState.create({ doc, extensions: [parserExtensions()] }));
  const lines: string[] = [];
  const b = byteOffsets(doc);
  let depth = 0;
  syntaxTree(state).iterate({
    enter(node) {
      lines.push(`${"  ".repeat(depth)}${node.name} ${b(node.from)}..${b(node.to)}`);
      depth++;
      if (node.name === "FrontmatterContent") {
        depth--;
        return false;
      }
    },
    leave(node) {
      if (node.name !== "FrontmatterContent") depth--;
    },
  });
  return lines;
}

function describe(from: number, to: number, deco: any): string {
  const spec = deco.spec ?? {};
  const widget = spec.widget;
  if (widget) {
    const name = widget.constructor?.name ?? "Widget";
    let detail = "";
    if ("url" in widget) detail = ` ${widget.url}`;
    if ("dashCount" in widget) detail = ` ${widget.dashCount}`;
    if ("emoji" in widget) detail = ` ${widget.emoji}`;
    const block = spec.block ? "block" : "inline";
    const kind = from === to ? "widget" : "replace";
    return `${kind} ${block} ${name}${detail} ${from}..${to}`;
  }
  if (deco.point || deco.constructor?.name === "PointDecoration") {
    return spec.block ? `hide-block ${from}..${to}` : `hide ${from}..${to}`;
  }
  if (spec.class === "cm-hidden-token") return `hide ${from}..${to}`;
  if (spec.class === "cm-transparent-token") return `invisible ${from}..${to}`;
  if (spec.class) return `mark ${spec.class} ${from}..${to}`;
  if (spec.attributes?.class) return `line ${spec.attributes.class} ${from}`;
  return `other ${JSON.stringify(spec)} ${from}..${to}`;
}

function decorations(state: InstanceType<typeof EditorState>, b: (pos: number) => number): string[] {
  const out: string[] = [];
  for (const field of [core.hideExtension, core.foldExtension]) {
    state.field(field).between(0, state.doc.length, (from: number, to: number, deco: any) => {
      out.push(describe(b(from), b(to), deco));
    });
  }
  return [...new Set(out)].sort(compareDescriptions);
}

function rangeOf(description: string): [number, number] {
  const match = / (\d+)\.\.(\d+)$/.exec(description) ?? / (\d+)$/.exec(description);
  if (!match) return [0, 0];
  return [Number(match[1]), Number(match[2] ?? match[1])];
}

function compareDescriptions(a: string, b: string): number {
  const [af, at] = rangeOf(a);
  const [bf, bt] = rangeOf(b);
  return af - bf || at - bt || (a < b ? -1 : a > b ? 1 : 0);
}

function caretPositions(doc: string): number[] {
  const state = fullParse(EditorState.create({ doc, extensions: [parserExtensions()] }));
  const positions = new Set<number>([0, doc.length]);
  for (let n = 1; n <= state.doc.lines; n++) {
    const line = state.doc.line(n);
    positions.add(line.from);
    positions.add(line.to);
  }
  syntaxTree(state).iterate({
    enter(node) {
      if (node.name === "Document" || node.name === "Paragraph") return;
      positions.add(node.from);
      positions.add(node.to);
    },
  });
  return [...positions].sort((a, b) => a - b).slice(0, 40);
}

function snapshots(out: string, fixtureDirs: string[]) {
  mkdirSync(out, { recursive: true });
  const files = fixtureDirs
    .flatMap((dir) => readdirSync(dir).filter((f) => f.endsWith(".md")).map((f) => join(dir, f)))
    .sort();
  for (const path of files) {
    const file = basename(path);
    const doc = readFileSync(path, "utf8");
    const b = byteOffsets(doc);
    const cases: Array<{ selection: Array<[number, number]>; decorations: string[] }> = [];
    const push = (selection: Array<[number, number]>) =>
      cases.push({
        selection: selection.map(([a, h]) => [b(a), b(h)]),
        decorations: decorations(makeState(doc, selection), b),
      });
    const positions = caretPositions(doc);
    for (const pos of positions) push([[pos, pos]]);
    const n = positions.length;
    for (let i = 0; i < Math.min(8, Math.floor(n / 2)); i++) {
      push([[positions[i]!, positions[i]!], [positions[n - 1 - i]!, positions[n - 1 - i]!]]);
    }
    for (let i = 0; i + 1 < n && i < 8; i += 2) push([[positions[i]!, positions[i + 1]!]]);
    push([[0, doc.length]]);
    const target = join(out, `${basename(file, ".md")}.json`);
    writeFileSync(target, `${JSON.stringify({ fixture: file, cases }, null, 1)}\n`);
  }
  console.log(`${files.length} snapshots written to ${out}`);
}

function trees(out: string, fixtureDirs: string[]) {
  mkdirSync(out, { recursive: true });
  const files = fixtureDirs.flatMap((dir) =>
    readdirSync(dir).filter((f) => f.endsWith(".md")).map((f) => join(dir, f)),
  );
  for (const path of files) {
    const doc = readFileSync(path, "utf8");
    writeFileSync(join(out, `${basename(path, ".md")}.tree`), `${dumpTree(doc).join("\n")}\n`);
  }
  console.log(`${files.length} trees written to ${out}`);
}

async function emojiTable(out: string) {
  const emoji = await import("node-emoji");
  const rows = emoji
    .search("")
    .map(({ name, emoji: glyph }: { name: string; emoji: string }) => `${name}\t${glyph}`)
    .sort();
  writeFileSync(out, `${rows.join("\n")}\n`);
  console.log(`${rows.length} emoji written to ${out}`);
}

const [command, ...args] = process.argv.slice(2);
switch (command) {
  case "tree":
    for (const file of args) console.log(dumpTree(readFileSync(file, "utf8")).join("\n"));
    break;
  case "trees":
    trees(args[0]!, args.slice(1));
    break;
  case "snapshots":
    snapshots(args[0]!, args.slice(1));
    break;
  case "emoji":
    await emojiTable(args[0]!);
    break;
  default:
    console.error(
      [
        "usage (after `bun install`):",
        "  bun oracle.ts tree <file.md>",
        "  bun oracle.ts trees <out> <fixtures>...",
        "  bun oracle.ts snapshots <out> <fixtures>...",
        "  bun oracle.ts emoji <out.tsv>",
        `WRITER_DIR selects the reference checkout (now ${WRITER_DIR}); offsets are UTF-8 bytes.`,
      ].join("\n"),
    );
    process.exit(2);
}
