//! The month/week budget blocks inside `Fin_journal.md`.
//!
//! Layout the app maintains:
//!
//! ```text
//! # Sal = 25000
//!
//! # November 2026
//! | Sub | Cost |
//! | --- | --- |
//! | Spotify | ~70 |
//! ...
//! \t- # Week1
//! \t  | Sub | Cost |
//! \t  | --- | --- |
//! \t  | Petrol | ~1000 |
//! \t  | WnT | ~2100 |
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::expr;
use crate::template::{self, Period, Template};
use crate::util;

#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub raw: String,
    pub value: Option<f64>,
    /// Zero-based line index in the journal file.
    pub line: usize,
    /// Byte range of the trimmed cost cell inside the line.
    pub cell: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct WeekBlock {
    pub number: u32,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone)]
pub struct MonthBlock {
    pub key: String,
    pub name: String,
    pub line: usize,
    /// Exclusive line index where the block ends.
    pub end: usize,
    pub rows: Vec<Row>,
    pub weeks: Vec<WeekBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    Month { month: usize, row: usize },
    Week { month: usize, week: usize, row: usize },
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub sal: Option<f64>,
    pub month_total: f64,
    pub week_totals: Vec<(u32, f64)>,
    pub mtotal: f64,
    pub left: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Journal {
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub sal: Option<f64>,
    pub sal_line: Option<usize>,
    pub sal_cell: Option<(usize, usize)>,
    pub months: Vec<MonthBlock>,
    trailing_newline: bool,
}

fn is_total_label(label: &str) -> bool {
    label.trim().eq_ignore_ascii_case("wnt") || label.trim().eq_ignore_ascii_case("mtotal")
}

fn week_number(line: &str) -> Option<u32> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("- #")?.trim();
    let lower = rest.to_lowercase();
    let digits = lower.strip_prefix("week")?.trim();
    digits.parse::<u32>().ok()
}

impl Journal {
    pub fn load(path: &Path) -> Journal {
        match fs::read_to_string(path) {
            Ok(content) => Journal::parse(path, &content),
            Err(_) => Journal::parse(path, ""),
        }
    }

    pub fn parse(path: &Path, content: &str) -> Journal {
        let trailing_newline = content.is_empty() || content.ends_with('\n');
        let lines: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        let mut journal = Journal {
            path: path.to_path_buf(),
            lines,
            sal: None,
            sal_line: None,
            sal_cell: None,
            months: Vec::new(),
            trailing_newline,
        };

        let mut current_month: Option<usize> = None;
        let mut current_week: Option<usize> = None;

        for index in 0..journal.lines.len() {
            let line = journal.lines[index].clone();
            let trimmed = line.trim();

            if let Some(rest) = trimmed.strip_prefix("# ") {
                let title = rest.trim();
                if title.to_lowercase().starts_with("sal") {
                    journal.sal_line = Some(index);
                    if let Some(span) = template::value_span(title) {
                        let origin = line.find(title).unwrap_or(0);
                        let value_text = title[span.0..span.1].trim();
                        journal.sal = expr::plain_amount(value_text);
                        journal.sal_cell = Some((span.0 + origin, span.1 + origin));
                    }
                    current_month = None;
                    current_week = None;
                    continue;
                }
                if let Some(key) = util::month_key_from_name(title) {
                    journal.months.push(MonthBlock {
                        key,
                        name: title.trim_end_matches(':').trim().to_string(),
                        line: index,
                        end: journal.lines.len(),
                        rows: Vec::new(),
                        weeks: Vec::new(),
                    });
                    current_month = Some(journal.months.len() - 1);
                    current_week = None;
                    continue;
                }
                current_month = None;
                current_week = None;
                continue;
            }

            if let Some(number) = week_number(&line) {
                if let Some(month_index) = current_month {
                    let month = &mut journal.months[month_index];
                    month.weeks.push(WeekBlock {
                        number,
                        rows: Vec::new(),
                    });
                    current_week = Some(month.weeks.len() - 1);
                }
                continue;
            }

            if trimmed.starts_with('|') {
                let spans = template::cell_spans(&line);
                if spans.len() < 2 {
                    continue;
                }
                let cells: Vec<&str> = spans.iter().map(|(s, e)| &line[*s..*e]).collect();
                if template::is_separator(&cells) {
                    continue;
                }
                let label = cells[0].trim();
                if label.is_empty() || label.eq_ignore_ascii_case("sub") {
                    continue;
                }
                let raw = cells[1].trim().to_string();
                let row = Row {
                    label: label.to_string(),
                    value: expr::plain_amount(&raw),
                    raw,
                    line: index,
                    cell: spans.get(1).copied(),
                };
                if let Some(month_index) = current_month {
                    let month = &mut journal.months[month_index];
                    match current_week {
                        Some(week_index) if week_index < month.weeks.len() => {
                            month.weeks[week_index].rows.push(row);
                        }
                        _ => month.rows.push(row),
                    }
                }
            }
        }

        for index in 0..journal.months.len() {
            let end = journal
                .months
                .get(index + 1)
                .map(|month| month.line)
                .unwrap_or(journal.lines.len());
            journal.months[index].end = end;
        }

        journal
    }

    pub fn text(&self) -> String {
        let mut content = self.lines.join("\n");
        if self.trailing_newline {
            if !content.ends_with('\n') {
                content.push('\n');
            }
        } else {
            while content.ends_with('\n') {
                content.pop();
            }
        }
        content
    }

    pub fn write(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
        }
        let mut content = self.text();
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        fs::write(&self.path, content).map_err(|error| format!("{}: {error}", self.path.display()))
    }

    fn reparse(&mut self) {
        let content = self.text();
        *self = Journal::parse(&self.path, &content);
    }

    pub fn month(&self, key: &str) -> Option<&MonthBlock> {
        self.months.iter().find(|month| month.key == key)
    }

    pub fn month_index(&self, key: &str) -> Option<usize> {
        self.months.iter().position(|month| month.key == key)
    }

    pub fn set_sal(&mut self, value: f64) -> Result<(), String> {
        let text = template::format_number(value);
        if let (Some(line_index), Some((start, end))) = (self.sal_line, self.sal_cell) {
            let line = self
                .lines
                .get_mut(line_index)
                .ok_or_else(|| "stale SAL line".to_string())?;
            if end <= line.len() && start <= end {
                line.replace_range(start..end, &text);
                self.sal = Some(value);
                return Ok(());
            }
        }
        self.lines.insert(0, String::new());
        self.lines.insert(0, format!("# Sal = {text}"));
        self.reparse();
        self.sal = Some(value);
        Ok(())
    }

    /// Appends the current month's block, seeding the template's monthly rows.
    /// `overrides` holds the values the user answered during setup.
    pub fn create_month(
        &mut self,
        template: &Template,
        sheet_index: usize,
        name: &str,
        key: &str,
        sal: f64,
        left_prev: Option<f64>,
        overrides: &HashMap<String, f64>,
    ) -> Result<(), String> {
        if self.month(key).is_some() {
            return Ok(());
        }
        // A fresh Logseq page holds a single empty bullet; drop it so the
        // generated page starts with the salary heading.
        if self
            .lines
            .iter()
            .all(|line| line.trim().is_empty() || line.trim() == "-")
        {
            self.lines.clear();
        }
        let sheet = template
            .sheets
            .get(sheet_index)
            .ok_or_else(|| "template has no monthly sheet".to_string())?;
        if sheet.period != Period::Month {
            return Err("selected template sheet is not monthly".into());
        }

        let mut vars: HashMap<String, f64> = HashMap::new();
        vars.insert("SAL".into(), sal);
        vars.insert("LEFT_PREV".into(), left_prev.unwrap_or(0.0));
        vars.insert("TOTAL".into(), 0.0);
        vars.insert("MTOTAL".into(), 0.0);

        let mut block = vec![
            String::new(),
            format!("# {}", name),
            "| Sub | Cost |".to_string(),
            "| --- | --- |".to_string(),
        ];
        for row in &sheet.rows {
            if row.is_total || row.label.eq_ignore_ascii_case("left") {
                continue;
            }
            let value = match overrides.get(&row.label) {
                Some(value) => *value,
                None if row.editable => expr::plain_amount(&row.raw_cost).unwrap_or(0.0),
                None => row.expr.eval(&vars).unwrap_or(0.0),
            };
            block.push(format!(
                "| {} | ~{} |",
                row.label,
                template::format_number(value)
            ));
        }

        self.set_sal(sal)?;
        self.lines.extend(block);
        self.reparse();
        self.write()
    }

    /// Adds an indented `- # WeekN` bullet, a Logseq child of the month, with
    /// the template's weekly rows. Returns false when the week already exists.
    pub fn ensure_week(
        &mut self,
        template: &Template,
        weekly_sheet: usize,
        month_key: &str,
        number: u32,
    ) -> Result<bool, String> {
        let (insert_at, exists) = {
            let month = self
                .month(month_key)
                .ok_or_else(|| format!("no journal block for {month_key}"))?;
            (
                month.end,
                month.weeks.iter().any(|week| week.number == number),
            )
        };
        if exists {
            return Ok(false);
        }

        let sheet = template
            .sheets
            .get(weekly_sheet)
            .ok_or_else(|| "template has no weekly sheet".to_string())?;
        let mut vars: HashMap<String, f64> = HashMap::new();
        vars.insert("SAL".into(), self.sal.unwrap_or(0.0));
        vars.insert("TOTAL".into(), 0.0);

        let mut block = vec![
            format!("\t- # Week{}", number),
            "\t  | Sub | Cost |".to_string(),
            "\t  | --- | --- |".to_string(),
        ];
        let mut sum = 0.0;
        for row in &sheet.rows {
            let value = if row.is_total {
                vars.insert("TOTAL".into(), sum);
                row.expr.eval(&vars).unwrap_or(sum)
            } else if row.editable {
                expr::plain_amount(&row.raw_cost).unwrap_or(0.0)
            } else {
                row.expr.eval(&vars).unwrap_or(0.0)
            };
            if !row.is_total {
                sum += value;
                vars.insert("TOTAL".into(), sum);
            }
            block.push(format!(
                "\t  | {} | ~{} |",
                row.label,
                template::format_number(value)
            ));
        }

        let mut insert_at = insert_at.min(self.lines.len());
        // Keep the bullet directly under the month table, not after the blank
        // line that separates blocks.
        while insert_at > 0 && self.lines[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        for (offset, line) in block.into_iter().enumerate() {
            self.lines.insert(insert_at + offset, line);
        }
        self.reparse();
        self.write()?;
        Ok(true)
    }

    pub fn set_row_value(&mut self, target: RowRef, value: f64) -> Result<(), String> {
        let row = match target {
            RowRef::Month { month, row } => self
                .months
                .get(month)
                .and_then(|block| block.rows.get(row))
                .cloned(),
            RowRef::Week { month, week, row } => self
                .months
                .get(month)
                .and_then(|block| block.weeks.get(week))
                .and_then(|block| block.rows.get(row))
                .cloned(),
        };
        let Some(row) = row else {
            return Err("no such journal row".into());
        };
        let approximate = row.raw.trim_start().starts_with('~');
        let text = format!(
            "{}{}",
            if approximate { "~" } else { "" },
            template::format_number(value)
        );
        let (start, end) = row.cell.ok_or_else(|| "row has no cost cell".to_string())?;
        let line = self
            .lines
            .get_mut(row.line)
            .ok_or_else(|| "stale journal line".to_string())?;
        if end > line.len() || start > end {
            return Err("stale journal cell".into());
        }
        line.replace_range(start..end, &text);

        // Keep the in-memory model in sync so a following save sees it.
        let row = match target {
            RowRef::Month { month, row } => self
                .months
                .get_mut(month)
                .and_then(|block| block.rows.get_mut(row)),
            RowRef::Week { month, week, row } => self
                .months
                .get_mut(month)
                .and_then(|block| block.weeks.get_mut(week))
                .and_then(|block| block.rows.get_mut(row)),
        };
        if let Some(row) = row {
            row.raw = text;
            row.value = Some(value);
        }
        Ok(())
    }

    /// Re-evaluates template formula rows (percentages, savings) for the
    /// current month and its weeks after the salary changed.
    pub fn recompute_month(
        &mut self,
        template: &Template,
        month_key: &str,
        left_prev: Option<f64>,
    ) -> Result<(), String> {
        let Some(month_index) = self.month_index(month_key) else {
            return Ok(());
        };
        let Some(monthly_sheet) = template
            .sheets
            .iter()
            .find(|sheet| sheet.period == Period::Month)
        else {
            return Ok(());
        };
        let weekly_sheet = template
            .sheets
            .iter()
            .find(|sheet| sheet.period == Period::Week);

        let mut vars: HashMap<String, f64> = HashMap::new();
        vars.insert("SAL".into(), self.sal.unwrap_or(0.0));
        vars.insert("LEFT_PREV".into(), left_prev.unwrap_or(0.0));
        vars.insert("TOTAL".into(), 0.0);
        vars.insert("MTOTAL".into(), 0.0);

        let mut edits: Vec<(usize, (usize, usize), String)> = Vec::new();

        for row in &self.months[month_index].rows {
            let Some(model) = monthly_sheet
                .rows
                .iter()
                .find(|model| model.label.eq_ignore_ascii_case(&row.label))
            else {
                continue;
            };
            if model.editable {
                continue;
            }
            if let Ok(value) = model.expr.eval(&vars) {
                if let Some(cell) = row.cell {
                    edits.push((
                        row.line,
                        cell,
                        format!("~{}", template::format_number(value)),
                    ));
                }
            }
        }

        if let Some(weekly_sheet) = weekly_sheet {
            for week in &self.months[month_index].weeks {
                let mut sum = 0.0;
                let mut week_edits: Vec<(usize, (usize, usize), f64)> = Vec::new();
                for row in &week.rows {
                    let Some(model) = weekly_sheet
                        .rows
                        .iter()
                        .find(|model| model.label.eq_ignore_ascii_case(&row.label))
                    else {
                        continue;
                    };
                    let value = if model.is_total {
                        sum
                    } else if model.editable {
                        row.value.unwrap_or(0.0)
                    } else {
                        model.expr.eval(&vars).unwrap_or(0.0)
                    };
                    if !model.is_total {
                        sum += value;
                        vars.insert("TOTAL".into(), sum);
                    }
                    if let Some(cell) = row.cell {
                        week_edits.push((row.line, cell, value));
                    }
                }
                for (line, cell, value) in week_edits {
                    edits.push((
                        line,
                        cell,
                        format!("~{}", template::format_number(value)),
                    ));
                }
            }
        }

        for (line_index, (start, end), text) in edits {
            if let Some(line) = self.lines.get_mut(line_index) {
                if end <= line.len() && start <= end {
                    line.replace_range(start..end, &text);
                }
            }
        }
        self.write()
    }

    /// Rewrites a week's `WnT` row from the other rows.
    pub fn refresh_week_total(&mut self, month_key: &str, week_number: u32) -> Result<(), String> {
        let Some(month_index) = self.month_index(month_key) else {
            return Ok(());
        };
        let mut edit = None;
        {
            let month = &self.months[month_index];
            let Some(week) = month.weeks.iter().find(|week| week.number == week_number) else {
                return Ok(());
            };
            let mut sum = 0.0;
            for row in &week.rows {
                if is_total_label(&row.label) {
                    continue;
                }
                sum += row.value.unwrap_or(0.0);
            }
            if let Some(row) = week.rows.iter().find(|row| is_total_label(&row.label)) {
                if let Some(cell) = row.cell {
                    edit = Some((row.line, cell, sum));
                }
            }
        }
        if let Some((line_index, (start, end), sum)) = edit {
            if let Some(line) = self.lines.get_mut(line_index) {
                if end <= line.len() && start <= end {
                    let text = format!("~{}", template::format_number(sum));
                    line.replace_range(start..end, &text);
                }
            }
            self.write()?;
        }
        Ok(())
    }

    pub fn summary(&self, month_key: &str) -> Summary {
        let Some(month) = self.month(month_key) else {
            return Summary {
                sal: self.sal,
                ..Summary::default()
            };
        };
        let month_total: f64 = month.rows.iter().filter_map(|row| row.value).sum();
        let mut week_totals = Vec::new();
        for week in &month.weeks {
            let total = week
                .rows
                .iter()
                .find(|row| is_total_label(&row.label))
                .and_then(|row| row.value)
                .unwrap_or_else(|| {
                    week.rows
                        .iter()
                        .filter(|row| !is_total_label(&row.label))
                        .filter_map(|row| row.value)
                        .sum()
                });
            week_totals.push((week.number, total));
        }
        let mtotal = month_total + week_totals.iter().map(|(_, total)| total).sum::<f64>();
        Summary {
            sal: self.sal,
            month_total,
            week_totals,
            mtotal,
            left: self.sal.map(|sal| sal - mtotal),
        }
    }
}

/// `finCAL --edit-sal N`: creates the month block when needed, stores the
/// salary and recalculates the derived (percentage) rows.
pub fn set_salary(template: &Template, path: &Path, value: f64) -> Result<String, String> {
    let monthly = template
        .sheets
        .iter()
        .position(|sheet| sheet.period == Period::Month)
        .ok_or_else(|| "template has no monthly CMS section".to_string())?;
    let month = util::current_month();
    let name = util::month_name(&month);
    let left_prev = crate::state::History::load().left_before(&month);
    let mut journal = Journal::load(path);
    journal.create_month(
        template,
        monthly,
        &name,
        &month,
        value,
        left_prev,
        &HashMap::new(),
    )?;
    journal.set_sal(value)?;
    journal.recompute_month(template, &month, left_prev)?;
    Ok(format!(
        "{name} salary set to {} in {}",
        util::money(value),
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = "## PerMonth CMS\n\
\t- SAL = XXX\n\
\t  | Sub | Cost |\n\
\t  |---| --- |\n\
\t  | Spotify | 70 |\n\
\t  | Home Items / Products | 5% of SAL |\n\
\t  | Savings Account #wishlist | 20% of SAL + LEFT from Prev Month|\n\
\t  | MTotal | Total + W1T + W2T + W3T + W4T + W5T |\n\
\t  LEFT = SAL - MTOTAL\n\
- ## PerWeek CMS\n\
\t  | Sub | Cost |\n\
\t  | --- | --- |\n\
\t  | Petrol | ~1,000 |\n\
\t  | Leisure | 2% of SAL |\n\
\t  | WnT | total |";

    fn template() -> Template {
        Template::parse(Path::new("template.md"), TEMPLATE).unwrap()
    }

    #[test]
    fn creates_month_with_computed_rows() {
        let template = template();
        let mut journal = Journal::parse(Path::new("journal.md"), "-");
        journal
            .create_month(&template, 0, "November 2026", "2026-11", 10_000.0, None, &HashMap::new())
            .unwrap();
        let text = journal.text();
        assert!(text.starts_with("# Sal = 10000"));
        assert!(text.contains("# November 2026"));
        assert!(text.contains("| Spotify | ~70 |"));
        assert!(text.contains("| Home Items / Products | ~500 |"));
        assert!(!text.contains("MTotal"));

        let summary = journal.summary("2026-11");
        assert_eq!(summary.mtotal, 2_570.0);
        assert_eq!(summary.left, Some(7_430.0));
    }

    #[test]
    fn adds_week_block_under_month() {
        let template = template();
        let mut journal = Journal::parse(Path::new("journal.md"), "");
        journal
            .create_month(&template, 0, "November 2026", "2026-11", 10_000.0, None, &HashMap::new())
            .unwrap();
        assert!(journal.ensure_week(&template, 1, "2026-11", 1).unwrap());
        let text = journal.text();
        assert!(text.contains("\t- # Week1\n"));
        assert!(text.contains("\t  | Petrol | ~1000 |"));
        assert!(text.contains("| WnT | ~1200 |"));
        assert!(!journal.ensure_week(&template, 1, "2026-11", 1).unwrap());

        let summary = journal.summary("2026-11");
        assert_eq!(summary.mtotal, 3_770.0);
    }

    #[test]
    fn edit_salary_recomputes_percentages() {
        let template = template();
        let mut journal = Journal::parse(Path::new("journal.md"), "");
        journal
            .create_month(&template, 0, "November 2026", "2026-11", 10_000.0, None, &HashMap::new())
            .unwrap();
        journal.set_sal(20_000.0).unwrap();
        journal.recompute_month(&template, "2026-11", None).unwrap();
        let text = journal.text();
        assert!(text.contains("# Sal = 20000"));
        assert!(text.contains("| Home Items / Products | ~1000 |"));
        assert!(text.contains("| Spotify | ~70 |"));
    }

    #[test]
    fn edits_single_row() {
        let template = template();
        let mut journal = Journal::parse(Path::new("journal.md"), "");
        journal
            .create_month(&template, 0, "November 2026", "2026-11", 10_000.0, None, &HashMap::new())
            .unwrap();
        journal
            .set_row_value(RowRef::Month { month: 0, row: 0 }, 123.0)
            .unwrap();
        assert!(journal.text().contains("| Spotify | ~123 |"));
    }
}
