//! Terminal session runner: owns the alternate screen and dispatches keys
//! for both the interactive wizard and the running mode widgets.
//!
//! Layout of a session:
//!
//! ```text
//! run(app, bare)
//!   ├─ bare == false → validate countdown CLI args → run mode loop → exit
//!   └─ bare == true  → wizard ⇄ running loop, `q` in a mode returns to the
//!                      wizard, quitting the wizard ends the process
//! ```

use std::error::Error;
use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::wizard::{Wizard, WizardOutcome};
use crate::app::{App, Mode, ModeConfig, ModeKind};
use crate::config::Config;

/// `true` when the command line contained no arguments at all (argv[0] only).
/// `tclock -c red` has `mode == None` too, but is NOT a bare invocation.
pub fn is_bare_invocation(args_len: usize) -> bool {
    args_len <= 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Wizard,
    Running,
}

/// Entry point shared by CLI and wizard sessions.
pub fn run(mut app: App, start_with_wizard: bool) -> Result<(), Box<dyn Error>> {
    // CLI-only validation; the wizard validates inline instead.
    if !start_with_wizard {
        validate_countdown_cli(&app);
    }

    // Setup terminal. The backend owns its own Stdout handle so `stdout`
    // stays usable for the leave/re-enter dance on mode exit.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = session_loop(&mut terminal, &mut stdout, &mut app, start_with_wizard);

    // Restore terminal no matter how the loop ended.
    terminal.show_cursor()?;
    drop(terminal);
    disable_raw_mode()?;
    stdout.execute(LeaveAlternateScreen)?;

    // `on_exit` must run after leaving the alternate screen (it prints e.g.
    // the stopwatch time).
    app.on_exit();
    io::stdout().flush()?;

    result
}

fn session_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    stdout: &mut io::Stdout,
    app: &mut App,
    wizard_enabled: bool,
) -> Result<(), Box<dyn Error>> {
    let mut wizard = Wizard::new(ModeKind::Clock);
    let mut phase = if wizard_enabled {
        Phase::Wizard
    } else {
        app.init_app();
        Phase::Running
    };

    loop {
        match phase {
            Phase::Wizard => {
                terminal.draw(|f| wizard.ui(f))?;
                if event::poll(Duration::from_millis(250))? {
                    if let Event::Key(key) = event::read()? {
                        match wizard.on_key(key) {
                            WizardOutcome::Continue => {}
                            WizardOutcome::Quit => break,
                            WizardOutcome::Run(mode_config) => {
                                app.apply_mode_config(mode_config);
                                phase = Phase::Running;
                            }
                        }
                    }
                }
            }
            Phase::Running => {
                if app.is_ended() {
                    // Timer with auto_quit finished.
                    if wizard_enabled {
                        phase = Phase::Wizard;
                    } else {
                        break;
                    }
                    continue;
                }
                terminal.draw(|f| app.ui(f))?;
                if event::poll(Duration::from_millis(250))? {
                    if let Event::Key(key) = event::read()? {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match key.code {
                            KeyCode::Char('c') if ctrl => break,
                            KeyCode::Char('q') | KeyCode::Esc => {
                                if wizard_enabled {
                                    // Print stopwatch results on the main
                                    // screen, then redraw the wizard.
                                    leave_for_exit_print(terminal, stdout, app)?;
                                    app.clear_widgets();
                                    phase = Phase::Wizard;
                                } else {
                                    break;
                                }
                            }
                            KeyCode::Char(' ') => app.on_key(KeyCode::Char(' ')),
                            KeyCode::Char('c') => switch_mode(
                                app, &mut wizard, &mut phase, ModeKind::Clock, wizard_enabled,
                            ),
                            KeyCode::Char('w') => switch_mode(
                                app,
                                &mut wizard,
                                &mut phase,
                                ModeKind::Stopwatch,
                                wizard_enabled,
                            ),
                            KeyCode::Char('t') => switch_mode(
                                app, &mut wizard, &mut phase, ModeKind::Timer, wizard_enabled,
                            ),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `c` / `w` / `t` handling: in wizard sessions jump back to the wizard with
/// the mode preselected; in CLI sessions keep the original in-place switch.
fn switch_mode(
    app: &mut App,
    wizard: &mut Wizard,
    phase: &mut Phase,
    kind: ModeKind,
    wizard_enabled: bool,
) {
    if wizard_enabled {
        wizard.preselect(kind);
        *phase = Phase::Wizard;
    } else {
        app.apply_mode_config(ModeConfig::preset(kind));
    }
}

/// Leave the alternate screen, run `on_exit` (prints e.g. the stopwatch
/// time), then re-enter so the wizard redraws on a clean screen. The print
/// must happen outside the alternate screen or it would be wiped by the next
/// full redraw.
fn leave_for_exit_print(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    stdout: &mut io::Stdout,
    app: &App,
) -> Result<(), Box<dyn Error>> {
    terminal.show_cursor()?;
    disable_raw_mode()?;
    stdout.execute(LeaveAlternateScreen)?;
    app.on_exit();
    io::stdout().flush()?;
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}

/// CLI-only guard: `countdown` without `--time` needs a config file that
/// provides `[countdown].time` or `[[countdown.items]]`.
fn validate_countdown_cli(app: &App) {
    if let Some(Mode::Countdown { time: None, .. }) = &app.mode {
        let config = app.config.as_deref().and_then(Config::load_from);
        let has_config_time = config
            .as_ref()
            .map(|c| c.countdown.time.is_some())
            .unwrap_or(false);
        let has_items = config
            .as_ref()
            .map(|c| !c.countdown.items.is_empty())
            .unwrap_or(false);
        if !has_config_time && !has_items {
            eprintln!(
                "Error: countdown mode requires --time, or --config <PATH> with [countdown].time / [[countdown.items]]"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_invocation_means_zero_arguments() {
        assert!(is_bare_invocation(1)); // argv[0] only
        assert!(is_bare_invocation(0)); // defensive
        assert!(!is_bare_invocation(2)); // tclock -c / tclock timer ...
        assert!(!is_bare_invocation(5));
    }
}
