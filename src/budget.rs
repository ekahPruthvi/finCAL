//! Turns a parsed template into concrete amounts.

use std::collections::HashMap;

use crate::template::{Period, Template};

#[derive(Debug, Clone, Default)]
pub struct RowReport {
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct AssignmentReport {
    pub name: String,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct SheetReport {
    pub period: Period,
    pub sal: Option<f64>,
    pub rows: Vec<RowReport>,
    pub assignments: Vec<AssignmentReport>,
    /// Value of the sheet's grand-total row.
    pub total: Option<f64>,
}

impl SheetReport {
    /// The value of a top-level assignment such as `LEFT`.
    pub fn assignment(&self, name: &str) -> Option<f64> {
        self.assignments
            .iter()
            .find(|assignment| assignment.name.eq_ignore_ascii_case(name))
            .and_then(|assignment| assignment.value)
    }
}

impl Default for Period {
    fn default() -> Self {
        Period::Month
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub sheets: Vec<SheetReport>,
}

impl Report {
    pub fn monthly(&self) -> Option<&SheetReport> {
        self.sheets
            .iter()
            .find(|sheet| sheet.period == Period::Month)
    }

    pub fn monthly_left(&self) -> Option<f64> {
        self.monthly().and_then(|sheet| sheet.assignment("LEFT"))
    }

    pub fn monthly_total(&self) -> Option<f64> {
        self.monthly().and_then(|sheet| sheet.total)
    }
}

/// Variables available to every expression. Weekly totals come from the
/// template's `W1T`..`W5T` slots and default to zero because the page holds a
/// single reusable week block.
fn base_vars(sal: Option<f64>, left_prev: Option<f64>) -> HashMap<String, f64> {
    let mut vars = HashMap::new();
    vars.insert("SAL".to_string(), sal.unwrap_or(0.0));
    vars.insert("LEFT_PREV".to_string(), left_prev.unwrap_or(0.0));
    vars.insert("TOTAL".to_string(), 0.0);
    vars.insert("MTOTAL".to_string(), 0.0);
    vars.insert("LEFT".to_string(), 0.0);
    vars.insert("WNT".to_string(), 0.0);
    for week in 1..=5 {
        vars.insert(format!("W{}T", week), 0.0);
    }
    vars
}

pub fn compute(template: &Template, left_prev: Option<f64>) -> Report {
    let mut report = Report { sheets: Vec::new() };

    // `SAL` is declared once (in the monthly block) but used by every sheet.
    let shared_sal = (0..template.sheets.len()).find_map(|index| template.sal_value(index));

    for (sheet_index, sheet) in template.sheets.iter().enumerate() {
        let sal = template.sal_value(sheet_index).or(shared_sal);
        let mut vars = base_vars(sal, left_prev);
        let mut sum = 0.0;
        let mut rows = Vec::new();
        let mut total = None;

        for row in &sheet.rows {
            let value = row.expr.eval(&vars).ok();
            if row.is_total {
                if let Some(value) = value {
                    total = Some(value);
                    vars.insert(row.label.trim().to_uppercase(), value);
                }
            } else if let Some(value) = value {
                sum += value;
            }
            vars.insert("TOTAL".to_string(), sum);
            rows.push(RowReport { value });
        }

        let mut assignments = Vec::new();
        for assignment in &sheet.assignments {
            let value = assignment.expr.eval(&vars).ok();
            if let Some(value) = value {
                vars.insert(assignment.name.clone(), value);
            }
            assignments.push(AssignmentReport {
                name: assignment.name.clone(),
                value,
            });
        }

        report.sheets.push(SheetReport {
            period: sheet.period,
            sal,
            rows,
            assignments,
            total,
        });
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SAMPLE: &str = "## PerMonth CMS \n\
title:: Fin_Template\n\
\t- SAL = 10000\n\
\t  | Sub | Cost |\n\
\t  |---| --- |\n\
\t  | Spotify | 70 |\n\
\t  | Electricity | ~500 |\n\
\t  | Water bill | ~500 |\n\
\t  | Wifi | ~250 |\n\
\t  | Home Items / Products | 5% of SAL |\n\
\t  | Savings Account #wishlist | 20% of SAL + LEFT from Prev Month|\n\
\t  | Emergency | 10% of SAL |\n\
\t  | Investment | 10% of SAL |\n\
\t  | Bike R&M | 1,000 |\n\
\t  | MTotal | Total + W1T + W2T + W3T + W4T + W5T  |\n\
\t  LEFT = SAL - MTOTAL\n\
- ## PerWeek CMS\n\
\t- id:: 6ae36f04-8341-4406-b673-c2094714012c\n\
\t  | Sub | Cost |\n\
\t  | --- | --- |\n\
\t  | Petrol | ~1,000 |\n\
\t  | Groceries | ~500 |\n\
\t  | Leisure | 2% of SAL |\n\
\t  | Social/Dates | 4% of SAL |\n\
\t  | WnT | total |";

    fn sample() -> Template {
        Template::parse(Path::new("sample.md"), SAMPLE).expect("sample parses")
    }

    #[test]
    fn parses_both_sheets() {
        let template = sample();
        assert_eq!(template.sheets.len(), 2);
        assert_eq!(template.sheets[0].name, "PerMonth CMS");
        assert_eq!(template.sheets[1].period, Period::Week);
        assert_eq!(template.sheets[0].period, Period::Month);
        assert_eq!(template.sheets[0].rows.len(), 10);
        assert!(template.sheets[0].rows[9].is_total);
    }

    #[test]
    fn computes_monthly_budget() {
        let report = compute(&sample(), None);
        let monthly = report.monthly().expect("monthly sheet");
        assert_eq!(monthly.sal, Some(10_000.0));
        assert_eq!(monthly.total, Some(6_820.0));
        assert_eq!(report.monthly_left(), Some(3_180.0));
    }

    #[test]
    fn carries_left_from_previous_month() {
        let report = compute(&sample(), Some(500.0));
        let monthly = report.monthly().expect("monthly sheet");
        assert_eq!(monthly.total, Some(7_320.0));
        assert_eq!(report.monthly_left(), Some(2_680.0));
    }

    #[test]
    fn computes_weekly_budget() {
        let report = compute(&sample(), None);
        let weekly = &report.sheets[1];
        assert_eq!(weekly.total, Some(2_100.0));
    }

}
