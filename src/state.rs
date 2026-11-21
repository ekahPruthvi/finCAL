//! Persistent monthly history, used to feed `LEFT from Prev Month`.

use std::collections::BTreeMap;
use std::fs;
use std::io;

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub month: String,
    pub sal: Option<f64>,
    pub mtotal: Option<f64>,
    pub left: Option<f64>,
    pub updated: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub months: BTreeMap<String, Snapshot>,
}

impl History {
    pub fn load() -> History {
        let path = paths::history_path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let dir = paths::data_dir();
        fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self)?;
        fs::write(paths::history_path(), text)
    }

    pub fn record_month(
        &mut self,
        month: &str,
        sal: Option<f64>,
        mtotal: Option<f64>,
        left: Option<f64>,
        updated: u64,
    ) {
        let snapshot = Snapshot {
            month: month.to_string(),
            sal,
            mtotal,
            left,
            updated,
        };
        self.months.insert(month.to_string(), snapshot);
        while self.months.len() > 24 {
            let oldest = self.months.keys().next().cloned();
            match oldest {
                Some(key) => {
                    self.months.remove(&key);
                }
                None => break,
            }
        }
    }

    /// LEFT carried over from the month before `month`.
    pub fn left_before(&self, month: &str) -> Option<f64> {
        self.months
            .get(&crate::util::previous_month(month))
            .and_then(|snapshot| snapshot.left)
    }
}
