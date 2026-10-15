use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize};

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

    /// Silent variant of `load_from` for the interactive wizard: prints
    /// nothing (stderr output would corrupt the alternate screen), the
    /// wizard surfaces failures in its own status line instead.
    pub fn load_from_quiet(path: &std::path::Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }
}

/// Small persisted state for the interactive wizard, written next to the
/// executable as `tclock-wizard.toml`. Currently only remembers the last
/// successfully loaded `--config` path so the wizard can prefill it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WizardState {
    #[serde(default)]
    pub config_path: Option<std::path::PathBuf>,
}

impl WizardState {
    /// `tclock-wizard.toml` in the executable's directory.
    fn state_file_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(|dir| dir.join("tclock-wizard.toml"))
    }

    /// Load the persisted state; missing or corrupt file yields defaults.
    pub fn load() -> Self {
        Self::load_from_path(Self::state_file_path())
    }

    fn load_from_path(path: Option<std::path::PathBuf>) -> Self {
        path.and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| toml::from_str(&c).ok())
            .unwrap_or_default()
    }

    /// Persist the state. `config_path = None` deletes the state file.
    /// All IO errors are ignored: this is a convenience, not critical state.
    pub fn save(&self) {
        Self::save_to(Self::state_file_path(), self)
    }

    fn save_to(path: Option<std::path::PathBuf>, state: &Self) {
        let Some(path) = path else { return };
        match &state.config_path {
            Some(_) => {
                if let Ok(content) = toml::to_string(state) {
                    let _ = std::fs::write(path, content);
                }
            }
            None => {
                let _ = std::fs::remove_file(path);
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

    #[test]
    fn wizard_state_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tclock-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tclock-wizard.toml");

        // Missing file → defaults.
        assert!(WizardState::load_from_path(Some(path.clone())).config_path.is_none());

        // Save with a path → loads back.
        let state = WizardState {
            config_path: Some(std::path::PathBuf::from(r"E:\Repo\config.toml")),
        };
        WizardState::save_to(Some(path.clone()), &state);
        let loaded = WizardState::load_from_path(Some(path.clone()));
        assert_eq!(
            loaded.config_path.as_deref(),
            Some(std::path::Path::new(r"E:\Repo\config.toml"))
        );

        // Windows backslashes survive TOML serialization.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("config_path"));

        // Clearing removes the file.
        WizardState::save_to(Some(path.clone()), &WizardState::default());
        assert!(!path.exists());
        let _ = std::fs::remove_dir(&dir);
    }
}
