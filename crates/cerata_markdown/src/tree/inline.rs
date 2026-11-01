//! Inline parser: a port of `@lezer/markdown` 1.6.3's `InlineContext` and
//! `DefaultInline` (dist/index.js:1372-1820), GFM's `Strikethrough` and
//! `Autolink` (:2026-2262), and Writer's inline extensions
//! (`prosemark-core/markdown/*.ts`, `hide/index.ts:109-126`,
//! `fold/dashes.ts`, `fold/emoji.ts`), in Lezer's parser order.
//!
//! Positions are absolute byte offsets. `text` is the leaf content with
//! container prefixes (`>` of continuation lines) blanked to spaces, as
//! Lezer's `Line.scrub()` does, so offsets stay those of the document.

use super::{Elt, NodeKind};
use regex::Regex;
use std::sync::LazyLock;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DelimType {
    EmphasisUnderscore,
    EmphasisAsterisk,
    LinkStart,
    ImageStart,
    Strikethrough,
    Emoji,
}

impl DelimType {
    fn resolve(self) -> Option<NodeKind> {
        match self {
            DelimType::EmphasisUnderscore | DelimType::EmphasisAsterisk => Some(NodeKind::Emphasis),
            DelimType::Strikethrough => Some(NodeKind::Strikethrough),
            DelimType::Emoji => Some(NodeKind::Emoji),
            DelimType::LinkStart | DelimType::ImageStart => None,
        }
    }

    fn mark(self) -> Option<NodeKind> {
        match self {
            DelimType::EmphasisUnderscore | DelimType::EmphasisAsterisk => {
                Some(NodeKind::EmphasisMark)
            }
            DelimType::Strikethrough => Some(NodeKind::StrikethroughMark),
            DelimType::Emoji => Some(NodeKind::EmojiMark),
            DelimType::LinkStart | DelimType::ImageStart => None,
        }
    }

    fn is_emphasis(self) -> bool {
        matches!(
            self,
            DelimType::EmphasisUnderscore | DelimType::EmphasisAsterisk
        )
    }
}

const OPEN: u8 = 1;
const CLOSE: u8 = 2;

#[derive(Clone, Debug)]
struct Delim {
    ty: DelimType,
    from: usize,
    to: usize,
    side: u8,
}

#[derive(Clone, Debug)]
enum Part {
    Elt(Elt),
    Delim(Delim),
}

struct Cx<'a> {
    text: &'a str,
    offset: usize,
    parts: Vec<Option<Part>>,
}

/// Parses the inline content `text`, which starts at document offset
/// `offset`. Returns the elements in order.
pub(crate) fn parse_inline(text: &str, offset: usize) -> Vec<Elt> {
    let mut cx = Cx {
        text,
        offset,
        parts: Vec::new(),
    };
    let end = cx.end();
    let bytes = text.as_bytes();
    let has_at = bytes.contains(&b'@');
    let mut pos = offset;
    'outer: while pos < end {
        // Fast path: no parser can start on this byte (a word's inner
        // letter, a lone space, a UTF-8 byte), so skip it.
        let i = pos - offset;
        let b = bytes[i];
        let quiet = if b >= 0x80 {
            true
        } else if b == b' ' {
            bytes.get(i + 1) != Some(&b' ')
        } else if TRIGGERS[b as usize] {
            false
        } else if may_start_autolink(b as char) {
            // Autolinks start a word: `www.`, `http`, `mailto:`, `xmpp:`,
            // or an e-mail address, impossible without an `@` ahead.
            (i > 0 && is_word(bytes[i - 1] as char))
                || !(matches!(b, b'w' | b'h' | b'm' | b'x') || has_at)
        } else {
            true
        };
        if quiet {
            pos += 1;
            continue;
        }
        let Some(next) = cx.char(pos) else { break };
        for parser in parsers_for(next) {
            if let Some(result) = parser(&mut cx, next, pos) {
                // A parser must advance; guard against a zero-width result.
                pos = result.max(pos + 1);
                continue 'outer;
            }
        }
        if may_start_autolink(next)
            && let Some(result) = autolink(&mut cx, next, pos)
        {
            pos = result.max(pos + 1);
            continue 'outer;
        }
        pos += next.len_utf8();
    }
    cx.resolve_markers(0)
}

type InlineParser = fn(&mut Cx<'_>, char, usize) -> Option<usize>;

/// ASCII bytes some parser of `parsers_for` starts on.
const TRIGGERS: [bool; 128] = {
    let mut table = [false; 128];
    let chars = b"\\$&`<:-_*~ [!]";
    let mut i = 0;
    while i < chars.len() {
        table[chars[i] as usize] = true;
        i += 1;
    }
    table
};

/// Lezer's parser order after Writer's and GFM's `before`/`after`
/// constraints: EscapeMark, Math, (Escape), Entity, InlineCode, HTMLTag,
/// Emoji, Dash, Emphasis, Strikethrough, HardBreak, NestedLinkAsPlainText,
/// Link, Image, SpaceDestinationLinkEnd, LinkEnd, Autolink. Every parser but
/// the autolink one starts by checking one character, so only the parsers
/// for `next` run, in that order; the autolink parser runs last.
fn parsers_for(next: char) -> &'static [InlineParser] {
    match next {
        '\\' => &[escape_mark],
        '$' => &[math],
        '&' => &[entity],
        '`' => &[inline_code],
        '<' => &[html_tag],
        ':' => &[emoji],
        '-' => &[dash],
        '_' | '*' => &[emphasis],
        '~' => &[strikethrough],
        ' ' => &[hard_break],
        '[' => &[nested_link_as_plain_text, link],
        '!' => &[image],
        ']' => &[space_destination_link_end, link_end],
        _ => &[],
    }
}

/// Characters `autolinkRE` can start with: `www.`, `http`, `mailto:`,
/// `xmpp:` or an e-mail local part `[w.+-]`.
fn may_start_autolink(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-')
}

impl Cx<'_> {
    fn end(&self) -> usize {
        self.offset + self.text.len()
    }

    /// The character at document position `pos`, or `None` past the end.
    fn char(&self, pos: usize) -> Option<char> {
        if pos < self.offset {
            return None;
        }
        self.text.get(pos - self.offset..)?.chars().next()
    }

    fn byte(&self, pos: usize) -> Option<u8> {
        if pos < self.offset {
            return None;
        }
        self.text.as_bytes().get(pos - self.offset).copied()
    }

    /// The character ending at `pos`, if `pos` is inside this section.
    fn char_before(&self, pos: usize) -> Option<char> {
        if pos <= self.offset {
            return None;
        }
        self.text.get(..pos - self.offset)?.chars().next_back()
    }

    fn slice(&self, from: usize, to: usize) -> &str {
        let from = from.max(self.offset) - self.offset;
        let to = to.min(self.end()).max(self.offset) - self.offset;
        if from >= to {
            return "";
        }
        self.text.get(from..to).unwrap_or("")
    }

    fn append(&mut self, elt: Elt) -> usize {
        let to = elt.to;
        self.parts.push(Some(Part::Elt(elt)));
        to
    }

    fn add_delimiter(
        &mut self,
        ty: DelimType,
        from: usize,
        to: usize,
        open: bool,
        close: bool,
    ) -> usize {
        let side = if open { OPEN } else { 0 } | if close { CLOSE } else { 0 };
        self.parts
            .push(Some(Part::Delim(Delim { ty, from, to, side })));
        to
    }

    fn has_open_link(&self) -> bool {
        self.parts.iter().rev().flatten().any(|part| {
            matches!(part, Part::Delim(d) if matches!(d.ty, DelimType::LinkStart | DelimType::ImageStart))
        })
    }

    fn skip_space(&self, from: usize) -> usize {
        let mut pos = from.max(self.offset);
        while let Some(b) = self.byte(pos) {
            if !is_space(b) {
                break;
            }
            pos += 1;
        }
        pos.min(self.end()).max(from)
    }

    fn find_opening_delimiter(&self, ty: DelimType) -> Option<usize> {
        (0..self.parts.len()).rev().find(
            |&i| matches!(&self.parts[i], Some(Part::Delim(d)) if d.ty == ty && d.side & OPEN != 0),
        )
    }

    fn delimiter_at(&self, index: usize) -> Option<&Delim> {
        match self.parts.get(index) {
            Some(Some(Part::Delim(d))) => Some(d),
            _ => None,
        }
    }

    fn take_content(&mut self, start: usize) -> Vec<Elt> {
        let content = self.resolve_markers(start);
        self.parts.truncate(start);
        content
    }

    fn resolve_markers(&mut self, from: usize) -> Vec<Elt> {
        let mut i = from;
        while i < self.parts.len() {
            let close = match &self.parts[i] {
                Some(Part::Delim(d)) if d.ty.resolve().is_some() && d.side & CLOSE != 0 => {
                    d.clone()
                }
                _ => {
                    i += 1;
                    continue;
                }
            };
            let emp = close.ty.is_emphasis();
            let close_size = close.to - close.from;
            let mut found = None;
            let mut j = i;
            while j > from {
                j -= 1;
                if let Some(Part::Delim(part)) = &self.parts[j] {
                    let part_size = part.to - part.from;
                    let skip = emp
                        && (close.side & OPEN != 0 || part.side & CLOSE != 0)
                        && (part_size + close_size) % 3 == 0
                        && (part_size % 3 != 0 || close_size % 3 != 0);
                    if part.side & OPEN != 0 && part.ty == close.ty && !skip {
                        found = Some((j, part.clone()));
                        break;
                    }
                }
            }
            let Some((j, open)) = found else {
                i += 1;
                continue;
            };
            let mut kind = close.ty.resolve().unwrap_or(NodeKind::Emphasis);
            let mut start = open.from;
            let mut end = close.to;
            if emp {
                let size = 2.min(open.to - open.from).min(close_size);
                start = open.to - size;
                end = close.from + size;
                kind = if size == 1 {
                    NodeKind::Emphasis
                } else {
                    NodeKind::StrongEmphasis
                };
            }
            let mut content = Vec::new();
            if let Some(mark) = open.ty.mark() {
                content.push(Elt::new(mark, start, open.to));
            }
            for k in j + 1..i {
                if let Some(Part::Elt(e)) = self.parts[k].take() {
                    content.push(e);
                } else {
                    self.parts[k] = None;
                }
            }
            if let Some(mark) = close.ty.mark() {
                content.push(Elt::new(mark, close.from, end));
            }
            let element = Elt::with(kind, start, end, content);
            self.parts[j] = (emp && open.from != start).then_some(Part::Delim(Delim {
                ty: open.ty,
                from: open.from,
                to: start,
                side: open.side,
            }));
            let keep = (emp && close.to != end).then_some(Part::Delim(Delim {
                ty: close.ty,
                from: end,
                to: close.to,
                side: close.side,
            }));
            if let Some(keep) = keep {
                self.parts[i] = Some(keep);
                self.parts.insert(i, Some(Part::Elt(element)));
            } else {
                self.parts[i] = Some(Part::Elt(element));
            }
            i += 1;
        }
        let mut result = Vec::new();
        for part in self.parts[from..].iter_mut() {
            if let Some(Part::Elt(_)) = part
                && let Some(Part::Elt(e)) = part.take()
            {
                result.push(e);
            }
        }
        result
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Lezer's `Punctuation`: `[\p{S}|\p{P}]`.
fn is_punctuation(c: Option<char>) -> bool {
    c.is_some_and(|c| {
        c == '|'
            || matches!(
                c.general_category_group(),
                GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Symbol
            )
    })
}

/// `/\s|^$/` on a one-character slice: whitespace, or no character.
fn is_space_or_edge(c: Option<char>) -> bool {
    c.is_none_or(char::is_whitespace)
}

// --- Writer: escape of any character (`hide/index.ts:109-126`) ----------

fn escape_mark(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '\\' {
        return None;
    }
    let escaped = cx.char(pos + 1).map_or(0, char::len_utf8);
    Some(cx.append(Elt::with(
        NodeKind::Escape,
        pos,
        pos + 1 + escaped,
        vec![Elt::new(NodeKind::EscapeMark, pos, pos + 1)],
    )))
}

// --- Writer: Pandoc math (`markdown/mathMarkdown.ts:57-101`) -------------

fn is_escaped_dollar(cx: &Cx<'_>, pos: usize) -> bool {
    let mut backslashes = 0;
    let mut p = pos;
    while p > cx.offset && cx.byte(p - 1) == Some(b'\\') {
        backslashes += 1;
        p -= 1;
    }
    backslashes % 2 == 1
}

fn find_closing_double_dollar(cx: &Cx<'_>, from: usize) -> Option<usize> {
    let end = cx.end();
    (from..end.saturating_sub(1)).find(|&pos| {
        cx.byte(pos) == Some(b'$') && cx.byte(pos + 1) == Some(b'$') && !is_escaped_dollar(cx, pos)
    })
}

fn find_closing_single_dollar(cx: &Cx<'_>, from: usize) -> Option<usize> {
    let end = cx.end();
    (from..end).find(|&pos| {
        cx.byte(pos) == Some(b'$')
            && !is_escaped_dollar(cx, pos)
            && !(pos > from && cx.byte(pos - 1).is_some_and(is_space))
            && !(pos + 1 < end && cx.byte(pos + 1).is_some_and(|b| b.is_ascii_digit()))
    })
}

fn math(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '$' || is_escaped_dollar(cx, pos) {
        return None;
    }
    let end = cx.end();
    let display = pos + 1 < end && cx.byte(pos + 1) == Some(b'$');
    let content_from = if display { pos + 2 } else { pos + 1 };
    if !display && (content_from >= end || cx.byte(content_from).is_some_and(is_space)) {
        return None;
    }
    let close = if display {
        find_closing_double_dollar(cx, content_from)?
    } else {
        find_closing_single_dollar(cx, content_from)?
    };
    let outer_to = if display { close + 2 } else { close + 1 };
    Some(cx.append(Elt::with(
        NodeKind::Math,
        pos,
        outer_to,
        vec![
            Elt::new(NodeKind::MathMark, pos, content_from),
            Elt::new(NodeKind::MathFormula, content_from, close),
            Elt::new(NodeKind::MathMark, close, outer_to),
        ],
    )))
}

// --- Lezer defaults ------------------------------------------------------

static ENTITY: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:#[0-9]+|#x[a-f0-9]+|[A-Za-z0-9_]+);").ok());

fn entity(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '&' {
        return None;
    }
    let after = cx.slice(pos + 1, pos + 31);
    let m = ENTITY.as_ref()?.find(after)?;
    Some(cx.append(Elt::new(NodeKind::Entity, pos, pos + 1 + m.end())))
}

fn inline_code(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '`' || (pos > 0 && cx.byte(pos.wrapping_sub(1)) == Some(b'`')) {
        return None;
    }
    let end = cx.end();
    let mut p = pos + 1;
    while p < end && cx.byte(p) == Some(b'`') {
        p += 1;
    }
    let size = p - pos;
    let mut current = 0;
    while p < end {
        if cx.byte(p) == Some(b'`') {
            current += 1;
            if current == size && cx.byte(p + 1) != Some(b'`') {
                return Some(cx.append(Elt::with(
                    NodeKind::InlineCode,
                    pos,
                    p + 1,
                    vec![
                        Elt::new(NodeKind::CodeMark, pos, pos + size),
                        Elt::new(NodeKind::CodeMark, p + 1 - size, p + 1),
                    ],
                )));
            }
        } else {
            current = 0;
        }
        p += 1;
    }
    None
}

static AUTOLINK_TAG: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[a-z][-A-Za-z0-9_+.]+:[^\s>]+|[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*)>",
    )
    .ok()
});
static HTML_COMMENT: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)^!--[^>](?:-[^-]|[^-])*?-->").ok());
static HTML_PROC: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?s)^\?.*?\?>").ok());
static HTML_TAG: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)^(?:![A-Z].*?>|!\[CDATA\[.*?\]\]>|/\s*[a-zA-Z][A-Za-z0-9_-]*\s*>|\s*[a-zA-Z][A-Za-z0-9_-]*(\s+[a-zA-Z:_][A-Za-z0-9_.:-]*(?:\s*=\s*(?:[^\s"'=<>`]+|'[^']*'|"[^"]*"))?)*\s*(/\s*)?>)"#,
    )
    .ok()
});

fn html_tag(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '<' || pos + 1 == cx.end() {
        return None;
    }
    let after = cx.slice(pos + 1, cx.end());
    if let Some(m) = AUTOLINK_TAG.as_ref().and_then(|r| r.find(after)) {
        let len = m.end();
        return Some(cx.append(Elt::with(
            NodeKind::Autolink,
            pos,
            pos + 1 + len,
            vec![
                Elt::new(NodeKind::LinkMark, pos, pos + 1),
                Elt::new(NodeKind::Url, pos + 1, pos + len),
                Elt::new(NodeKind::LinkMark, pos + len, pos + 1 + len),
            ],
        )));
    }
    if let Some(m) = HTML_COMMENT.as_ref().and_then(|r| r.find(after)) {
        return Some(cx.append(Elt::new(NodeKind::Comment, pos, pos + 1 + m.end())));
    }
    if let Some(m) = HTML_PROC.as_ref().and_then(|r| r.find(after)) {
        return Some(cx.append(Elt::new(
            NodeKind::ProcessingInstruction,
            pos,
            pos + 1 + m.end(),
        )));
    }
    let m = HTML_TAG.as_ref()?.find(after)?;
    Some(cx.append(Elt::new(NodeKind::HtmlTag, pos, pos + 1 + m.end())))
}

// --- Writer: emoji `:word:` (`fold/emoji.ts:9-37`) -----------------------

fn emoji(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != ':' {
        return None;
    }
    // `/^\w+:/` after the colon.
    let after = cx.slice(pos + 1, cx.end());
    let words = after.bytes().take_while(|b| is_word(*b as char)).count();
    let open = words > 0 && after.as_bytes().get(words) == Some(&b':');
    // `/:\w+$/` before it, from the start of the section.
    let before = cx.slice(cx.offset, pos);
    let back = before
        .bytes()
        .rev()
        .take_while(|b| is_word(*b as char))
        .count();
    let close =
        back > 0 && before.len() > back && before.as_bytes()[before.len() - back - 1] == b':';
    if !open && !close {
        return None;
    }
    Some(cx.add_delimiter(DelimType::Emoji, pos, pos + 1, open, close))
}

// --- Writer: dashes (`fold/dashes.ts:6-27`) ------------------------------

fn dash(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '-' || (pos > 1 && cx.byte(pos - 1) == Some(b'-')) {
        return None;
    }
    let end = cx.end();
    let mut i = pos;
    while i < end && cx.byte(i) == Some(b'-') {
        i += 1;
    }
    if i - pos > 3 {
        return None;
    }
    Some(cx.append(Elt::new(NodeKind::Dash, pos, i)))
}

fn emphasis(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '_' && next != '*' {
        return None;
    }
    let mark = next as u8;
    let mut end = pos + 1;
    while cx.byte(end) == Some(mark) {
        end += 1;
    }
    let before = cx.char_before(pos);
    let after = cx.char(end);
    let (p_before, p_after) = (is_punctuation(before), is_punctuation(after));
    let (s_before, s_after) = (is_space_or_edge(before), is_space_or_edge(after));
    let left = !s_after && (!p_after || s_before || p_before);
    let right = !s_before && (!p_before || s_after || p_after);
    let can_open = left && (next == '*' || !right || p_before);
    let can_close = right && (next == '*' || !left || p_after);
    let ty = if next == '_' {
        DelimType::EmphasisUnderscore
    } else {
        DelimType::EmphasisAsterisk
    };
    Some(cx.add_delimiter(ty, pos, end, can_open, can_close))
}

fn strikethrough(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '~' || cx.byte(pos + 1) != Some(b'~') || cx.byte(pos + 2) == Some(b'~') {
        return None;
    }
    let before = cx.char_before(pos);
    let after = cx.char(pos + 2);
    let (s_before, s_after) = (is_space_or_edge(before), is_space_or_edge(after));
    let (p_before, p_after) = (is_punctuation(before), is_punctuation(after));
    Some(cx.add_delimiter(
        DelimType::Strikethrough,
        pos,
        pos + 2,
        !s_after && (!p_after || s_before || p_before),
        !s_before && (!p_before || s_after || p_after),
    ))
}

fn hard_break(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    // `\` + newline never reaches here: Writer's escape takes every `\`.
    if next != ' ' {
        return None;
    }
    let mut p = pos + 1;
    while cx.byte(p) == Some(b' ') {
        p += 1;
    }
    if cx.byte(p) == Some(b'\n') && p >= pos + 2 {
        return Some(cx.append(Elt::new(NodeKind::HardBreak, pos, p + 1)));
    }
    None
}

// --- Writer: nested links read as text (`nestedLinkAsPlainText.ts`) ------

fn parse_bracketed_segment(cx: &Cx<'_>, start: usize) -> Option<usize> {
    if cx.byte(start) != Some(b'[') {
        return None;
    }
    let mut depth = 0;
    let mut pos = start + 1;
    while pos < cx.end() {
        match cx.byte(pos) {
            Some(b'\\') => pos += 1,
            Some(b'[') => depth += 1,
            Some(b']') => {
                if depth == 0 {
                    return Some(pos + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
        pos += 1;
    }
    None
}

fn parse_parenthesized_segment(cx: &Cx<'_>, start: usize) -> Option<usize> {
    if cx.byte(start) != Some(b'(') {
        return None;
    }
    let mut depth = 1;
    let mut pos = start + 1;
    while pos < cx.end() {
        match cx.byte(pos) {
            Some(b'\\') => pos += 1,
            Some(b'(') => depth += 1,
            Some(b')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos + 1);
                }
            }
            _ => {}
        }
        pos += 1;
    }
    None
}

fn nested_link_as_plain_text(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    if next != '[' || !cx.has_open_link() {
        return None;
    }
    let Some(label_end) = parse_bracketed_segment(cx, pos) else {
        return Some(pos + 1);
    };
    match cx.byte(label_end) {
        Some(b'(') => Some(parse_parenthesized_segment(cx, label_end).unwrap_or(label_end)),
        Some(b'[') => Some(parse_bracketed_segment(cx, label_end).unwrap_or(label_end)),
        _ => Some(label_end),
    }
}

fn link(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    (next == '[').then(|| cx.add_delimiter(DelimType::LinkStart, pos, pos + 1, true, false))
}

fn image(cx: &mut Cx<'_>, next: char, pos: usize) -> Option<usize> {
    (next == '!' && cx.byte(pos + 1) == Some(b'['))
        .then(|| cx.add_delimiter(DelimType::ImageStart, pos, pos + 2, true, false))
}

// --- Writer: destinations with spaces (`spaceDestinationLinks.ts`) -------

fn is_inline_space(b: Option<u8>) -> bool {
    matches!(b, Some(b' ' | b'\t'))
}

fn trim_inline_space_end(cx: &Cx<'_>, from: usize, to: usize) -> usize {
    let mut end = to;
    while end > from && is_inline_space(cx.byte(end - 1)) {
        end -= 1;
    }
    end
}

fn is_escaped(cx: &Cx<'_>, pos: usize) -> bool {
    let mut backslashes = 0;
    let mut index = pos;
    while index > 0 && cx.byte(index - 1) == Some(b'\\') {
        backslashes += 1;
        index -= 1;
    }
    backslashes % 2 == 1
}

struct Destination {
    content_from: usize,
    content_to: usize,
    close_paren: usize,
}

fn parse_space_destination(cx: &Cx<'_>, open_paren: usize) -> Option<Destination> {
    if cx.byte(open_paren) != Some(b'(') {
        return None;
    }
    let content_from = cx.skip_space(open_paren + 1);
    let mut depth = 0;
    let mut escaped = false;
    let mut pos = content_from;
    while pos < cx.end() {
        let b = cx.byte(pos);
        if escaped {
            escaped = false;
        } else {
            match b {
                Some(b'\\') => escaped = true,
                Some(b'\n') => return None,
                Some(b'(') => depth += 1,
                Some(b')') => {
                    if depth == 0 {
                        return Some(Destination {
                            content_from,
                            content_to: trim_inline_space_end(cx, content_from, pos),
                            close_paren: pos,
                        });
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        pos += 1;
    }
    None
}

fn find_opening_quote(cx: &Cx<'_>, from: usize, to: usize, quote: u8) -> Option<usize> {
    let mut pos = to.checked_sub(2)?;
    loop {
        if pos < from {
            return None;
        }
        if cx.byte(pos) == Some(quote) && !is_escaped(cx, pos) {
            return Some(pos);
        }
        pos = pos.checked_sub(1)?;
    }
}

fn find_opening_paren_title(cx: &Cx<'_>, from: usize, to: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut pos = to.checked_sub(1)?;
    loop {
        if pos < from {
            return None;
        }
        if !is_escaped(cx, pos) {
            match cx.byte(pos) {
                Some(b')') => depth += 1,
                Some(b'(') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(pos);
                    }
                }
                _ => {}
            }
        }
        pos = pos.checked_sub(1)?;
    }
}

struct TitleSplit {
    url_from: usize,
    url_to: usize,
    title: Option<(usize, usize)>,
}

fn split_trailing_title(cx: &Cx<'_>, content_from: usize, content_to: usize) -> TitleSplit {
    let plain = TitleSplit {
        url_from: content_from,
        url_to: content_to,
        title: None,
    };
    if content_to <= content_from {
        return plain;
    }
    let last = cx.byte(content_to - 1);
    let opening = match last {
        Some(q @ (b'"' | b'\'')) if !is_escaped(cx, content_to - 1) => {
            find_opening_quote(cx, content_from, content_to, q)
        }
        Some(b')') if !is_escaped(cx, content_to - 1) => {
            find_opening_paren_title(cx, content_from, content_to)
        }
        _ => None,
    };
    match opening {
        Some(opening) if opening > content_from && is_inline_space(cx.byte(opening - 1)) => {
            TitleSplit {
                url_from: content_from,
                url_to: trim_inline_space_end(cx, content_from, opening - 1),
                title: Some((opening, content_to)),
            }
        }
        _ => plain,
    }
}

fn space_destination_link_end(cx: &mut Cx<'_>, next: char, close_bracket: usize) -> Option<usize> {
    if next != ']' {
        return None;
    }
    let link_opening = cx.find_opening_delimiter(DelimType::LinkStart);
    let image_opening = cx.find_opening_delimiter(DelimType::ImageStart);
    let index = link_opening.into_iter().chain(image_opening).max()?;
    let opening = cx.delimiter_at(index)?.clone();
    let open_paren = close_bracket + 1;
    if cx.byte(open_paren) != Some(b'(') {
        return None;
    }
    let destination = parse_space_destination(cx, open_paren)?;
    let split = split_trailing_title(cx, destination.content_from, destination.content_to);
    if !(split.url_from..split.url_to).any(|p| is_inline_space(cx.byte(p))) {
        return None;
    }
    let mut content = cx.take_content(index);
    content.insert(0, Elt::new(NodeKind::LinkMark, opening.from, opening.to));
    content.push(Elt::new(
        NodeKind::LinkMark,
        close_bracket,
        close_bracket + 1,
    ));
    content.push(Elt::new(NodeKind::LinkMark, open_paren, open_paren + 1));
    content.push(Elt::new(NodeKind::Url, split.url_from, split.url_to));
    if let Some((from, to)) = split.title {
        content.push(Elt::new(NodeKind::LinkTitle, from, to));
    }
    content.push(Elt::new(
        NodeKind::LinkMark,
        destination.close_paren,
        destination.close_paren + 1,
    ));
    let kind = if opening.ty == DelimType::ImageStart {
        NodeKind::Image
    } else {
        NodeKind::Link
    };
    Some(cx.append(Elt::with(
        kind,
        opening.from,
        destination.close_paren + 1,
        content,
    )))
}

// --- Lezer: link end ------------------------------------------------------

fn link_end(cx: &mut Cx<'_>, next: char, start: usize) -> Option<usize> {
    if next != ']' {
        return None;
    }
    let mut i = cx.parts.len();
    while i > 0 {
        i -= 1;
        let Some(Part::Delim(part)) = &cx.parts[i] else {
            continue;
        };
        if !matches!(part.ty, DelimType::LinkStart | DelimType::ImageStart) {
            continue;
        }
        let part = part.clone();
        let after = cx.byte(start + 1);
        if part.side == 0
            || (cx.skip_space(part.to) == start && !matches!(after, Some(b'(' | b'[')))
        {
            cx.parts[i] = None;
            return None;
        }
        let content = cx.take_content(i);
        let kind = if part.ty == DelimType::LinkStart {
            NodeKind::Link
        } else {
            NodeKind::Image
        };
        let link = finish_link(cx, content, kind, part.from, start + 1);
        let to = link.to;
        cx.parts.push(Some(Part::Elt(link)));
        if part.ty == DelimType::LinkStart {
            for p in cx.parts[..i].iter_mut().flatten() {
                if let Part::Delim(d) = p
                    && d.ty == DelimType::LinkStart
                {
                    d.side = 0;
                }
            }
        }
        return Some(to);
    }
    None
}

fn finish_link(
    cx: &Cx<'_>,
    mut content: Vec<Elt>,
    kind: NodeKind,
    start: usize,
    start_pos: usize,
) -> Elt {
    let mark_len = if kind == NodeKind::Image { 2 } else { 1 };
    content.insert(0, Elt::new(NodeKind::LinkMark, start, start + mark_len));
    content.push(Elt::new(NodeKind::LinkMark, start_pos - 1, start_pos));
    let mut end_pos = start_pos;
    match cx.byte(start_pos) {
        Some(b'(') => {
            let mut pos = cx.skip_space(start_pos + 1);
            let dest = parse_url(cx, pos);
            let mut title = None;
            if let Some(Some(dest)) = &dest {
                pos = cx.skip_space(dest.to);
                if pos != dest.to {
                    title = parse_link_title(cx, pos).flatten();
                    if let Some(t) = &title {
                        pos = cx.skip_space(t.to);
                    }
                }
            }
            if cx.byte(pos) == Some(b')') {
                content.push(Elt::new(NodeKind::LinkMark, start_pos, start_pos + 1));
                end_pos = pos + 1;
                if let Some(Some(dest)) = dest {
                    content.push(dest);
                }
                if let Some(title) = title {
                    content.push(title);
                }
                content.push(Elt::new(NodeKind::LinkMark, pos, end_pos));
            }
        }
        Some(b'[') => {
            if let Some(Some(label)) = parse_link_label(cx, start_pos, false) {
                end_pos = label.to;
                content.push(label);
            }
        }
        _ => {}
    }
    Elt::with(kind, start, end_pos, content)
}

/// `None`: ran off the end; `Some(None)`: failed; `Some(Some(url))`.
pub(crate) fn parse_url_in(text: &str, offset: usize, start: usize) -> Option<Option<Elt>> {
    let cx = Cx {
        text,
        offset,
        parts: Vec::new(),
    };
    parse_url(&cx, start)
}

fn parse_url(cx: &Cx<'_>, start: usize) -> Option<Option<Elt>> {
    let end = cx.end();
    if cx.byte(start) == Some(b'<') {
        let mut pos = start + 1;
        while pos < end {
            match cx.byte(pos) {
                Some(b'>') => return Some(Some(Elt::new(NodeKind::Url, start, pos + 1))),
                Some(b'<' | b'\n') => return Some(None),
                _ => {}
            }
            pos += 1;
        }
        return None;
    }
    let mut depth = 0;
    let mut pos = start;
    let mut escaped = false;
    while pos < end {
        let Some(b) = cx.byte(pos) else { break };
        if is_space(b) {
            break;
        } else if escaped {
            escaped = false;
        } else if b == b'(' {
            depth += 1;
        } else if b == b')' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if b == b'\\' {
            escaped = true;
        }
        pos += 1;
    }
    if pos > start {
        Some(Some(Elt::new(NodeKind::Url, start, pos)))
    } else if pos == end {
        None
    } else {
        Some(None)
    }
}

pub(crate) fn parse_link_title_in(text: &str, offset: usize, start: usize) -> Option<Option<Elt>> {
    let cx = Cx {
        text,
        offset,
        parts: Vec::new(),
    };
    parse_link_title(&cx, start)
}

fn parse_link_title(cx: &Cx<'_>, start: usize) -> Option<Option<Elt>> {
    let close = match cx.byte(start) {
        Some(b'"') => b'"',
        Some(b'\'') => b'\'',
        Some(b'(') => b')',
        _ => return Some(None),
    };
    let mut escaped = false;
    let mut pos = start + 1;
    while pos < cx.end() {
        let b = cx.byte(pos);
        if escaped {
            escaped = false;
        } else if b == Some(close) {
            return Some(Some(Elt::new(NodeKind::LinkTitle, start, pos + 1)));
        } else if b == Some(b'\\') {
            escaped = true;
        }
        pos += 1;
    }
    None
}

pub(crate) fn parse_link_label_in(
    text: &str,
    offset: usize,
    start: usize,
    require_non_ws: bool,
) -> Option<Option<Elt>> {
    let cx = Cx {
        text,
        offset,
        parts: Vec::new(),
    };
    parse_link_label(&cx, start, require_non_ws)
}

fn parse_link_label(cx: &Cx<'_>, start: usize, mut require_non_ws: bool) -> Option<Option<Elt>> {
    let end = cx.end().min(start + 1 + 999);
    let mut escaped = false;
    let mut pos = start + 1;
    while pos < end {
        let b = cx.byte(pos);
        if escaped {
            escaped = false;
        } else if b == Some(b']') {
            return Some(if require_non_ws {
                None
            } else {
                Some(Elt::new(NodeKind::LinkLabel, start, pos + 1))
            });
        } else {
            if require_non_ws && !b.is_some_and(is_space) {
                require_non_ws = false;
            }
            if b == Some(b'[') {
                return Some(None);
            } else if b == Some(b'\\') {
                escaped = true;
            }
        }
        pos += 1;
    }
    None
}

// --- GFM autolink literals -----------------------------------------------
//
// Lezer's regexes (dist/index.js:2185-2262), hand-written: they run at every
// word start, where a regex engine's setup dominates.

fn is_url_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_email_local(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-')
}

/// `urlRE`, `/[\w-]+(\.[\w-]+)+(\/[^\s<]*)?/y`: end of the match at `from`
/// and end of its domain part.
fn match_url_body(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let run = |at: usize| {
        bytes[at.min(bytes.len())..]
            .iter()
            .take_while(|b| is_url_word(**b))
            .count()
    };
    let first = run(from);
    if first == 0 {
        return None;
    }
    let mut end = from + first;
    let mut labels = 1;
    while bytes.get(end) == Some(&b'.') {
        let next = run(end + 1);
        if next == 0 {
            break;
        }
        end += 1 + next;
        labels += 1;
    }
    if labels < 2 {
        return None;
    }
    let domain_end = end;
    if bytes.get(end) == Some(&b'/') {
        end += 1;
        while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'<' {
            // `\s` is Unicode whitespace: stop at it as well.
            if bytes[end] >= 0x80 {
                let rest = std::str::from_utf8(&bytes[end..])
                    .ok()
                    .and_then(|s| s.chars().next());
                if rest.is_some_and(char::is_whitespace) {
                    break;
                }
            }
            end += 1;
        }
    }
    Some((end, domain_end))
}

/// `/&(?:#\d+|#x[a-f\d]+|\w+);$/`: start of a trailing entity in `segment`.
fn trailing_entity(segment: &[u8]) -> Option<usize> {
    let amp = segment.iter().rposition(|b| *b == b'&')?;
    let body = segment.get(amp + 1..segment.len().checked_sub(1)?)?;
    let valid = if let Some(hex) = body.strip_prefix(b"#x") {
        !hex.is_empty()
            && hex
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    } else if let Some(digits) = body.strip_prefix(b"#") {
        !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
    } else {
        !body.is_empty() && body.iter().all(|b| is_word(*b as char))
    };
    valid.then_some(amp)
}

/// Lezer's `autolinkURLEnd`; `text` is the section, `from` an index into it.
fn autolink_url_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (body_end, domain_end) = match_url_body(bytes, from)?;
    // `lastTwoDomainWords` must not contain `_`.
    let domain = &bytes[from..domain_end];
    let mut dots = domain
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'.')
        .map(|(i, _)| i);
    let second_last_dot = {
        let all: Vec<usize> = dots.by_ref().collect();
        all.len().checked_sub(2).map(|i| all[i] + 1).unwrap_or(0)
    };
    if domain[second_last_dot..].contains(&b'_') {
        return None;
    }
    let mut end = body_end;
    let mut open = bytes[from..end].iter().filter(|b| **b == b'(').count();
    let mut close = bytes[from..end].iter().filter(|b| **b == b')').count();
    while end > from {
        let last = bytes[end - 1];
        if matches!(last, b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~')
            || (last == b')' && close > open)
        {
            if last == b')' {
                close -= 1;
            }
            if last == b'(' {
                open -= 1;
            }
            end -= 1;
        } else if last == b';' {
            match trailing_entity(&bytes[from..end]) {
                Some(amp) => {
                    let cut = &bytes[from + amp..end];
                    open -= cut.iter().filter(|b| **b == b'(').count();
                    close -= cut.iter().filter(|b| **b == b')').count();
                    end = from + amp;
                }
                None => break,
            }
        } else {
            break;
        }
    }
    Some(end)
}

/// Lezer's `autolinkEmailEnd` (`emailRE`, `/[\w.+-]+@[\w-]+(\.[\w.-]+)+/y`).
fn autolink_email_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let local = bytes
        .get(from..)?
        .iter()
        .take_while(|b| is_email_local(**b))
        .count();
    if local == 0 || bytes.get(from + local) != Some(&b'@') {
        return None;
    }
    let mut end = from + local + 1;
    let host = bytes[end..].iter().take_while(|b| is_url_word(**b)).count();
    if host == 0 {
        return None;
    }
    end += host;
    let mut parts = 0;
    while bytes.get(end) == Some(&b'.') {
        let part = bytes[end + 1..]
            .iter()
            .take_while(|b| is_url_word(**b) || **b == b'.')
            .count();
        if part == 0 {
            break;
        }
        end += 1 + part;
        parts += 1;
    }
    if parts == 0 {
        return None;
    }
    let last = bytes[end - 1];
    if last == b'_' || last == b'-' {
        return None;
    }
    Some(end - usize::from(last == b'.'))
}

/// `/([^\[\]]|\[[^\]]*\])*/` on `bytes`: length of the bracket-balanced prefix.
fn no_bracket_prefix(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b']' => break,
            b'[' => match bytes[i + 1..].iter().position(|b| *b == b']') {
                Some(close) => i += close + 2,
                None => break,
            },
            _ => i += 1,
        }
    }
    i
}

fn autolink(cx: &mut Cx<'_>, _next: char, abs: usize) -> Option<usize> {
    let pos = abs - cx.offset;
    if pos > 0 && cx.char_before(abs).is_some_and(is_word) {
        return None;
    }
    let text = cx.text;
    let rest = text.get(pos..)?;
    let lower_starts = |prefix: &str| rest.starts_with(prefix);
    let end = if lower_starts("www.") || lower_starts("http://") || lower_starts("https://") {
        let skip = if lower_starts("www.") {
            4
        } else if lower_starts("http://") {
            7
        } else {
            8
        };
        let mut end = autolink_url_end(text, pos + skip)?;
        if cx.has_open_link() {
            end = pos + no_bracket_prefix(text.as_bytes().get(pos..end)?);
        }
        end
    } else if lower_starts("mailto:") || lower_starts("xmpp:") {
        let skip = if lower_starts("mailto:") { 7 } else { 5 };
        let end = autolink_email_end(text, pos + skip)?;
        if lower_starts("xmpp:") && text.as_bytes().get(end) == Some(&b'/') {
            let resource = text.as_bytes()[end + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.'))
                .count();
            if resource > 0 {
                end + 1 + resource
            } else {
                end
            }
        } else {
            end
        }
    } else {
        // `[\w.+-]{1,100}@`: an e-mail address.
        let local = rest
            .bytes()
            .take(101)
            .take_while(|b| is_email_local(*b))
            .count();
        if local == 0 || local > 100 || rest.as_bytes().get(local) != Some(&b'@') {
            return None;
        }
        autolink_email_end(text, pos)?
    };
    let end = end.min(text.len());
    cx.append(Elt::new(NodeKind::Url, abs, end + cx.offset));
    Some(end + cx.offset)
}

/// Lezer's `injectMarks`: puts container marks (quote marks of continuation
/// lines) into the inline elements that contain them.
pub(crate) fn inject_marks(elements: Vec<Elt>, marks: &[Elt]) -> Vec<Elt> {
    if marks.is_empty() {
        return elements;
    }
    if elements.is_empty() {
        return marks.to_vec();
    }
    let mut elts = elements;
    let mut e = 0;
    for mark in marks {
        while e < elts.len() && elts[e].to < mark.to {
            e += 1;
        }
        if e < elts.len() && elts[e].from < mark.from {
            let target = &mut elts[e];
            let children = std::mem::take(&mut target.children);
            target.children = inject_marks(children, std::slice::from_ref(mark));
        } else {
            elts.insert(e, mark.clone());
            e += 1;
        }
    }
    elts
}
