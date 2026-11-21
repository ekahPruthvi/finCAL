//! Reading and writing the Logseq budget template.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::expr::{self, Expr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Month,
    Week,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub raw_cost: String,
    pub expr: Expr,
    pub editable: bool,
    pub is_total: bool,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub period: Period,
    pub rows: Vec<Row>,
    pub assignments: Vec<Assignment>,
    pub sal_raw: String,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub path: PathBuf,
    pub sheets: Vec<Sheet>,
}

fn is_total_label(label: &str) -> bool {
    let upper = label.trim().to_uppercase();
    if matches!(upper.as_str(), "MTOTAL" | "TOTAL" | "WNT") {
        return true;
    }
    let bytes = upper.as_bytes();
    bytes.len() == 3
        && bytes[0] == b'W'
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'T'
}

/// Trimmed byte ranges of every `|` separated cell on a table line.
pub(crate) fn cell_spans(line: &str) -> Vec<(usize, usize)> {
    let pipes: Vec<usize> = line
        .match_indices('|')
        .map(|(index, _)| index)
        .collect();
    let mut spans = Vec::new();
    for pair in pipes.windows(2) {
        let (start, end) = (pair[0] + 1, pair[1]);
        if start > end {
            continue;
        }
        let raw = &line[start..end];
        let leading = raw.len() - raw.trim_start().len();
        let trailing = raw.len() - raw.trim_end().len();
        let s = start + leading;
        let e = end.saturating_sub(trailing);
        spans.push((s, e.max(s)));
    }
    spans
}

pub(crate) fn is_separator(cells: &[&str]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, '-' | ':' | '='))
        })
}

fn header_name(line: &str) -> Option<String> {
    let trimmed = strip_bullet(line);
    let rest = trimmed.strip_prefix("##")?;
    if rest.starts_with('#') {
        return None;
    }
    let name = rest.trim().trim_end_matches(':').trim();
    if name.to_lowercase().contains("cms") {
        Some(name.to_string())
    } else {
        None
    }
}

fn period_of(name: &str) -> Period {
    if name.to_lowercase().contains("week") {
        Period::Week
    } else {
        Period::Month
    }
}

/// Extracts the trimmed value range after the first `=` on a line.
pub(crate) fn value_span(line: &str) -> Option<(usize, usize)> {
    let eq = line.find('=')?;
    let raw = &line[eq + 1..];
    let leading = raw.len() - raw.trim_start().len();
    let trailing = raw.len() - raw.trim_end().len();
    let start = eq + 1 + leading;
    let end = line.len().saturating_sub(trailing);
    Some((start, end.max(start)))
}

fn strip_bullet(line: &str) -> &str {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("- ").unwrap_or(trimmed).trim()
}

fn sanitize_name(left: &str) -> Option<String> {
    let cleaned: String = left
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if cleaned.is_empty()
        || !cleaned
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '#')
    {
        return None;
    }
    Some(cleaned.to_uppercase())
}

impl Template {
    pub fn load(path: &Path) -> io::Result<Template> {
        let content = fs::read_to_string(path)?;
        Template::parse(path, &content).map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
    }

    pub fn parse(path: &Path, content: &str) -> Result<Template, String> {
        let lines: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();

        let mut sheets: Vec<Sheet> = Vec::new();
        let mut current: Option<usize> = None;

        for line in lines.iter() {
            if let Some(name) = header_name(line) {
                sheets.push(Sheet {
                    period: period_of(&name),
                    name,
                    rows: Vec::new(),
                    assignments: Vec::new(),
                    sal_raw: String::new(),
                });
                current = Some(sheets.len() - 1);
                continue;
            }
            if strip_bullet(line).starts_with("## ") {
                current = None;
                continue;
            }
            let Some(sheet_index) = current else {
                continue;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed.starts_with('|') {
                let spans = cell_spans(line);
                if spans.len() < 2 {
                    continue;
                }
                let cells: Vec<&str> = spans.iter().map(|(s, e)| &line[*s..*e]).collect();
                if is_separator(&cells) {
                    continue;
                }
                let label = cells[0].trim();
                if label.is_empty() || label.eq_ignore_ascii_case("sub") {
                    continue;
                }
                let raw_cost = cells[1].trim().to_string();
                let sheet = &mut sheets[sheet_index];
                let expr = match expr::parse(&raw_cost) {
                    Ok(expr) => expr,
                    Err(_) => continue,
                };
                let is_total = is_total_label(label);
                sheet.rows.push(Row {
                    label: label.to_string(),
                    editable: !is_total && expr::plain_amount(&raw_cost).is_some(),
                    expr,
                    raw_cost,
                    is_total,
                });
                continue;
            }

            let content_line = strip_bullet(trimmed);
            let upper = content_line.to_uppercase();
            if upper.starts_with("SAL") && content_line.contains('=') {
                let span = value_span(content_line);
                let sheet = &mut sheets[sheet_index];
                sheet.sal_raw = span
                    .map(|(s, e)| content_line[s..e].trim().to_string())
                    .unwrap_or_default();
                continue;
            }

            if content_line.contains('=') && !content_line.contains("::") {
                let Some(span) = value_span(content_line) else {
                    continue;
                };
                let left = &content_line[..content_line.find('=').unwrap_or(0)];
                let Some(name) = sanitize_name(left) else {
                    continue;
                };
                let value = content_line[span.0..span.1].trim();
                let Ok(expr) = expr::parse(value) else {
                    continue;
                };
                sheets[sheet_index].assignments.push(Assignment { name, expr });
            }
        }

        if sheets.is_empty() {
            return Err(format!(
                "{} has no '## ... CMS' sections",
                path.display()
            ));
        }

        Ok(Template {
            path: path.to_path_buf(),
            sheets,
        })
    }

    pub fn sal_value(&self, sheet_index: usize) -> Option<f64> {
        self.sheets
            .get(sheet_index)
            .and_then(|sheet| expr::plain_amount(&sheet.sal_raw))
    }
}

pub fn format_number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        let text = format!("{:.2}", value);
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
