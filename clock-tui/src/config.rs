use chrono_tz::Tz;
use serde::{Deserialize, Deserializer};

fn deserialize_timezone<'de, D>(deserializer: D) -> Result<Option<Tz>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: DefaultConfig,
    #[serde(default)]
    pub clock: ClockConfig,
    #[serde(default)]
    pub timer: TimerConfig,
    #[serde(default)]
    pub stopwatch: StopwatchConfig,
    #[serde(default)]
    pub countdown: CountdownConfig,
}

#[derive(Debug, Deserialize)]
pub struct DefaultConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_size")]
    pub size: u16,
}

#[derive(Debug, Deserialize)]
pub struct ClockConfig {
    #[serde(default = "default_true")]
    pub show_date: bool,
    #[serde(default = "default_true")]
    pub show_seconds: bool,
    #[serde(default = "default_false")]
    pub show_millis: bool,
    #[serde(default, deserialize_with = "deserialize_timezone")]
    pub timezone: Option<Tz>,
}

#[derive(Debug, Deserialize)]
pub struct TimerConfig {
    #[serde(default = "default_timer_durations")]
    pub durations: Vec<String>,
    #[serde(default)]
    pub titles: Vec<String>,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default = "default_true")]
    pub show_millis: bool,
    #[serde(default)]
    pub start_paused: bool,
    #[serde(default)]
    pub auto_quit: bool,
    #[serde(default)]
    pub execute: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StopwatchConfig {}

#[derive(Debug, Deserialize)]
pub struct CountdownConfig {
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub show_millis: bool,
    #[serde(default)]
    pub continue_on_zero: bool,
    #[serde(default)]
    pub reverse: bool,
    /// Sort strategy for items: "time" (by remaining time ascending) or "config" (config file order)
    #[serde(default = "default_countdown_sort")]
    pub sort: String,
    /// Multiple countdowns displayed simultaneously in a grid
    #[serde(default)]
    pub items: Vec<CountdownItemConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CountdownItemConfig {
    /// The target time to countdown to, same formats as [countdown].time
    #[serde(default)]
    pub time: Option<String>,
    /// Title shown above the countdown, defaults to the target time string
    #[serde(default)]
    pub title: Option<String>,
    /// Per-item color, falls back to the global color
    #[serde(default)]
    pub color: Option<String>,
    /// Per-item size, falls back to the global size.
    /// Rendered exactly as configured (same as timer/clock); if the grid cell
    /// is too small the text is clipped instead of auto-shrunk.
    #[serde(default)]
    pub size: Option<u16>,
    /// Per-item millisecond display, falls back to [countdown].show_millis
    #[serde(default)]
    pub show_millis: Option<bool>,
    /// Per-item reverse (count up), falls back to [countdown].reverse
    #[serde(default)]
    pub reverse: Option<bool>,
    /// Pin this countdown to a fixed grid position (1-based, row-major)
    #[serde(default)]
    pub order: Option<u16>,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            color: default_color(),
            size: default_size(),
        }
    }
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            show_date: default_true(),
            show_seconds: default_true(),
            show_millis: default_false(),
            timezone: None,
        }
    }
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            durations: default_timer_durations(),
            titles: Vec::new(),
            repeat: false,
            show_millis: default_true(),
            start_paused: false,
            auto_quit: false,
            execute: Vec::new(),
        }
    }
}

impl Default for StopwatchConfig {
    fn default() -> Self {
        Self {}
    }
}

impl Default for CountdownConfig {
    fn default() -> Self {
        Self {
            time: None,
            title: None,
            show_millis: false,
            continue_on_zero: false,
            reverse: false,
            sort: default_countdown_sort(),
            items: Vec::new(),
        }
    }
}

fn default_mode() -> String {
    "clock".to_string()
}

fn default_color() -> String {
    "green".to_string()
}

fn default_size() -> u16 {
    1
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_timer_durations() -> Vec<String> {
    vec!["25m".to_string(), "5m".to_string()]
}

fn default_countdown_sort() -> String {
    "time".to_string()
}

impl Config {
    /// Load from the given path. There is no default config path:
    /// a config file is only used when explicitly specified via `--config`.
    pub fn load_from(path: &std::path::Path) -> Option<Self> {
        if !path.exists() {
            eprintln!("配置文件不存在: {}", path.display());
            return None;
        };

        let content = std::fs::read_to_string(path).ok()?;
        match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("解析配置文件失败: {}", e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_template_example_parses() {
        let content = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../countdown.toml.example"),
        )
        .unwrap();
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(config.countdown.items.len(), 6);
        assert_eq!(config.countdown.sort, "time");
        assert_eq!(config.countdown.items[0].order, Some(1));
        assert_eq!(config.countdown.items[1].size, Some(2));
        assert_eq!(
            config.countdown.items[1].time.as_deref(),
            Some("2026-10-05 09:00:00")
        );
    }

    #[test]
    fn full_config_example_parses() {
        let content = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/examples/config.toml"),
        )
        .unwrap();
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(config.countdown.items.len(), 2);
    }

    #[test]
    fn load_from_custom_path() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("countdown.toml.example");
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.countdown.items.len(), 6);

        let missing = std::path::Path::new("definitely/not/exist.toml");
        assert!(Config::load_from(missing).is_none());
    }
}
