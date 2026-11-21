//! Line-delimited JSON over a Unix socket between the TUI and the service.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Status,
    Reload,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub month: String,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub sal: Option<f64>,
    #[serde(default)]
    pub mtotal: Option<f64>,
    #[serde(default)]
    pub left: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn encode(request: &Request) -> String {
    serde_json::to_string(request).unwrap_or_default()
}

pub fn decode_request(line: &str) -> Option<Request> {
    serde_json::from_str(line.trim()).ok()
}

pub fn decode_status(line: &str) -> Option<Status> {
    serde_json::from_str(line.trim()).ok()
}

/// Asks a running service for its status. Returns `None` when it is not up.
pub fn status() -> Option<Status> {
    send(&Request::Status)
}

/// Tells a running service that the template changed on disk.
pub fn reload() -> bool {
    send(&Request::Reload).is_some()
}

fn send(request: &Request) -> Option<Status> {
    let stream = UnixStream::connect(paths::socket_path()).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    let mut writer = stream.try_clone().ok()?;
    writer.write_all(encode(request).as_bytes()).ok()?;
    writer.write_all(b"\n").ok()?;
    writer.flush().ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    decode_status(&line)
}
