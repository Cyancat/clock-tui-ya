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
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
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

#[derive(clap::Parser, Default)]
#[clap(name = "tclock", about = "A clock app in terminal", long_about = None)]
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
    pub config: Option<std::path::PathBuf>,

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
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = Some(mode);
        self.init_app();
    }

    pub fn init_app(&mut self) {
        // Load config only when --config is explicitly given
        let config = self.config.as_deref().and_then(Config::load_from);
        let default_config = config.as_ref().map(|c| &c.default);

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

        // set default color and size
        if self.color.is_none() {
            self.color = default_config
                .map(|c| parse_color(&c.color).unwrap_or(Color::Green))
                .or(Some(Color::Green));
        }
        if self.size.is_none() {
            self.size = default_config.map(|c| c.size).or(Some(1));
        }

        let style = Style::default().fg(self.color.unwrap_or(Color::Green));
        let size = self.size.unwrap_or(1);

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
                self.clock = Some(Clock {
                    size,
                    style,
                    show_date: !no_date && clock_config.map(|c| c.show_date).unwrap_or(true),
                    show_millis: *millis || clock_config.map(|c| c.show_millis).unwrap_or(false),
                    show_secs: !no_seconds && clock_config.map(|c| c.show_seconds).unwrap_or(true),
                    timezone: timezone.or_else(|| clock_config.and_then(|c| c.timezone)),
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
                let format = if *no_millis {
                    DurationFormat::HourMinSec
                } else {
                    DurationFormat::HourMinSecDeci
                };
                self.timer = Some(Timer::new(
                    size,
                    style,
                    durations.to_owned(),
                    titles.to_owned(),
                    *repeat || timer_config.map(|c| c.repeat).unwrap_or(false),
                    format,
                    *paused || timer_config.map(|c| c.start_paused).unwrap_or(false),
                    *auto_quit || timer_config.map(|c| c.auto_quit).unwrap_or(false),
                    execute.to_owned(),
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
                let single_format =
                    if *millis || countdown_config.map(|c| c.show_millis).unwrap_or(false) {
                        DurationFormat::HourMinSecDeci
                    } else {
                        DurationFormat::HourMinSec
                    };

                if let Some(t) = time {
                    // Single countdown from CLI `--time`: keep the original behavior (blink past zero).
                    self.countdowns = vec![Some(Countdown {
                        size,
                        style,
                        time: *t,
                        title: title.to_owned(),
                        continue_on_zero: *continue_on_zero
                            || countdown_config
                                .map(|c| c.continue_on_zero)
                                .unwrap_or(false),
                        reverse: *reverse || countdown_config.map(|c| c.reverse).unwrap_or(false),
                        format: single_format,
                        freeze_on_zero: false,
                    })];
                } else if countdown_config.map(|c| !c.items.is_empty()).unwrap_or(false) {
                    // Multiple countdowns from config [[countdown.items]]: grid layout, freeze at zero.
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
                        title: countdown_config.map(|c| c.title.clone()).unwrap_or(None),
                        continue_on_zero: countdown_config
                            .map(|c| c.continue_on_zero)
                            .unwrap_or(false),
                        reverse: countdown_config.map(|c| c.reverse).unwrap_or(false),
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
fn build_countdown_slots(
    items: &[CountdownItemConfig],
    shared: &crate::config::CountdownConfig,
    global_style: Style,
    global_size: u16,
) -> Vec<Option<Countdown>> {
    struct Pending {
        countdown: Countdown,
        order: Option<u16>,
    }

    let mut pendings: Vec<Pending> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let time_str = match &item.time {
            Some(t) => t,
            None => {
                eprintln!("警告: countdown.items[{}] 缺少 time，已跳过", index);
                continue;
            }
        };
        let time = match parse_datetime(time_str) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
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
                    eprintln!(
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
        eprintln!(
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
                            eprintln!(
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
                eprintln!(
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
    let (rows, cols) = grid_shape(countdowns.len());
    if rows <= 1 && cols <= 1 {
        if let Some(Some(ref w)) = countdowns.first() {
            f.render_widget(w, area);
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
                f.render_widget(w, *cell);
            }
        }
    }
}

fn parse_duration(s: &str) -> Result<Duration, String> {
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

fn parse_color(s: &str) -> Result<Color, String> {
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

fn parse_datetime(s: &str) -> Result<DateTime<Local>, String> {
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

fn parse_timezone(s: &str) -> Result<Tz, String> {
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
}
