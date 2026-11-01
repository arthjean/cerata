//! Editor actions and their key bindings: the editing section of Writer's
//! `docs/keyboard-shortcuts.md:50-98` (Ctrl in place of Cmd on Windows and
//! Linux, through GPUI's `secondary` modifier), the formatting registry of
//! `cerata_markdown::commands`, the clipboard and the find bar.

use cerata_markdown::commands::{COMMANDS, Clock, Os};
use cerata_markdown::{Change, Selection, SelectionRange, Transaction, UserEvent};
use gpui::{
    Action, App, ClipboardEntry, ClipboardItem, Context, Div, InteractiveElement, KeyBinding,
    Stateful, Window, actions,
};

use super::{Editor, images};

actions!(
    editor,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Newline,
        Indent,
        Outdent,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        Undo,
        Redo,
        SelectNextOccurrence,
        SelectAllOccurrences,
        MoveLineUp,
        MoveLineDown,
        CopyLineUp,
        CopyLineDown,
        DeleteLine,
        InsertLineBelow,
        IndentMore,
        IndentLess,
        Cancel,
        Copy,
        Cut,
        Paste,
        Find,
        FindNext,
        FindPrevious,
    ]
);

/// A formatting command of the registry, by id (`format.bold`).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = editor, no_json)]
pub struct RunCommand {
    pub id: &'static str,
}

pub const CONTEXT: &str = "Editor";

pub fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Linux
    }
}

/// A registry chord as a GPUI keystroke (`ctrl-shift-1`).
pub fn keystroke(chord: &cerata_markdown::commands::Chord) -> String {
    let mut parts = Vec::new();
    if chord.ctrl {
        parts.push("ctrl".to_owned());
    }
    if chord.alt {
        parts.push("alt".to_owned());
    }
    if chord.shift {
        parts.push("shift".to_owned());
    }
    if chord.cmd {
        parts.push("cmd".to_owned());
    }
    parts.push(chord.key.to_lowercase());
    parts.join("-")
}

/// Every editor binding, for the current OS.
pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    let mac = cfg!(target_os = "macos");
    let mut b = vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("tab", Indent, c),
        KeyBinding::new("shift-tab", Outdent, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new(secondary!("a"), SelectAll, c),
        KeyBinding::new(secondary!("z"), Undo, c),
        KeyBinding::new(secondary!("shift-z"), Redo, c),
        KeyBinding::new(secondary!("d"), SelectNextOccurrence, c),
        KeyBinding::new(secondary!("shift-l"), SelectAllOccurrences, c),
        KeyBinding::new("alt-up", MoveLineUp, c),
        KeyBinding::new("alt-down", MoveLineDown, c),
        KeyBinding::new("alt-shift-up", CopyLineUp, c),
        KeyBinding::new("alt-shift-down", CopyLineDown, c),
        KeyBinding::new(secondary!("shift-k"), DeleteLine, c),
        KeyBinding::new(secondary!("enter"), InsertLineBelow, c),
        KeyBinding::new(secondary!("]"), IndentMore, c),
        KeyBinding::new(secondary!("["), IndentLess, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new(secondary!("c"), Copy, c),
        KeyBinding::new(secondary!("x"), Cut, c),
        KeyBinding::new(secondary!("v"), Paste, c),
        KeyBinding::new(secondary!("f"), Find, Some("Editor || SearchBar")),
        KeyBinding::new(secondary!("g"), FindNext, Some("Editor || SearchBar")),
        KeyBinding::new(
            secondary!("shift-g"),
            FindPrevious,
            Some("Editor || SearchBar"),
        ),
    ];
    if mac {
        b.extend([
            KeyBinding::new("alt-left", WordLeft, c),
            KeyBinding::new("alt-right", WordRight, c),
            KeyBinding::new("alt-backspace", DeleteWordLeft, c),
            KeyBinding::new("alt-delete", DeleteWordRight, c),
            KeyBinding::new("cmd-left", LineStart, c),
            KeyBinding::new("cmd-right", LineEnd, c),
            KeyBinding::new("cmd-shift-left", SelectLineStart, c),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
            KeyBinding::new("cmd-up", DocStart, c),
            KeyBinding::new("cmd-down", DocEnd, c),
            KeyBinding::new("cmd-shift-up", SelectDocStart, c),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
        ]);
    } else {
        b.extend([
            KeyBinding::new("ctrl-left", WordLeft, c),
            KeyBinding::new("ctrl-right", WordRight, c),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
            KeyBinding::new("ctrl-delete", DeleteWordRight, c),
            KeyBinding::new("ctrl-home", DocStart, c),
            KeyBinding::new("ctrl-end", DocEnd, c),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
            KeyBinding::new("ctrl-y", Redo, c),
            KeyBinding::new("shift-insert", Paste, c),
            KeyBinding::new("ctrl-insert", Copy, c),
            KeyBinding::new("shift-delete", Cut, c),
        ]);
    }
    let os = current_os();
    for command in COMMANDS {
        if let Some(chord) = command.shortcut(os) {
            // `KeyBinding::new` takes a static string; the registry's
            // bindings are built once per app.
            let keys: &'static str = Box::leak(keystroke(&chord).into_boxed_str());
            b.push(KeyBinding::new(keys, RunCommand { id: command.id }, c));
        }
    }
    b
}

/// Registers the editor bindings once per app.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
    cx.bind_keys(super::text_field::bindings());
}

/// The local date and time for the insert commands.
pub fn clock() -> Clock {
    use chrono::{Datelike as _, Timelike as _};
    let now = chrono::Local::now();
    Clock {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
    }
}

macro_rules! on {
    ($root:ident, $cx:ident, $action:ty, |$editor:ident, $window:ident, $ecx:ident| $body:expr) => {
        $root = $root.on_action($cx.listener(
            |$editor: &mut Editor,
             _: &$action,
             $window: &mut Window,
             $ecx: &mut Context<Editor>| {
                $body;
            },
        ));
    };
}

/// Attaches every action handler to the editor's root element.
pub fn register(mut root: Stateful<Div>, cx: &mut Context<Editor>) -> Stateful<Div> {
    on!(root, cx, Backspace, |e, _w, cx| e
        .update_state(cx, |s| s.delete_backward()));
    on!(root, cx, Delete, |e, _w, cx| e
        .update_state(cx, |s| s.delete_forward()));
    on!(root, cx, DeleteWordLeft, |e, _w, cx| e
        .update_state(cx, |s| s.delete_group(false)));
    on!(root, cx, DeleteWordRight, |e, _w, cx| e
        .update_state(cx, |s| s.delete_group(true)));
    on!(root, cx, Newline, |e, _w, cx| e
        .update_state(cx, |s| s.insert_newline()));
    on!(root, cx, Indent, |e, _w, cx| e
        .update_state(cx, |s| s.indent()));
    on!(root, cx, Outdent, |e, _w, cx| e
        .update_state(cx, |s| s.outdent()));
    on!(root, cx, Left, |e, _w, cx| e.horizontal(false, false, cx));
    on!(root, cx, Right, |e, _w, cx| e.horizontal(true, false, cx));
    on!(root, cx, SelectLeft, |e, _w, cx| e
        .horizontal(false, true, cx));
    on!(root, cx, SelectRight, |e, _w, cx| e
        .horizontal(true, true, cx));
    on!(root, cx, Up, |e, w, cx| e.vertical(false, false, w, cx));
    on!(root, cx, Down, |e, w, cx| e.vertical(true, false, w, cx));
    on!(root, cx, SelectUp, |e, w, cx| e
        .vertical(false, true, w, cx));
    on!(root, cx, SelectDown, |e, w, cx| e
        .vertical(true, true, w, cx));
    on!(root, cx, WordLeft, |e, _w, cx| e
        .update_state(cx, |s| s.move_group(false, false)));
    on!(root, cx, WordRight, |e, _w, cx| e
        .update_state(cx, |s| s.move_group(true, false)));
    on!(root, cx, SelectWordLeft, |e, _w, cx| e
        .update_state(cx, |s| s.move_group(false, true)));
    on!(root, cx, SelectWordRight, |e, _w, cx| e
        .update_state(cx, |s| s.move_group(true, true)));
    on!(root, cx, LineStart, |e, w, cx| e
        .line_boundary(false, false, w, cx));
    on!(root, cx, LineEnd, |e, w, cx| e
        .line_boundary(true, false, w, cx));
    on!(root, cx, SelectLineStart, |e, w, cx| e
        .line_boundary(false, true, w, cx));
    on!(root, cx, SelectLineEnd, |e, w, cx| e
        .line_boundary(true, true, w, cx));
    on!(root, cx, DocStart, |e, _w, cx| e
        .update_state(cx, |s| s.move_doc_boundary(false, false)));
    on!(root, cx, DocEnd, |e, _w, cx| e
        .update_state(cx, |s| s.move_doc_boundary(true, false)));
    on!(root, cx, SelectDocStart, |e, _w, cx| e
        .update_state(cx, |s| s.move_doc_boundary(false, true)));
    on!(root, cx, SelectDocEnd, |e, _w, cx| e
        .update_state(cx, |s| s.move_doc_boundary(true, true)));
    on!(root, cx, PageUp, |e, w, cx| e.page(false, false, w, cx));
    on!(root, cx, PageDown, |e, w, cx| e.page(true, false, w, cx));
    on!(root, cx, SelectPageUp, |e, w, cx| e
        .page(false, true, w, cx));
    on!(root, cx, SelectPageDown, |e, w, cx| e
        .page(true, true, w, cx));
    on!(root, cx, SelectAll, |e, _w, cx| e
        .update_state(cx, |s| s.select_all()));
    on!(root, cx, Undo, |e, _w, cx| e.update_state(cx, |s| s.undo()));
    on!(root, cx, Redo, |e, _w, cx| e.update_state(cx, |s| s.redo()));
    on!(root, cx, SelectNextOccurrence, |e, _w, cx| e
        .update_state(cx, |s| s.select_next_occurrence()));
    on!(root, cx, SelectAllOccurrences, |e, _w, cx| e
        .update_state(cx, |s| s.select_all_occurrences()));
    on!(root, cx, MoveLineUp, |e, _w, cx| e
        .update_state(cx, |s| s.move_lines(true)));
    on!(root, cx, MoveLineDown, |e, _w, cx| e
        .update_state(cx, |s| s.move_lines(false)));
    on!(root, cx, CopyLineUp, |e, _w, cx| e
        .update_state(cx, |s| s.copy_lines(true)));
    on!(root, cx, CopyLineDown, |e, _w, cx| e
        .update_state(cx, |s| s.copy_lines(false)));
    on!(root, cx, DeleteLine, |e, _w, cx| e
        .update_state(cx, |s| s.delete_lines()));
    on!(root, cx, InsertLineBelow, |e, _w, cx| e
        .update_state(cx, |s| s.insert_blank_line()));
    on!(root, cx, IndentMore, |e, _w, cx| e
        .update_state(cx, |s| s.indent_lines(true)));
    on!(root, cx, IndentLess, |e, _w, cx| e
        .update_state(cx, |s| s.indent_lines(false)));
    on!(root, cx, Cancel, |e, w, cx| e.cancel(w, cx));
    on!(root, cx, Copy, |e, _w, cx| e.copy(cx));
    on!(root, cx, Cut, |e, _w, cx| e.cut(cx));
    on!(root, cx, Paste, |e, _w, cx| e.paste(cx));
    on!(root, cx, Find, |e, w, cx| e.open_search(w, cx));
    on!(root, cx, FindNext, |e, w, cx| e.search_step(true, w, cx));
    on!(root, cx, FindPrevious, |e, w, cx| e
        .search_step(false, w, cx));
    root.on_action(
        cx.listener(|editor: &mut Editor, action: &RunCommand, _, cx| {
            let clock = clock();
            editor.update_state(cx, |state| state.run_command(action.id, &clock));
        }),
    )
}

impl Editor {
    fn horizontal(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.update_state(cx, |state| state.move_char(forward, extend));
    }

    /// Up and Down: into an adjacent widget's source first, else one wrapped
    /// row, keeping the pixel column across moves.
    pub fn vertical(
        &mut self,
        down: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let direction = if down {
            cerata_markdown::Vertical::Down
        } else {
            cerata_markdown::Vertical::Up
        };
        if !extend
            && self.state.selection().ranges().len() == 1
            && self.state.selection().main().is_empty()
        {
            let goal = self.goal_x;
            if self.update_state(cx, |state| state.reveal_block_on_arrow(direction)) {
                self.goal_x = goal;
                return;
            }
        }
        let ranges: Vec<SelectionRange> = self.state.selection().ranges().to_vec();
        let main = self.state.selection().main_index();
        let mut goal = self.goal_x;
        let moved: Vec<SelectionRange> = ranges
            .iter()
            .enumerate()
            .map(|(i, range)| {
                let start = if !extend && !range.is_empty() {
                    if down { range.to() } else { range.from() }
                } else {
                    range.head
                };
                let (target, x) = self.vertical_target(
                    start,
                    down,
                    if i == main { goal } else { None },
                    window,
                    cx,
                );
                if i == main {
                    goal = Some(x);
                }
                if extend {
                    SelectionRange::new(range.anchor, target)
                } else {
                    SelectionRange::cursor(target)
                }
            })
            .collect();
        self.select(Selection::new(moved, main), cx);
        self.goal_x = goal;
    }

    /// The position one row above or below `pos` at pixel column `goal`
    /// (the caret's own x when `None`), and that column.
    fn vertical_target(
        &mut self,
        pos: usize,
        down: bool,
        goal: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, f32) {
        let (line, row) = self.row_at(pos, window, cx);
        let x = goal.unwrap_or_else(|| {
            line.wrapped.as_ref().map_or(line.text_left, |wrapped| {
                let display = line.display.to_display(pos - line.start);
                wrapped
                    .position_for_index(display, gpui::px(line.metrics.row_height))
                    .map_or(0.0, |p| p.x.as_f32())
                    + line.text_left
                    - line.code.as_ref().map_or(0.0, |c| c.scroll_x)
            })
        });
        let rows = line.rows().max(1);
        if down && row + 1 < rows {
            return (Self::offset_in_row(&line, row + 1, x), x);
        }
        if !down && row > 0 {
            return (Self::offset_in_row(&line, row - 1, x), x);
        }
        let count = self.state.text().line_count();
        let mut ix = line.ix;
        loop {
            if down && ix + 1 >= count {
                return (self.state.text().len(), x);
            }
            if !down && ix == 0 {
                return (0, x);
            }
            ix = if down { ix + 1 } else { ix - 1 };
            let next = self.layout_line(ix, window, cx);
            if next.height <= 0.0 {
                continue;
            }
            let rows = next.rows().max(1);
            let target_row = if down { 0 } else { rows - 1 };
            return (Self::offset_in_row(&next, target_row, x), x);
        }
    }

    /// Home and End on wrapped rows (`lineBoundary.ts`).
    fn line_boundary(
        &mut self,
        forward: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let heads: Vec<usize> = self
            .state
            .selection()
            .ranges()
            .iter()
            .map(|r| r.head)
            .collect();
        let rows: Vec<(usize, std::ops::Range<usize>)> = heads
            .into_iter()
            .map(|head| {
                let (line, row) = self.row_at(head, window, cx);
                (head, Self::row_range(&line, row))
            })
            .collect();
        self.update_state(cx, |state| {
            let visual = |pos: usize| {
                rows.iter()
                    .find(|(head, _)| *head == pos)
                    .map_or(pos..pos, |(_, range)| range.clone())
            };
            state.move_line_boundary(forward, extend, &visual)
        });
    }

    /// PageUp and PageDown: the caret moves by the viewport height and the
    /// view follows.
    fn page(&mut self, down: bool, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(caret) = self.caret_bounds(self.cursor(), window, cx) else {
            return;
        };
        let delta = self.viewport_height() as f32 * if down { 1.0 } else { -1.0 };
        let target = self.offset_for_point(
            gpui::point(
                caret.origin.x,
                caret.origin.y + caret.size.height / 2.0 + gpui::px(delta),
            ),
            window,
            cx,
        );
        let scroll = self.scroll_top() + delta as f64;
        self.set_scroll_top(scroll);
        let main = self.state.selection().main();
        let selection = if extend {
            Selection::single(main.anchor, target)
        } else {
            Selection::cursor(target)
        };
        self.select(selection, cx);
    }

    /// Escape: closes the find bar or the menu, else collapses a multiple
    /// selection to its main range.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.search.is_some() {
            self.close_search(window, cx);
            return;
        }
        if self.state.selection().ranges().len() > 1 {
            let main = self.state.selection().main();
            self.select(Selection::single(main.anchor, main.head), cx);
        }
    }

    /// The selected text, ranges joined by line breaks.
    pub fn selected_text(&self) -> String {
        let text = self.state.text();
        self.state
            .selection()
            .ranges()
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| text.slice(r.range()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.update_state(cx, |state| {
            let changes: Vec<Change> = state
                .selection()
                .ranges()
                .iter()
                .filter(|r| !r.is_empty())
                .map(|r| Change::delete(r.range()))
                .collect();
            let _ = state.apply(
                Transaction::new()
                    .changes(changes)
                    .event(UserEvent::Delete)
                    .scroll(),
            );
        });
    }

    /// Paste as plain text; an image is saved next to the note; a whole
    /// document pasted into a note without frontmatter moves its YAML to
    /// the top (`editor-clipboard.ts:37-78`).
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        for entry in item.entries() {
            if let ClipboardEntry::Image(image) = entry {
                self.paste_image(image, cx);
                return;
            }
        }
        if let Some(text) = item.text() {
            self.paste_text(&text, cx);
        }
    }

    pub fn paste_image(&mut self, image: &gpui::Image, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            self.show_notice(images::PasteError::NoFolder.to_string(), cx);
            return;
        };
        match images::save_pasted_image(&path, image, chrono::Local::now()) {
            Ok(markdown) => self.insert_at_selection(&markdown, UserEvent::Command, cx),
            Err(error) => self.show_notice(error.to_string(), cx),
        }
    }

    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let has_frontmatter = cerata_markdown::frontmatter::detect(self.state.doc()).is_some();
        if !has_frontmatter && let (Some(yaml), body) = cerata_markdown::frontmatter::split(text) {
            let yaml = yaml.trim_end_matches(['\n', '\r']);
            let block = if yaml.is_empty() {
                "---\n---\n".to_owned()
            } else {
                format!("---\n{yaml}\n---\n")
            };
            let body = body.to_owned();
            self.update_state(cx, |state| {
                let mut changes = vec![Change::insert(0, block.clone())];
                let mut carets = Vec::new();
                let mut shift = block.len() as isize;
                for range in state.selection().ranges() {
                    changes.push(Change::new(range.range(), body.clone()));
                    let end = range.from() as isize + shift + body.len() as isize;
                    carets.push(SelectionRange::cursor(end.max(0) as usize));
                    shift += body.len() as isize - range.range().len() as isize;
                }
                let main = state.selection().main_index();
                let _ = state.apply(
                    Transaction::new()
                        .changes(changes)
                        .select(Selection::new(carets, main))
                        .event(UserEvent::Command)
                        .scroll(),
                );
            });
            return;
        }
        self.insert_at_selection(text, UserEvent::Command, cx);
    }

    /// Replaces every selection range with `text`; carets land after it.
    pub fn insert_at_selection(&mut self, text: &str, event: UserEvent, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.update_state(cx, |state| {
            let mut changes = Vec::new();
            let mut carets = Vec::new();
            let mut shift: isize = 0;
            for range in state.selection().ranges() {
                changes.push(Change::new(range.range(), text.clone()));
                let end = range.from() as isize + shift + text.len() as isize;
                carets.push(SelectionRange::cursor(end.max(0) as usize));
                shift += text.len() as isize - range.range().len() as isize;
            }
            let main = state.selection().main_index();
            let _ = state.apply(
                Transaction::new()
                    .changes(changes)
                    .select(Selection::new(carets, main))
                    .event(event)
                    .scroll(),
            );
        });
    }
}
