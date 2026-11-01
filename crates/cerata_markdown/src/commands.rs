//! Formatting commands and their registry (`markdown-formatting.ts`): one
//! table of stable ids feeds the shortcuts and the menus.
//!
//! Shortcuts follow `docs/keyboard-shortcuts.md:50-98` on macOS. On Windows
//! and Linux, Cmd becomes Ctrl, and no editor shortcut uses Ctrl+Alt: that
//! chord is AltGr on European layouts (AZERTY types `~ # { [ | \ @ ] }` with
//! it), so the headings move from Cmd+Alt+N to Ctrl+Shift+N there.

use std::ops::Range;

use crate::change::{Bias, Change, ChangeSet};
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::tree::NodeKind;

/// Local date and time, supplied by the caller (the editor crate has no
/// clock or time zone of its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
}

/// A key chord: modifiers and the key (`b`, `1`, `.`, `Enter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Cmd on macOS.
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: &'static str,
}

impl Chord {
    /// `Mod-Shift-x` style, `Mod` meaning Cmd on macOS and Ctrl elsewhere.
    fn parse(spec: &'static str, os: Os) -> Chord {
        let mut chord = Chord {
            cmd: false,
            ctrl: false,
            alt: false,
            shift: false,
            key: "",
        };
        let mut parts: Vec<&'static str> = spec.split('-').collect();
        // `Mod-Shift--`would split oddly; no command binds `-`.
        chord.key = parts.pop().unwrap_or("");
        for part in parts {
            match part {
                "Mod" if os == Os::MacOs => chord.cmd = true,
                "Mod" | "Ctrl" => chord.ctrl = true,
                "Alt" => chord.alt = true,
                "Shift" => chord.shift = true,
                _ => {}
            }
        }
        chord
    }

    /// How menus show it: `Cmd+Shift+X`, `Ctrl+Alt+1`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.cmd {
            parts.push("Cmd".to_owned());
        }
        if self.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        parts.push(if self.key.chars().count() == 1 {
            self.key.to_uppercase()
        } else {
            self.key.to_owned()
        });
        parts.join("+")
    }
}

type Run = fn(&mut EditorState, &Clock) -> bool;

/// One registry entry.
pub struct EditorCommand {
    /// Stable id (`format.bold`), used by menus, the palette and settings.
    pub id: &'static str,
    pub label: &'static str,
    mac: Option<&'static str>,
    /// Windows and Linux, when it differs from `mac`.
    other: Option<&'static str>,
    run: Run,
}

impl EditorCommand {
    pub fn shortcut(&self, os: Os) -> Option<Chord> {
        let spec = match os {
            Os::MacOs => self.mac,
            Os::Windows | Os::Linux => self.other.or(self.mac),
        }?;
        Some(Chord::parse(spec, os))
    }
}

macro_rules! command {
    ($id:literal, $label:literal, $mac:expr, $other:expr, $run:expr) => {
        EditorCommand {
            id: $id,
            label: $label,
            mac: $mac,
            other: $other,
            run: $run,
        }
    };
}

/// Every formatting command (`markdown-formatting.ts:421-448`).
pub static COMMANDS: &[EditorCommand] = &[
    command!("format.bold", "Bold", Some("Mod-b"), None, |s, _| s
        .toggle_inline("**", NodeKind::StrongEmphasis)),
    command!("format.italic", "Italic", Some("Mod-i"), None, |s, _| s
        .toggle_inline("*", NodeKind::Emphasis)),
    command!(
        "format.link",
        "Link\u{2026}",
        Some("Mod-k"),
        None,
        |s, _| s.insert_link()
    ),
    command!("format.code", "Inline code", Some("Mod-e"), None, |s, _| s
        .toggle_inline("`", NodeKind::InlineCode)),
    command!(
        "format.strikethrough",
        "Strikethrough",
        Some("Mod-Shift-x"),
        None,
        |s, _| { s.toggle_inline("~~", NodeKind::Strikethrough) }
    ),
    command!(
        "format.clearFormatting",
        "Clear formatting",
        None,
        None,
        |s, _| s.clear_inline_formatting()
    ),
    command!(
        "format.bulletList",
        "Bullet list",
        Some("Mod-Shift-8"),
        None,
        |s, _| s.line_command(LineCommand::Bullet)
    ),
    command!(
        "format.numberedList",
        "Numbered list",
        Some("Mod-Shift-7"),
        None,
        |s, _| s.line_command(LineCommand::Numbered)
    ),
    command!(
        "format.blockquote",
        "Blockquote",
        Some("Mod-Shift-."),
        None,
        |s, _| s.line_command(LineCommand::Quote)
    ),
    command!(
        "format.taskList",
        "Task list",
        Some("Mod-Shift-Enter"),
        None,
        |s, _| s.line_command(LineCommand::Task)
    ),
    command!("format.codeBlock", "Code block", None, None, |s, _| s
        .toggle_fenced_code_block()),
    command!(
        "format.heading1",
        "Heading 1",
        Some("Mod-Alt-1"),
        Some("Mod-Shift-1"),
        |s, _| s.line_command(LineCommand::Heading(1))
    ),
    command!(
        "format.heading2",
        "Heading 2",
        Some("Mod-Alt-2"),
        Some("Mod-Shift-2"),
        |s, _| s.line_command(LineCommand::Heading(2))
    ),
    command!(
        "format.heading3",
        "Heading 3",
        Some("Mod-Alt-3"),
        Some("Mod-Shift-3"),
        |s, _| s.line_command(LineCommand::Heading(3))
    ),
    command!(
        "format.heading4",
        "Heading 4",
        Some("Mod-Alt-4"),
        Some("Mod-Shift-4"),
        |s, _| s.line_command(LineCommand::Heading(4))
    ),
    command!(
        "format.heading5",
        "Heading 5",
        Some("Mod-Alt-5"),
        Some("Mod-Shift-5"),
        |s, _| s.line_command(LineCommand::Heading(5))
    ),
    command!(
        "format.heading6",
        "Heading 6",
        Some("Mod-Alt-6"),
        Some("Mod-Shift-6"),
        |s, _| s.line_command(LineCommand::Heading(6))
    ),
    command!(
        "format.paragraph",
        "Paragraph",
        Some("Mod-Alt-0"),
        Some("Mod-Shift-0"),
        |s, _| s.line_command(LineCommand::Paragraph)
    ),
    command!("insert.table", "Table", None, None, |s, _| s.insert_table()),
    command!(
        "insert.horizontalRule",
        "Horizontal rule",
        None,
        None,
        |s, _| s.insert_horizontal_rule()
    ),
    command!("insert.today", "Current date", None, None, |s, c| {
        let date = format!("{:04}-{:02}-{:02}", c.year, c.month, c.day);
        s.insert_at_head(date)
    }),
    command!("insert.now", "Current time", None, None, |s, c| {
        let time = format!("{:02}:{:02}", c.hour, c.minute);
        s.insert_at_head(time)
    }),
];

pub fn command(id: &str) -> Option<&'static EditorCommand> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// The command a chord triggers on `os`, if any.
pub fn command_for(chord: &Chord, os: Os) -> Option<&'static EditorCommand> {
    COMMANDS
        .iter()
        .find(|c| c.shortcut(os).as_ref() == Some(chord))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineCommand {
    Heading(usize),
    Paragraph,
    Bullet,
    Numbered,
    Quote,
    Task,
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

/// `^(#{1,6})\s`: length of a heading prefix.
fn heading_prefix(s: &str) -> Option<usize> {
    let hashes = s.bytes().take_while(|b| *b == b'#').count();
    ((1..=6).contains(&hashes) && s[hashes..].starts_with(char::is_whitespace))
        .then(|| hashes + s[hashes..].chars().next().map_or(0, char::len_utf8))
}

/// `^([ \t]*)- `: indent length of a bullet line.
fn bullet(s: &str) -> Option<usize> {
    let ws = leading_ws(s);
    s[ws..].starts_with("- ").then_some(ws)
}

/// `^([ \t]*)- \[[ x]\] `: indent length of a task line.
fn task(s: &str) -> Option<usize> {
    let ws = leading_ws(s);
    let rest = &s[ws..];
    (rest.starts_with("- [")
        && matches!(rest.as_bytes().get(3), Some(b' ' | b'x'))
        && rest[4..].starts_with("] "))
    .then_some(ws)
}

/// `^\d+\.\s`: length of a numbered prefix.
fn numbered(s: &str) -> Option<usize> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && s[digits..].starts_with('.') && s[digits + 1..].starts_with(char::is_whitespace))
        .then(|| digits + 1 + s[digits + 1..].chars().next().map_or(0, char::len_utf8))
}

fn transform(command: LineCommand, line: &str, index: usize, lines: &[&str]) -> String {
    match command {
        LineCommand::Heading(level) => {
            let prefix = "#".repeat(level) + " ";
            match heading_prefix(line) {
                Some(len) => prefix + &line[len..],
                None => prefix + line,
            }
        }
        LineCommand::Paragraph => {
            heading_prefix(line).map_or_else(|| line.to_owned(), |len| line[len..].to_owned())
        }
        LineCommand::Bullet => {
            let plain = |l: &str| bullet(l).is_some() && task(l).is_none();
            if lines.iter().all(|l| plain(l)) {
                let ws = bullet(line).unwrap_or(0);
                return format!("{}{}", &line[..ws], &line[ws + 2..]);
            }
            if let Some(ws) = task(line) {
                return format!("{}- {}", &line[..ws], &line[ws + 6..]);
            }
            if bullet(line).is_some() {
                return line.to_owned();
            }
            let ws = leading_ws(line);
            format!("{}- {}", &line[..ws], &line[ws..])
        }
        LineCommand::Numbered => {
            if lines.iter().all(|l| numbered(l).is_some()) {
                return line[numbered(line).unwrap_or(0)..].to_owned();
            }
            if numbered(line).is_some() {
                return line.to_owned();
            }
            format!("{}. {line}", index + 1)
        }
        LineCommand::Quote => {
            if lines.iter().all(|l| l.starts_with("> ")) {
                return line.strip_prefix("> ").unwrap_or(line).to_owned();
            }
            if line.starts_with("> ") {
                return line.to_owned();
            }
            format!("> {line}")
        }
        LineCommand::Task => {
            if lines.iter().all(|l| task(l).is_some()) {
                let ws = task(line).unwrap_or(0);
                return format!("{}{}", &line[..ws], &line[ws + 6..]);
            }
            if task(line).is_some() {
                return line.to_owned();
            }
            if let Some(ws) = bullet(line) {
                return format!("{}- [ ] {}", &line[..ws], &line[ws + 2..]);
            }
            let ws = leading_ws(line);
            format!("{}- [ ] {}", &line[..ws], &line[ws..])
        }
    }
}

/// The smallest single replacement turning `before` into `after`.
fn minimal_edit(before: &str, after: &str) -> Option<(Range<usize>, String)> {
    if before == after {
        return None;
    }
    let start = before
        .char_indices()
        .zip(after.chars())
        .find(|((_, a), b)| a != b)
        .map_or(before.len().min(after.len()), |((i, _), _)| i);
    let start = (0..=start)
        .rev()
        .find(|&i| before.is_char_boundary(i) && after.is_char_boundary(i))
        .unwrap_or(0);
    let max = before.len().min(after.len()) - start;
    let mut end = 0;
    for (a, b) in before[start..]
        .chars()
        .rev()
        .zip(after[start..].chars().rev())
    {
        if a != b || end + a.len_utf8() > max {
            break;
        }
        end += a.len_utf8();
    }
    Some((
        start..before.len() - end,
        after[start..after.len() - end].to_owned(),
    ))
}

/// Word characters around `pos` (CodeMirror's `wordAt`).
fn word_at(s: &str, pos: usize) -> Option<Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = s[..pos]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(pos, |(i, _)| i);
    let end = s[pos..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(s.len(), |(i, _)| pos + i);
    (start < end).then_some(start..end)
}

impl EditorState {
    /// Runs the command `id`: `false` when it does not exist, did nothing,
    /// or the document is still loading.
    pub fn run_command(&mut self, id: &str, clock: &Clock) -> bool {
        match command(id) {
            Some(command) if !self.is_loading() => (command.run)(self, clock),
            _ => false,
        }
    }

    /// Applies one edit per selection range: `f` returns the range's changes
    /// (in the current document) and its new selection (in the document
    /// after those changes alone), as CodeMirror's `changeByRange`.
    fn change_by_range(
        &mut self,
        event: UserEvent,
        f: impl Fn(&EditorState, SelectionRange) -> (Vec<Change>, SelectionRange),
    ) -> bool {
        let mut changes = Vec::new();
        let mut ranges = Vec::new();
        let mut shift: isize = 0;
        let delta = |c: &Change| c.insert.len() as isize - (c.to - c.from) as isize;
        for range in self.selection().ranges().to_vec() {
            let (edits, new_range) = f(self, range);
            // Two carets in one node or on one line ask for the same edit:
            // made once, and not shifted past by itself.
            let (repeated, fresh): (Vec<Change>, Vec<Change>) =
                edits.into_iter().partition(|c| changes.contains(c));
            let own = shift - repeated.iter().map(delta).sum::<isize>();
            let moved = |p: usize| (p as isize + own).max(0) as usize;
            ranges.push(SelectionRange::new(
                moved(new_range.anchor),
                moved(new_range.head),
            ));
            shift += fresh.iter().map(delta).sum::<isize>();
            changes.extend(fresh);
        }
        if changes.is_empty() && ranges.as_slice() == self.selection().ranges() {
            return false;
        }
        let selection = Selection::new(ranges, self.selection().main_index());
        self.apply(
            Transaction::new()
                .changes(changes)
                .select(selection)
                .event(event)
                .scroll(),
        )
        .is_ok()
    }

    /// Bold, italic, inline code, strikethrough: unwraps the node holding
    /// the selection, else wraps the selection, the word under the caret, or
    /// inserts empty markers with the caret between them.
    fn toggle_inline(&mut self, marker: &'static str, kind: NodeKind) -> bool {
        let len = marker.len();
        // Consumed even when nothing changes, as Writer's commands are.
        let _ = self.change_by_range(UserEvent::Command, |state, range| {
            let node = state.tree().resolve(range.from(), -1);
            let existing = std::iter::once(node)
                .chain(node.ancestors())
                .find(|n| n.kind() == kind);
            let s = state.doc();
            if let Some(node) = existing {
                let (from, to) = (node.from(), node.to());
                let inner = s
                    .get(from + len..to.saturating_sub(len).max(from + len))
                    .unwrap_or("")
                    .to_owned();
                let clamp = |p: usize| p.saturating_sub(len).clamp(from, from + inner.len());
                let selection = SelectionRange::new(clamp(range.anchor), clamp(range.head));
                return (vec![Change::new(from..to, inner)], selection);
            }
            if range.is_empty() {
                if let Some(word) = word_at(s, range.from()) {
                    let text = &s[word.clone()];
                    let start = word.start + len;
                    return (
                        vec![Change::new(word.clone(), format!("{marker}{text}{marker}"))],
                        SelectionRange::new(start, start + text.len()),
                    );
                }
                return (
                    vec![Change::insert(range.from(), format!("{marker}{marker}"))],
                    SelectionRange::cursor(range.from() + len),
                );
            }
            let selected = &s[range.range()];
            let start = range.from() + len;
            (
                vec![Change::new(
                    range.range(),
                    format!("{marker}{selected}{marker}"),
                )],
                SelectionRange::new(start, start + selected.len()),
            )
        });
        true
    }

    /// `[](url)` at the caret, or `[selection](url)`, with `url` selected; a
    /// caret already in a link does nothing.
    fn insert_link(&mut self) -> bool {
        self.change_by_range(UserEvent::Command, |state, range| {
            let node = state.tree().resolve(range.from(), -1);
            if std::iter::once(node)
                .chain(node.ancestors())
                .any(|n| n.kind() == NodeKind::Link)
            {
                return (Vec::new(), range);
            }
            let selected = &state.doc()[range.range()];
            let url = range.from() + 1 + selected.len() + 2;
            (
                vec![Change::new(range.range(), format!("[{selected}](url)"))],
                SelectionRange::new(url, url + 3),
            )
        })
    }

    /// Rewrites each touched line's prefix with a minimal edit and maps the
    /// selection through it, so the caret stays in its text.
    fn line_command(&mut self, command: LineCommand) -> bool {
        let _ = self.change_by_range(UserEvent::Command, |state, range| {
            let text = state.text();
            let (first, last) = (
                text.line_at(range.from()).number,
                text.line_at(range.to()).number,
            );
            let lines: Vec<&str> = (first..=last)
                .map(|n| text.slice(text.line(n).range()))
                .collect();
            let mut edits = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if let Some((range, insert)) =
                    minimal_edit(line, &transform(command, line, index, &lines))
                {
                    let from = text.line(first + index).from;
                    edits.push(Change::new(from + range.start..from + range.end, insert));
                }
            }
            let Ok(set) = ChangeSet::new(text, edits.clone()) else {
                return (Vec::new(), range);
            };
            // Positions here are in the document after this range's edits;
            // `change_by_range` shifts them past the earlier ranges' edits.
            let map = |p: usize| set.map_pos(p, Bias::Right);
            (
                edits,
                SelectionRange::new(map(range.anchor), map(range.head)),
            )
        });
        true
    }

    /// Strips `**`, `*`, `~~` and `` ` `` pairs inside each selection.
    fn clear_inline_formatting(&mut self) -> bool {
        static PATTERNS: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
        let patterns = PATTERNS.get_or_init(|| {
            [r"\*\*(.+?)\*\*", r"\*(.+?)\*", r"~~(.+?)~~", r"`(.+?)`"]
                .iter()
                .filter_map(|p| regex::Regex::new(p).ok())
                .collect()
        });
        self.change_by_range(UserEvent::Command, |state, range| {
            if range.is_empty() {
                return (Vec::new(), range);
            }
            let mut text = state.doc()[range.range()].to_owned();
            for pattern in patterns {
                text = pattern.replace_all(&text, "$1").into_owned();
            }
            let end = range.from() + text.len();
            (
                vec![Change::new(range.range(), text)],
                SelectionRange::new(range.from(), end),
            )
        })
    }

    /// Wraps the main selection's lines in a fence, or unwraps them when
    /// they already start and end with one.
    fn toggle_fenced_code_block(&mut self) -> bool {
        let main = self.selection().main();
        let text = self.text();
        let (first, last) = (text.line_at(main.from()), text.line_at(main.to()));
        let block = text.slice(first.from..last.to);
        let lines: Vec<&str> = block.split('\n').collect();
        let fenced = lines.len() >= 2
            && lines
                .first()
                .is_some_and(|l| l.trim_end().starts_with("```"))
            && lines.last().is_some_and(|l| l.trim_end() == "```");
        let insert = if fenced {
            lines[1..lines.len() - 1].join("\n")
        } else {
            format!("```\n{block}\n```")
        };
        self.apply(
            Transaction::new()
                .change(Change::new(first.from..last.to, insert))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    fn insert_table(&mut self) -> bool {
        const TABLE: &str = "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n|  |  |  |";
        let pos = self.selection().main().head;
        let caret = pos + TABLE.rfind("|  |  |  |").unwrap_or(0) + 2;
        self.apply(
            Transaction::new()
                .change(Change::insert(pos, TABLE))
                .select(Selection::cursor(caret))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    fn insert_horizontal_rule(&mut self) -> bool {
        let pos = self.selection().main().head;
        let line = self.text().line_at(pos);
        let prefix = if self.text().slice(line.range()).trim().is_empty() {
            ""
        } else {
            "\n"
        };
        self.apply(
            Transaction::new()
                .change(Change::insert(pos, format!("{prefix}---\n")))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    fn insert_at_head(&mut self, insert: String) -> bool {
        let pos = self.selection().main().head;
        self.apply(
            Transaction::new()
                .change(Change::insert(pos, insert))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }
}

/// A keyboard layout, for the shortcut checks: each key's character
/// without and with Shift.
pub struct Layout {
    pub name: &'static str,
    pub keys: &'static [(&'static str, &'static str)],
}

/// US QWERTY, the keys the registry binds.
pub static QWERTY_US: Layout = Layout {
    name: "QWERTY (US)",
    keys: &[
        ("1", "!"),
        ("2", "@"),
        ("3", "#"),
        ("4", "$"),
        ("5", "%"),
        ("6", "^"),
        ("7", "&"),
        ("8", "*"),
        ("9", "("),
        ("0", ")"),
        (".", ">"),
        (",", "<"),
        ("b", "B"),
        ("i", "I"),
        ("k", "K"),
        ("e", "E"),
        ("x", "X"),
        ("Enter", "Enter"),
    ],
};

/// French AZERTY: digits need Shift, `.` is Shift+`;`.
pub static AZERTY_FR: Layout = Layout {
    name: "AZERTY (FR)",
    keys: &[
        ("&", "1"),
        ("\u{e9}", "2"),
        ("\"", "3"),
        ("'", "4"),
        ("(", "5"),
        ("-", "6"),
        ("\u{e8}", "7"),
        ("_", "8"),
        ("\u{e7}", "9"),
        ("\u{e0}", "0"),
        (";", "."),
        (",", "?"),
        ("b", "B"),
        ("i", "I"),
        ("k", "K"),
        ("e", "E"),
        ("x", "X"),
        ("Enter", "Enter"),
    ],
};

impl Layout {
    /// The chord fires from a key of this layout: a key whose character on
    /// either layer is the chord's key. Shortcuts match the physical key, as
    /// CodeMirror does by falling back to the key code when a modifier is
    /// held, so `Ctrl+Shift+1` is Ctrl+Shift+`&` on AZERTY and
    /// `Ctrl+Shift+.` is Ctrl+Shift+`;`.
    pub fn can_type(&self, chord: &Chord) -> bool {
        let key = chord.key.to_lowercase();
        self.keys
            .iter()
            .any(|(plain, shifted)| plain.to_lowercase() == key || shifted.to_lowercase() == key)
    }
}

#[cfg(test)]
mod tests;
