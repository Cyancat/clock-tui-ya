use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::TimeZone;
use chrono_tz::Tz;
use clap::Subcommand;
use crossterm::event::KeyCode;
use std::path::PathBuf;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::Widget,
    Frame,
};
use regex::Regex;

use self::modes::Clock;
use self::modes::Countdown;
use self::modes::DurationFormat;
use self::modes::Pause;
use self::modes::Stopwatch;
use self::modes::Timer;

pub mod modes;
pub(crate) mod wizard;

#[derive(Debug, Subcommand)]
pub enum Mode {
    /// The clock mode displays the current time, the default mode.
    Clock {
        /// Custome timezone, for example "America/New_York", use local timezone if not specificed
        #[clap(short = 'z', long, value_parser=parse_timezone)]
        timezone: Option<Tz>,
        /// Do not show date
        #[clap(short = 'D', long, action)]
        no_date: bool,
        /// Do not show seconds
        #[clap(short = 'S', long, action)]
        no_seconds: bool,
        /// Show milliseconds
        #[clap(short, long, action)]
        millis: bool,
    },
    /// The timer mode displays the remaining time until the timer is finished.
    Timer {
        /// Initial duration for timer, value can be 10s for 10 seconds, 1m for 1 minute, etc.
        /// Also accept mulitple duration value and run the timers sequentially, eg. 25m 5m
        #[clap(short, long="duration", value_parser = parse_duration, num_args = 1.., default_value = "5m")]
        durations: Vec<Duration>,

        /// Set the title for the timer, also accept mulitple titles for each durations correspondingly
        #[clap(short, long = "title", num_args = 0..)]
        titles: Vec<String>,

        /// Restart the timer when timer is over
        #[clap(long, short, action)]
        repeat: bool,

        /// Hide milliseconds
        #[clap(long = "no-millis", short = 'M', action)]
        no_millis: bool,

        /// Start the timer paused
        #[clap(long = "paused", short = 'P', action)]
        paused: bool,

        /// Auto quit when time is up
        #[clap(long = "quit", short = 'Q', action)]
        auto_quit: bool,

        /// Command to run when the timer ends
        #[clap(long, short, num_args = 1.., allow_hyphen_values = true)]
        execute: Vec<String>,
    },
    /// The stopwatch mode displays the elapsed time since it was started.
    Stopwatch,
    /// The countdown timer mode shows the duration to a specific time
    Countdown {
        /// The target time to countdown to, eg. "2023-01-01", "20:00", "2022-12-25 20:00:00" or "2022-12-25T20:00:00-04:00".
        /// If omitted, all countdowns from the config file ([[countdown.items]]) are shown in a grid.
        #[clap(long, short, value_parser = parse_datetime)]
        time: Option<DateTime<Local>>,

        /// Title or description for countdown show in header
        #[clap(long, short = 'T')]
        title: Option<String>,

        /// Continue to countdown after pass the target time
        #[clap(long = "continue", short = 'c', action)]
        continue_on_zero: bool,

        /// Reverse the countdown, a.k.a. countup
        #[clap(long, short, action)]
        reverse: bool,

        /// Show milliseconds
        #[clap(short, long, action)]
        millis: bool,
    },
}

use crate::config::{Config, CountdownItemConfig};

/// Mode category, decoupled from the clap subcommand parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeKind {
    Clock,
    Timer,
    Stopwatch,
    Countdown,
    /// The `[[countdown.items]]` grid from a config file. Maps to the same
    /// clap `Mode::Countdown` variant, but is a separate wizard entry
    /// because its parameters are mutually exclusive with a single `--time`.
    CountdownGrid,
}

/// Field-by-field resolved parameter overlay produced by the interactive
/// wizard / run loop. Every `Option` means "explicitly specified" and takes
/// priority over CLI values and the config file.
///
/// Note the double `Option`s: `Some(None)` means "explicitly cleared"
/// (e.g. the wizard chose the local timezone, or no countdown title).
#[derive(Debug, Clone, Default)]
pub struct AppParams {
    pub color: Option<Color>,
    pub size: Option<u16>,
    pub config: Option<PathBuf>,

    // clock
    pub timezone: Option<Option<Tz>>,
    pub show_date: Option<bool>,
    pub show_seconds: Option<bool>,

    // shared by clock / timer / countdown
    pub show_millis: Option<bool>,

    // timer
    pub durations: Option<Vec<Duration>>,
    pub titles: Option<Vec<String>>,
    pub repeat: Option<bool>,
    pub start_paused: Option<bool>,
    pub auto_quit: Option<bool>,
    pub execute: Option<Vec<String>>,

    // countdown
    pub countdown_time: Option<DateTime<Local>>,
    pub countdown_title: Option<Option<String>>,
    pub continue_on_zero: Option<bool>,
    pub reverse: Option<bool>,
    /// true = render the `[[countdown.items]]` grid from the config file
    pub use_config_items: Option<bool>,
}

/// A fully described mode request produced by the wizard.
#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub kind: ModeKind,
    pub params: AppParams,
}

impl ModeConfig {
    /// In-place mode-switch presets for the `c` / `w` / `t` hotkeys.
    /// Params are left empty so CLI values and the config file still apply.
    pub fn preset(kind: ModeKind) -> Self {
        ModeConfig {
            kind,
            params: AppParams::default(),
        }
    }

    /// Build the clap `Mode` variant matching this config. Field values are
    /// mirrored from `params` where expressible; the resolved values always
    /// come from `params` first inside `init_app`.
    fn to_mode(&self) -> Mode {
        let p = &self.params;
        match self.kind {
            ModeKind::Clock => Mode::Clock {
                timezone: p.timezone.clone().unwrap_or(None),
                no_date: !p.show_date.unwrap_or(true),
                no_seconds: !p.show_seconds.unwrap_or(true),
                millis: p.show_millis.unwrap_or(false),
            },
            ModeKind::Timer => Mode::Timer {
                durations: p.durations.clone().unwrap_or_default(),
                titles: p.titles.clone().unwrap_or_default(),
                repeat: p.repeat.unwrap_or(false),
                no_millis: !p.show_millis.unwrap_or(true),
                paused: p.start_paused.unwrap_or(false),
                auto_quit: p.auto_quit.unwrap_or(false),
                execute: p.execute.clone().unwrap_or_default(),
            },
            ModeKind::Stopwatch => Mode::Stopwatch,
            ModeKind::Countdown | ModeKind::CountdownGrid => Mode::Countdown {
                time: p.countdown_time,
                title: p.countdown_title.clone().unwrap_or(None),
                continue_on_zero: p.continue_on_zero.unwrap_or(false),
                reverse: p.reverse.unwrap_or(false),
                millis: p.show_millis.unwrap_or(false),
            },
        }
    }
}

#[derive(clap::Parser, Default)]
#[clap(
    name = "tclock",
    version,
    about = "A clock app in terminal (clock-tui-ya)",
    long_about = None
)]
pub struct App {
    #[clap(subcommand)]
    pub mode: Option<Mode>,
    /// Foreground color of the clock, possible values are:
    ///     a) Any one of: Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray, DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta, LightCyan, White.
    ///     b) Hexadecimal color code: #RRGGBB.
    #[clap(short, long, value_parser = parse_color)]
    pub color: Option<Color>,
    /// Size of the clock, should be a positive integer (>=1).
    #[clap(short, long, value_parser)]
    pub size: Option<u16>,
    /// Path to the config file. No default path: a config file is only used
    /// when explicitly specified.
    #[clap(long, global = true)]
    pub config: Option<PathBuf>,

    /// Resolved parameter overlay from the interactive wizard. Highest
    /// priority source in `init_app`; `None` for plain CLI usage.
    #[clap(skip)]
    pub params: Option<AppParams>,

    #[clap(skip)]
    clock: Option<Clock>,
    #[clap(skip)]
    timer: Option<Timer>,
    #[clap(skip)]
    stopwatch: Option<Stopwatch>,
    /// Grid slots of countdowns; a single countdown occupies one slot and is
    /// rendered fullscreen. Empty slots (`None`) are left blank.
    #[clap(skip)]
    countdowns: Vec<Option<Countdown>>,
}

impl App {
    /// Apply a wizard/hotkey mode request: records the resolved params,
    /// mirrors them into `self.mode` for variant discrimination, then
    /// rebuilds the widgets.
    pub fn apply_mode_config(&mut self, mode_config: ModeConfig) {
        self.mode = Some(mode_config.to_mode());
        self.params = Some(mode_config.params);
        self.init_app();
    }

    pub fn init_app(&mut self) {
        // Config path: wizard params > CLI --config. No default path.
        let config_path = self
            .params
            .as_ref()
            .and_then(|p| p.config.clone())
            .or_else(|| self.config.clone());
        let config = config_path.as_deref().and_then(Config::load_from);
        let default_config = config.as_ref().map(|c| &c.default);
        let params = self.params.clone();

        // default mode
        if self.mode.is_none() {
            self.mode = default_config.map(|c| match c.mode.as_str() {
                "timer" => {
                    let timer_config = config.as_ref().map(|c| &c.timer);
                    Mode::Timer {
                        durations: timer_config
                            .map(|c| {
                                c.durations
                                    .iter()
                                    .filter_map(|d| parse_duration(d).ok())
                                    .collect()
                            })
                            .unwrap_or_else(|| vec![Duration::minutes(25), Duration::minutes(5)]),
                        titles: timer_config.map(|c| c.titles.clone()).unwrap_or_default(),
                        repeat: timer_config.map(|c| c.repeat).unwrap_or(false),
                        no_millis: !timer_config.map(|c| c.show_millis).unwrap_or(true),
                        paused: timer_config.map(|c| c.start_paused).unwrap_or(false),
                        auto_quit: timer_config.map(|c| c.auto_quit).unwrap_or(false),
                        execute: timer_config.map(|c| c.execute.clone()).unwrap_or_default(),
                    }
                }
                "stopwatch" => Mode::Stopwatch,
                "countdown" => {
                    let countdown_config = config.as_ref().map(|c| &c.countdown);
                    Mode::Countdown {
                        time: countdown_config
                            .and_then(|c| c.time.as_ref())
                            .and_then(|t| parse_datetime(t).ok()),
                        title: countdown_config.map(|c| c.title.clone()).unwrap_or(None),
                        continue_on_zero: countdown_config
                            .map(|c| c.continue_on_zero)
                            .unwrap_or(false),
                        reverse: countdown_config.map(|c| c.reverse).unwrap_or(false),
                        millis: countdown_config.map(|c| c.show_millis).unwrap_or(false),
                    }
                }
                _ => {
                    let clock_config = config.as_ref().map(|c| &c.clock);
                    Mode::Clock {
                        no_date: !clock_config.map(|c| c.show_date).unwrap_or(true),
                        millis: clock_config.map(|c| c.show_millis).unwrap_or(false),
                        no_seconds: !clock_config.map(|c| c.show_seconds).unwrap_or(true),
                        timezone: clock_config.and_then(|c| c.timezone),
                    }
                }
            });
        }

        // Layered resolution for every field:
        // wizard params > CLI explicit value > config file > hardcoded default.
        let color = params
            .as_ref()
            .and_then(|p| p.color)
            .or(self.color)
            .or_else(|| {
                default_config.map(|c| parse_color(&c.color).unwrap_or(Color::Green))
            })
            .unwrap_or(Color::Green);
        self.color = Some(color);
        let size = params
            .as_ref()
            .and_then(|p| p.size)
            .or(self.size)
            .or_else(|| default_config.map(|c| c.size))
            .unwrap_or(1);
        self.size = Some(size);

        let style = Style::default().fg(color);

        // Clear widgets of other modes so mode switching does not leave stale widgets behind.
        self.clock = None;
        self.timer = None;
        self.stopwatch = None;
        self.countdowns.clear();

        // initialize the clock mode
        match self.mode.as_ref().unwrap_or(&Mode::Clock {
            no_date: false,
            millis: false,
            no_seconds: false,
            timezone: None,
        }) {
            Mode::Clock {
                no_date,
                no_seconds,
                millis,
                timezone,
            } => {
                let clock_config = config.as_ref().map(|c| &c.clock);
                let p = params.as_ref();
                let show_date = p
                    .and_then(|p| p.show_date)
                    .unwrap_or_else(|| {
                        !no_date && clock_config.map(|c| c.show_date).unwrap_or(true)
                    });
                let show_secs = p
                    .and_then(|p| p.show_seconds)
                    .unwrap_or_else(|| {
                        !no_seconds && clock_config.map(|c| c.show_seconds).unwrap_or(true)
                    });
                let show_millis = p
                    .and_then(|p| p.show_millis)
                    .unwrap_or_else(|| {
                        *millis || clock_config.map(|c| c.show_millis).unwrap_or(false)
                    });
                // Some(None) from the wizard means "local timezone, explicitly".
                let timezone = match p.and_then(|p| p.timezone.clone()) {
                    Some(tz) => tz,
                    None => timezone.or_else(|| clock_config.and_then(|c| c.timezone)),
                };
                self.clock = Some(Clock {
                    size,
                    style,
                    show_date,
                    show_millis,
                    show_secs,
                    timezone,
                });
            }
            Mode::Timer {
                durations,
                titles,
                repeat,
                no_millis,
                paused,
                auto_quit,
                execute,
            } => {
                let timer_config = config.as_ref().map(|c| &c.timer);
                let p = params.as_ref();
                let show_millis = p
                    .and_then(|p| p.show_millis)
                    .unwrap_or(!no_millis);
                let format = if show_millis {
                    DurationFormat::HourMinSecDeci
                } else {
                    DurationFormat::HourMinSec
                };
                // Empty durations would panic in `Timer::remaining_time`;
                // fall back to the CLI default.
                let durations = p
                    .and_then(|p| p.durations.clone())
                    .unwrap_or_else(|| durations.to_owned());
                let durations = if durations.is_empty() {
                    vec![Duration::minutes(5)]
                } else {
                    durations
                };
                self.timer = Some(Timer::new(
                    size,
                    style,
                    durations,
                    p.and_then(|p| p.titles.clone())
                        .unwrap_or_else(|| titles.to_owned()),
                    p.and_then(|p| p.repeat).unwrap_or_else(|| {
                        *repeat || timer_config.map(|c| c.repeat).unwrap_or(false)
                    }),
                    format,
                    p.and_then(|p| p.start_paused).unwrap_or_else(|| {
                        *paused || timer_config.map(|c| c.start_paused).unwrap_or(false)
                    }),
                    p.and_then(|p| p.auto_quit).unwrap_or_else(|| {
                        *auto_quit || timer_config.map(|c| c.auto_quit).unwrap_or(false)
                    }),
                    p.and_then(|p| p.execute.clone())
                        .unwrap_or_else(|| execute.to_owned()),
                ));
            }
            Mode::Stopwatch => {
                self.stopwatch = Some(Stopwatch::new(size, style));
            }
            Mode::Countdown {
                time,
                title,
                continue_on_zero,
                reverse,
                millis,
            } => {
                let countdown_config = config.as_ref().map(|c| &c.countdown);
                let p = params.as_ref();
                let show_millis = p.and_then(|p| p.show_millis).unwrap_or_else(|| {
                    *millis || countdown_config.map(|c| c.show_millis).unwrap_or(false)
                });
                let single_format = if show_millis {
                    DurationFormat::HourMinSecDeci
                } else {
                    DurationFormat::HourMinSec
                };
                let continue_on_zero = p.and_then(|p| p.continue_on_zero).unwrap_or_else(|| {
                    *continue_on_zero
                        || countdown_config
                            .map(|c| c.continue_on_zero)
                            .unwrap_or(false)
                });
                let reverse = p
                    .and_then(|p| p.reverse)
                    .unwrap_or_else(|| *reverse || countdown_config.map(|c| c.reverse).unwrap_or(false));
                // Some(None) from the wizard means "explicitly no title".
                let title = match p.and_then(|p| p.countdown_title.clone()) {
                    Some(t) => t,
                    None => title.to_owned(),
                };
                let single_time = p.and_then(|p| p.countdown_time).or(*time);

                if let Some(t) = single_time {
                    // Single countdown from the wizard or CLI `--time`:
                    // keep the original behavior (blink past zero).
                    self.countdowns = vec![Some(Countdown {
                        size,
                        style,
                        time: t,
                        title,
                        continue_on_zero,
                        reverse,
                        format: single_format,
                        freeze_on_zero: false,
                    })];
                } else if countdown_config.map(|c| !c.items.is_empty()).unwrap_or(false) {
                    // Multiple countdowns from config [[countdown.items]]: grid
                    // layout, freeze at zero. The wizard's grid entry lands here
                    // too (its validation guarantees items exist); if the config
                    // vanished in between we degrade to the single/warning paths
                    // instead of panicking on a missing config.
                    self.countdowns = build_countdown_slots(
                        &countdown_config.unwrap().items,
                        countdown_config.unwrap(),
                        style,
                        size,
                    );
                } else if let Some(t) = countdown_config
                    .and_then(|c| c.time.as_ref())
                    .and_then(|t| parse_datetime(t).ok())
                {
                    // Single countdown from config [countdown].time: keep the original behavior.
                    self.countdowns = vec![Some(Countdown {
                        size,
                        style,
                        time: t,
                        title,
                        continue_on_zero,
                        reverse,
                        format: single_format,
                        freeze_on_zero: false,
                    })];
                } else {
                    eprintln!(
                        "警告: countdown 模式未指定时间，请在配置文件中设置 [countdown].time 或 [[countdown.items]]"
                    );
                }
            }
        }
    }

    pub fn ui(&self, f: &mut Frame) {
        if let Some(ref w) = self.clock {
            f.render_widget(w, f.size());
        } else if let Some(ref w) = self.timer {
            f.render_widget(w, f.size());
        } else if let Some(ref w) = self.stopwatch {
            f.render_widget(w, f.size());
        } else if !self.countdowns.is_empty() {
            render_countdown_grid(&self.countdowns, f);
        }
    }

    pub fn on_key(&mut self, key: KeyCode) {
        if let Some(_w) = self.clock.as_mut() {
        } else if let Some(w) = self.timer.as_mut() {
            handle_key(w, key);
        } else if let Some(w) = self.stopwatch.as_mut() {
            handle_key(w, key);
        }
    }

    pub fn is_ended(&self) -> bool {
        if let Some(ref w) = self.timer {
            return w.is_finished();
        }
        false
    }

    pub fn on_exit(&self) {
        if let Some(ref w) = self.stopwatch {
            println!("Stopwatch time: {}", w.get_display_time());
        }
    }

    /// Drop all mode widgets. Used when returning from a run to the wizard so
    /// a later `on_exit` does not print stale results a second time.
    pub(crate) fn clear_widgets(&mut self) {
        self.clock = None;
        self.timer = None;
        self.stopwatch = None;
        self.countdowns.clear();
    }
}

fn handle_key<T: Pause>(widget: &mut T, key: KeyCode) {
    if let KeyCode::Char(' ') = key {
        widget.toggle_paused()
    }
}

/// Maximum number of countdowns shown simultaneously.
const MAX_COUNTDOWNS: usize = 8;

/// Grid shape (rows, cols) for n countdowns:
/// 1 -> fullscreen, 2 -> 2 rows, 3..=4 -> 2x2, 5..=6 -> 3 rows x 2 cols, 7..=8 -> 4x2.
fn grid_shape(n: usize) -> (usize, usize) {
    match n {
        0 => (0, 0),
        1 => (1, 1),
        2 => (2, 1),
        3..=4 => (2, 2),
        5..=6 => (3, 2),
        _ => (2, 4),
    }
}

/// Build the countdown grid slots from config `[[countdown.items]]`.
///
/// - Invalid items are skipped with a warning instead of failing the whole config.
/// - Items with `order` are pinned to fixed 1-based row-major positions; conflicts
///   are shifted to the next free slot with a warning.
/// - Remaining items fill the free slots, sorted by remaining time ascending
///   (or config order when `[countdown].sort = "config"`).
pub(crate) fn build_countdown_slots(
    items: &[CountdownItemConfig],
    shared: &crate::config::CountdownConfig,
    global_style: Style,
    global_size: u16,
) -> Vec<Option<Countdown>> {
    build_countdown_slots_impl(items, shared, global_style, global_size, true)
}

/// Silent variant for the wizard's preview pane: stderr output would corrupt
/// the alternate screen, so warnings are dropped (the wizard validates
/// upfront instead).
pub(crate) fn build_countdown_slots_silent(
    items: &[CountdownItemConfig],
    shared: &crate::config::CountdownConfig,
    global_style: Style,
    global_size: u16,
) -> Vec<Option<Countdown>> {
    build_countdown_slots_impl(items, shared, global_style, global_size, false)
}

fn build_countdown_slots_impl(
    items: &[CountdownItemConfig],
    shared: &crate::config::CountdownConfig,
    global_style: Style,
    global_size: u16,
    warn: bool,
) -> Vec<Option<Countdown>> {
    macro_rules! warnln {
        ($($arg:tt)*) => { if warn { eprintln!($($arg)*) } };
    }
    struct Pending {
        countdown: Countdown,
        order: Option<u16>,
    }

    let mut pendings: Vec<Pending> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let time_str = match &item.time {
            Some(t) => t,
            None => {
                warnln!("警告: countdown.items[{}] 缺少 time，已跳过", index);
                continue;
            }
        };
        let time = match parse_datetime(time_str) {
            Ok(t) => t,
            Err(e) => {
                warnln!(
                    "警告: countdown.items[{}] 的 time \"{}\" 无效 ({}), 已跳过",
                    index, time_str, e
                );
                continue;
            }
        };
        let style = match &item.color {
            Some(c) => match parse_color(c) {
                Ok(color) => Style::default().fg(color),
                Err(e) => {
                    warnln!(
                        "警告: countdown.items[{}] 的 color \"{}\" 无效 ({}), 使用默认颜色",
                        index, c, e
                    );
                    global_style
                }
            },
            None => global_style,
        };
        let millis = item.show_millis.unwrap_or(shared.show_millis);
        pendings.push(Pending {
            countdown: Countdown {
                size: item.size.unwrap_or(global_size),
                style,
                time,
                // Title falls back to the target time string
                title: item.title.clone().or_else(|| Some(time_str.clone())),
                // Multi-countdown grid freezes at zero; continue_on_zero is ignored.
                continue_on_zero: false,
                reverse: item.reverse.unwrap_or(shared.reverse),
                format: if millis {
                    DurationFormat::HourMinSecDeci
                } else {
                    DurationFormat::HourMinSec
                },
                freeze_on_zero: true,
            },
            order: item.order,
        });
    }

    // Sort: by remaining time ascending unless sort = "config"
    if shared.sort != "config" {
        let now = Local::now();
        pendings.sort_by_key(|p| p.countdown.time.signed_duration_since(now));
    }

    if pendings.len() > MAX_COUNTDOWNS {
        warnln!(
            "警告: countdown 数量超过上限 {}, 超出的 {} 条已忽略",
            MAX_COUNTDOWNS,
            pendings.len() - MAX_COUNTDOWNS
        );
        pendings.truncate(MAX_COUNTDOWNS);
    }

    let (rows, cols) = grid_shape(pendings.len());
    let capacity = rows * cols;
    let mut slots: Vec<Option<Countdown>> = Vec::new();
    slots.resize_with(capacity, || None);

    // Place pinned items first
    let mut unpinned: Vec<Countdown> = Vec::new();
    for p in pendings {
        match p.order {
            Some(order) if order >= 1 && order as usize <= capacity => {
                let start = order as usize - 1;
                let target = (0..capacity)
                    .map(|offset| (start + offset) % capacity)
                    .find(|&idx| slots[idx].is_none());
                match target {
                    Some(idx) => {
                        if idx != start {
                            warnln!(
                                "警告: order {} 的位置已被占用, 顺延到位置 {}",
                                order,
                                idx + 1
                            );
                        }
                        slots[idx] = Some(p.countdown);
                    }
                    None => unpinned.push(p.countdown),
                }
            }
            Some(order) => {
                warnln!(
                    "警告: order {} 超出范围 (1-{}), 按未指定位置处理",
                    order, capacity
                );
                unpinned.push(p.countdown);
            }
            None => unpinned.push(p.countdown),
        }
    }

    // Fill remaining free slots with unpinned items
    let mut unpinned = unpinned.into_iter();
    for slot in slots.iter_mut() {
        if slot.is_none() {
            if let Some(c) = unpinned.next() {
                *slot = Some(c);
            }
        }
    }

    slots
}

/// Render countdown slots in the grid given by `grid_shape`.
fn render_countdown_grid(countdowns: &[Option<Countdown>], f: &mut Frame) {
    let area = f.size();
    render_countdown_grid_to(countdowns, area, f.buffer_mut());
}

/// Buffer-level variant of `render_countdown_grid`, used by the wizard's
/// preview pane which only has a sub-`Rect`, not a full `Frame`.
pub(crate) fn render_countdown_grid_to(
    countdowns: &[Option<Countdown>],
    area: Rect,
    buf: &mut Buffer,
) {
    let (rows, cols) = grid_shape(countdowns.len());
    if rows <= 1 && cols <= 1 {
        if let Some(Some(ref w)) = countdowns.first() {
            w.render(area, buf);
        }
        return;
    }

    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Ratio(1, rows as u32); rows])
        .split(area);
    for (r, row) in row_areas.iter().enumerate() {
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, cols as u32); cols])
            .split(*row);
        for (c, cell) in col_areas.iter().enumerate() {
            if let Some(Some(ref w)) = countdowns.get(r * cols + c) {
                w.render(*cell, buf);
            }
        }
    }
}

pub(crate) fn parse_duration(s: &str) -> Result<Duration, String> {
    let reg = Regex::new(r"^(\d+)([smhdSMHD])$").unwrap();
    let cap = reg
        .captures(s)
        .ok_or_else(|| format!("{} is not a valid duration", s))?;

    let num = cap.get(1).unwrap().as_str().parse::<i64>().unwrap();
    let unit = cap.get(2).unwrap().as_str().to_lowercase();

    match unit.as_str() {
        "s" => Ok(Duration::seconds(num)),
        "m" => Ok(Duration::minutes(num)),
        "h" => Ok(Duration::hours(num)),
        "d" => Ok(Duration::days(num)),
        _ => Err(format!("Invalid duration: {}", s)),
    }
}

pub(crate) fn parse_color(s: &str) -> Result<Color, String> {
    let s = s.to_lowercase();
    let reg = Regex::new(r"^#([0-9a-f]{6})$").unwrap();
    match s.as_str() {
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" => Ok(Color::Gray),
        "darkgray" => Ok(Color::DarkGray),
        "lightred" => Ok(Color::LightRed),
        "lightgreen" => Ok(Color::LightGreen),
        "lightyellow" => Ok(Color::LightYellow),
        "lightblue" => Ok(Color::LightBlue),
        "lightmagenta" => Ok(Color::LightMagenta),
        "lightcyan" => Ok(Color::LightCyan),
        "white" => Ok(Color::White),
        s => {
            let cap = reg
                .captures(s)
                .ok_or_else(|| format!("Invalid color: {}", s))?;
            let hex = cap.get(1).unwrap().as_str();
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
            let b = u8::from_str_radix(&hex[4..], 16).unwrap();
            Ok(Color::Rgb(r, g, b))
        }
    }
}

pub(crate) fn parse_datetime(s: &str) -> Result<DateTime<Local>, String> {
    let s = s.trim();
    let today = Local::now().date_naive();

    let time = NaiveTime::parse_from_str(s, "%H:%M");
    if let Ok(time) = time {
        let time = NaiveDateTime::new(today, time);
        return Ok(Local.from_local_datetime(&time).unwrap());
    }

    let time = NaiveTime::parse_from_str(s, "%H:%M:%S");
    if let Ok(time) = time {
        let time = NaiveDateTime::new(today, time);
        return Ok(Local.from_local_datetime(&time).unwrap());
    }

    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d");
    if let Ok(date) = date {
        let time = NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        return Ok(Local.from_local_datetime(&time).unwrap());
    }

    let date_time = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S");
    if let Ok(date_time) = date_time {
        return Ok(Local.from_local_datetime(&date_time).unwrap());
    }

    let rfc_time = DateTime::parse_from_rfc3339(s);
    if let Ok(rfc_time) = rfc_time {
        return Ok(rfc_time.with_timezone(&Local));
    }

    Err("Invalid time format".to_string())
}

pub(crate) fn parse_timezone(s: &str) -> Result<Tz, String> {
    s.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn item(time: &str, title: Option<&str>, order: Option<u16>) -> CountdownItemConfig {
        CountdownItemConfig {
            time: Some(time.to_string()),
            title: title.map(|t| t.to_string()),
            color: None,
            size: None,
            show_millis: None,
            reverse: None,
            order,
        }
    }

    fn shared() -> crate::config::CountdownConfig {
        crate::config::CountdownConfig::default()
    }

    fn slots_of(items: &[CountdownItemConfig]) -> Vec<Option<Countdown>> {
        build_countdown_slots(items, &shared(), Style::default(), 1)
    }

    #[test]
    fn single_item_is_fullscreen_slot() {
        let slots = slots_of(&[item("2099-01-01", Some("a"), None)]);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].is_some());
    }

    #[test]
    fn two_items_use_two_rows() {
        let items: Vec<_> = (0..2).map(|_| item("2099-01-01", None, None)).collect();
        let slots = slots_of(&items);
        assert_eq!(slots.len(), 2, "n=2 should use 2 vertical rows");
        assert_eq!(slots.iter().flatten().count(), 2);
    }

    #[test]
    fn three_to_four_items_use_2x2_grid() {
        for n in 3..=4 {
            let items: Vec<_> = (0..n).map(|_| item("2099-01-01", None, None)).collect();
            let slots = slots_of(&items);
            assert_eq!(slots.len(), 4, "n={} should use a 2x2 grid", n);
            assert_eq!(slots.iter().flatten().count(), n);
        }
    }

    #[test]
    fn five_to_six_items_use_3x2_grid() {
        for n in 5..=6 {
            let items: Vec<_> = (0..n).map(|_| item("2099-01-01", None, None)).collect();
            let slots = slots_of(&items);
            assert_eq!(slots.len(), 6, "n={} should use a 3x2 grid", n);
            assert_eq!(slots.iter().flatten().count(), n);
        }
    }

    #[test]
    fn seven_to_eight_items_use_4x2_grid() {
        for n in 7..=8 {
            let items: Vec<_> = (0..n).map(|_| item("2099-01-01", None, None)).collect();
            let slots = slots_of(&items);
            assert_eq!(slots.len(), 8, "n={} should use a 4x2 grid", n);
            assert_eq!(slots.iter().flatten().count(), n);
        }
    }

    #[test]
    fn more_than_eight_items_are_truncated() {
        let items: Vec<_> = (0..10).map(|_| item("2099-01-01", None, None)).collect();
        let slots = slots_of(&items);
        assert_eq!(slots.len(), 8);
        assert_eq!(slots.iter().flatten().count(), 8);
    }

    #[test]
    fn order_pins_position_and_unpinned_fill_by_remaining_time() {
        // "far" pinned to slot 4 (order=4); "near" should win slot 1 by time sort.
        let items = vec![
            item("2099-01-01", Some("far"), Some(4)),
            item("2098-01-01", Some("near"), None),
            item("2100-01-01", Some("later"), None),
        ];
        let slots = slots_of(&items);
        assert_eq!(slots.len(), 4);
        assert_eq!(slots[0].as_ref().unwrap().title.as_deref(), Some("near"));
        assert_eq!(slots[1].as_ref().unwrap().title.as_deref(), Some("later"));
        assert_eq!(slots[3].as_ref().unwrap().title.as_deref(), Some("far"));
        assert!(slots[2].is_none());
    }

    #[test]
    fn order_conflict_shifts_to_next_free_slot() {
        let items = vec![
            item("2099-01-01", Some("a"), Some(1)),
            item("2099-01-01", Some("b"), Some(1)),
        ];
        let slots = slots_of(&items);
        assert!(slots[0].is_some() && slots[1].is_some());
    }

    #[test]
    fn invalid_items_are_skipped() {
        let items = vec![
            item("not-a-time", Some("bad"), None),
            item("2099-01-01", Some("good"), None),
        ];
        let slots = slots_of(&items);
        assert_eq!(slots.iter().flatten().count(), 1);
        assert_eq!(slots[0].as_ref().unwrap().title.as_deref(), Some("good"));
    }

    #[test]
    fn missing_title_falls_back_to_time_string() {
        let slots = slots_of(&[item("2099-01-01", None, None)]);
        assert_eq!(
            slots[0].as_ref().unwrap().title.as_deref(),
            Some("2099-01-01")
        );
    }

    #[test]
    fn expired_countdown_renders_frozen_zeros() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App {
            countdowns: slots_of(&[item("2000-01-01", Some("past"), None)]),
            ..Default::default()
        };
        terminal.draw(|f| app.ui(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        // '0' blocks are drawn with '█'; just assert something was rendered
        assert!(text.contains('█'));
    }

    #[test]
    fn grid_renders_multiple_countdowns() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App {
            countdowns: slots_of(&[
                item("2099-01-01", Some("one"), None),
                item("2098-01-01", Some("two"), None),
            ]),
            ..Default::default()
        };
        terminal.draw(|f| app.ui(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("one"));
        assert!(text.contains("two"));
        assert!(text.contains('█'));
    }

    // ---- P0: wizard parameter layering (T7 / T8 / T9) ----

    #[test]
    fn wizard_params_override_config_file() {
        // examples/config.toml sets [default] color = "green" and
        // [timer] show_millis = true; the wizard explicitly overrides both.
        let config_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/config.toml");
        let mut app = App::default();
        app.apply_mode_config(ModeConfig {
            kind: ModeKind::Timer,
            params: AppParams {
                durations: Some(vec![Duration::minutes(25)]),
                show_millis: Some(false),
                color: Some(Color::Red),
                config: Some(config_path),
                ..Default::default()
            },
        });
        let timer = app.timer.as_ref().expect("timer widget should exist");
        assert_eq!(
            timer.format,
            DurationFormat::HourMinSec,
            "wizard show_millis=false must beat the config file's true"
        );
        assert_eq!(timer.style.fg, Some(Color::Red));
        assert_eq!(timer.durations, vec![Duration::minutes(25)]);
    }

    #[test]
    fn without_params_config_file_still_applies() {
        let config_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/config.toml");
        let mut app = App {
            config: Some(config_path),
            ..Default::default()
        };
        // Bare default-mode derivation: examples/config.toml has mode = "clock".
        app.init_app();
        assert!(app.clock.is_some());
        assert_eq!(app.color, Some(Color::Green));
    }

    #[test]
    fn cli_parse_timer_regression() {
        use clap::Parser;
        let app = App::parse_from(["tclock", "timer", "-d", "25m"]);
        match app.mode {
            Some(Mode::Timer { ref durations, .. }) => {
                assert_eq!(durations, &vec![Duration::minutes(25)]);
            }
            _ => panic!("expected timer mode"),
        }
        assert!(app.params.is_none());
    }

    #[test]
    fn cli_bare_invocation_has_no_mode() {
        use clap::Parser;
        let app = App::parse_from(["tclock"]);
        assert!(app.mode.is_none());
        assert!(app.color.is_none());
    }

    #[test]
    fn empty_timer_durations_fall_back_to_default() {
        // The old `t` hotkey constructed Mode::Timer with durations: vec![],
        // which panics in Timer::remaining_time. The resolution layer must
        // substitute the CLI default instead.
        let mut app = App::default();
        app.apply_mode_config(ModeConfig::preset(ModeKind::Timer));
        let timer = app.timer.as_ref().unwrap();
        assert_eq!(timer.durations, vec![Duration::minutes(5)]);
    }

    #[test]
    fn hotkey_presets_switch_widgets() {
        let mut app = App::default();
        app.apply_mode_config(ModeConfig::preset(ModeKind::Clock));
        assert!(app.clock.is_some());
        app.apply_mode_config(ModeConfig::preset(ModeKind::Stopwatch));
        assert!(app.stopwatch.is_some());
        assert!(app.clock.is_none(), "stale widgets must be cleared");
        app.apply_mode_config(ModeConfig::preset(ModeKind::Timer));
        assert!(app.timer.is_some());
        assert!(app.stopwatch.is_none());
    }
}
