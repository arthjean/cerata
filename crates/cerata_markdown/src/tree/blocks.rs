//! Block structure: pulldown-cmark finds the blocks (ADR 0002), and each
//! block is rebuilt with the children and ranges Lezer gives it
//! (`@lezer/markdown` 1.6.3 `DefaultBlockParsers`), plus Writer's block
//! rules: frontmatter at offset 0 and the single-dash setext guard
//! (`markdown/setextDashUnderline.ts`).

use super::inline::{self, inject_marks};
use super::{Elt, NodeKind};
use crate::frontmatter::{self, lines};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

pub(crate) fn parse_document(text: &str) -> Elt {
    let len = text.len();
    let mut children = Vec::new();
    let mut body = 0;
    if let Some(fm) = frontmatter::detect(text) {
        let mut marks = vec![Elt::new(
            NodeKind::FrontmatterMark,
            fm.opening_mark.start,
            fm.opening_mark.end,
        )];
        if let Some(content) = &fm.content {
            marks.push(Elt::new(
                NodeKind::FrontmatterContent,
                content.start,
                content.end,
            ));
        }
        marks.push(Elt::new(
            NodeKind::FrontmatterMark,
            fm.closing_mark.start,
            fm.closing_mark.end,
        ));
        children.push(Elt::with(
            NodeKind::Frontmatter,
            fm.range.start,
            fm.range.end,
            marks,
        ));
        body = fm.body_start;
    }
    let mut blocks = Blocks { text }.parse(body, len);
    if let Some(math_from) = unclosed_display_math(text, body, &blocks) {
        let line_start = line_start_of(text, math_from);
        blocks = Blocks { text }.parse(body, line_start);
        blocks.push(Elt::with(
            NodeKind::Math,
            math_from,
            len,
            vec![
                Elt::new(NodeKind::MathMark, math_from, math_from + 2),
                Elt::new(NodeKind::MathFormula, math_from + 2, len),
            ],
        ));
    }
    merge_details(text, &mut blocks);
    children.extend(blocks);
    let mut document = Elt::with(NodeKind::Document, 0, len, children);
    wrap_wiki_links(text, &mut document);
    document
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn opens_details(block: &str) -> bool {
    let rest = block.trim_start();
    rest.get(..8)
        .is_some_and(|open| open.eq_ignore_ascii_case("<details"))
        && rest
            .as_bytes()
            .get(8)
            .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>')
}

/// Whether `text` may hold a `<details>` block, which a window reparse
/// cannot see whole.
pub(crate) fn has_details(text: &str) -> bool {
    contains_ignore_case(text, "<details")
}

/// `html-block-decorations.ts:312-332` (`detailsBlockParser`): a top-level
/// HTML block opening `<details>` runs to the block that closes it, blank
/// lines and Markdown between included. An unclosed one stays as
/// CommonMark reads it.
fn merge_details(text: &str, blocks: &mut Vec<Elt>) {
    let mut i = 0;
    while i < blocks.len() {
        let open = &blocks[i];
        let source = &text[open.from..open.to];
        if open.kind == NodeKind::HtmlBlock
            && opens_details(source)
            && !contains_ignore_case(source, "</details")
            && let Some(j) = (i + 1..blocks.len())
                .find(|&j| contains_ignore_case(&text[blocks[j].from..blocks[j].to], "</details"))
        {
            let merged = Elt::new(NodeKind::HtmlBlock, open.from, blocks[j].to);
            blocks.splice(i..=j, [merged]);
        }
        i += 1;
    }
}

/// Blocks of `text[from..to]`, parsed as a document of their own, with the
/// positions of `text` (the windowed reparse of `MarkdownTree::reparse`).
pub(crate) fn parse_window(text: &str, from: usize, to: usize) -> Vec<Elt> {
    Blocks { text }.parse(from, to)
}

/// A `$$` that opens a line of a top-level paragraph and is never closed:
/// like an unclosed fence, it runs to the end of the document (PRD US-010).
/// Writer leaves it as text; `$$` closed later, or inside a line, keep
/// Writer's inline rule.
fn unclosed_display_math(text: &str, body: usize, blocks: &[Elt]) -> Option<usize> {
    let pos = body + text.get(body..)?.rfind("$$")?;
    let line_start = line_start_of(text, pos);
    let indent = &text[line_start..pos];
    if !indent.bytes().all(|b| b == b' ') || indent.len() >= 4 {
        return None;
    }
    let paragraph = blocks
        .iter()
        .find(|b| b.from <= pos && pos < b.to.max(b.from + 1))?;
    if paragraph.kind != NodeKind::Paragraph {
        return None;
    }
    let covered = paragraph
        .children
        .iter()
        .any(|c| c.from <= pos && pos < c.to);
    (!covered).then_some(pos)
}

fn line_start_of(text: &str, pos: usize) -> usize {
    memchr::memrchr2(b'\n', b'\r', &text.as_bytes()[..pos]).map_or(0, |i| i + 1)
}

fn line_end_of(text: &str, pos: usize) -> usize {
    memchr::memchr2(b'\n', b'\r', &text.as_bytes()[pos..]).map_or(text.len(), |i| pos + i)
}

/// End of a block: its range without trailing line breaks and blank lines.
fn trim_end(text: &str, from: usize, to: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = to.min(text.len());
    loop {
        while end > from && matches!(bytes[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        let start = line_start_of(text, end).max(from);
        if start > from && bytes[start..end].iter().all(|b| *b == b' ' || *b == b'\t') {
            end = start;
            continue;
        }
        return end;
    }
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

#[derive(Clone, Copy, Debug)]
enum Container {
    Quote,
    Item { content_col: usize },
}

/// The prefix of one line inside containers.
struct Prefix {
    /// Quote marks found, one per quote container reached, in order, with
    /// the index of the container that owns each.
    marks: Vec<(usize, Range<usize>)>,
    /// Position and column where the content starts.
    base_pos: usize,
    base_col: usize,
}

fn next_col(col: usize, b: u8) -> usize {
    if b == b'\t' {
        col + 4 - col % 4
    } else {
        col + 1
    }
}

fn walk_prefix(text: &str, line_start: usize, containers: &[Container]) -> Prefix {
    let bytes = text.as_bytes();
    let line_end = line_end_of(text, line_start);
    let mut pos = line_start;
    let mut col = 0;
    let mut marks = Vec::new();
    for (index, container) in containers.iter().enumerate() {
        match container {
            Container::Quote => {
                let (mut p, mut c) = (pos, col);
                while p < line_end && is_ws(bytes[p]) && c < col + 4 {
                    c = next_col(c, bytes[p]);
                    p += 1;
                }
                if p < line_end && bytes[p] == b'>' && c < col + 4 {
                    marks.push((index, p..p + 1));
                    pos = p + 1;
                    col = c + 1;
                    if pos < line_end && is_ws(bytes[pos]) {
                        col = next_col(col, bytes[pos]);
                        pos += 1;
                    }
                } else {
                    return Prefix {
                        marks,
                        base_pos: pos,
                        base_col: col,
                    };
                }
            }
            Container::Item { content_col } => {
                let blank = bytes[pos..line_end].iter().all(|b| is_ws(*b));
                while pos < line_end && col < *content_col && is_ws(bytes[pos]) {
                    col = next_col(col, bytes[pos]);
                    pos += 1;
                }
                if col < *content_col && !blank {
                    return Prefix {
                        marks,
                        base_pos: pos,
                        base_col: col,
                    };
                }
            }
        }
    }
    Prefix {
        marks,
        base_pos: pos,
        base_col: col,
    }
}

/// Column of `pos` on its line.
fn column(text: &str, pos: usize) -> usize {
    let start = line_start_of(text, pos);
    text.as_bytes()[start..pos]
        .iter()
        .fold(0, |col, b| next_col(col, *b))
}

/// Position of column `target` on the line starting at `line_start`.
fn find_column(text: &str, line_start: usize, target: usize) -> usize {
    let bytes = text.as_bytes();
    let end = line_end_of(text, line_start);
    let (mut pos, mut col) = (line_start, 0);
    while pos < end && col < target {
        col = next_col(col, bytes[pos]);
        pos += 1;
    }
    pos
}

fn skip_space_back(bytes: &[u8], mut i: usize, to: usize) -> usize {
    while i > to && matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r') {
        i -= 1;
    }
    i
}

fn add_code_text(marks: &mut Vec<Elt>, from: usize, to: usize) {
    if let Some(last) = marks.last_mut()
        && last.kind == NodeKind::CodeText
        && last.to == from
    {
        last.to = to;
        return;
    }
    marks.push(Elt::new(NodeKind::CodeText, from, to));
}

fn quote_marks(prefix: &Prefix) -> impl Iterator<Item = Elt> + '_ {
    prefix
        .marks
        .iter()
        .map(|(_, r)| Elt::new(NodeKind::QuoteMark, r.start, r.end))
}

struct Frame {
    elt: Elt,
    container: Option<Container>,
}

struct Blocks<'a> {
    text: &'a str,
}

fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

impl<'a> Blocks<'a> {
    fn push_child(stack: &mut [Frame], elt: Elt) {
        let Some(top) = stack.last_mut() else { return };
        // Writer's single-dash guard leaves a one-item bullet list that the
        // next items of the same marker continue (pulldown made a heading of
        // it, then started a new list).
        if elt.kind == NodeKind::BulletList
            && let Some(previous) = top.elt.children.last_mut()
            && previous.kind == NodeKind::BulletList
            && previous.dash_guard
        {
            previous.to = elt.to;
            previous.children.extend(elt.children);
            previous.dash_guard = false;
            return;
        }
        top.elt.children.push(elt);
    }

    fn parse(&self, from: usize, to: usize) -> Vec<Elt> {
        let text = self.text;
        let Some(slice) = text.get(from..to) else {
            return Vec::new();
        };
        let mut iter = Parser::new_ext(slice, Options::ENABLE_TABLES).into_offset_iter();
        let mut stack = vec![Frame {
            elt: Elt::new(NodeKind::Document, from, to),
            container: None,
        }];
        // Containers (quotes, list items) of the open frames, outermost first.
        let mut open: Vec<Container> = Vec::new();
        let mut skip = 0usize;
        let mut run: Option<Range<usize>> = None;
        for (event, range) in iter.by_ref() {
            let mut r = range.start + from..range.end + from;
            if matches!(
                event,
                Event::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item)
            ) {
                // A nested list's range can start on the previous line break.
                r.start += text[r.start..r.end]
                    .bytes()
                    .take_while(|b| b.is_ascii_whitespace())
                    .count();
            }
            if skip > 0 {
                match event {
                    Event::Start(_) => skip += 1,
                    Event::End(_) => skip -= 1,
                    _ => {}
                }
                continue;
            }
            match event {
                Event::Start(tag) if is_inline_tag(&tag) => {
                    run = Some(extend(run, &r));
                    skip = 1;
                }
                Event::Text(_)
                | Event::Code(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::InlineHtml(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::TaskListMarker(_)
                | Event::FootnoteReference(_)
                | Event::Html(_) => run = Some(extend(run, &r)),
                Event::Start(tag) => {
                    self.flush_run(&mut stack, &mut run, &open);
                    let containers = open.as_slice();
                    match tag {
                        Tag::Paragraph => {
                            let end = trim_end(text, r.start, r.end);
                            let elt = self.paragraph(&stack, r.start, end, containers);
                            Self::push_child(&mut stack, elt);
                            skip = 1;
                        }
                        Tag::Heading { .. } => {
                            for elt in self.heading(&stack, r.clone(), containers) {
                                Self::push_child(&mut stack, elt);
                            }
                            skip = 1;
                        }
                        Tag::BlockQuote(_) => {
                            stack.push(Frame {
                                elt: Elt::new(
                                    NodeKind::Blockquote,
                                    r.start,
                                    trim_end(text, r.start, r.end),
                                ),
                                container: Some(Container::Quote),
                            });
                            open.push(Container::Quote);
                        }
                        Tag::List(start) => {
                            let kind = if start.is_some() {
                                NodeKind::OrderedList
                            } else {
                                NodeKind::BulletList
                            };
                            stack.push(Frame {
                                elt: Elt::new(kind, r.start, trim_end(text, r.start, r.end)),
                                container: None,
                            });
                        }
                        Tag::Item => {
                            let (mark, content_col) = self.list_mark(r.start);
                            let mut elt = Elt::new(
                                NodeKind::ListItem,
                                r.start,
                                trim_end(text, r.start, r.end),
                            );
                            elt.children
                                .push(Elt::new(NodeKind::ListMark, mark.start, mark.end));
                            stack.push(Frame {
                                elt,
                                container: Some(Container::Item { content_col }),
                            });
                            open.push(Container::Item { content_col });
                        }
                        Tag::CodeBlock(kind) => {
                            let end = trim_end(text, r.start, r.end);
                            let elt = match kind {
                                CodeBlockKind::Fenced(_) => {
                                    self.fenced_code(r.start, r.end, containers)
                                }
                                CodeBlockKind::Indented => {
                                    self.indented_code(r.start, end, containers)
                                }
                            };
                            Self::push_child(&mut stack, elt);
                            skip = 1;
                        }
                        Tag::HtmlBlock => {
                            let end = trim_end(text, r.start, r.end);
                            Self::push_child(&mut stack, self.html_block(r.start, end, containers));
                            skip = 1;
                        }
                        Tag::Table(_) => {
                            let end = trim_end(text, r.start, r.end);
                            Self::push_child(&mut stack, self.table(r.start, end, containers));
                            skip = 1;
                        }
                        _ => skip = 1,
                    }
                }
                Event::Rule => {
                    self.flush_run(&mut stack, &mut run, &open);
                    let end = trim_end(text, r.start, r.end);
                    let start =
                        r.start + text[r.start..end].bytes().take_while(|b| is_ws(*b)).count();
                    Self::push_child(&mut stack, Elt::new(NodeKind::HorizontalRule, start, end));
                }
                Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item) => {
                    self.flush_run(&mut stack, &mut run, &open);
                    if stack.len() > 1
                        && let Some(frame) = stack.pop()
                    {
                        let mut elt = frame.elt;
                        if elt.kind == NodeKind::ListItem && elt.children.len() == 1 {
                            // An empty item: Lezer ends it at its line end,
                            // with an empty paragraph there.
                            let line_end = line_end_of(text, elt.from);
                            elt.to = line_end;
                            elt.children
                                .push(Elt::new(NodeKind::Paragraph, line_end, line_end));
                        }
                        if let Some(last) = elt.children.last() {
                            elt.to = elt.to.max(last.to);
                        }
                        if frame
                            .container
                            .is_some_and(|c| matches!(c, Container::Quote))
                        {
                            self.distribute_quote_marks(&mut elt, &open);
                        }
                        if frame.container.is_some() {
                            open.pop();
                        }
                        Self::push_child(&mut stack, elt);
                    }
                }
                _ => {}
            }
        }
        self.flush_run(&mut stack, &mut run, &open);
        while stack.len() > 1 {
            if let Some(frame) = stack.pop() {
                Self::push_child(&mut stack, frame.elt);
            }
        }
        let definitions: Vec<Range<usize>> = iter
            .reference_definitions()
            .iter()
            .map(|(_, def)| def.span.clone())
            .collect();
        let mut root = stack
            .pop()
            .map(|f| f.elt)
            .unwrap_or_else(|| Elt::new(NodeKind::Document, from, to));
        let first = definitions.len();
        let mut definitions = with_duplicate_definitions(slice, definitions);
        definitions.sort_by_key(|r| r.start);
        let duplicates = definitions.len() > first;
        for span in definitions {
            let span = span.start + from..span.end + from;
            if duplicates && overlaps_leaf(&root, &span) {
                continue;
            }
            if let Some(reference) = self.link_reference(span.start) {
                insert_block(&mut root, reference);
            }
        }
        root.children
    }

    /// A tight list item's text has no paragraph event: Lezer still has one.
    fn flush_run(
        &self,
        stack: &mut [Frame],
        run: &mut Option<Range<usize>>,
        containers: &[Container],
    ) {
        let Some(range) = run.take() else { return };
        let text = self.text;
        let mut end = range.end;
        while end > range.start && matches!(text.as_bytes()[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        let end = line_end_of(text, end.max(range.start));
        let elt = self.paragraph(stack, range.start, end, containers);
        Self::push_child(stack, elt);
    }

    /// Leaf content with the container marks of its continuation lines
    /// blanked, parsed inline.
    fn inline(&self, from: usize, to: usize, containers: &[Container]) -> Vec<Elt> {
        let text = self.text;
        let Some(slice) = text.get(from..to) else {
            return Vec::new();
        };
        let mut scrubbed: Option<Vec<u8>> = None;
        // Only quote marks are scrubbed; list indentation is spaces already.
        if containers.iter().any(|c| matches!(c, Container::Quote)) {
            let bytes = slice.as_bytes();
            let starts = (0..bytes.len()).filter(|&i| {
                bytes[i] == b'\n' || (bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
            });
            for line_start in starts.map(|i| from + i + 1) {
                let prefix = walk_prefix(text, line_start, containers);
                for (_, mark) in &prefix.marks {
                    let bytes = scrubbed.get_or_insert_with(|| slice.as_bytes().to_vec());
                    if let Some(b) = bytes.get_mut(mark.start - from) {
                        *b = b' ';
                    }
                }
            }
        }
        match scrubbed.and_then(|bytes| String::from_utf8(bytes).ok()) {
            Some(scrubbed) => inline::parse_inline(&scrubbed, from),
            None => inline::parse_inline(slice, from),
        }
    }

    fn paragraph(&self, stack: &[Frame], from: usize, to: usize, containers: &[Container]) -> Elt {
        let text = self.text;
        let in_item = stack
            .last()
            .is_some_and(|f| f.elt.kind == NodeKind::ListItem);
        let content = &text[from..to];
        let bytes = content.as_bytes();
        let is_task = in_item
            && bytes.len() >= 4
            && bytes[0] == b'['
            && matches!(bytes[1], b' ' | b'x' | b'X')
            && bytes[2] == b']'
            && is_ws(bytes[3]);
        if is_task {
            let mut children = vec![Elt::new(NodeKind::TaskMarker, from, from + 3)];
            children.extend(self.inline(from + 3, to, containers));
            return Elt::with(NodeKind::Task, from, to, children);
        }
        Elt::with(
            NodeKind::Paragraph,
            from,
            to,
            self.inline(from, to, containers),
        )
    }

    fn heading(&self, stack: &[Frame], r: Range<usize>, containers: &[Container]) -> Vec<Elt> {
        let text = self.text;
        let bytes = text.as_bytes();
        let start = r.start
            + text[r.start..r.end]
                .bytes()
                .take_while(|b| is_ws(*b))
                .count();
        if bytes.get(start) == Some(&b'#') {
            let line_end = line_end_of(text, start);
            let size = bytes[start..line_end]
                .iter()
                .take_while(|b| **b == b'#')
                .count();
            let end_of_space = skip_space_back(bytes, line_end, start);
            let mut after = end_of_space;
            while after > start && bytes[after - 1] == b'#' {
                after -= 1;
            }
            if after == end_of_space || after == start || !matches!(bytes[after - 1], b' ' | b'\t')
            {
                after = line_end;
            }
            let mut children = vec![Elt::new(NodeKind::HeaderMark, start, start + size)];
            let content_from = (start + size + 1).min(line_end);
            if content_from < after {
                children.extend(self.inline(content_from, after, containers));
            }
            if after < line_end {
                children.push(Elt::new(NodeKind::HeaderMark, after, end_of_space));
            }
            return vec![Elt::with(
                NodeKind::AtxHeading(size.clamp(1, 6) as u8),
                start,
                line_end,
                children,
            )];
        }
        // Setext: text lines, then the underline line.
        let end = trim_end(text, r.start, r.end);
        let underline_line = line_start_of(text, end);
        let prefix = walk_prefix(text, underline_line, containers);
        let mark_from = prefix.base_pos
            + text[prefix.base_pos..end]
                .bytes()
                .take_while(|b| is_ws(*b))
                .count();
        let ch = bytes.get(mark_from).copied().unwrap_or(b'=');
        let mark_to = mark_from
            + text[mark_from..end]
                .bytes()
                .take_while(|b| *b == ch)
                .count();
        let content_end = trim_end(text, r.start, underline_line);
        let rest_blank = text[mark_to..end].bytes().all(is_ws);
        if ch == b'-' && mark_to == mark_from + 1 && rest_blank {
            // Writer: a single `-` under a line does not make an H2; the
            // `-` starts an empty bullet item.
            let paragraph = self.paragraph(stack, r.start, content_end, containers);
            let item = Elt::with(
                NodeKind::ListItem,
                mark_from,
                end,
                vec![
                    Elt::new(NodeKind::ListMark, mark_from, mark_to),
                    Elt::new(NodeKind::Paragraph, end, end),
                ],
            );
            let mut list = Elt::with(NodeKind::BulletList, mark_from, end, vec![item]);
            list.dash_guard = true;
            return vec![paragraph, list];
        }
        let mut children = self.inline(r.start, content_end, containers);
        children.push(Elt::new(NodeKind::HeaderMark, mark_from, mark_to));
        let level = if ch == b'=' { 1 } else { 2 };
        vec![Elt::with(
            NodeKind::SetextHeading(level),
            r.start,
            end,
            children,
        )]
    }

    /// The marker of the item starting at `pos` and its content column.
    fn list_mark(&self, pos: usize) -> (Range<usize>, usize) {
        let text = self.text;
        let bytes = text.as_bytes();
        let line_end = line_end_of(text, pos);
        let mut end = pos;
        while end < line_end && bytes[end].is_ascii_digit() {
            end += 1;
        }
        end = if end > pos { end + 1 } else { pos + 1 };
        end = end.min(line_end);
        let after = column(text, end);
        let mut spaced = end;
        let mut col = after;
        while spaced < line_end && is_ws(bytes[spaced]) {
            col = next_col(col, bytes[spaced]);
            spaced += 1;
        }
        let content_col = if col >= after + 5 { after + 1 } else { col };
        (pos..end, content_col)
    }

    /// `end` is the trimmed block end, `raw_end` pulldown's: an unclosed
    /// fence keeps its trailing blank lines, as Lezer's does.
    fn fenced_code(&self, start: usize, raw_end: usize, containers: &[Container]) -> Elt {
        let end = trim_end(self.text, start, raw_end);
        let text = self.text;
        let bytes = text.as_bytes();
        let from = start + text[start..end].bytes().take_while(|b| is_ws(*b)).count();
        let ch = bytes.get(from).copied().unwrap_or(b'`');
        let first_end = line_end_of(text, from);
        let len = bytes[from..first_end]
            .iter()
            .take_while(|b| **b == ch)
            .count();
        let mut marks = vec![Elt::new(NodeKind::CodeMark, from, from + len)];
        let info_from = from
            + len
            + bytes[from + len..first_end]
                .iter()
                .take_while(|b| is_ws(**b))
                .count();
        let info_to = skip_space_back(bytes, first_end, info_from);
        if info_from < info_to {
            marks.push(Elt::new(NodeKind::CodeInfo, info_from, info_to));
        }
        let mut last_end = first_end;
        let (mut first, mut empty, mut has_line) = (true, true, false);
        for (line_start, line_end, _) in lines(text, first_end).skip(1) {
            let inside = line_start < raw_end || (line_start == raw_end && raw_end == text.len());
            if !inside {
                break;
            }
            let prefix = walk_prefix(text, line_start, containers);
            let base = prefix.base_pos;
            let pos = base
                + bytes[base..line_end]
                    .iter()
                    .take_while(|b| is_ws(**b))
                    .count();
            let indent = column(text, pos) - prefix.base_col;
            let mut i = pos;
            if indent < 4 {
                while i < line_end && bytes[i] == ch {
                    i += 1;
                }
            }
            last_end = line_end;
            if i - pos >= len && bytes[i..line_end].iter().all(|b| is_ws(*b)) {
                marks.extend(quote_marks(&prefix));
                if empty && has_line {
                    add_code_text(&mut marks, break_start(text, line_start), line_start);
                }
                marks.push(Elt::new(NodeKind::CodeMark, pos, i));
                break;
            }
            has_line = true;
            if !first {
                add_code_text(&mut marks, break_start(text, line_start), line_start);
                empty = false;
            }
            marks.extend(quote_marks(&prefix));
            if base < line_end {
                add_code_text(&mut marks, base, line_end);
                empty = false;
            }
            first = false;
        }
        Elt::with(NodeKind::FencedCode, from, last_end, marks)
    }

    fn indented_code(&self, start: usize, end: usize, containers: &[Container]) -> Elt {
        let text = self.text;
        let bytes = text.as_bytes();
        let first_line = line_start_of(text, start);
        let prefix = walk_prefix(text, first_line, containers);
        let base = prefix.base_col + 4;
        let from = find_column(text, first_line, base);
        let mut to = line_end_of(text, from);
        let mut marks = Vec::new();
        let mut pending: Vec<Elt> = Vec::new();
        add_code_text(&mut marks, from, to);
        for (line_start, line_end, _) in lines(text, first_line).skip(1) {
            if line_start > end {
                break;
            }
            let prefix = walk_prefix(text, line_start, containers);
            let blank = bytes[prefix.base_pos..line_end].iter().all(|b| is_ws(*b));
            if blank {
                add_code_text(&mut pending, break_start(text, line_start), line_start);
                pending.extend(quote_marks(&prefix));
                continue;
            }
            for mark in pending.drain(..) {
                if mark.kind == NodeKind::CodeText {
                    add_code_text(&mut marks, mark.from, mark.to);
                } else {
                    marks.push(mark);
                }
            }
            add_code_text(&mut marks, break_start(text, line_start), line_start);
            marks.extend(quote_marks(&prefix));
            to = line_end;
            let code_start = find_column(text, line_start, prefix.base_col + 4);
            if code_start < to {
                add_code_text(&mut marks, code_start, to);
            }
        }
        Elt::with(NodeKind::CodeBlock, from, to, marks)
    }

    fn html_block(&self, start: usize, end: usize, containers: &[Container]) -> Elt {
        let text = self.text;
        let from = start + text[start..end].bytes().take_while(|b| is_ws(*b)).count();
        let rest = &text[from..end];
        let kind = if rest.starts_with("<!--") {
            NodeKind::CommentBlock
        } else if rest.starts_with("<?") {
            NodeKind::ProcessingInstructionBlock
        } else {
            NodeKind::HtmlBlock
        };
        let mut marks = Vec::new();
        for (line_start, _, _) in lines(text, from).skip(1) {
            if line_start > end {
                break;
            }
            marks.extend(quote_marks(&walk_prefix(text, line_start, containers)));
        }
        Elt::with(kind, from, end, marks)
    }

    fn table(&self, start: usize, end: usize, containers: &[Container]) -> Elt {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut rows = Vec::new();
        let mut from = start;
        for (index, (line_start, line_end, _)) in
            lines(text, line_start_of(text, start)).enumerate()
        {
            if line_start > end {
                break;
            }
            let prefix = walk_prefix(text, line_start, containers);
            if index > 0 {
                rows.extend(quote_marks(&prefix));
            }
            let base = if index == 0 {
                start.max(prefix.base_pos)
            } else {
                prefix.base_pos
            };
            let pos = base
                + bytes[base..line_end]
                    .iter()
                    .take_while(|b| is_ws(**b))
                    .count();
            if index == 0 {
                from = pos;
            }
            match index {
                0 => rows.push(Elt::with(
                    NodeKind::TableHeader,
                    pos,
                    line_end,
                    self.table_row(pos, line_end),
                )),
                1 => rows.push(Elt::new(NodeKind::TableDelimiter, pos, line_end)),
                _ => rows.push(Elt::with(
                    NodeKind::TableRow,
                    pos,
                    line_end,
                    self.table_row(pos, line_end),
                )),
            }
        }
        Elt::with(NodeKind::Table, from, end, rows)
    }

    /// Lezer's `parseRow` (GFM, dist/index.js:2052-2078).
    fn table_row(&self, from: usize, to: usize) -> Vec<Elt> {
        let bytes = self.text.as_bytes();
        let mut elts = Vec::new();
        let (mut cell_start, mut cell_end): (Option<usize>, usize) = (None, 0);
        let mut escaped = false;
        let push_cell = |elts: &mut Vec<Elt>, start: usize, end: usize| {
            let content = inline::parse_inline(&self.text[start..end], start);
            elts.push(Elt::with(NodeKind::TableCell, start, end, content));
        };
        for (offset, &b) in bytes[from..to].iter().enumerate() {
            let i = from + offset;
            if b == b'|' && !escaped {
                if let Some(start) = cell_start {
                    push_cell(&mut elts, start, cell_end);
                }
                elts.push(Elt::new(NodeKind::TableDelimiter, i, i + 1));
                cell_start = None;
            } else if escaped || !is_ws(b) {
                if cell_start.is_none() {
                    cell_start = Some(i);
                }
                cell_end = i + 1;
                // Keep cells on character boundaries.
                while cell_end < to && !self.text.is_char_boundary(cell_end) {
                    cell_end += 1;
                }
            }
            escaped = !escaped && b == b'\\';
        }
        if let Some(start) = cell_start {
            push_cell(&mut elts, start, cell_end);
        }
        elts
    }

    /// Lezer's `LinkReference` from `[label]:` to the destination or title.
    fn link_reference(&self, start: usize) -> Option<Elt> {
        let text = self.text;
        let bytes = text.as_bytes();
        let from = start + text.get(start..)?.bytes().take_while(|b| is_ws(*b)).count();
        let label = inline::parse_link_label_in(text, 0, from, true).flatten()?;
        if bytes.get(label.to) != Some(&b':') {
            return None;
        }
        let colon = label.to;
        let mut children = vec![label, Elt::new(NodeKind::LinkMark, colon, colon + 1)];
        let mut pos = colon + 1;
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        let url = inline::parse_url_in(text, 0, pos).flatten()?;
        let mut to = url.to;
        children.push(url);
        let mut after = to;
        while after < bytes.len() && matches!(bytes[after], b' ' | b'\t' | b'\n' | b'\r') {
            after += 1;
        }
        if after > to
            && let Some(Some(title)) = inline::parse_link_title_in(text, 0, after)
        {
            let line_end = line_end_of(text, title.to);
            if text[title.to..line_end].bytes().all(is_ws) {
                to = title.to;
                children.push(title);
            }
        }
        Some(Elt::with(NodeKind::LinkReference, from, to, children))
    }

    /// Puts every quote mark of `quote` (the innermost container in
    /// `containers`) where Lezer has it: inside the leaf of a continuation
    /// line, or as a child of the deepest block holding it.
    fn distribute_quote_marks(&self, quote: &mut Elt, containers: &[Container]) {
        let text = self.text;
        let own = containers.len() - 1;
        let mut marks = Vec::new();
        for (line_start, _, _) in lines(text, line_start_of(text, quote.from)) {
            if line_start > quote.to {
                break;
            }
            if line_start <= quote.from && quote.from <= line_end_of(text, line_start) {
                marks.push(quote.from..quote.from + 1);
                continue;
            }
            let prefix = walk_prefix(text, line_start, containers);
            if let Some((_, mark)) = prefix.marks.iter().find(|(index, _)| *index == own) {
                marks.push(mark.clone());
            }
        }
        for mark in marks {
            place_mark(quote, Elt::new(NodeKind::QuoteMark, mark.start, mark.end));
        }
    }
}

fn break_start(text: &str, line_start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = line_start;
    if pos > 0 && bytes[pos - 1] == b'\n' {
        pos -= 1;
    }
    if pos > 0 && bytes[pos - 1] == b'\r' {
        pos -= 1;
    }
    pos
}

fn extend(run: Option<Range<usize>>, r: &Range<usize>) -> Range<usize> {
    match run {
        Some(run) => run.start.min(r.start)..run.end.max(r.end),
        None => r.clone(),
    }
}

fn is_inline_leaf(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Paragraph
            | NodeKind::AtxHeading(_)
            | NodeKind::SetextHeading(_)
            | NodeKind::Task
            | NodeKind::TableCell
    )
}

fn place_mark(elt: &mut Elt, mark: Elt) {
    if elt
        .children
        .iter()
        .any(|c| c.kind == NodeKind::QuoteMark && c.from == mark.from)
    {
        return;
    }
    if let Some(index) = elt
        .children
        .iter()
        .position(|c| !c.kind.is_mark_like() && c.from < mark.from && mark.to <= c.to)
    {
        let child = &mut elt.children[index];
        if is_inline_leaf(child.kind) {
            let children = std::mem::take(&mut child.children);
            child.children = inject_marks(children, std::slice::from_ref(&mark));
        } else {
            place_mark(child, mark);
        }
        return;
    }
    let at = elt
        .children
        .iter()
        .position(|c| c.from >= mark.to)
        .unwrap_or(elt.children.len());
    elt.children.insert(at, mark);
}

/// Inserts a block (a link reference definition) into the deepest
/// container holding it, in position order.
/// pulldown-cmark keeps one definition per label (the first) and consumes
/// later duplicates without an event; Lezer has a `LinkReference` for each,
/// and the tree must not depend on text outside a block (windowed reparse).
/// Every definition holds a `]:`: when one lies outside the known spans,
/// blank the known definitions and parse again, until no new one surfaces.
fn with_duplicate_definitions(slice: &str, mut spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let unexplained = |spans: &[Range<usize>]| {
        memchr::memmem::find_iter(slice.as_bytes(), b"]:")
            .any(|at| !spans.iter().any(|s| s.start <= at && at < s.end))
    };
    while !spans.is_empty() && unexplained(&spans) {
        let mut blanked = slice.as_bytes().to_vec();
        for span in &spans {
            for b in &mut blanked[span.clone()] {
                if !matches!(*b, b'\n' | b'\r') {
                    *b = b' ';
                }
            }
        }
        // Only ASCII bytes outside line breaks were replaced by spaces, so
        // the text is still UTF-8 (a multi-byte character lies wholly inside
        // or outside a span, spans being on character boundaries).
        let Ok(blanked) = String::from_utf8(blanked) else {
            break;
        };
        let parser = Parser::new_ext(&blanked, Options::ENABLE_TABLES);
        let found: Vec<Range<usize>> = parser
            .reference_definitions()
            .iter()
            .map(|(_, def)| def.span.clone())
            .filter(|s| {
                !spans
                    .iter()
                    .any(|known| known.start < s.end && s.start < known.end)
            })
            .collect();
        if found.is_empty() {
            break;
        }
        spans.extend(found);
    }
    spans
}

/// `span` intersects a block other than a container.
fn overlaps_leaf(parent: &Elt, span: &Range<usize>) -> bool {
    parent.children.iter().any(|c| {
        if c.from >= span.end || span.start >= c.to {
            return false;
        }
        match c.kind {
            NodeKind::Blockquote
            | NodeKind::ListItem
            | NodeKind::BulletList
            | NodeKind::OrderedList => overlaps_leaf(c, span),
            _ => true,
        }
    })
}

fn insert_block(parent: &mut Elt, block: Elt) {
    if let Some(child) = parent.children.iter_mut().find(|c| {
        matches!(
            c.kind,
            NodeKind::Blockquote
                | NodeKind::ListItem
                | NodeKind::BulletList
                | NodeKind::OrderedList
        ) && c.from <= block.from
            && block.to <= c.to
    }) {
        insert_block(child, block);
        return;
    }
    let at = parent
        .children
        .iter()
        .position(|c| c.from >= block.to)
        .unwrap_or(parent.children.len());
    parent.children.insert(at, block);
}

/// Writer's wiki-link pattern `(!?)\[\[([^\]]+)\]\]` outside code
/// (`wiki-link-extension.ts:41-62`): wraps the Lezer nodes it covers.
fn wrap_wiki_links(text: &str, document: &mut Elt) {
    wrap_wiki_links_in(text, document, 0..text.len());
}

/// The scan always runs over the whole text, so its matches are those of
/// Writer's global regex; only the matches inside `window` are wrapped.
pub(crate) fn wrap_wiki_links_in(text: &str, document: &mut Elt, window: Range<usize>) {
    let bytes = text.as_bytes();
    let mut pos = 0;
    while let Some(found) = text.get(pos..).and_then(|t| t.find("[[")) {
        let open = pos + found;
        let start = if open > 0 && bytes[open - 1] == b'!' {
            open - 1
        } else {
            open
        };
        let inner_from = open + 2;
        let inner_len = bytes[inner_from..]
            .iter()
            .take_while(|b| **b != b']')
            .count();
        let close = inner_from + inner_len;
        if inner_len == 0 || bytes.get(close + 1) != Some(&b']') {
            pos = open + 1;
            continue;
        }
        let end = close + 2;
        if window.start <= start && end <= window.end && !touches_code(document, start) {
            wrap(document, start, end);
        }
        pos = end;
    }
}

/// Children that may touch `pos` (children are sorted and disjoint).
fn children_near(elt: &Elt, pos: usize) -> &[Elt] {
    let first = elt.children.partition_point(|c| c.to < pos);
    let last = elt.children.partition_point(|c| c.from <= pos);
    elt.children.get(first..last.max(first)).unwrap_or(&[])
}

fn touches_code(elt: &Elt, pos: usize) -> bool {
    children_near(elt, pos).iter().any(|c| {
        matches!(
            c.kind,
            NodeKind::FencedCode
                | NodeKind::InlineCode
                | NodeKind::CodeBlock
                | NodeKind::CodeText
                | NodeKind::CodeInfo
        ) || touches_code(c, pos)
    })
}

fn wrap(elt: &mut Elt, from: usize, to: usize) {
    let near = elt.children.partition_point(|c| c.to < to);
    if let Some(child) = elt.children.get_mut(near)
        && child.from <= from
        && to <= child.to
        && (child.kind.is_block() || !(child.from == from && child.to == to))
    {
        wrap(child, from, to);
        return;
    }
    let inside: Vec<usize> = (0..elt.children.len())
        .filter(|&i| elt.children[i].from >= from && elt.children[i].to <= to)
        .collect();
    let straddles = elt
        .children
        .iter()
        .any(|c| c.from < to && c.to > from && !(c.from >= from && c.to <= to));
    if straddles {
        return;
    }
    let at = inside.first().copied().unwrap_or_else(|| {
        elt.children
            .iter()
            .position(|c| c.from >= to)
            .unwrap_or(elt.children.len())
    });
    // Sorted, disjoint and none straddling: the children inside are one run.
    let wrapped: Vec<Elt> = match (inside.first(), inside.last()) {
        (Some(&first), Some(&last)) => elt.children.drain(first..=last).collect(),
        _ => Vec::new(),
    };
    elt.children
        .insert(at, Elt::with(NodeKind::WikiLink, from, to, wrapped));
}

impl NodeKind {
    /// Blocks a wiki link sits inside, even when it spans the whole block.
    fn is_block(self) -> bool {
        matches!(
            self,
            NodeKind::Paragraph
                | NodeKind::AtxHeading(_)
                | NodeKind::SetextHeading(_)
                | NodeKind::Blockquote
                | NodeKind::BulletList
                | NodeKind::OrderedList
                | NodeKind::ListItem
                | NodeKind::Task
                | NodeKind::Table
                | NodeKind::TableHeader
                | NodeKind::TableRow
                | NodeKind::TableCell
        )
    }

    /// Marker-like nodes never contain a quote mark.
    fn is_mark_like(self) -> bool {
        matches!(
            self,
            NodeKind::QuoteMark | NodeKind::ListMark | NodeKind::CodeMark | NodeKind::CodeText
        )
    }
}
