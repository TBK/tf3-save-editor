//! Graphical Transport Fever 3 save editor built on gpui-base.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, Context, Entity, FontWeight, Image, ImageFormat,
    IntoElement, ParentElement as _, PathPromptOptions, Render, ScrollHandle, StatefulInteractiveElement as _,
    Styled as _, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, img, prelude::*, px,
    rgb, size, uniform_list,
};
use gpui_base::input::{InputEvent, InputState};
use gpui_base::{Button, Input, Progress, ProgressIndicator, ProgressTrack, Root, Scrollbar, h_flex, v_flex};
use tf3save::SaveFile;
use tf3save::lua::{Leaf, Value, format_number, format_scalar};

mod colors {
    pub const BG: u32 = 0x1e2127;
    pub const PANEL: u32 = 0x262a31;
    pub const SIDEBAR: u32 = 0x1a1d22;
    pub const BORDER: u32 = 0x3a3f48;
    pub const TEXT: u32 = 0xe6e8eb;
    pub const MUTED: u32 = 0x9aa3ad;
    pub const ACCENT: u32 = 0x3b82f6;
    pub const ACCENT_HOVER: u32 = 0x2563eb;
    pub const ROW_HOVER: u32 = 0x2f343c;
    pub const ROW_SELECTED: u32 = 0x1e3a5f;
    pub const OK: u32 = 0x4ade80;
    pub const ERROR: u32 = 0xf87171;
    pub const NUMBER: u32 = 0x93c5fd;
    pub const STRING: u32 = 0xfcd34d;
    pub const BOOL: u32 = 0xc4b5fd;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Money,
    Settings,
    Scripts,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Overview, Tab::Money, Tab::Settings, Tab::Scripts];

    fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Money => "Money",
            Tab::Settings => "Game settings",
            Tab::Scripts => "Script state",
        }
    }
}

/// What the value editor at the bottom of the Settings / Scripts tab edits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Setting(usize),
    Leaf(usize),
}

struct Status {
    text: String,
    error: bool,
}

struct SaveEditor {
    path: Option<PathBuf>,
    save: Option<SaveFile>,
    busy: Option<&'static str>,
    /// File being loaded, shown in the loading view.
    loading: Option<PathBuf>,
    dirty: bool,
    backed_up: bool,
    status: Option<Status>,
    tab: Tab,
    thumbnail: Option<Arc<Image>>,
    money_input: Entity<InputState>,
    value_input: Entity<InputState>,
    selection: Option<Selection>,
    script: usize,
    rows: Rc<Vec<Leaf>>,
    rows_scroll: UniformListScrollHandle,
    scripts_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    journal_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SaveEditor {
    fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let money_input = cx.new(|cx| InputState::new(window, cx).placeholder("New balance"));
        let value_input = cx.new(|cx| InputState::new(window, cx).placeholder("Select a value to edit"));
        let subscriptions = vec![
            cx.subscribe_in(&money_input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_money(window, cx);
                }
            }),
            cx.subscribe_in(&value_input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_value(window, cx);
                }
            }),
        ];
        let mut this = Self {
            path: None,
            save: None,
            busy: None,
            loading: None,
            dirty: false,
            backed_up: false,
            status: None,
            tab: Tab::Overview,
            thumbnail: None,
            money_input,
            value_input,
            selection: None,
            script: 0,
            rows: Rc::default(),
            rows_scroll: UniformListScrollHandle::new(),
            scripts_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            journal_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        if let Some(path) = initial {
            this.load(path, window, cx);
        }
        this
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status { text: text.into(), error });
    }

    // ----- file handling ---------------------------------------------------

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Transport Fever 3 save".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                this.update_in(cx, |this, window, cx| this.load(path, window, cx)).ok();
            }
        })
        .detach();
    }

    fn load(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("Loading…");
        self.loading = Some(path.clone());
        self.status = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let p = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let save = SaveFile::open(&p)?;
                    let png = save.header.thumbnail.as_ref().and_then(|t| t.to_png().ok());
                    Ok::<_, tf3save::Error>((save, png))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.loading = None;
                match result {
                    Ok((save, png)) => this.loaded(path, save, png, window, cx),
                    Err(e) => this.set_status(format!("Could not open {}: {e}", path.display()), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn loaded(
        &mut self,
        path: PathBuf,
        save: SaveFile,
        png: Option<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut notes = Vec::new();
        if save.header.version != tf3save::KNOWN_VERSION {
            notes.push(format!(
                "save format version {} differs from the tested version {}",
                save.header.version,
                tf3save::KNOWN_VERSION
            ));
        }
        if save.journal.is_none() {
            notes.push("money section not found, money is read-only".into());
        }
        if save.scripts.is_none() {
            notes.push("script state section not found".into());
        }
        let money = save.money().to_string();
        self.money_input.update(cx, |s, cx| s.set_value(money, window, cx));
        self.thumbnail = png.map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
        self.save = Some(save);
        self.path = Some(path.clone());
        self.dirty = false;
        self.backed_up = false;
        self.selection = None;
        self.script = 0;
        self.refresh_rows();
        window.set_window_title(&format!("{} — TF3 Save Editor", file_name(&path)));
        if notes.is_empty() {
            self.set_status(format!("Opened {}", file_name(&path)), false);
        } else {
            self.set_status(format!("Opened with warnings: {}", notes.join("; ")), true);
        }
    }

    fn save_to(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(save) = self.save.take() else { return };
        let make_backup = Some(&target) == self.path.as_ref() && !self.backed_up && target.exists();
        self.busy = Some("Saving…");
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let t = target.clone();
            let (save, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = (|| {
                        let backup = if make_backup { Some(tf3save::backup(&t)?) } else { None };
                        save.save(&t)?;
                        Ok::<_, tf3save::Error>(backup)
                    })();
                    (save, result)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.save = Some(save);
                match result {
                    Ok(backup) => {
                        this.dirty = false;
                        if backup.is_some() || Some(&target) != this.path.as_ref() {
                            this.backed_up = Some(&target) == this.path.as_ref() || this.backed_up;
                        }
                        let mut msg = format!("Saved {}", target.display());
                        if let Some(b) = backup {
                            msg.push_str(&format!(" (backup: {})", file_name(&b)));
                        }
                        this.path = Some(target.clone());
                        window.set_window_title(&format!("{} — TF3 Save Editor", file_name(&target)));
                        this.set_status(msg, false);
                    }
                    Err(e) => this.set_status(format!("Saving failed: {e}"), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn prompt_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else { return };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let suggested = format!("{} (edited).sav", path.file_stem().and_then(|s| s.to_str()).unwrap_or("savegame"));
        let rx = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(target))) = rx.await {
                this.update_in(cx, |this, window, cx| this.save_to(target, window, cx)).ok();
            }
        })
        .detach();
    }

    // ----- editing ---------------------------------------------------------

    fn refresh_rows(&mut self) {
        let rows = self
            .save
            .as_ref()
            .and_then(|s| s.scripts.as_ref())
            .and_then(|s| s.scripts.get(self.script))
            .map(|s| s.state.flatten())
            .unwrap_or_default();
        self.rows = Rc::new(rows);
    }

    fn select_script(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.script = ix;
        self.selection = None;
        self.value_input.update(cx, |s, cx| s.set_value("", window, cx));
        self.refresh_rows();
        cx.notify();
    }

    fn select(&mut self, sel: Selection, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self.selected_value(sel) else { return };
        self.selection = Some(sel);
        let text = match &value {
            Value::Number(n) => format_number(*n),
            Value::Bool(b) => b.to_string(),
            Value::String(s) => s.as_str_lossy().into_owned(),
            _ => String::new(),
        };
        self.value_input.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    fn selected_value(&self, sel: Selection) -> Option<Value> {
        let save = self.save.as_ref()?;
        match sel {
            Selection::Setting(i) => save.header.config.settings()?.entries.get(i).map(|(_, v)| v.clone()),
            Selection::Leaf(i) => {
                let leaf = self.rows.get(i)?;
                leaf.is_editable().then(|| leaf.value.clone())
            }
        }
    }

    fn apply_value(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.selection else { return };
        let input = self.value_input.read(cx).value().to_string();
        let script = self.script;
        let rows = self.rows.clone();
        let Some(save) = self.save.as_mut() else { return };
        let slot = match sel {
            Selection::Setting(i) => {
                save.header.config.settings_mut().and_then(|t| t.entries.get_mut(i)).map(|(_, v)| v)
            }
            Selection::Leaf(i) => rows
                .get(i)
                .and_then(|leaf| save.scripts.as_mut()?.scripts.get_mut(script)?.state.lookup_mut(&leaf.path)),
        };
        let Some(slot) = slot else { return };
        match slot.parse_like(&input) {
            Ok(new) if new == *slot => {}
            Ok(new) => {
                let msg = format!("Changed {} → {}", format_scalar(slot), format_scalar(&new));
                *slot = new;
                self.dirty = true;
                if matches!(sel, Selection::Leaf(_)) {
                    self.refresh_rows();
                }
                self.set_status(msg, false);
            }
            Err(e) => self.set_status(e.to_string(), true),
        }
        cx.notify();
    }

    fn toggle_bool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.selection else { return };
        if let Some(Value::Bool(b)) = self.selected_value(sel) {
            self.value_input.update(cx, |s, cx| s.set_value((!b).to_string(), window, cx));
            self.apply_value(window, cx);
        }
    }

    fn apply_money(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let input = self.money_input.read(cx).value().to_string();
        let cleaned: String = input.chars().filter(|c| !matches!(c, '_' | ',' | ' ' | '$')).collect();
        let Some(save) = self.save.as_mut() else { return };
        match cleaned.parse::<i64>() {
            Ok(v) if v == save.money() => {}
            Ok(v) => match save.set_money(v) {
                Ok(()) => {
                    self.dirty = true;
                    self.set_status(format!("Money set to {}", group(v)), false);
                }
                Err(e) => self.set_status(e.to_string(), true),
            },
            Err(_) => self.set_status(format!("{input:?} is not a whole number"), true),
        }
        cx.notify();
    }

    // ----- rendering -------------------------------------------------------

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_save = self.save.is_some() && self.busy.is_none();
        let title = match &self.path {
            Some(p) => format!("{}{}", file_name(p), if self.dirty { " •" } else { "" }),
            None => "No save loaded".into(),
        };
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .items_center()
            .bg(rgb(colors::PANEL))
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .child(
                button("open", "Open…", true).on_click(cx.listener(|this, _, window, cx| this.prompt_open(window, cx))),
            )
            .child(button("save", "Save", has_save).on_click(cx.listener(|this, _, window, cx| {
                if let Some(p) = this.path.clone() {
                    this.save_to(p, window, cx);
                }
            })))
            .child(
                button("save-as", "Save as…", has_save)
                    .on_click(cx.listener(|this, _, window, cx| this.prompt_save_as(window, cx))),
            )
            .child(div().ml_2().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().flex_1())
            .when_some(self.busy, |d, busy| {
                d.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().text_sm().text_color(rgb(colors::MUTED)).child(busy))
                        .child(busy_bar("toolbar-busy", busy, 140.)),
                )
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(180.))
            .h_full()
            .py_2()
            .bg(rgb(colors::SIDEBAR))
            .border_r_1()
            .border_color(rgb(colors::BORDER))
            .children(Tab::ALL.into_iter().map(|tab| {
                let active = self.tab == tab;
                div()
                    .id(tab.label())
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(colors::ROW_SELECTED)).text_color(rgb(colors::TEXT)))
                    .when(!active, |d| d.text_color(rgb(colors::MUTED)).hover(|s| s.bg(rgb(colors::ROW_HOVER))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        this.selection = None;
                        cx.notify();
                    }))
                    .child(tab.label())
            }))
    }

    fn render_overview(&self, save: &SaveFile) -> impl IntoElement {
        let h = &save.header;
        let mut facts: Vec<(&str, String)> = vec![
            ("Title", h.title()),
            ("Year", h.year.to_string()),
            ("Money", group(save.money())),
            ("Format version", h.version.to_string()),
        ];
        for m in &h.mods {
            facts.push(("Mod", format!("{} ({})", m.name, m.id)));
        }
        for (k, v) in &h.config.resources {
            facts.push(("Resource", format!("{k}: {v}")));
        }
        facts.push((
            "Money section",
            match &save.journal {
                Some(j) => format!("{} bookings", j.entries.len()),
                None => format!("not found ({} candidates)", save.journal_matches),
            },
        ));
        facts.push((
            "Script states",
            save.scripts.as_ref().map_or("not found".into(), |s| format!("{} scripts", s.scripts.len())),
        ));
        v_flex()
            .gap_4()
            .p_4()
            .when_some(self.thumbnail.clone(), |d, image| {
                d.child(img(image).w(px(480.)).h(px(270.)).rounded(px(6.)).border_1().border_color(rgb(colors::BORDER)))
            })
            .child(v_flex().gap_1().children(facts.into_iter().map(|(k, v)| {
                h_flex().gap_3().child(div().w(px(140.)).text_color(rgb(colors::MUTED)).child(k)).child(div().child(v))
            })))
    }

    fn render_money(&self, save: &SaveFile, cx: &mut Context<Self>) -> impl IntoElement {
        let editable = save.journal.is_some();
        let now = save.journal.as_ref().and_then(|j| j.entries.last()).map_or(0, |e| e.time);
        let year = save.header.year;
        let recent: Vec<_> =
            save.journal.as_ref().map(|j| j.entries.iter().rev().take(200).copied().collect()).unwrap_or_default();
        v_flex()
            .size_full()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_color(rgb(colors::MUTED)).child("Current balance"))
                    .child(div().text_xl().font_weight(FontWeight::BOLD).child(group(save.money()))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(rgb(colors::MUTED)).child("New balance"))
                    .child(text_box(&self.money_input, 220.))
                    .child(button("apply-money", "Apply", editable).on_click(cx.listener(|this, _, window, cx| this.apply_money(window, cx)))),
            )
            .child(div().text_sm().text_color(rgb(colors::MUTED)).child(
                "The difference is booked as an \"Other\" entry in the finance journal, as the game does for its own balance edits.",
            ))
            .child(div().mt_2().font_weight(FontWeight::SEMIBOLD).child("Latest bookings"))
            .child(
                scroll_panel(
                    "journal",
                    &self.journal_scroll,
                    v_flex().children(recent.into_iter().map(|e| {
                        h_flex()
                            .gap_3()
                            .px_2()
                            .py(px(2.))
                            .child(div().w(px(110.)).text_color(rgb(colors::MUTED)).child(approx_year(e.time, now, year)))
                            .child(
                                div()
                                    .w(px(140.))
                                    .flex()
                                    .justify_end()
                                    .text_color(rgb(if e.amount < 0 { colors::ERROR } else { colors::OK }))
                                    .child(group(e.amount)),
                            )
                            .child(div().child(e.category.label()))
                    })),
                ),
            )
    }

    fn render_settings(&self, save: &SaveFile, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, Value)> = save
            .header
            .config
            .settings()
            .map(|t| t.entries.iter().map(|(k, v)| (k.as_str().unwrap_or("?").to_string(), v.clone())).collect())
            .unwrap_or_default();
        let selected = self.selection;
        v_flex()
            .size_full()
            .gap_2()
            .p_4()
            .child(div().text_sm().text_color(rgb(colors::MUTED)).child(
                "Options picked when the game was created. Values are option indices, as in the new-game screen.",
            ))
            .child(scroll_panel(
                "settings",
                &self.settings_scroll,
                v_flex().children(entries.into_iter().enumerate().map(|(i, (k, v))| {
                    let sel = Selection::Setting(i);
                    value_row(("setting", i), 0, k, &v, selected == Some(sel))
                        .on_click(cx.listener(move |this, _, window, cx| this.select(sel, window, cx)))
                })),
            ))
            .child(self.render_value_editor(cx))
    }

    fn render_scripts(&self, save: &SaveFile, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(states) = &save.scripts else {
            return div().p_4().child("Script states were not found in this save.").into_any_element();
        };
        let names: Vec<String> = states.scripts.iter().map(|s| s.name.clone()).collect();
        let current = self.script;
        let rows = self.rows.clone();
        let selected = self.selection;
        let list = uniform_list(
            "rows",
            rows.len(),
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|i| {
                        let leaf = &rows[i];
                        let sel = Selection::Leaf(i);
                        value_row(("leaf", i), leaf.depth, leaf.key.clone(), &leaf.value, selected == Some(sel))
                            .when(leaf.is_editable(), |d| {
                                d.on_click(cx.listener(move |this, _, window, cx| this.select(sel, window, cx)))
                            })
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.rows_scroll)
        .size_full();

        h_flex()
            .size_full()
            .child(
                div()
                    .relative()
                    .w(px(300.))
                    .h_full()
                    .border_r_1()
                    .border_color(rgb(colors::BORDER))
                    .child(
                        div()
                            .id("script-list")
                            .track_scroll(&self.scripts_scroll)
                            .overflow_y_scroll()
                            .size_full()
                            .py_1()
                            .children(names.into_iter().enumerate().map(|(i, name)| {
                                div()
                                    .id(("script", i))
                                    .px_3()
                                    .py_1()
                                    .text_sm()
                                    .cursor_pointer()
                                    .when(i == current, |d| d.bg(rgb(colors::ROW_SELECTED)))
                                    .when(i != current, |d| d.hover(|s| s.bg(rgb(colors::ROW_HOVER))))
                                    .on_click(cx.listener(move |this, _, window, cx| this.select_script(i, window, cx)))
                                    .child(name)
                            })),
                    )
                    .child(Scrollbar::vertical(&self.scripts_scroll)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .gap_2()
                    .p_4()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .rounded(px(4.))
                            .child(list)
                            .child(Scrollbar::vertical(&self.rows_scroll)),
                    )
                    .child(self.render_value_editor(cx)),
            )
            .into_any_element()
    }

    fn render_value_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, is_bool, enabled) = match self.selection.and_then(|s| self.selected_value(s).map(|v| (s, v))) {
            Some((sel, v)) => {
                let name = match sel {
                    Selection::Setting(i) => self
                        .save
                        .as_ref()
                        .and_then(|s| s.header.config.settings())
                        .and_then(|t| t.entries.get(i))
                        .and_then(|(k, _)| k.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    Selection::Leaf(i) => self.rows.get(i).map(Leaf::path_string).unwrap_or_default(),
                };
                (format!("{name} ({})", v.type_name()), matches!(v, Value::Bool(_)), true)
            }
            None => ("Click a value to edit it".to_string(), false, false),
        };
        v_flex().gap_1().child(div().text_sm().text_color(rgb(colors::MUTED)).child(label)).child(
            h_flex()
                .gap_2()
                .items_center()
                .child(text_box(&self.value_input, 360.))
                .child(
                    button("apply-value", "Apply", enabled)
                        .on_click(cx.listener(|this, _, window, cx| this.apply_value(window, cx))),
                )
                .when(is_bool, |d| {
                    d.child(
                        button("toggle", "Toggle", true)
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_bool(window, cx))),
                    )
                }),
        )
    }

    fn render_status(&self) -> impl IntoElement {
        h_flex().px_3().py_1().text_sm().bg(rgb(colors::PANEL)).border_t_1().border_color(rgb(colors::BORDER)).child(
            match &self.status {
                Some(s) => {
                    div().text_color(rgb(if s.error { colors::ERROR } else { colors::MUTED })).child(s.text.clone())
                }
                None => div().text_color(rgb(colors::MUTED)).child("Ready"),
            },
        )
    }
}

impl Render for SaveEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.save.as_ref() {
            None if self.loading.is_some() => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Loading save…"))
                .child(
                    div()
                        .text_color(rgb(colors::MUTED))
                        .child(self.loading.as_deref().map(file_name).unwrap_or_default()),
                )
                .child(busy_bar("body-busy", "Loading save", 320.))
                .into_any_element(),
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Transport Fever 3 Save Editor"))
                .child(
                    div()
                        .text_color(rgb(colors::MUTED))
                        .child("Open a .sav file to begin. Keep a copy of saves you care about."),
                )
                .into_any_element(),
            Some(save) => {
                let content = match self.tab {
                    Tab::Overview => div()
                        .id("overview")
                        .size_full()
                        .overflow_y_scroll()
                        .child(self.render_overview(save))
                        .into_any_element(),
                    Tab::Money => self.render_money(save, cx).into_any_element(),
                    Tab::Settings => self.render_settings(save, cx).into_any_element(),
                    Tab::Scripts => self.render_scripts(save, cx).into_any_element(),
                };
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().h_full().min_w_0().child(content))
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .bg(rgb(colors::BG))
            .text_color(rgb(colors::TEXT))
            .child(self.render_toolbar(cx))
            .child(body)
            .child(self.render_status())
    }
}

// ----- small widgets -------------------------------------------------------

fn button(id: &'static str, label: &'static str, enabled: bool) -> Button {
    Button::new(id)
        .px_3()
        .py_1()
        .rounded(px(5.))
        .text_sm()
        .disabled(!enabled)
        .when(enabled, |b| {
            b.bg(rgb(colors::ACCENT)).text_color(rgb(0xffffff)).hover(|s| s.bg(rgb(colors::ACCENT_HOVER)))
        })
        .when(!enabled, |b| b.bg(rgb(colors::ROW_HOVER)).text_color(rgb(colors::MUTED)))
        .child(label)
}

/// Indeterminate progress bar: a highlight sweeping across a track.
fn busy_bar(id: &'static str, label: &'static str, width: f32) -> impl IntoElement {
    let glide = width * 0.3;
    Progress::new(id).indeterminate(true).accessibility_label(label).w(px(width)).h(px(6.)).child(
        ProgressTrack::new().relative().size_full().overflow_hidden().rounded(px(3.)).bg(rgb(colors::ROW_HOVER)).child(
            ProgressIndicator::new()
                .absolute()
                .top_0()
                .h_full()
                .w(px(glide))
                .rounded(px(3.))
                .bg(rgb(colors::ACCENT))
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1100)).repeat().with_easing(gpui::ease_in_out),
                    move |bar, t| bar.left(px(-glide + t * (width + glide))),
                ),
        ),
    )
}

fn text_box(state: &Entity<InputState>, width: f32) -> impl IntoElement {
    div()
        .w(px(width))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .bg(rgb(colors::SIDEBAR))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .rounded(px(4.))
        .child(Input::new(state))
}

fn scroll_panel(id: &'static str, handle: &ScrollHandle, content: impl IntoElement) -> impl IntoElement {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .border_1()
        .border_color(rgb(colors::BORDER))
        .rounded(px(4.))
        .child(div().id(id).track_scroll(handle).overflow_y_scroll().size_full().p_1().child(content))
        .child(Scrollbar::vertical(handle))
}

fn value_row(
    id: impl Into<gpui::ElementId>,
    depth: usize,
    key: String,
    value: &Value,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let (text, color) = match value {
        Value::Table(Some(_)) => (String::new(), colors::MUTED),
        Value::Number(_) => (format_scalar(value), colors::NUMBER),
        Value::String(_) => (format_scalar(value), colors::STRING),
        Value::Bool(_) => (format_scalar(value), colors::BOOL),
        _ => (format_scalar(value), colors::MUTED),
    };
    h_flex()
        .id(id.into())
        .h(px(24.))
        .pl(px(8. + depth as f32 * 16.))
        .pr_2()
        .gap_3()
        .text_sm()
        .when(selected, |d| d.bg(rgb(colors::ROW_SELECTED)))
        .when(!selected, |d| d.hover(|s| s.bg(rgb(colors::ROW_HOVER))))
        .child(div().min_w(px(200.)).child(key))
        .child(div().text_color(rgb(color)).child(text))
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `-1234567` → `-1,234,567`.
fn group(v: i64) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0 { format!("-{out}") } else { out }
}

/// Game time is in milliseconds and one in-game year lasts 730.5 s by
/// default; the newest booking is taken as "now" in the save's year.
fn approx_year(ms: i64, now_ms: i64, year: u32) -> String {
    let years_ago = ((now_ms - ms) as f64 / 730_500.0).floor() as i64;
    format!("{}", year as i64 - years_ago)
}

fn main() {
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_base::init(cx);
        let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |window, cx| {
                window.set_window_title("TF3 Save Editor");
                let view = cx.new(|cx| SaveEditor::new(initial, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn grouping() {
        assert_eq!(super::group(-2747963), "-2,747,963");
        assert_eq!(super::group(100), "100");
        assert_eq!(super::group(1000), "1,000");
    }
}
