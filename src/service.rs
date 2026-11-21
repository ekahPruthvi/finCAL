//! The `finCAL -s` background service.
//!
//! It watches the template and journal, keeps a computed snapshot in
//! `~/.local/share/fincal/state.json`, records the current month in
//! `history.json` (so `LEFT from Prev Month` resolves), answers status/reload
//! requests from the TUI over a Unix socket, and sends the monthly/weekly
//! reminders hourly while they are still outstanding.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::budget::{self, Report};
use crate::ipc::{self, Request, Status};
use crate::journal::{Journal, Summary};
use crate::notify::{self, NotifyState};
use crate::paths;
use crate::state::History;
use crate::template::Template;
use crate::util;

#[derive(Serialize)]
struct PersistedState {
    pid: u32,
    template: String,
    journal: String,
    month: String,
    updated: u64,
    sal: Option<f64>,
    month_total: Option<f64>,
    mtotal: Option<f64>,
    left: Option<f64>,
    weeks: Vec<PersistedWeek>,
    month_created: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct PersistedWeek {
    week: u32,
    total: f64,
}

struct Service {
    template_path: PathBuf,
    journal_path: PathBuf,
    history: History,
    notify: NotifyState,
    fallback: Option<Report>,
    summary: Option<Summary>,
    month_created: bool,
    updated: u64,
    last_error: Option<String>,
}

fn pid_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{}", pid)).exists()
}

fn running_pid() -> Option<u32> {
    let text = fs::read_to_string(paths::pid_path()).ok()?;
    let pid = text.trim().parse::<u32>().ok()?;
    if pid_alive(pid) {
        Some(pid)
    } else {
        None
    }
}

pub fn run(template_path: PathBuf, journal_path: PathBuf) -> Result<(), String> {
    if let Some(pid) = running_pid() {
        return Err(format!(
            "finCAL service already running (pid {}); socket {}",
            pid,
            paths::socket_path().display()
        ));
    }

    let data_dir = paths::data_dir();
    fs::create_dir_all(&data_dir).map_err(|error| format!("{}: {error}", data_dir.display()))?;

    let pid = process::id();
    fs::write(paths::pid_path(), pid.to_string())
        .map_err(|error| format!("cannot write pid file: {error}"))?;
    let socket = paths::socket_path();
    let _ = fs::remove_file(&socket);

    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("cannot bind {}: {error}", socket.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure socket: {error}"))?;

    let mut service = Service {
        template_path: template_path.clone(),
        journal_path: journal_path.clone(),
        history: History::load(),
        notify: NotifyState::load(),
        fallback: None,
        summary: None,
        month_created: false,
        updated: 0,
        last_error: None,
    };

    eprintln!(
        "finCAL service {pid} watching {} and {} (socket {})",
        template_path.display(),
        journal_path.display(),
        socket.display()
    );

    service.refresh();
    service.log_status();
    service.supervise();

    let mut last_seen = seen_stamp(&template_path, &journal_path);

    loop {
        match listener.accept() {
            Ok((stream, _)) => match read_request(&stream) {
                Some(Request::Reload) => {
                    service.refresh();
                    service.supervise();
                    service.log_status();
                    service.respond(stream);
                }
                Some(Request::Status) => service.respond(stream),
                None => drop(stream),
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => eprintln!("finCAL service: accept failed: {error}"),
        }

        let stamp = seen_stamp(&template_path, &journal_path);
        if stamp != last_seen {
            last_seen = stamp;
            service.refresh();
            service.log_status();
        }
        service.supervise();

        thread::sleep(Duration::from_millis(500));
    }
}

fn seen_stamp(template: &PathBuf, journal: &PathBuf) -> Option<(SystemTime, SystemTime)> {
    let t = fs::metadata(template).and_then(|meta| meta.modified()).ok();
    let j = fs::metadata(journal).and_then(|meta| meta.modified()).ok();
    match (t, j) {
        (Some(t), Some(j)) => Some((t, j)),
        _ => None,
    }
}

fn read_request(stream: &UnixStream) -> Option<Request> {
    stream
        .set_read_timeout(Some(Duration::from_millis(1000)))
        .ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    ipc::decode_request(&line)
}

impl Service {
    fn refresh(&mut self) {
        let now = util::now_unix();
        let month = util::current_month();

        match Template::load(&self.template_path) {
            Ok(template) => {
                let left_prev = self.history.left_before(&month);
                self.fallback = Some(budget::compute(&template, left_prev));
                self.last_error = None;
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                eprintln!(
                    "finCAL service: cannot read {}: {error}",
                    self.template_path.display()
                );
            }
        }

        let journal = Journal::load(&self.journal_path);
        self.month_created = journal.month(&month).is_some();
        self.summary = if self.month_created {
            Some(journal.summary(&month))
        } else {
            None
        };

        self.updated = now;
        if let Some(summary) = &self.summary {
            self.history.record_month(
                &month,
                summary.sal,
                Some(summary.mtotal),
                summary.left,
                now,
            );
            if let Err(error) = self.history.save() {
                eprintln!("finCAL service: cannot save history: {error}");
            }
        }
        self.write_state(&month);
    }

    fn write_state(&self, month: &str) {
        let state = PersistedState {
            pid: process::id(),
            template: self.template_path.display().to_string(),
            journal: self.journal_path.display().to_string(),
            month: month.to_string(),
            updated: self.updated,
            sal: self.summary.as_ref().and_then(|summary| summary.sal),
            month_total: self.summary.as_ref().map(|summary| summary.month_total),
            mtotal: self
                .summary
                .as_ref()
                .map(|summary| summary.mtotal)
                .or_else(|| self.fallback.as_ref().and_then(Report::monthly_total)),
            left: self
                .summary
                .as_ref()
                .and_then(|summary| summary.left)
                .or_else(|| self.fallback.as_ref().and_then(Report::monthly_left)),
            weeks: self
                .summary
                .as_ref()
                .map(|summary| {
                    summary
                        .week_totals
                        .iter()
                        .map(|(week, total)| PersistedWeek {
                            week: *week,
                            total: *total,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            month_created: self.month_created,
            error: self.last_error.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            if let Err(error) = fs::write(paths::state_path(), text) {
                eprintln!("finCAL service: cannot write state: {error}");
            }
        }
    }

    /// Sends the two reminders, at most once an hour for each pending item.
    fn supervise(&mut self) {
        let now = util::now_unix();
        let month = util::current_month();
        let journal = Journal::load(&self.journal_path);

        if journal.month(&month).is_none() {
            let key = format!("month:{month}");
            if self.notify.month_due(&key, now) {
                notify::send(
                    "finCAL",
                    &format!("Hey Create {}'s Journal", util::month_word(&month)),
                );
                self.notify.mark_month(key, now);
                self.notify.save();
            }
        }

        if util::is_friday(now) {
            if let Some(block) = journal.month(&month) {
                let week = util::week_of_month(now);
                if !block.weeks.iter().any(|entry| entry.number == week) {
                    let key = format!("week:{month}:{week}");
                    if self.notify.week_due(&key, now) {
                        notify::send("finCAL", "Hey Add weekly financial report");
                        self.notify.mark_week(key, now);
                        self.notify.save();
                    }
                }
            }
        }
    }

    fn status(&self) -> Status {
        let monthly = self.fallback.as_ref().and_then(Report::monthly);
        let sal = self
            .summary
            .as_ref()
            .and_then(|summary| summary.sal)
            .or_else(|| monthly.and_then(|sheet| sheet.sal));
        let mtotal = self
            .summary
            .as_ref()
            .map(|summary| summary.mtotal)
            .or_else(|| self.fallback.as_ref().and_then(Report::monthly_total));
        let left = self
            .summary
            .as_ref()
            .and_then(|summary| summary.left)
            .or_else(|| self.fallback.as_ref().and_then(Report::monthly_left));
        Status {
            ok: self.last_error.is_none(),
            pid: process::id(),
            template: self.template_path.display().to_string(),
            month: util::current_month(),
            updated: self.updated,
            sal,
            mtotal,
            left,
            error: self.last_error.clone(),
        }
    }

    fn respond(&self, mut stream: UnixStream) {
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let text = serde_json::to_string(&self.status()).unwrap_or_default();
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }

    fn log_status(&self) {
        let month = util::current_month();
        match &self.summary {
            Some(summary) => eprintln!(
                "finCAL service: {month} from journal: SAL {}, MTotal {}, LEFT {}",
                summary.sal.map(util::money).unwrap_or_else(|| "unset".into()),
                util::money(summary.mtotal),
                summary
                    .left
                    .map(util::money)
                    .unwrap_or_else(|| "unset".into()),
            ),
            None => eprintln!("finCAL service: waiting for the {month} journal block"),
        }
    }
}
