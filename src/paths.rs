use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Default Logseq page used as the budget template.
pub fn template_path() -> PathBuf {
    if let Ok(path) = std::env::var("FINCAL_TEMPLATE") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    home().join("Projects/logseq/pages/Fin_Template.md")
}

/// The Logseq journal page that finCAL writes month/week blocks into.
pub fn journal_path() -> PathBuf {
    if let Ok(path) = std::env::var("FINCAL_JOURNAL") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    home().join("Projects/logseq/pages/Fin_journal.md")
}

/// `$XDG_DATA_HOME/fincal` or `~/.local/share/fincal`.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("fincal");
        }
    }
    home().join(".local/share/fincal")
}

pub fn socket_path() -> PathBuf {
    data_dir().join("fincal.sock")
}

pub fn pid_path() -> PathBuf {
    data_dir().join("fincal.pid")
}

pub fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

pub fn history_path() -> PathBuf {
    data_dir().join("history.json")
}

pub fn notify_path() -> PathBuf {
    data_dir().join("notify.json")
}

pub fn notify_log_path() -> PathBuf {
    data_dir().join("notifications.log")
}

/// The SVG app icon, used for desktop notifications.
pub fn icon_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FINCAL_ICON") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icon/finCAL.svg");
    if bundled.exists() {
        return Some(bundled);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("icon/finCAL.svg");
            if beside.exists() {
                return Some(beside);
            }
            let beside = dir.join("finCAL.svg");
            if beside.exists() {
                return Some(beside);
            }
        }
    }
    None
}
