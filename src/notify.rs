//! Desktop notifications with an hourly throttle.

use std::fs;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::util;

const HOUR: u64 = 60 * 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotifyState {
    #[serde(default)]
    pub month_key: String,
    #[serde(default)]
    pub month_at: u64,
    #[serde(default)]
    pub week_key: String,
    #[serde(default)]
    pub week_at: u64,
}

impl NotifyState {
    pub fn load() -> NotifyState {
        fs::read_to_string(paths::notify_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = paths::data_dir();
        let _ = fs::create_dir_all(&dir);
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(paths::notify_path(), text);
        }
    }

    /// Due when the pending item changed or the last alert was an hour ago.
    fn due(key: &str, last_key: &str, last_at: u64, now: u64) -> bool {
        last_key != key || now.saturating_sub(last_at) >= HOUR
    }

    pub fn month_due(&self, key: &str, now: u64) -> bool {
        Self::due(key, &self.month_key, self.month_at, now)
    }

    pub fn mark_month(&mut self, key: String, now: u64) {
        self.month_key = key;
        self.month_at = now;
    }

    pub fn week_due(&self, key: &str, now: u64) -> bool {
        Self::due(key, &self.week_key, self.week_at, now)
    }

    pub fn mark_week(&mut self, key: String, now: u64) {
        self.week_key = key;
        self.week_at = now;
    }
}

/// Sends a desktop notification and always records it in a log file, so the
/// reminder is not lost when no notification daemon is available.
pub fn send(summary: &str, body: &str) {
    log(body);
    let mut command = Command::new("notify-send");
    command.args(["--app-name", "finCAL"]);
    if let Some(icon) = paths::icon_path() {
        command.arg("--icon").arg(icon);
    }
    let shown = command
        .arg(summary)
        .arg(body)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !shown {
        eprintln!("finCAL notification: {body}");
    }
}

fn log(body: &str) {
    let dir = paths::data_dir();
    let _ = fs::create_dir_all(&dir);
    let line = format!("{} {}\n", util::timestamp(util::now_unix()), body);
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::notify_log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}
