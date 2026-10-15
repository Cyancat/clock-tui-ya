//! Interactive setup wizard shown on a bare `tclock` invocation.
//!
//! Flow: mode list → parameter form (with live preview) → run the mode →
//! `q` inside the mode returns to the list. Quitting from the list exits
//! the process.
//!
//! Layout constraints: fields render one per line with ASCII markers. The
//! edit cursor column is computed with display-cell widths (unicode-width),
//! so CJK labels/values position it correctly.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Widget},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::modes::{Clock, Countdown, DurationFormat, Stopwatch, Timer};
use super::{
    build_countdown_slots_silent, parse_color, parse_datetime, parse_duration, parse_timezone,
    render_countdown_grid_to, AppParams, ModeConfig, ModeKind,
};
use crate::config::{Config, WizardState};

/// Minimum terminal size for the wizard; below this a notice is shown.
const MIN_WIDTH: u16 = 50;
const MIN_HEIGHT: u16 = 15;
/// Below this width the preview pane is hidden.
const PREVIEW_MIN_WIDTH: u16 = 100;

const MODE_ENTRIES: &[(ModeKind, &str, &str)] = &[
    (ModeKind::Clock, "Clock", "显示当前时间，支持时区"),
    (ModeKind::Timer, "Timer", "倒计时 / 番茄钟，可多段顺序执行"),
    (ModeKind::Stopwatch, "Stopwatch", "秒表"),
    (ModeKind::Countdown, "Countdown", "倒数到单个目标时间"),
    (
        ModeKind::CountdownGrid,
        "Countdown Grid",
        "配置文件 [[countdown.items]] 网格同屏",
    ),
];

fn mode_index(kind: ModeKind) -> usize {
    MODE_ENTRIES
        .iter()
        .position(|(k, ..)| *k == kind)
        .unwrap_or(0)
}

/// Stable field identity, independent of per-mode field ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Timezone,
    ShowDate,
    ShowSeconds,
    ShowMillis,
    Color,
    Size,
    ConfigPath,
    Durations,
    Titles,
    Repeat,
    Paused,
    AutoQuit,
    Execute,
    CdTime,
    CdTitle,
    CdContinue,
    CdReverse,
    CdMillis,
}

struct Field {
    id: FieldId,
    label: &'static str,
    /// One-line hint shown under the field while it is focused.
    hint: &'static str,
    kind: FieldKind,
    visible: bool,
}

enum FieldKind {
    Toggle(bool),
    Text { value: String, placeholder: &'static str },
    Int { value: u16, min: u16, max: u16 },
    StrList { items: Vec<String>, cursor: usize },
}

impl Field {
    fn toggle(id: FieldId, label: &'static str, on: bool) -> Self {
        Field {
            id,
            label,
            hint: "",
            kind: FieldKind::Toggle(on),
            visible: true,
        }
    }

    fn text(id: FieldId, label: &'static str, placeholder: &'static str) -> Self {
        Field {
            id,
            label,
            hint: "",
            kind: FieldKind::Text {
                value: String::new(),
                placeholder,
            },
            visible: true,
        }
    }

    fn str_list(id: FieldId, label: &'static str, items: Vec<String>) -> Self {
        Field {
            id,
            label,
            hint: "",
            kind: FieldKind::StrList { items, cursor: 0 },
            visible: true,
        }
    }
}

/// What a key press wants the wizard to do, decided by `Wizard::on_key`.
enum FormAction {
    Continue,
    Back,
    Submit,
    LoadConfig(String),
    Quit,
}

struct Form {
    kind: ModeKind,
    fields: Vec<Field>,
    focus: usize,
    editing: bool,
    dirty: bool,
    /// Insert position (char index) inside the string being edited.
    /// Only meaningful while `editing` is true.
    edit_cursor: usize,
}

impl Form {
    fn new(kind: ModeKind) -> Self {
        let mut fields: Vec<Field> = match kind {
            ModeKind::Clock => vec![
                Field::text(FieldId::Timezone, "时区", "留空 = 本地时区，例 Asia/Shanghai"),
                Field::toggle(FieldId::ShowDate, "显示日期", true),
                Field::toggle(FieldId::ShowSeconds, "显示秒", true),
                Field::toggle(FieldId::ShowMillis, "显示毫秒", false),
            ],
            ModeKind::Timer => vec![
                Field::str_list(
                    FieldId::Durations,
                    "时长",
                    vec!["25m".to_string(), "5m".to_string()],
                ),
                Field::str_list(FieldId::Titles, "标题", vec![]),
                Field::toggle(FieldId::Repeat, "重复", false),
                Field::toggle(FieldId::Paused, "暂停启动", false),
                Field::toggle(FieldId::ShowMillis, "显示毫秒", true),
                Field::toggle(FieldId::AutoQuit, "结束自动退出", false),
                Field::str_list(FieldId::Execute, "结束执行命令", vec![]),
            ],
            ModeKind::Stopwatch => vec![],
            ModeKind::Countdown => vec![
                Field::text(
                    FieldId::CdTime,
                    "目标时间",
                    "例 20:00 / 2026-01-01 / 2026-01-01 20:00:00",
                ),
                Field::text(FieldId::CdTitle, "标题", "可选"),
                Field::toggle(FieldId::CdContinue, "过零继续计时", false),
                Field::toggle(FieldId::CdReverse, "反向计时", false),
                Field::toggle(FieldId::CdMillis, "显示毫秒", false),
            ],
            ModeKind::CountdownGrid => vec![],
        };
        // Global fields shared by every mode.
        fields.push(Field::text(
            FieldId::Color,
            "颜色",
            "留空 = green，例 red / #e63946",
        ));
        fields.push(Field {
            id: FieldId::Size,
            label: "字号",
            hint: "",
            kind: FieldKind::Int {
                value: 1,
                min: 1,
                max: 10,
            },
            visible: true,
        });
        let mut config_field = Field::text(
            FieldId::ConfigPath,
            "config 路径",
            "可选：TOML 配置路径，Enter 载入",
        );
        config_field.hint = if kind == ModeKind::CountdownGrid {
            "网格模式必须载入含 [[countdown.items]] 的配置文件"
        } else {
            "没有默认配置路径，必须显式指定"
        };
        fields.push(config_field);

        let mut form = Form {
            kind,
            fields,
            focus: 0,
            editing: false,
            dirty: false,
            edit_cursor: 0,
        };
        if kind == ModeKind::Timer {
            if let Some(f) = form.get_mut(FieldId::Durations) {
                f.hint = "格式 10s / 5m / 1h / 1d，至少 1 项；a 添加，d 删除";
            }
            if let Some(f) = form.get_mut(FieldId::Execute) {
                f.hint = "所有项拼接后交给 sh -c 执行（Windows 上无 sh）";
            }
        }
        form
    }

    /// Set a text field's value without marking the form dirty
    /// (used to prefill the persisted config path).
    fn set_text(&mut self, id: FieldId, value: &str) {
        if let Some(Field {
            kind: FieldKind::Text { value: v, .. },
            ..
        }) = self.get_mut(id)
        {
            *v = value.to_string();
        }
    }

    fn get(&self, id: FieldId) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    fn get_mut(&mut self, id: FieldId) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.id == id)
    }

    fn text(&self, id: FieldId) -> String {
        match self.get(id).map(|f| &f.kind) {
            Some(FieldKind::Text { value, .. }) => value.trim().to_string(),
            _ => String::new(),
        }
    }

    fn toggle_value(&self, id: FieldId) -> bool {
        match self.get(id).map(|f| &f.kind) {
            Some(FieldKind::Toggle(on)) => *on,
            _ => false,
        }
    }

    fn int_value(&self, id: FieldId) -> u16 {
        match self.get(id).map(|f| &f.kind) {
            Some(FieldKind::Int { value, .. }) => *value,
            _ => 1,
        }
    }

    /// Trimmed, non-empty list items.
    fn list_items(&self, id: FieldId) -> Vec<String> {
        match self.get(id).map(|f| &f.kind) {
            Some(FieldKind::StrList { items, .. }) => items
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => vec![],
        }
    }

    /// Raw list items including empties (for validation messages).
    fn raw_list(&self, id: FieldId) -> &[String] {
        match self.get(id).map(|f| &f.kind) {
            Some(FieldKind::StrList { items, .. }) => items,
            _ => &[],
        }
    }

    fn move_focus(&mut self, dir: i32) {
        let n = self.fields.len() as i32;
        let mut i = self.focus as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.fields[i as usize].visible {
                break;
            }
        }
        self.focus = i as usize;
    }

    fn current_is_list(&self) -> bool {
        matches!(self.fields[self.focus].kind, FieldKind::StrList { .. })
    }

    fn current_edit_string(&mut self) -> Option<&mut String> {
        match &mut self.fields[self.focus].kind {
            FieldKind::Text { value, .. } => Some(value),
            FieldKind::StrList { items, cursor } => items.get_mut(*cursor),
            _ => None,
        }
    }

    /// Char count of the string currently being edited (0 when not editing).
    fn edit_len(&self) -> usize {
        match &self.fields[self.focus].kind {
            FieldKind::Text { value, .. } => value.chars().count(),
            FieldKind::StrList { items, cursor } => items
                .get(*cursor)
                .map(|s| s.chars().count())
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Start editing the focused field with the insert cursor at the end.
    fn begin_editing(&mut self) {
        self.edit_cursor = self.edit_len();
        self.editing = true;
    }

    fn on_key(&mut self, key: KeyEvent) -> FormAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('r') | KeyCode::Char('R') => return FormAction::Submit,
                KeyCode::Char('c') => return FormAction::Quit,
                _ => {}
            }
        }
        if self.editing {
            return self.on_edit_key(key);
        }
        match key.code {
            KeyCode::Esc => FormAction::Back,
            KeyCode::Tab => {
                self.move_focus(1);
                FormAction::Continue
            }
            KeyCode::BackTab => {
                self.move_focus(-1);
                FormAction::Continue
            }
            KeyCode::Up => self.move_up_down(-1),
            KeyCode::Down => self.move_up_down(1),
            KeyCode::Enter => self.activate(),
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                let dir = if key.code == KeyCode::Left { -1 } else { 1 };
                self.adjust(dir);
                FormAction::Continue
            }
            KeyCode::Char('a') if self.current_is_list() => {
                if let FieldKind::StrList { items, cursor } = &mut self.fields[self.focus].kind {
                    items.push(String::new());
                    *cursor = items.len() - 1;
                }
                self.begin_editing();
                self.dirty = true;
                FormAction::Continue
            }
            KeyCode::Char('d') if self.current_is_list() => {
                if let FieldKind::StrList { items, cursor } = &mut self.fields[self.focus].kind {
                    if !items.is_empty() {
                        items.remove((*cursor).min(items.len() - 1));
                        if *cursor >= items.len() {
                            *cursor = items.len().saturating_sub(1);
                        }
                        self.dirty = true;
                    }
                }
                FormAction::Continue
            }
            _ => FormAction::Continue,
        }
    }

    fn on_edit_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Enter => {
                self.editing = false;
                self.dirty = true;
                if self.fields[self.focus].id == FieldId::ConfigPath {
                    let path = self.text(FieldId::ConfigPath);
                    return FormAction::LoadConfig(path);
                }
                FormAction::Continue
            }
            KeyCode::Esc => {
                self.editing = false;
                FormAction::Continue
            }
            KeyCode::Left => {
                self.edit_cursor = self.edit_cursor.saturating_sub(1);
                FormAction::Continue
            }
            KeyCode::Right => {
                self.edit_cursor = (self.edit_cursor + 1).min(self.edit_len());
                FormAction::Continue
            }
            KeyCode::Home => {
                self.edit_cursor = 0;
                FormAction::Continue
            }
            KeyCode::End => {
                self.edit_cursor = self.edit_len();
                FormAction::Continue
            }
            KeyCode::Backspace => {
                let cur = self.edit_cursor;
                if cur > 0 {
                    if let Some(s) = self.current_edit_string() {
                        s.replace_range(char_to_byte(s, cur - 1)..char_to_byte(s, cur), "");
                    }
                    self.edit_cursor = cur - 1;
                    self.dirty = true;
                }
                FormAction::Continue
            }
            KeyCode::Delete => {
                let cur = self.edit_cursor;
                if cur < self.edit_len() {
                    if let Some(s) = self.current_edit_string() {
                        s.replace_range(char_to_byte(s, cur)..char_to_byte(s, cur + 1), "");
                    }
                    self.dirty = true;
                }
                FormAction::Continue
            }
            KeyCode::Char(c) => {
                let cur = self.edit_cursor;
                if let Some(s) = self.current_edit_string() {
                    s.insert(char_to_byte(s, cur), c);
                }
                self.edit_cursor = cur + 1;
                self.dirty = true;
                FormAction::Continue
            }
            _ => FormAction::Continue,
        }
    }

    /// Enter on the focused field.
    fn activate(&mut self) -> FormAction {
        match &mut self.fields[self.focus].kind {
            FieldKind::Toggle(on) => {
                *on = !*on;
                self.dirty = true;
            }
            FieldKind::Text { .. } => {
                self.begin_editing();
            }
            FieldKind::Int { .. } => {} // adjusted with Left/Right
            FieldKind::StrList { items, cursor } => {
                if items.is_empty() {
                    items.push(String::new());
                    *cursor = 0;
                }
                self.begin_editing();
            }
        }
        FormAction::Continue
    }

    /// Space / Left / Right on the focused field.
    fn adjust(&mut self, dir: i32) {
        match &mut self.fields[self.focus].kind {
            FieldKind::Toggle(on) => {
                *on = !*on;
                self.dirty = true;
            }
            FieldKind::Int { value, min, max } => {
                let v = *value as i32 + dir;
                *value = v.clamp(*min as i32, *max as i32) as u16;
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Up/Down: inside a list field move the item cursor first, leaving the
    /// field at the list edges.
    fn move_up_down(&mut self, dir: i32) -> FormAction {
        if let FieldKind::StrList { items, cursor } = &mut self.fields[self.focus].kind {
            if !items.is_empty() {
                let next = *cursor as i32 + dir;
                if next >= 0 && next < items.len() as i32 {
                    *cursor = next as usize;
                    return FormAction::Continue;
                }
            }
        }
        self.move_focus(dir);
        FormAction::Continue
    }

    /// Validate all visible fields; returns (field, message) pairs.
    fn validate(&self, loaded_config: Option<&Config>) -> Vec<(FieldId, String)> {
        let mut errors = Vec::new();

        let timezone = self.text(FieldId::Timezone);
        if self.get(FieldId::Timezone).map(|f| f.visible).unwrap_or(false)
            && !timezone.is_empty()
            && parse_timezone(&timezone).is_err()
        {
            errors.push((
                FieldId::Timezone,
                "时区无效（大小写敏感），例 Asia/Shanghai".to_string(),
            ));
        }

        let color = self.text(FieldId::Color);
        if !color.is_empty() && parse_color(&color).is_err() {
            errors.push((
                FieldId::Color,
                "颜色无效，例 green / lightred / #e63946".to_string(),
            ));
        }

        match self.kind {
            ModeKind::Timer => {
                let raw = self.raw_list(FieldId::Durations);
                let items = self.list_items(FieldId::Durations);
                if items.is_empty() {
                    errors.push((
                        FieldId::Durations,
                        "至少需要 1 个有效时长（空列表会导致 panic）".to_string(),
                    ));
                } else if let Some(bad) =
                    raw.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).find(|d| parse_duration(d).is_err())
                {
                    errors.push((
                        FieldId::Durations,
                        format!("时长无效: \"{}\"，格式 10s / 5m / 1h / 1d", bad),
                    ));
                }
            }
            ModeKind::Countdown => {
                let time = self.text(FieldId::CdTime);
                if time.is_empty() {
                    errors.push((FieldId::CdTime, "目标时间必填".to_string()));
                } else if parse_datetime(&time).is_err() {
                    errors.push((
                        FieldId::CdTime,
                        "格式无效，例 20:00 / 2026-01-01 / 2026-01-01 20:00:00".to_string(),
                    ));
                }
            }
            ModeKind::CountdownGrid => match loaded_config {
                None => errors.push((
                    FieldId::ConfigPath,
                    "请先在 config 路径字段载入配置文件".to_string(),
                )),
                Some(c) if c.countdown.items.is_empty() && c.countdown.time.is_none() => {
                    errors.push((
                        FieldId::ConfigPath,
                        "配置文件不含 [[countdown.items]] 或 [countdown].time".to_string(),
                    ));
                }
                _ => {}
            },
            _ => {}
        }
        errors
    }
}

pub(crate) enum WizardOutcome {
    Continue,
    Run(ModeConfig),
    Quit,
}

pub(crate) struct Wizard {
    selected: usize,
    /// Per-mode form cache: edits survive mode switches within the session.
    forms: Vec<Option<Form>>,
    active: Option<usize>,
    confirming_discard: bool,
    config_path: Option<PathBuf>,
    loaded_config: Option<Config>,
    status: Option<String>,
}

impl Wizard {
    pub(crate) fn new(preselect: ModeKind) -> Self {
        // Prefill the config path persisted from the last session
        // (stored next to the executable as tclock-wizard.toml).
        let state = WizardState::load();
        let config_path = state.config_path;
        let loaded_config = config_path.as_deref().and_then(Config::load_from_quiet);
        Wizard {
            selected: mode_index(preselect),
            forms: (0..MODE_ENTRIES.len()).map(|_| None).collect(),
            active: None,
            confirming_discard: false,
            config_path,
            loaded_config,
            status: None,
        }
    }

    /// Jump back to the mode list with `kind` preselected (`c`/`w`/`t`).
    pub(crate) fn preselect(&mut self, kind: ModeKind) {
        self.selected = mode_index(kind);
        self.active = None;
        self.confirming_discard = false;
    }

    fn ensure_form(&mut self, idx: usize) {
        if self.forms[idx].is_none() {
            let mut form = Form::new(MODE_ENTRIES[idx].0);
            if let Some(path) = &self.config_path {
                form.set_text(FieldId::ConfigPath, &path.to_string_lossy());
            }
            self.forms[idx] = Some(form);
        }
    }

    fn load_config(&mut self, path: String) {
        let path = path.trim();
        if path.is_empty() {
            self.config_path = None;
            self.loaded_config = None;
            WizardState::default().save(); // clears the persisted state file
            self.status = Some("已清除配置文件".to_string());
            return;
        }
        let pb = PathBuf::from(path);
        match Config::load_from_quiet(&pb) {
            Some(config) => {
                self.status = Some(format!("已载入 {}", pb.display()));
                self.config_path = Some(pb.clone());
                self.loaded_config = Some(config);
                // Persist an absolute path so the prefill survives a
                // different working directory next launch.
                let persisted = std::path::absolute(&pb).unwrap_or(pb);
                WizardState {
                    config_path: Some(persisted),
                }
                .save();
            }
            None => {
                self.status = Some(format!("配置载入失败（不存在或无法解析）: {}", pb.display()));
            }
        }
    }

    fn build_mode_config(&self, form: &Form) -> Result<ModeConfig, String> {
        let errors = form.validate(self.loaded_config.as_ref());
        if !errors.is_empty() {
            return Err(format!("{} 处参数无效: {}", errors.len(), errors[0].1));
        }

        let mut params = AppParams {
            config: self.config_path.clone(),
            size: Some(form.int_value(FieldId::Size)),
            ..Default::default()
        };
        let color = form.text(FieldId::Color);
        if !color.is_empty() {
            params.color = Some(parse_color(&color).map_err(|e| e)?);
        }

        match form.kind {
            ModeKind::Clock => {
                let tz = form.text(FieldId::Timezone);
                params.timezone = Some(if tz.is_empty() {
                    None
                } else {
                    Some(parse_timezone(&tz)?)
                });
                params.show_date = Some(form.toggle_value(FieldId::ShowDate));
                params.show_seconds = Some(form.toggle_value(FieldId::ShowSeconds));
                params.show_millis = Some(form.toggle_value(FieldId::ShowMillis));
            }
            ModeKind::Timer => {
                let durations: Result<Vec<_>, _> = form
                    .list_items(FieldId::Durations)
                    .iter()
                    .map(|d| parse_duration(d))
                    .collect();
                params.durations = Some(durations?);
                params.titles = Some(form.list_items(FieldId::Titles));
                params.repeat = Some(form.toggle_value(FieldId::Repeat));
                params.start_paused = Some(form.toggle_value(FieldId::Paused));
                params.show_millis = Some(form.toggle_value(FieldId::ShowMillis));
                params.auto_quit = Some(form.toggle_value(FieldId::AutoQuit));
                params.execute = Some(form.list_items(FieldId::Execute));
            }
            ModeKind::Stopwatch => {}
            ModeKind::Countdown => {
                let time = form.text(FieldId::CdTime);
                params.countdown_time = Some(parse_datetime(&time)?);
                let title = form.text(FieldId::CdTitle);
                params.countdown_title = Some(if title.is_empty() { None } else { Some(title) });
                params.continue_on_zero = Some(form.toggle_value(FieldId::CdContinue));
                params.reverse = Some(form.toggle_value(FieldId::CdReverse));
                params.show_millis = Some(form.toggle_value(FieldId::CdMillis));
                params.use_config_items = Some(false);
            }
            ModeKind::CountdownGrid => {
                params.use_config_items = Some(true);
            }
        }
        Ok(ModeConfig {
            kind: form.kind,
            params,
        })
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> WizardOutcome {
        if self.confirming_discard {
            self.confirming_discard = false;
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                if let Some(idx) = self.active.take() {
                    if let Some(form) = &mut self.forms[idx] {
                        form.dirty = false;
                    }
                }
            }
            return WizardOutcome::Continue;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.active {
            Some(idx) => {
                let action = self.forms[idx].as_mut().unwrap().on_key(key);
                match action {
                    FormAction::Continue => WizardOutcome::Continue,
                    FormAction::Back => {
                        if self.forms[idx].as_ref().unwrap().dirty {
                            self.confirming_discard = true;
                        } else {
                            self.active = None;
                        }
                        WizardOutcome::Continue
                    }
                    FormAction::Submit => {
                        let result = {
                            let form = self.forms[idx].as_ref().unwrap();
                            self.build_mode_config(form)
                        };
                        match result {
                            Ok(mode_config) => {
                                self.forms[idx].as_mut().unwrap().dirty = false;
                                self.status = None;
                                WizardOutcome::Run(mode_config)
                            }
                            Err(msg) => {
                                self.status = Some(msg);
                                WizardOutcome::Continue
                            }
                        }
                    }
                    FormAction::LoadConfig(path) => {
                        self.load_config(path);
                        WizardOutcome::Continue
                    }
                    FormAction::Quit => WizardOutcome::Quit,
                }
            }
            None => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => WizardOutcome::Quit,
                KeyCode::Char('c') if ctrl => WizardOutcome::Quit,
                KeyCode::Up => {
                    self.selected =
                        (self.selected + MODE_ENTRIES.len() - 1) % MODE_ENTRIES.len();
                    WizardOutcome::Continue
                }
                KeyCode::Down => {
                    self.selected = (self.selected + 1) % MODE_ENTRIES.len();
                    WizardOutcome::Continue
                }
                KeyCode::Enter => {
                    self.ensure_form(self.selected);
                    self.active = Some(self.selected);
                    self.status = None;
                    WizardOutcome::Continue
                }
                KeyCode::Char('r') | KeyCode::Char('R') if ctrl => {
                    self.ensure_form(self.selected);
                    let result = {
                        let form = self.forms[self.selected].as_ref().unwrap();
                        self.build_mode_config(form)
                    };
                    match result {
                        Ok(mode_config) => WizardOutcome::Run(mode_config),
                        Err(msg) => {
                            // Open the form so the user sees the failing field.
                            self.status = Some(msg);
                            self.active = Some(self.selected);
                            WizardOutcome::Continue
                        }
                    }
                }
                _ => WizardOutcome::Continue,
            },
        }
    }

    pub(crate) fn ui(&self, f: &mut Frame) {
        let area = f.size();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            let msg = Paragraph::new(vec![
                Line::from(format!(
                    "终端窗口过小（至少 {}x{}，当前 {}x{}）",
                    MIN_WIDTH, MIN_HEIGHT, area.width, area.height
                )),
                Line::from("请调整窗口大小"),
            ])
            .alignment(Alignment::Center);
            let y = area.height / 2;
            let h = 2.min(area.height);
            f.render_widget(msg, Rect::new(area.x, y.saturating_sub(1), area.width, h));
            return;
        }
        match self.active {
            Some(idx) => {
                let form = self.forms[idx].as_ref().unwrap();
                self.ui_form(f, form, idx, area);
            }
            None => self.ui_mode_list(f, area),
        }
        if self.confirming_discard {
            ui_confirm_discard(f, area);
        }
    }

    fn ui_mode_list(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" tclock · 选择模式 ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(inner);

        let items: Vec<ListItem> = MODE_ENTRIES
            .iter()
            .map(|(_, name, desc)| ListItem::new(format!("{:<10} {}", name, desc)))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = ListState::default();
        state.select(Some(self.selected));
        f.render_stateful_widget(list, chunks[0], &mut state);

        let mut footer = vec![Line::from(Span::styled(
            "上下键 选择  Enter 参数设置  Ctrl+R 直接运行  q/Esc 退出",
            Style::default().fg(Color::DarkGray),
        ))];
        if let Some(status) = &self.status {
            footer.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(footer), chunks[1]);
    }

    fn ui_form(&self, f: &mut Frame, form: &Form, idx: usize, area: Rect) {
        let title = format!(" tclock · {} 参数 ", MODE_ENTRIES[idx].1);
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let (form_area, preview_area) = if inner.width >= PREVIEW_MIN_WIDTH {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(inner);
            (chunks[0], Some(chunks[1]))
        } else {
            (inner, None)
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(form_area);

        let errors = form.validate(self.loaded_config.as_ref());
        let (lines, field_starts, edit_cursor) =
            build_form_lines(form, &errors, self.loaded_config.as_ref());

        // Keep the focused field visible.
        let visible_height = chunks[0].height as usize;
        let focus_line = field_starts[form.focus].min(lines.len().saturating_sub(1));
        let scroll = if focus_line >= visible_height {
            (focus_line + 1 - visible_height) as u16
        } else {
            0
        };
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), chunks[0]);

        // Cursor for the field being edited (ASCII-column approximation).
        if form.editing {
            if let Some((x, y)) = edit_cursor {
                let y = y.saturating_sub(scroll);
                if y < chunks[0].height {
                    f.set_cursor(chunks[0].x + x, chunks[0].y + y);
                }
            }
        }

        let hints = if form.editing {
            "输入中…  ←/→ 移动光标  Enter 确认  Esc 结束编辑"
        } else if form.kind == ModeKind::Stopwatch {
            "无模式参数  Ctrl+R 运行  Esc 返回"
        } else {
            "Tab/上下键 移动  Enter 编辑  Space/左右键 切换  a/d 增删  Ctrl+R 运行  Esc 返回"
        };
        let mut footer = vec![Line::from(Span::styled(
            hints,
            Style::default().fg(Color::DarkGray),
        ))];
        if let Some(status) = &self.status {
            let style = if status.contains("无效") || status.contains("失败") {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Yellow)
            };
            footer.push(Line::from(Span::styled(status.clone(), style)));
        }
        f.render_widget(Paragraph::new(footer), chunks[1]);

        if let Some(preview_area) = preview_area {
            self.ui_preview(form, preview_area, f.buffer_mut());
        }
    }

    /// Live preview: renders the real widgets with the form's current values.
    fn ui_preview(&self, form: &Form, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(" 预览 ").borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        let color = {
            let c = form.text(FieldId::Color);
            if c.is_empty() {
                Color::Green
            } else {
                parse_color(&c).unwrap_or(Color::Green)
            }
        };
        let style = Style::default().fg(color);
        let size = form.int_value(FieldId::Size);

        let message = |buf: &mut Buffer, text: &str| {
            Paragraph::new(text.to_string())
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center)
                .render(inner, buf);
        };

        match form.kind {
            ModeKind::Clock => {
                let tz = {
                    let t = form.text(FieldId::Timezone);
                    if t.is_empty() {
                        None
                    } else {
                        parse_timezone(&t).ok()
                    }
                };
                let clock = Clock {
                    size,
                    style,
                    show_date: form.toggle_value(FieldId::ShowDate),
                    show_millis: form.toggle_value(FieldId::ShowMillis),
                    show_secs: form.toggle_value(FieldId::ShowSeconds),
                    timezone: tz,
                };
                (&clock).render(inner, buf);
            }
            ModeKind::Timer => {
                let durations: Vec<_> = form
                    .list_items(FieldId::Durations)
                    .iter()
                    .filter_map(|d| parse_duration(d).ok())
                    .collect();
                if durations.is_empty() {
                    message(buf, "（时长为空或无效，无法预览）");
                } else {
                    let format = if form.toggle_value(FieldId::ShowMillis) {
                        DurationFormat::HourMinSecDeci
                    } else {
                        DurationFormat::HourMinSec
                    };
                    let timer = Timer::new(
                        size,
                        style,
                        durations,
                        form.list_items(FieldId::Titles),
                        form.toggle_value(FieldId::Repeat),
                        format,
                        false, // preview always runs from the full duration
                        false,
                        vec![],
                    );
                    (&timer).render(inner, buf);
                }
            }
            ModeKind::Stopwatch => {
                let stopwatch = Stopwatch::new(size, style);
                (&stopwatch).render(inner, buf);
            }
            ModeKind::Countdown => {
                let time = form.text(FieldId::CdTime);
                match parse_datetime(&time) {
                    Ok(time) => {
                        let title = form.text(FieldId::CdTitle);
                        let countdown = Countdown {
                            size,
                            style,
                            time,
                            title: if title.is_empty() { None } else { Some(title) },
                            continue_on_zero: form.toggle_value(FieldId::CdContinue),
                            reverse: form.toggle_value(FieldId::CdReverse),
                            format: if form.toggle_value(FieldId::CdMillis) {
                                DurationFormat::HourMinSecDeci
                            } else {
                                DurationFormat::HourMinSec
                            },
                            freeze_on_zero: true, // no blinking in the preview
                        };
                        (&countdown).render(inner, buf);
                    }
                    Err(_) => message(buf, "（目标时间为空或无效，无法预览）"),
                }
            }
            ModeKind::CountdownGrid => match &self.loaded_config {
                Some(config) if !config.countdown.items.is_empty() => {
                    let slots = build_countdown_slots_silent(
                        &config.countdown.items,
                        &config.countdown,
                        style,
                        size,
                    );
                    render_countdown_grid_to(&slots, inner, buf);
                }
                _ => message(buf, "（尚未载入含 countdown 条目的配置文件）"),
            },
        }
    }
}

fn ui_confirm_discard(f: &mut Frame, area: Rect) {
    let width = 34.min(area.width);
    let height = 5.min(area.height);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    f.render_widget(Clear, rect);
    let block = Block::default().title(" 确认 ").borders(Borders::ALL);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let text = Paragraph::new(vec![
        Line::from("放弃未保存的修改？"),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Yellow)),
            Span::raw(" 放弃    "),
            Span::styled("其它键", Style::default().fg(Color::Yellow)),
            Span::raw(" 继续编辑"),
        ]),
    ])
    .alignment(Alignment::Center);
    f.render_widget(text, inner);
}

/// Byte index of the `idx`-th char boundary in `s` (`s.len()` when `idx`
/// reaches the end).
fn char_to_byte(s: &str, idx: usize) -> usize {
    s.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(s.len())
}

/// Display-cell width of the first `char_count` chars of `s`.
fn prefix_width(s: &str, char_count: usize) -> usize {
    UnicodeWidthStr::width(&s[..char_to_byte(s, char_count)])
}

/// Build the display lines for a form. Returns the lines, the start line of
/// each field (for scrolling), and the edit cursor position (column, line).
fn build_form_lines(
    form: &Form,
    errors: &[(FieldId, String)],
    loaded_config: Option<&Config>,
) -> (Vec<Line<'static>>, Vec<usize>, Option<(u16, u16)>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut starts = vec![0usize; form.fields.len()];
    let mut edit_cursor = None;

    let error_span = |id: FieldId| -> Option<Span<'static>> {
        errors
            .iter()
            .find(|(eid, _)| *eid == id)
            .map(|(_, msg)| Span::styled(format!("  ! {}", msg), Style::default().fg(Color::Red)))
    };

    for (i, field) in form.fields.iter().enumerate() {
        if !field.visible {
            starts[i] = usize::MAX;
            continue;
        }
        starts[i] = lines.len();
        let focused = i == form.focus;
        let marker = if focused { "> " } else { "  " };
        let label_style = if focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let label = format!("{}: ", field.label);
        // Display-cell width of the marker + label prefix (CJK chars count as
        // 2 cells), so the edit cursor lands on the right column.
        let prefix_cols = 2 + UnicodeWidthStr::width(field.label) + 2;

        match &field.kind {
            FieldKind::StrList { items, cursor } => {
                let mut spans = vec![
                    Span::raw(marker.to_string()),
                    Span::styled(label, label_style),
                ];
                if let Some(e) = error_span(field.id) {
                    spans.push(e);
                }
                lines.push(Line::from(spans));
                if items.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "    （空，按 a 添加）".to_string(),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                for (j, item) in items.iter().enumerate() {
                    let item_marker = if focused && j == *cursor { "  > " } else { "    " };
                    let editing_this = focused && form.editing && j == *cursor;
                    let item_style = if editing_this {
                        Style::default().add_modifier(Modifier::UNDERLINED)
                    } else {
                        Style::default()
                    };
                    if editing_this {
                        edit_cursor = Some((
                            (item_marker.len() + prefix_width(item, form.edit_cursor)) as u16,
                            lines.len() as u16,
                        ));
                    }
                    lines.push(Line::from(vec![
                        Span::raw(item_marker.to_string()),
                        Span::styled(item.clone(), item_style),
                    ]));
                }
            }
            _ => {
                let control = control_text(field);
                let control_style = if focused && form.editing {
                    Style::default().add_modifier(Modifier::UNDERLINED)
                } else {
                    control_style_for(field)
                };
                let mut spans = vec![
                    Span::raw(marker.to_string()),
                    Span::styled(label, label_style),
                    Span::styled(control, control_style),
                ];
                if let Some(e) = error_span(field.id) {
                    spans.push(e);
                }
                if focused && form.editing {
                    if let FieldKind::Text { value, .. } = &field.kind {
                        edit_cursor = Some((
                            (prefix_cols + prefix_width(value, form.edit_cursor)) as u16,
                            lines.len() as u16,
                        ));
                    }
                }
                lines.push(Line::from(spans));
            }
        }

        if focused && !field.hint.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    {}", field.hint),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Countdown grid: show what is loaded.
    if form.kind == ModeKind::CountdownGrid {
        let info = match loaded_config {
            Some(c) if !c.countdown.items.is_empty() => format!(
                "    已载入 {} 个 countdown 条目（见右侧预览）",
                c.countdown.items.len()
            ),
            Some(_) => "    配置文件不含 [[countdown.items]] 条目".to_string(),
            None => "    尚未载入配置文件（在 config 路径字段输入后按 Enter）".to_string(),
        };
        lines.push(Line::from(Span::styled(info, Style::default().fg(Color::DarkGray))));
    }

    (lines, starts, edit_cursor)
}

fn control_text(field: &Field) -> String {
    match &field.kind {
        FieldKind::Toggle(on) => {
            if *on {
                "[ 开 ]".to_string()
            } else {
                "[ 关 ]".to_string()
            }
        }
        FieldKind::Text { value, placeholder } => {
            if value.is_empty() {
                placeholder.to_string()
            } else {
                value.clone()
            }
        }
        FieldKind::Int { value, .. } => format!("< {} >", value),
        FieldKind::StrList { .. } => String::new(),
    }
}

fn control_style_for(field: &Field) -> Style {
    match &field.kind {
        FieldKind::Toggle(on) => {
            if *on {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        }
        FieldKind::Text { value, .. } => {
            if value.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            }
        }
        _ => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::{backend::TestBackend, Terminal};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut s = String::new();
        for cell in buf.content().iter() {
            s.push_str(cell.symbol());
        }
        // ratatui 0.25 blanks the continuation cell of wide (CJK) glyphs with
        // a space; strip spaces so substring assertions don't depend on cell
        // spacing.
        s.replace(' ', "")
    }

    fn set_text(form: &mut Form, id: FieldId, value: &str) {
        if let Some(Field {
            kind: FieldKind::Text { value: v, .. },
            ..
        }) = form.get_mut(id)
        {
            *v = value.to_string();
        }
    }

    #[test]
    fn mode_select_renders_all_modes() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let wizard = Wizard::new(ModeKind::Clock);
        terminal.draw(|f| wizard.ui(f)).unwrap();
        let text = buffer_text(&terminal);
        for name in ["Clock", "Timer", "Stopwatch", "Countdown", "CountdownGrid"] {
            assert!(text.contains(name), "mode list should contain {}", name);
        }
    }

    #[test]
    fn tiny_terminal_shows_notice_instead_of_panicking() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let wizard = Wizard::new(ModeKind::Clock);
        terminal.draw(|f| wizard.ui(f)).unwrap();
        assert!(buffer_text(&terminal).contains("终端窗口过小"));

        // Even the form view must degrade gracefully.
        let mut wizard = Wizard::new(ModeKind::Timer);
        wizard.ensure_form(1);
        wizard.active = Some(1);
        terminal.draw(|f| wizard.ui(f)).unwrap();
        assert!(buffer_text(&terminal).contains("终端窗口过小"));
    }

    #[test]
    fn state_machine_list_form_run_quit() {
        let mut wizard = Wizard::new(ModeKind::Clock);

        // Enter opens the form for the selected mode.
        assert!(matches!(
            wizard.on_key(key(KeyCode::Enter)),
            WizardOutcome::Continue
        ));
        assert!(wizard.active.is_some());

        // Clean form: Esc goes straight back to the list.
        assert!(matches!(
            wizard.on_key(key(KeyCode::Esc)),
            WizardOutcome::Continue
        ));
        assert!(wizard.active.is_none());

        // q quits the process from the list.
        assert!(matches!(
            wizard.on_key(key(KeyCode::Char('q'))),
            WizardOutcome::Quit
        ));

        // Ctrl+C quits too.
        let mut wizard = Wizard::new(ModeKind::Clock);
        assert!(matches!(
            wizard.on_key(ctrl(KeyCode::Char('c'))),
            WizardOutcome::Quit
        ));
    }

    #[test]
    fn dirty_form_esc_requires_confirmation() {
        let mut wizard = Wizard::new(ModeKind::Clock);
        wizard.on_key(key(KeyCode::Enter)); // open Clock form
        wizard.on_key(key(KeyCode::Down)); // -> 显示日期
        wizard.on_key(key(KeyCode::Char(' '))); // toggle → dirty

        wizard.on_key(key(KeyCode::Esc));
        assert!(wizard.confirming_discard, "dirty Esc should ask first");
        assert!(wizard.active.is_some());

        wizard.on_key(key(KeyCode::Char('n'))); // stay editing
        assert!(wizard.active.is_some());

        wizard.on_key(key(KeyCode::Esc));
        assert!(wizard.confirming_discard);
        wizard.on_key(key(KeyCode::Char('y'))); // discard
        assert!(wizard.active.is_none());
    }

    #[test]
    fn ctrl_r_submits_valid_form() {
        let mut wizard = Wizard::new(ModeKind::Timer);
        wizard.on_key(key(KeyCode::Enter));
        match wizard.on_key(ctrl(KeyCode::Char('r'))) {
            WizardOutcome::Run(mc) => {
                assert_eq!(mc.kind, ModeKind::Timer);
                assert_eq!(
                    mc.params.durations.as_ref().unwrap().len(),
                    2,
                    "default 25m/5m should be submitted"
                );
            }
            _ => panic!("valid timer form must submit"),
        }
    }

    #[test]
    fn ctrl_r_with_errors_stays_and_reports() {
        let mut wizard = Wizard::new(ModeKind::Countdown);
        wizard.on_key(key(KeyCode::Enter)); // countdown form, empty time
        match wizard.on_key(ctrl(KeyCode::Char('r'))) {
            WizardOutcome::Continue => {}
            _ => panic!("invalid form must not run"),
        }
        assert!(wizard.status.as_deref().unwrap().contains("无效"));
    }

    #[test]
    fn validation_rules() {
        // Timer: default durations are valid.
        let form = Form::new(ModeKind::Timer);
        assert!(form.validate(None).is_empty());

        // Timer: empty durations rejected (would panic Timer::remaining_time).
        let mut form = Form::new(ModeKind::Timer);
        if let Some(Field {
            kind: FieldKind::StrList { items, .. },
            ..
        }) = form.get_mut(FieldId::Durations)
        {
            items.clear();
        }
        let errors = form.validate(None);
        assert!(errors.iter().any(|(id, _)| *id == FieldId::Durations));

        // Timer: unparseable duration rejected.
        let mut form = Form::new(ModeKind::Timer);
        if let Some(Field {
            kind: FieldKind::StrList { items, .. },
            ..
        }) = form.get_mut(FieldId::Durations)
        {
            items.clear();
            items.push("abc".to_string());
        }
        let errors = form.validate(None);
        assert!(errors.iter().any(|(id, _)| *id == FieldId::Durations));

        // Countdown: empty / bad time rejected, good time accepted.
        let mut form = Form::new(ModeKind::Countdown);
        assert!(form
            .validate(None)
            .iter()
            .any(|(id, _)| *id == FieldId::CdTime));
        set_text(&mut form, FieldId::CdTime, "not-a-time");
        assert!(form
            .validate(None)
            .iter()
            .any(|(id, _)| *id == FieldId::CdTime));
        set_text(&mut form, FieldId::CdTime, "20:00");
        assert!(form.validate(None).is_empty());

        // Countdown grid: requires a loaded config with items.
        let form = Form::new(ModeKind::CountdownGrid);
        let errors = form.validate(None);
        assert!(errors.iter().any(|(id, _)| *id == FieldId::ConfigPath));

        // Clock: bad timezone / color rejected.
        let mut form = Form::new(ModeKind::Clock);
        set_text(&mut form, FieldId::Timezone, "mars/olympus");
        set_text(&mut form, FieldId::Color, "notacolor");
        let errors = form.validate(None);
        assert!(errors.iter().any(|(id, _)| *id == FieldId::Timezone));
        assert!(errors.iter().any(|(id, _)| *id == FieldId::Color));
        set_text(&mut form, FieldId::Timezone, "Asia/Shanghai");
        set_text(&mut form, FieldId::Color, "#e63946");
        assert!(form.validate(None).is_empty());
    }

    #[test]
    fn form_renders_with_live_preview() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut wizard = Wizard::new(ModeKind::Clock);
        wizard.ensure_form(0);
        wizard.active = Some(0);
        terminal.draw(|f| wizard.ui(f)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("预览"), "preview pane should be visible at 120 cols");
        assert!(text.contains("时区"));
        assert!(text.contains('█'), "clock digits should render in the preview");
        assert!(text.contains("Ctrl+R"));
    }

    #[test]
    fn narrow_terminal_hides_preview() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut wizard = Wizard::new(ModeKind::Clock);
        wizard.ensure_form(0);
        wizard.active = Some(0);
        terminal.draw(|f| wizard.ui(f)).unwrap();
        let text = buffer_text(&terminal);
        assert!(!text.contains("预览"));
        assert!(text.contains("时区"));
    }

    #[test]
    fn countdown_grid_shows_items_info() {
        let config_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../countdown.toml.example");
        let mut wizard = Wizard::new(ModeKind::CountdownGrid);
        wizard.load_config(config_path.to_string_lossy().to_string());
        assert!(wizard.loaded_config.is_some());

        wizard.ensure_form(4);
        wizard.active = Some(4);

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| wizard.ui(f)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("已载入6个countdown条目"));
        assert!(text.contains('█'), "grid preview should render");

        // load_config persists the path next to the (test) executable;
        // clean up so other tests start from a blank state.
        WizardState::default().save();
    }

    #[test]
    fn form_prefills_wizard_config_path() {
        let mut wizard = Wizard::new(ModeKind::Clock);
        wizard.config_path = Some(PathBuf::from(r"C:\x\y.toml"));
        wizard.ensure_form(0);
        let form = wizard.forms[0].as_ref().unwrap();
        assert_eq!(form.text(FieldId::ConfigPath), r"C:\x\y.toml");
        assert!(!form.dirty, "prefill must not mark the form dirty");
    }

    #[test]
    fn edit_cursor_moves_and_edits_mid_string() {
        let mut form = Form::new(ModeKind::Clock);
        form.on_key(key(KeyCode::Enter)); // begin editing 时区
        assert!(form.editing);
        for c in "bcd".chars() {
            form.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.text(FieldId::Timezone), "bcd");

        // Move left twice and insert in the middle.
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Char('a')));
        assert_eq!(form.text(FieldId::Timezone), "bacd");

        // Home + Delete removes the first char.
        form.on_key(key(KeyCode::Home));
        form.on_key(key(KeyCode::Delete));
        assert_eq!(form.text(FieldId::Timezone), "acd");

        // End + Backspace removes the last char.
        form.on_key(key(KeyCode::End));
        form.on_key(key(KeyCode::Backspace));
        assert_eq!(form.text(FieldId::Timezone), "ac");

        // Cursor never leaves the string bounds.
        form.on_key(key(KeyCode::Home));
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Backspace));
        assert_eq!(form.text(FieldId::Timezone), "ac");
        form.on_key(key(KeyCode::End));
        form.on_key(key(KeyCode::Right));
        form.on_key(key(KeyCode::Delete));
        assert_eq!(form.text(FieldId::Timezone), "ac");
    }

    #[test]
    fn edit_cursor_handles_cjk_char_boundaries() {
        let mut form = Form::new(ModeKind::Clock);
        set_text(&mut form, FieldId::Timezone, "路径");
        form.on_key(key(KeyCode::Enter)); // begin editing, cursor at end
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Char('x')));
        assert_eq!(form.text(FieldId::Timezone), "路x径");
        form.on_key(key(KeyCode::Backspace));
        assert_eq!(form.text(FieldId::Timezone), "路径");
    }

    #[test]
    fn edit_cursor_column_accounts_for_cjk_label() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut wizard = Wizard::new(ModeKind::Clock);
        wizard.config_path = None; // ignore any persisted prefill
        wizard.ensure_form(0);
        wizard.active = Some(0);
        {
            let form = wizard.forms[0].as_mut().unwrap();
            for _ in 0..6 {
                form.on_key(key(KeyCode::Tab)); // -> config 路径
            }
            form.on_key(key(KeyCode::Enter));
            form.on_key(key(KeyCode::Char('a')));
            form.on_key(key(KeyCode::Char('b')));
        }
        terminal.draw(|f| wizard.ui(f)).unwrap();
        // chunks[0].x = 1 (border); column offset = "> " (2) + "config 路径"
        // (11 display cells) + ": " (2) + "ab" (2) = 17 → x = 18. The field
        // is the 7th visible line (index 6) below the border → y = 7.
        assert_eq!(terminal.get_cursor().unwrap(), (18, 7));
    }
}
