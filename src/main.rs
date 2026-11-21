mod budget;
mod expr;
mod ipc;
mod journal;
mod notify;
mod paths;
mod service;
mod state;
mod template;
mod tui;
mod update;
mod util;

use std::process;

const HELP: &str = "\
finCAL — a Logseq budget calculator

USAGE:
    finCAL                      Open the TUI (creates this month's journal block)
    finCAL -s                   Run the background service
    finCAL --edit-sal 25000     Set this month's salary
    finCAL --update             Download and install the latest release
    finCAL --template PATH      Use another template page
    finCAL --journal PATH       Use another journal page

    finCAL -h | --help          Show this help
    finCAL -V | --version       Show the version

The service watches the template and journal, publishes a computed snapshot
under $XDG_DATA_HOME/fincal, and reminds you hourly while the month's journal
block is missing and on Fridays while the weekly report is missing. Start it
in the background, e.g. `finCAL -s &` or from a user service.

Defaults:
    template ~/Projects/logseq/pages/Fin_Template.md  ($FINCAL_TEMPLATE)
    journal  ~/Projects/logseq/pages/Fin_journal.md   ($FINCAL_JOURNAL)
    icon     <repo>/icon/finCAL.svg                   ($FINCAL_ICON)";

fn main() {
    let mut service_mode = false;
    let mut update_mode = false;
    let mut edit_sal: Option<f64> = None;
    let mut template = paths::template_path();
    let mut journal = paths::journal_path();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--service" => service_mode = true,
            "--update" => update_mode = true,
            "-t" | "--template" => match args.next() {
                Some(path) => template = path.into(),
                None => fail("--template needs a path", 2),
            },
            "-j" | "--journal" => match args.next() {
                Some(path) => journal = path.into(),
                None => fail("--journal needs a path", 2),
            },
            "--edit-sal" => match args.next().and_then(|value| value.parse::<f64>().ok()) {
                Some(value) => edit_sal = Some(value),
                None => fail("--edit-sal needs an amount, e.g. --edit-sal 25000", 2),
            },
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            "-V" | "--version" => {
                println!("finCAL {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => fail(&format!("unknown argument '{other}'"), 2),
        }
    }

    if service_mode && edit_sal.is_some() {
        fail("--service and --edit-sal cannot be combined", 2);
    }
    if update_mode && (service_mode || edit_sal.is_some()) {
        fail("--update cannot be combined with --service or --edit-sal", 2);
    }

    let result = if update_mode {
        update::run()
    } else if let Some(value) = edit_sal {
        edit_salary(&template, &journal, value)
    } else if service_mode {
        service::run(template, journal)
    } else {
        tui::run(template, journal)
    };

    if let Err(error) = result {
        eprintln!("finCAL: {error}");
        process::exit(1);
    }
}

fn edit_salary(
    template_path: &std::path::Path,
    journal_path: &std::path::Path,
    value: f64,
) -> Result<(), String> {
    let template = template::Template::load(template_path)
        .map_err(|error| format!("{}: {error}", template_path.display()))?;
    let message = journal::set_salary(&template, journal_path, value)?;
    println!("{message}");
    Ok(())
}

fn fail(message: &str, code: i32) -> ! {
    eprintln!("finCAL: {message}\n\n{HELP}");
    process::exit(code);
}
