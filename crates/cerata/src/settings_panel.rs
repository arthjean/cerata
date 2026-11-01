//! The Settings screen, on Paneflow's layout: it replaces the sidebar with
//! a navigation (Back to the app, a search field, pages in groups) and the
//! main column with the active page. A page holds one or more cards of
//! Writer's settings panel (`settings-panel/index.tsx:11-88`): one control
//! per value type, and a Reset button in a slot every row reserves, so a
//! value leaving its default never moves the controls. Writes go through
//! the window, which applies them.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cerata_core::settings::{ConfigValue, SettingDef, ValueType};
use cerata_core::workspace::WindowState;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};

use crate::editor::text_field::{FieldEvent, TextField};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::select::{HasSelect, SelectPopup};
use crate::theme::{Mode, Preset};

const BEFORE_THEMES: [&str; 3] = ["Appearance", SIDEBAR_CARD, "Typography"];
const THEME_CATEGORY: &str = "Theme";
/// The Appearance keys of the sidebar get a card (and a page) of their own.
const SIDEBAR_PREFIX: &str = "appearance.sidebar-";
const SIDEBAR_CARD: &str = "Sidebar";
const PRIMARIES: [&str; 5] = [
    "accent",
    "background",
    "foreground",
    "translucent",
    "contrast",
];

/// What the panel asks the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelEvent {
    Set(String, ConfigValue),
    /// Back to the schema default.
    Reset(String),
    /// Back to the app.
    Close,
}

/// A card of the panel: its title and its keys, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub keys: Vec<String>,
}

/// The cards in Writer's order: Appearance, Sidebar and Typography, the
/// two theme cards (preset first, then the five primaries), then every
/// other category in schema order.
pub fn sections(definitions: &[SettingDef]) -> Vec<Section> {
    let mut categories: Vec<Section> = Vec::new();
    for def in definitions.iter().filter(|d| d.category != THEME_CATEGORY) {
        let title = if def.key.starts_with(SIDEBAR_PREFIX) {
            SIDEBAR_CARD
        } else {
            def.category.as_str()
        };
        match categories.iter_mut().find(|s| s.title == title) {
            Some(section) => section.keys.push(def.key.clone()),
            None => categories.push(Section {
                title: title.to_owned(),
                keys: vec![def.key.clone()],
            }),
        }
    }
    let mut ordered: Vec<Section> = BEFORE_THEMES
        .iter()
        .filter_map(|title| categories.iter().find(|s| s.title == *title).cloned())
        .collect();
    for (mode, title) in [(Mode::Light, "Light Theme"), (Mode::Dark, "Dark Theme")] {
        let preset = format!("theme.{}.preset", mode.id());
        let keys: Vec<String> = std::iter::once(preset)
            .chain(PRIMARIES.iter().map(|p| format!("theme.{}.{p}", mode.id())))
            .filter(|key| definitions.iter().any(|d| d.key == *key))
            .collect();
        ordered.push(Section {
            title: title.into(),
            keys,
        });
    }
    ordered.extend(
        categories
            .into_iter()
            .filter(|s| !BEFORE_THEMES.contains(&s.title.as_str()) && s.title != HIDDEN_CATEGORY),
    );
    ordered
}

/// A page of the navigation and the cards it shows, by title.
pub struct NavPage {
    pub title: &'static str,
    pub icon: Icon,
    pub cards: &'static [&'static str],
}

pub struct NavGroup {
    pub label: &'static str,
    pub pages: &'static [NavPage],
}

/// The navigation: every card of [`sections`] is on exactly one page.
pub const NAV: &[NavGroup] = &[
    NavGroup {
        label: "Interface",
        pages: &[
            NavPage {
                title: "Appearance",
                icon: Icon::PaintBoard,
                cards: &["Appearance", "Light Theme", "Dark Theme"],
            },
            NavPage {
                title: "Typography",
                icon: Icon::TextFont,
                cards: &["Typography"],
            },
            NavPage {
                title: SIDEBAR_CARD,
                icon: Icon::SidebarLeft,
                cards: &[SIDEBAR_CARD],
            },
        ],
    },
    NavGroup {
        label: "Documents",
        pages: &[
            NavPage {
                title: "Editor",
                icon: Icon::Pencil,
                cards: &["Editor", "Status Bar"],
            },
            NavPage {
                title: "Files",
                icon: Icon::Folder,
                cards: &["Files"],
            },
            NavPage {
                title: "Search",
                icon: Icon::Search,
                cards: &["Search"],
            },
            NavPage {
                title: "Workspace",
                icon: Icon::Dashboard,
                cards: &["Workspace", "Window"],
            },
        ],
    },
];

fn nav_pages() -> impl Iterator<Item = &'static NavPage> {
    NAV.iter().flat_map(|group| group.pages.iter())
}

/// The search field's text as it is matched: trimmed, lowercased.
pub fn normalize(query: &str) -> String {
    query.trim().to_lowercase()
}

fn matches(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(query)
}

/// The cards `page` shows for a normalized `query`: all of them when the
/// query is empty or names the page, a whole card when it names the card,
/// else the rows whose label or description holds it. Empty cards drop.
pub fn page_cards(
    page: &NavPage,
    cards: &[Section],
    definitions: &[SettingDef],
    query: &str,
) -> Vec<Section> {
    let whole = query.is_empty() || matches(page.title, query);
    page.cards
        .iter()
        .filter_map(|title| cards.iter().find(|card| card.title == *title))
        .filter_map(|card| {
            if whole || matches(&card.title, query) {
                return Some(card.clone());
            }
            let keys: Vec<String> = card
                .keys
                .iter()
                .filter(|key| {
                    definitions.iter().any(|def| {
                        def.key == **key
                            && (matches(&def.label, query) || matches(&def.description, query))
                    })
                })
                .cloned()
                .collect();
            (!keys.is_empty()).then(|| Section {
                title: card.title.clone(),
                keys,
            })
        })
        .collect()
}

/// Telemetry was dropped from v1: its switch and address would promise a
/// sharing that never happens, so the panel leaves them out.
const HIDDEN_CATEGORY: &str = "Privacy";

/// Installed families for the font picker (`fonts.rs:34-45`): no macOS
/// internal names (leading dot), sorted ignoring case, no duplicates.
pub fn font_families(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut families: Vec<String> = names.into_iter().filter(|n| !n.starts_with('.')).collect();
    families.sort_by(|a, b| (a.to_lowercase(), a).cmp(&(b.to_lowercase(), b)));
    families.dedup();
    families
}

fn strip_quotes(name: &str) -> &str {
    for quote in ['"', '\''] {
        if name.len() >= 2 && name.starts_with(quote) && name.ends_with(quote) {
            return &name[1..name.len() - 1];
        }
    }
    name
}

/// First family of a stack, unquoted (`font-stack.ts:10-12`).
pub fn first_family(stack: &str) -> String {
    strip_quotes(stack.split(',').next().unwrap_or("").trim()).to_owned()
}

/// `stack` with its first family replaced by `family`; the tail stays,
/// without a duplicate of `family` (`font-stack.ts:17-26`).
pub fn stack_with_family(family: &str, stack: &str) -> String {
    let name = family.trim();
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let quoted = if plain {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\\\""))
    };
    std::iter::once(quoted)
        .chain(
            stack
                .split(',')
                .skip(1)
                .map(str::trim)
                .filter(|part| !part.is_empty() && strip_quotes(part) != name)
                .map(str::to_owned),
        )
        .collect::<Vec<_>>()
        .join(", ")
}

/// A typed color is kept only as `#RGB` or `#RRGGBB`, upper-cased.
pub fn valid_color(text: &str) -> Option<String> {
    crate::theme::parse_hex(text.trim()).map(|_| text.trim().to_uppercase())
}

/// The value a slider at `fraction` of its track stands for.
pub fn slider_value(fraction: f32, min: f64, max: f64, step: f64) -> f64 {
    let raw = min + f64::from(fraction.clamp(0.0, 1.0)) * (max - min);
    let step = if step > 0.0 { step } else { 1.0 };
    (((raw - min) / step).round() * step + min).clamp(min, max)
}

fn display_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn text_of(value: Option<&ConfigValue>, def: &SettingDef) -> String {
    match (value, def.value_type) {
        (Some(ConfigValue::Number(n)), _) => display_number(*n),
        (Some(ConfigValue::String(s)), _) => s.clone(),
        _ => String::new(),
    }
}

pub struct SettingsPanel {
    state: Arc<WindowState>,
    definitions: Vec<SettingDef>,
    cards: Vec<Section>,
    /// The title of the page on show.
    pub page: &'static str,
    pub search: Entity<TextField>,
    content_scroll: ScrollHandle,
    fonts: Vec<String>,
    presets: Vec<Preset>,
    pub fields: HashMap<String, Entity<TextField>>,
    lists: HashMap<String, Vec<Entity<TextField>>>,
    sliders: HashMap<String, Rc<Cell<Bounds<Pixels>>>>,
    /// The slider being dragged and its value, written on release.
    dragging: Option<(String, f64)>,
    pub select: Option<SelectPopup>,
    /// Each select trigger's bounds, where its popup opens.
    triggers: HashMap<String, Rc<Cell<Bounds<Pixels>>>>,
    /// Each switch's motion, and the switch under a press.
    switches: HashMap<String, crate::switch::SwitchMotion>,
    /// The select triggers' and switches' focus, by key: they draw coss
    /// ui's ring for keyboard focus.
    control_focus: HashMap<String, FocusHandle>,
    pressed_switch: Option<String>,
    seen_revision: Option<u64>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl HasSelect for SettingsPanel {
    fn select(&mut self) -> &mut Option<SelectPopup> {
        &mut self.select
    }

    fn choose(&mut self, key: &str, option: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.choose_option(key, option, cx);
    }
}

impl SettingsPanel {
    pub fn new(state: Arc<WindowState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let definitions: Vec<SettingDef> = state.settings().schema().definitions().to_vec();
        let search = cx.new(|cx| TextField::new("Search settings", cx).bare(15.0));
        let search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let mut panel = Self {
            state,
            cards: sections(&definitions),
            definitions: definitions.clone(),
            page: NAV[0].pages[0].title,
            search,
            content_scroll: ScrollHandle::new(),
            fonts: font_families(window.text_system().all_font_names()),
            presets: crate::theme::presets(),
            fields: HashMap::new(),
            lists: HashMap::new(),
            sliders: HashMap::new(),
            dragging: None,
            select: None,
            triggers: HashMap::new(),
            switches: HashMap::new(),
            control_focus: HashMap::new(),
            pressed_switch: None,
            seen_revision: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![search_events],
        };
        for def in definitions {
            match def.value_type {
                ValueType::String | ValueType::Number | ValueType::Color
                    if !def.key.ends_with(".preset") =>
                {
                    let field = panel.text_field(&def, window, cx);
                    panel.fields.insert(def.key.clone(), field);
                }
                _ => {}
            }
        }
        panel.sync_fields(cx);
        panel
    }

    fn query(&self, cx: &App) -> String {
        normalize(&self.search.read(cx).text)
    }

    /// The cards a page shows for the current search.
    fn shown_cards(&self, page: &NavPage, cx: &App) -> Vec<Section> {
        page_cards(page, &self.cards, &self.definitions, &self.query(cx))
    }

    pub fn select_page(&mut self, title: &'static str, cx: &mut Context<Self>) {
        self.page = title;
        self.select = None;
        self.content_scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    /// A search that empties the page on show moves to the first page it
    /// keeps, as Paneflow's does.
    pub fn query_changed(&mut self, cx: &mut Context<Self>) {
        let current = nav_pages().find(|page| page.title == self.page);
        if current.is_some_and(|page| self.shown_cards(page, cx).is_empty())
            && let Some(first) = nav_pages().find(|page| !self.shown_cards(page, cx).is_empty())
        {
            self.select_page(first.title, cx);
        }
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| field.set_text("", cx));
        self.query_changed(cx);
    }

    /// Escape clears the search, or leaves Settings when it is empty.
    fn on_search_event(
        &mut self,
        field: &Entity<TextField>,
        event: &FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Changed => self.query_changed(cx),
            FieldEvent::Escape if field.read(cx).text.is_empty() => cx.emit(PanelEvent::Close),
            FieldEvent::Escape => {
                self.clear_search(cx);
                window.focus(&self.focus_handle, cx);
            }
            FieldEvent::Submit { .. } => {}
        }
    }

    fn value(&self, key: &str) -> Option<ConfigValue> {
        self.state.settings().get(key)
    }

    /// The value differs from the schema default: its Reset button shows.
    pub fn is_modified(&self, key: &str) -> bool {
        self.def(key)
            .is_some_and(|def| self.value(key).as_ref() != Some(&def.default))
    }

    pub fn reset(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(field) = self.fields.get(key) {
            field.update(cx, |field, _| field.invalid = false);
        }
        self.lists.remove(key);
        self.seen_revision = None;
        cx.emit(PanelEvent::Reset(key.to_owned()));
        cx.notify();
    }

    fn def(&self, key: &str) -> Option<SettingDef> {
        self.state.settings().schema().get(key).cloned()
    }

    fn text_field(
        &mut self,
        def: &SettingDef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let placeholder = def.placeholder.clone().unwrap_or_default();
        let field = cx.new(|cx| TextField::new(placeholder, cx).bare(14.0));
        let key = def.key.clone();
        self._subscriptions.push(cx.subscribe_in(&field, window, {
            let key = key.clone();
            move |this, field, event: &FieldEvent, _, cx| match event {
                FieldEvent::Submit { .. } => this.commit_field(&key, cx),
                FieldEvent::Changed => field.update(cx, |field, cx| {
                    field.invalid = false;
                    cx.notify();
                }),
                FieldEvent::Escape => {}
            }
        }));
        let focus = field.read(cx).focus_handle.clone();
        self._subscriptions
            .push(cx.on_blur(&focus, window, move |this, _, cx| {
                this.commit_field(&key, cx);
                cx.notify();
            }));
        // The frame shows the focus.
        self._subscriptions
            .push(cx.on_focus(&focus, window, |_, _, cx| cx.notify()));
        field
    }

    /// Fields that are not being edited show the current values.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let revision = self.state.settings().revision();
        if self.seen_revision == Some(revision) {
            return;
        }
        self.seen_revision = Some(revision);
        for (key, field) in self.fields.clone() {
            let Some(def) = self.def(&key) else { continue };
            let text = text_of(self.value(&key).as_ref(), &def);
            field.update(cx, |field, cx| {
                if !field.invalid && field.text != text {
                    field.set_text(&text, cx);
                }
            });
        }
    }

    /// Enter or a lost focus: a valid value is written, an invalid color or
    /// number turns red and is not.
    pub fn commit_field(&mut self, key: &str, cx: &mut Context<Self>) {
        let (Some(field), Some(def)) = (self.fields.get(key).cloned(), self.def(key)) else {
            return;
        };
        let text = field.read(cx).text.clone();
        let value = match def.value_type {
            ValueType::Color => valid_color(&text).map(ConfigValue::String),
            ValueType::Number => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(ConfigValue::Number),
            _ => Some(ConfigValue::String(text.clone())),
        };
        match value {
            Some(value) => {
                field.update(cx, |field, cx| {
                    field.invalid = false;
                    cx.notify();
                });
                if self.value(key).as_ref() != Some(&value) {
                    cx.emit(PanelEvent::Set(key.to_owned(), value));
                }
            }
            None => field.update(cx, |field, cx| {
                field.invalid = true;
                cx.notify();
            }),
        }
    }

    fn set(&mut self, key: &str, value: ConfigValue, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Set(key.to_owned(), value));
        cx.notify();
    }

    /// Choosing a preset writes its name and its five values for that mode.
    pub fn apply_preset(&mut self, mode: Mode, name: &str, cx: &mut Context<Self>) {
        let Some(preset) = self.presets.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        let values = match mode {
            Mode::Light => preset.light,
            Mode::Dark => preset.dark,
        };
        let key = |suffix: &str| format!("theme.{}.{suffix}", mode.id());
        self.set(&key("preset"), ConfigValue::String(preset.name.clone()), cx);
        self.set(&key("accent"), ConfigValue::String(values.accent), cx);
        self.set(
            &key("background"),
            ConfigValue::String(values.background),
            cx,
        );
        self.set(
            &key("foreground"),
            ConfigValue::String(values.foreground),
            cx,
        );
        self.set(
            &key("translucent"),
            ConfigValue::Number(values.translucent),
            cx,
        );
        self.set(&key("contrast"), ConfigValue::Number(values.contrast), cx);
        self.seen_revision = None;
    }

    /// Picking a font replaces the first family of the stack only.
    pub fn pick_font(&mut self, key: &str, family: &str, cx: &mut Context<Self>) {
        let current = self
            .value(key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.set(
            key,
            ConfigValue::String(stack_with_family(family, &current)),
            cx,
        );
    }

    fn list_fields(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<TextField>> {
        if let Some(fields) = self.lists.get(key) {
            return fields.clone();
        }
        let items: Vec<String> = self
            .value(key)
            .and_then(|v| v.as_list().map(<[String]>::to_vec))
            .unwrap_or_default();
        let fields: Vec<Entity<TextField>> = items
            .iter()
            .map(|item| self.list_item(key, item, window, cx))
            .collect();
        self.lists.insert(key.to_owned(), fields.clone());
        fields
    }

    fn list_item(
        &mut self,
        key: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let field = cx.new(|cx| {
            let mut field = TextField::new("", cx).bare(14.0);
            field.set_text(text, cx);
            field
        });
        let commit = key.to_owned();
        self._subscriptions.push(cx.subscribe_in(
            &field,
            window,
            move |this, _, event: &FieldEvent, _, cx| {
                if let FieldEvent::Submit { .. } = event {
                    this.commit_list(&commit, cx);
                }
            },
        ));
        let focus = field.read(cx).focus_handle.clone();
        let commit = key.to_owned();
        self._subscriptions
            .push(cx.on_blur(&focus, window, move |this, _, cx| {
                this.commit_list(&commit, cx);
                cx.notify();
            }));
        self._subscriptions
            .push(cx.on_focus(&focus, window, |_, _, cx| cx.notify()));
        field
    }

    /// The list's non-empty items, written when they changed.
    pub fn commit_list(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(fields) = self.lists.get(key) else {
            return;
        };
        let items: Vec<String> = fields
            .iter()
            .map(|field| field.read(cx).text.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect();
        let value = ConfigValue::List(items);
        if self.value(key).as_ref() != Some(&value) {
            self.set(key, value, cx);
        }
    }

    fn add_list_item(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.list_fields(key, window, cx);
        let field = self.list_item(key, "", window, cx);
        let focus = field.read(cx).focus_handle.clone();
        if let Some(fields) = self.lists.get_mut(key) {
            fields.push(field);
        }
        window.focus(&focus, cx);
        cx.notify();
    }

    fn remove_list_item(&mut self, key: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(fields) = self.lists.get_mut(key)
            && index < fields.len()
        {
            fields.remove(index);
        }
        self.commit_list(key, cx);
        cx.notify();
    }

    /// A select's options and the one the setting holds: preset names, the
    /// installed families (the stack's first one added when missing), or
    /// the schema's options.
    pub fn select_options(&self, key: &str) -> (Vec<String>, Option<usize>) {
        let Some(def) = self.def(key) else {
            return (Vec::new(), None);
        };
        let current = self
            .value(key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let (options, current) = if preset_mode(key).is_some() {
            let names = self.presets.iter().map(|p| p.name.clone()).collect();
            (names, current)
        } else if def.value_type == ValueType::Font {
            let selected = first_family(&current);
            let mut families = self.fonts.clone();
            if !selected.is_empty() && !families.contains(&selected) {
                families.insert(0, selected.clone());
            }
            (families, selected)
        } else {
            (def.options.clone().unwrap_or_default(), current)
        };
        let selected = options.iter().position(|option| *option == current);
        (options, selected)
    }

    /// Opens `key`'s popup over its trigger; a second press closes it.
    pub fn toggle_select(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.select.take().is_some_and(|open| open.key == key) {
            cx.notify();
            return;
        }
        let anchor = self
            .triggers
            .get(key)
            .map(|cell| cell.get())
            .unwrap_or_default();
        let (options, selected) = self.select_options(key);
        let options = options.into_iter().map(SharedString::from).collect();
        self.select = Some(SelectPopup::new(key, options, selected, anchor));
        cx.notify();
    }

    /// A preset writes its six values, a family replaces the stack's first
    /// one, any other option is the value.
    pub fn choose_option(&mut self, key: &str, option: &str, cx: &mut Context<Self>) {
        let font = self
            .def(key)
            .is_some_and(|d| d.value_type == ValueType::Font);
        match preset_mode(key) {
            Some(mode) => self.apply_preset(mode, option, cx),
            None if font => self.pick_font(key, option, cx),
            None => self.set(key, ConfigValue::String(option.to_owned()), cx),
        }
    }

    fn control_focus(&mut self, key: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.control_focus
            .entry(key.to_owned())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn release_switch(&mut self, cx: &mut Context<Self>) {
        if self.pressed_switch.take().is_some() {
            cx.notify();
        }
    }

    fn slider_at(&mut self, key: &str, x: Pixels) -> Option<f64> {
        let def = self.def(key)?;
        let bounds = self.sliders.get(key)?.get();
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return None;
        }
        let fraction = (f32::from(x) - f32::from(bounds.origin.x)) / width;
        Some(slider_value(
            fraction,
            def.min.unwrap_or(0.0),
            def.max.unwrap_or(100.0),
            def.step.unwrap_or(1.0),
        ))
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some((key, value)) = self.dragging.take() {
            if self.value(&key).and_then(|v| v.as_f64()) != Some(value) {
                self.set(&key, ConfigValue::Number(value), cx);
            }
            cx.notify();
        }
    }
}

fn preset_mode(key: &str) -> Option<Mode> {
    match key {
        "theme.light.preset" => Some(Mode::Light),
        "theme.dark.preset" => Some(Mode::Dark),
        _ => None,
    }
}

// ---- Rendering ----

impl SettingsPanel {
    fn control(
        &mut self,
        def: &SettingDef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = def.key.clone();
        let value = self.value(&key);
        let id = |suffix: &str| SharedString::from(format!("setting-{key}-{suffix}"));
        let name = def.label.clone();
        if preset_mode(&key).is_some()
            || matches!(def.value_type, ValueType::Enum | ValueType::Font)
        {
            let shown = match (def.value_type, value.as_ref().and_then(|v| v.as_str())) {
                (ValueType::Font, Some(stack)) => first_family(stack),
                (_, Some(text)) => text.to_owned(),
                _ => String::new(),
            };
            let open = self.select.as_ref().is_some_and(|popup| popup.key == key);
            let cell = self.triggers.entry(key.clone()).or_default().clone();
            let width = if def.value_type == ValueType::Font {
                256.0
            } else {
                176.0
            };
            let (click_key, press_key, arrow_key) = (key.clone(), key.clone(), key.clone());
            let focus = self.control_focus(&key, cx);
            return crate::select::trigger(
                div().id(id("select")),
                name,
                shown,
                width,
                open,
                &focus,
                window,
            )
            .child(
                canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if let ClickEvent::Keyboard(_) = event {
                    this.toggle_select(&click_key, cx);
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "down" | "up") {
                    cx.stop_propagation();
                    this.toggle_select(&arrow_key, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_select(&press_key, cx);
                }),
            )
            .into_any_element();
        }
        match def.value_type {
            ValueType::Boolean => {
                let on = value
                    .as_ref()
                    .and_then(ConfigValue::as_bool)
                    .unwrap_or(false);
                let focus = self.control_focus(&key, cx);
                let now = std::time::Instant::now();
                let pressed = self.pressed_switch.as_deref() == Some(key.as_str());
                let reduce = cx.reduce_motion();
                let motion = self
                    .switches
                    .entry(key.clone())
                    .or_insert_with(|| crate::switch::SwitchMotion::new(on));
                motion.sync(on, pressed, now, reduce);
                if motion.animating(now) {
                    window.request_animation_frame();
                }
                let press_key = key.clone();
                crate::switch::switch(
                    div().id(id("switch")),
                    name,
                    on,
                    motion,
                    now,
                    &focus,
                    window,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.pressed_switch = Some(press_key.clone());
                        cx.notify();
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.release_switch(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.release_switch(cx)),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set(&key, ConfigValue::Bool(!on), cx)),
                )
                .into_any_element()
            }
            ValueType::Range => {
                let min = def.min.unwrap_or(0.0);
                let max = def.max.unwrap_or(100.0);
                let current = match &self.dragging {
                    Some((k, v)) if *k == key => *v,
                    _ => value.as_ref().and_then(ConfigValue::as_f64).unwrap_or(min),
                };
                let fraction = if max > min {
                    ((current - min) / (max - min)) as f32
                } else {
                    0.0
                };
                let cell = self.sliders.entry(key.clone()).or_default().clone();
                let down_key = key.clone();
                let step_key = key.clone();
                let step = def.step.unwrap_or(1.0);
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        crate::a11y::control(div().id(id("slider")), Role::Slider, name)
                            .aria_numeric_value(current)
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                let next = match event.keystroke.key.as_str() {
                                    "left" | "down" => current - step,
                                    "right" | "up" => current + step,
                                    "home" => min,
                                    "end" => max,
                                    _ => return,
                                };
                                cx.stop_propagation();
                                let next = next.clamp(min, max);
                                if (next - current).abs() > f64::EPSILON {
                                    this.set(&step_key, ConfigValue::Number(next), cx);
                                }
                            }))
                            .relative()
                            .w(px(160.0))
                            .h(px(20.0))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .child(
                                canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded(px(2.0))
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(2.0))
                                            .bg(theme::accent())
                                            .w(gpui::relative(fraction.clamp(0.0, 1.0))),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(gpui::relative(fraction.clamp(0.0, 1.0)))
                                    .ml(px(-7.0))
                                    .size(px(14.0))
                                    .rounded(px(7.0))
                                    .bg(gpui::white())
                                    .border_1()
                                    .border_color(theme::border()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if let Some(value) = this.slider_at(&down_key, event.position.x)
                                    {
                                        this.dragging = Some((down_key.clone(), value));
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(36.0))
                            .text_size(px(13.0))
                            .text_color(theme::muted())
                            .child(display_number(current)),
                    )
                    .into_any_element()
            }
            ValueType::List => {
                let fields = self.list_fields(&key, window, cx);
                let mut column = div().flex().flex_col().items_end().gap(px(6.0));
                for (index, field) in fields.into_iter().enumerate() {
                    let remove_key = key.clone();
                    column = column.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(crate::input::field(&field, 256.0, window, cx))
                            .child(
                                crate::a11y::button(
                                    div().id(SharedString::from(format!(
                                        "setting-{key}-remove-{index}"
                                    ))),
                                    format!("Remove item {}", index + 1),
                                )
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .group("setting-remove")
                                .child(
                                    icon(Icon::Close, 14.0, theme::muted())
                                        .group_hover("setting-remove", |s| {
                                            s.text_color(theme::text())
                                        }),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove_list_item(&remove_key, index, cx)
                                    },
                                )),
                            ),
                    );
                }
                let add_key = key.clone();
                column
                    .child(
                        crate::a11y::button(div().id(id("add")), format!("Add to {name}"))
                            .mr(px(24.0))
                            .w(px(256.0))
                            .h(px(crate::input::HEIGHT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(crate::input::RADIUS))
                            .border_1()
                            .border_dashed()
                            .border_color(crate::input::border())
                            .text_size(px(14.0))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::text()))
                            .child("+ Add")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_list_item(&add_key, window, cx)
                            })),
                    )
                    .into_any_element()
            }
            ValueType::Color => {
                let Some(field) = self.fields.get(&key).cloned() else {
                    return div().into_any_element();
                };
                let swatch = value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .and_then(crate::theme::parse_hex)
                    .unwrap_or(gpui::rgb(0x000000));
                // The swatch sits inside the frame, as far from its edges
                // as from the text.
                crate::input::frame(&field, window, cx)
                    .w(px(176.0))
                    .pl(px(5.0))
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(20.0))
                            .rounded(px(crate::input::RADIUS * 0.6))
                            .border_1()
                            .border_color(crate::input::border())
                            .bg(swatch),
                    )
                    .child(div().flex_1().min_w_0().child(field))
                    .into_any_element()
            }
            _ => {
                let Some(field) = self.fields.get(&key).cloned() else {
                    return div().into_any_element();
                };
                let width = if def.value_type == ValueType::Number {
                    96.0
                } else {
                    256.0
                };
                crate::input::field(&field, width, window, cx).into_any_element()
            }
        }
    }

    fn row(
        &mut self,
        def: &SettingDef,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let control = self.control(def, window, cx);
        let label = if preset_mode(&def.key).is_some() {
            "Preset".to_owned()
        } else {
            def.label.clone()
        };
        let reset = self.reset_slot(def, &label, cx);
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .px(px(16.0))
            .py(px(14.0))
            .when(!first, |d| d.border_t_1().border_color(theme::rule()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label),
                    )
                    .when(!def.description.is_empty(), |d| {
                        d.child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(13.0))
                                .text_color(theme::muted())
                                .child(def.description.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(reset)
                    .child(control),
            )
            .into_any_element()
    }

    /// The Reset button's slot, as wide whether or not it shows.
    fn reset_slot(&self, def: &SettingDef, label: &str, cx: &mut Context<Self>) -> gpui::Div {
        let key = def.key.clone();
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .w(px(RESET_SLOT))
            .h(px(32.0))
            .when(self.is_modified(&key), |d| {
                d.child(
                    crate::a11y::button(
                        div().id(SharedString::from(format!("setting-{key}-reset"))),
                        format!("Reset {label} to its default"),
                    )
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::panel()).text_color(theme::text()))
                    .child("Reset")
                    .on_click(cx.listener(move |this, _, _, cx| this.reset(&key, cx))),
                )
            })
    }
}

/// Width of the Reset slot: the button at 12 px with its padding.
const RESET_SLOT: f32 = 52.0;

// The navigation's measures are the sidebar's (Paneflow's rows): 8 px
// margin, 32 px rows padded 7, radius 9, a 15 px icon 8 from a 14 px label.
const NAV_MARGIN: f32 = 8.0;
const NAV_PADDING: f32 = 7.0;

fn nav_row(
    id: SharedString,
    label: &'static str,
    glyph: Icon,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    crate::a11y::button(div().id(id), label)
        .mx(px(NAV_MARGIN))
        .px(px(NAV_PADDING))
        .h(px(32.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(9.0))
        .cursor_pointer()
        .map(|d| {
            if active {
                d.bg(theme::row_active())
            } else {
                d.hover(|s| s.bg(theme::row_hover()))
            }
        })
        .child(icon(glyph, 15.0, theme::muted()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(14.0))
                .line_height(px(20.0))
                .truncate()
                .child(label),
        )
}

impl SettingsPanel {
    fn nav_width(&self) -> f32 {
        self.state
            .settings()
            .get(crate::sidebar::WIDTH_KEY)
            .and_then(|v| v.as_f64())
            .map_or(300.0, |w| w as f32)
    }

    fn render_search(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let field = self.search.read(cx);
        let focus = field.focus_handle.clone();
        let focused = focus.is_focused(window);
        let has_query = !field.text.is_empty();
        let clear = crate::a11y::button(div().id("settings-search-clear"), "Clear search")
            .flex_none()
            .size(px(18.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme::muted().opacity(0.2))
            .cursor_pointer()
            .child(icon(Icon::Close, 12.0, theme::text()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                this.clear_search(cx);
                let focus = this.search.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }));
        div()
            .id("settings-search")
            .flex_none()
            .mx(px(NAV_MARGIN))
            .mt(px(4.0))
            .h(px(36.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded_full()
            .cursor_text()
            .bg(if focused || has_query {
                theme::row_active()
            } else {
                theme::row_hover()
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx);
                cx.stop_propagation();
            })
            .child(icon(
                Icon::Search,
                17.0,
                if focused {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .child(div().flex_1().min_w_0().child(self.search.clone()))
            .when(has_query, |d| d.child(clear))
            .into_any_element()
    }

    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let back = nav_row(
            "settings-back".into(),
            "Back to the app",
            Icon::ArrowLeft,
            false,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Close)));
        let mut list = div()
            .id("settings-nav-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pt(px(4.0))
            .pb(px(8.0));
        let mut any = false;
        for group in NAV {
            let pages: Vec<&'static NavPage> = group
                .pages
                .iter()
                .filter(|page| !self.shown_cards(page, cx).is_empty())
                .collect();
            if pages.is_empty() {
                continue;
            }
            any = true;
            list = list.child(
                div()
                    .mt(px(12.0))
                    .mb(px(2.0))
                    .px(px(NAV_MARGIN + NAV_PADDING))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted())
                    .child(group.label),
            );
            for page in pages {
                let title = page.title;
                let active = title == self.page;
                let row = nav_row(
                    SharedString::from(format!("settings-nav-{title}")),
                    title,
                    page.icon,
                    active,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_page(title, cx);
                    window.focus(&this.focus_handle, cx);
                }));
                list = list.child(row);
            }
        }
        if !any {
            list = list.child(
                div()
                    .mx(px(NAV_MARGIN))
                    .mt(px(8.0))
                    .px(px(NAV_PADDING))
                    .text_size(px(13.0))
                    .text_color(theme::muted())
                    .child("No matching settings"),
            );
        }
        crate::shell::frame::round_left(div(), crate::shell::frame::Free::of(window))
            .id("settings-nav")
            .flex_none()
            .w(px(self.nav_width()))
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .child(crate::shell::title_bar::wordmark())
            .child(back)
            .child(self.render_search(window, cx))
            .child(list)
            .into_any_element()
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(page) = nav_pages().find(|page| page.title == self.page) else {
            return div().into_any_element();
        };
        let mut column = div()
            .mx_auto()
            .w_full()
            .max_w(px(700.0))
            .px(px(28.0))
            .pt(px(28.0))
            .pb(px(72.0))
            .child(
                div()
                    .pb(px(20.0))
                    .text_size(px(26.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(page.title),
            );
        let cards = self.shown_cards(page, cx);
        if cards.is_empty() {
            column = column.child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::muted())
                    .child("No matching settings"),
            );
        }
        let definitions = self.definitions.clone();
        for section in cards {
            let mut card = div()
                .rounded(px(16.0))
                .border_1()
                .border_color(theme::rule())
                .bg(theme::code_background());
            for (ix, key) in section.keys.iter().enumerate() {
                if let Some(def) = definitions.iter().find(|d| d.key == *key) {
                    card = card.child(self.row(def, ix == 0, window, cx));
                }
            }
            // The card that carries the page's own name needs no heading.
            let heading = (section.title != page.title).then(|| {
                div()
                    .mb(px(12.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted())
                    .child(section.title.clone())
            });
            column = column.child(div().mb(px(32.0)).children(heading).child(card));
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                crate::shell::title_bar::row("settings-title-bar")
                    .w_full()
                    .child(div().flex_1())
                    .children(crate::shell::title_bar::caption_buttons(window)),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll)
                    .child(column),
            )
            .into_any_element()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(cx);
        let nav = self.render_nav(window, cx);
        let page = self.render_page(window, cx);
        let mut root = div()
            .id("settings-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .text_color(theme::text())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && this.select.is_none()
                    && this.focus_handle.is_focused(window)
                {
                    cx.stop_propagation();
                    cx.emit(PanelEvent::Close);
                }
            }))
            .child(nav)
            .child(div().flex_none().w(px(1.0)).h_full().bg(theme::border()))
            .child(page);
        if self.dragging.is_some() {
            root = root
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if let Some((key, _)) = this.dragging.clone()
                        && let Some(value) = this.slider_at(&key, event.position.x)
                    {
                        this.dragging = Some((key, value));
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.end_drag(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.end_drag(cx)),
                );
        }
        if let Some(select) = self.select.as_mut() {
            root = root.child(crate::select::render(select, window, cx));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO_DEFAULT: &str = r#"ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#;

    // Ported from Writer `tests/font-stack.test.ts`.

    #[test]
    fn first_family_is_the_unquoted_head_of_the_stack() {
        assert_eq!(first_family(MONO_DEFAULT), "ui-monospace");
        assert_eq!(
            first_family(r#""JetBrains Mono", monospace"#),
            "JetBrains Mono"
        );
        assert_eq!(first_family("'SF Mono', Menlo"), "SF Mono");
        assert_eq!(first_family("Menlo"), "Menlo");
        assert_eq!(first_family(""), "");
    }

    #[test]
    fn stack_with_family_replaces_the_head_and_keeps_the_tail() {
        assert_eq!(
            stack_with_family("Menlo", "ui-monospace, monospace"),
            "Menlo, monospace"
        );
        assert_eq!(
            stack_with_family("Menlo", r#"Custom, Georgia, "Times New Roman", serif"#),
            r#"Menlo, Georgia, "Times New Roman", serif"#
        );
        assert_eq!(
            stack_with_family("JetBrains Mono", "Old, monospace"),
            r#""JetBrains Mono", monospace"#
        );
        assert_eq!(
            stack_with_family("Menlo", MONO_DEFAULT),
            r#"Menlo, SFMono-Regular, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#
        );
        assert_eq!(
            stack_with_family("Courier New", r#"Menlo, "Courier New", monospace"#),
            r#""Courier New", monospace"#
        );
        let once = stack_with_family("Fira Code", MONO_DEFAULT);
        assert_eq!(
            stack_with_family("JetBrains Mono", &once),
            r#""JetBrains Mono", SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#
        );
        assert_eq!(stack_with_family("Menlo", "Old"), "Menlo");
        assert_eq!(stack_with_family("Menlo", ""), "Menlo");
    }

    #[test]
    fn font_families_hide_internal_names_sort_and_dedupe() {
        // Writer `fonts.rs:52-63`.
        assert_eq!(
            font_families(["Menlo", ".SF NS", "Arial", "menlo", "Menlo"].map(String::from)),
            ["Arial", "Menlo", "menlo"]
        );
    }

    #[test]
    fn sections_follow_writers_order() {
        let schema = cerata_core::settings::Schema::embedded().unwrap();
        let titles: Vec<String> = sections(schema.definitions())
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            [
                "Appearance",
                "Sidebar",
                "Typography",
                "Light Theme",
                "Dark Theme",
                "Editor",
                "Status Bar",
                "Files",
                "Workspace",
                "Window",
                "Search"
            ]
        );
        let light = &sections(schema.definitions())[3];
        assert_eq!(
            light.keys,
            [
                "theme.light.preset",
                "theme.light.accent",
                "theme.light.background",
                "theme.light.foreground",
                "theme.light.translucent",
                "theme.light.contrast"
            ]
        );
    }

    #[test]
    fn every_card_is_on_exactly_one_page() {
        let schema = cerata_core::settings::Schema::embedded().unwrap();
        let cards = sections(schema.definitions());
        for card in &cards {
            let pages = nav_pages()
                .filter(|page| page.cards.contains(&card.title.as_str()))
                .count();
            assert_eq!(pages, 1, "{}", card.title);
        }
        for page in nav_pages() {
            for title in page.cards {
                assert!(cards.iter().any(|c| c.title == *title), "{title}");
            }
        }
    }

    #[test]
    fn a_search_keeps_the_matching_rows_or_whole_named_cards() {
        let schema = cerata_core::settings::Schema::embedded().unwrap();
        let defs = schema.definitions();
        let cards = sections(defs);
        let page = |title: &str| nav_pages().find(|p| p.title == title).unwrap();
        let keys = |shown: Vec<Section>| -> Vec<String> {
            shown.into_iter().flat_map(|s| s.keys).collect()
        };
        assert_eq!(
            page_cards(page("Editor"), &cards, defs, "").len(),
            2,
            "an empty query shows every card"
        );
        assert_eq!(
            keys(page_cards(
                page("Editor"),
                &cards,
                defs,
                &normalize(" Tab Size ")
            )),
            ["editor.tab-size"]
        );
        assert_eq!(
            page_cards(page("Editor"), &cards, defs, "status bar")[0]
                .keys
                .len(),
            3,
            "a card's name keeps the whole card"
        );
        assert_eq!(
            page_cards(page("Typography"), &cards, defs, "typo")[0]
                .keys
                .len(),
            3,
            "the page's name keeps the whole page"
        );
        assert!(page_cards(page("Files"), &cards, defs, "zoom").is_empty());
    }

    #[test]
    fn colors_and_sliders_validate() {
        assert_eq!(valid_color("#ff6a00"), Some("#FF6A00".into()));
        assert_eq!(valid_color("#abc"), Some("#ABC".into()));
        assert_eq!(valid_color("ff6a00"), None);
        assert_eq!(valid_color("#ff6a0"), None);
        assert_eq!(slider_value(0.5, 0.0, 100.0, 1.0), 50.0);
        assert_eq!(slider_value(1.5, 0.0, 100.0, 1.0), 100.0);
        assert_eq!(slider_value(0.333, 50.0, 300.0, 10.0), 130.0);
    }
}
