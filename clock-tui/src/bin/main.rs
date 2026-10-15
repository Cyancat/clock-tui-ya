use std::error::Error;

use clap::Parser;
use clock_tui_ya::app::App;

fn main() -> Result<(), Box<dyn Error>> {
    // Parse command line arguments first so `--help` / `--version` print
    // before any terminal setup.
    let app = App::parse();
    // Strictly zero arguments (argv[0] only) → interactive wizard.
    // `tclock -c red` has no subcommand either but is NOT a bare invocation.
    let bare = clock_tui_ya::run::is_bare_invocation(std::env::args_os().count());
    clock_tui_ya::run::run(app, bare)
}
