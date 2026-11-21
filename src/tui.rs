//! The interactive terminal front end.

use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row as TuiRow, Table, TableState};
use ratatui::{Frame, Terminal};

use crate::budget::{self, Report};
use crate::ipc;
use crate::journal::{Journal, RowRef, Summary};
use crate::state::History;
use crate::template::{self, Period, Template};
use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    Sal,
    Row(RowRef),
}

#[derive(Debug, Clone)]
struct Edit {
    target: EditTarget,
    buffer: String,
}

#[derive(Debug, Clone)]
enum Step {
    Salary { default: f64 },
    Cost { label: String, default: f64 },
}

#[derive(Debug, Clone)]
struct Setup {
    steps: Vec<Step>,
    index: usize,
    buffer: String,
    sal: f64,
    overrides: HashMap<String, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DerivedKind {
    Total,
    Left,
}

#[derive(Debug, Clone)]
enum ItemKind {
    Section(String),
    ColumnHeader,
    Row {
        target: Option<RowRef>,
        label: String,
        raw: String,
        value: Option<f64>,
        bold: bool,
        editable: bool,
    },
    Derived {
        label: String,
        value: Option<f64>,
        kind: DerivedKind,
    },
    Spacer,
}

struct App {
    template: Template,
    monthly_sheet: Option<usize>,
    weekly_sheet: Option<usize>,
    journal: Journal,
    journal_path: PathBuf,
    history: History,
    month: String,
    summary: Summary,
    items: Vec<ItemKind>,
    selected: usize,
    editing: Option<Edit>,
    setup: Option<Setup>,
    message: String,
    service: Option<ipc::Status>,
    quit: bool,
    last_poll: Instant,
}

pub fn run(template_path: PathBuf, journal_path: PathBuf) -> Result<(), String> {
    let template = Template::load(&template_path)
        .map_err(|error| format!("{}: {error}", template_path.display()))?;
    let monthly_sheet = template
        .sheets
        .iter()
        .position(|sheet| sheet.period == Period::Month);
    if monthly_sheet.is_none() {
        return Err(format!(
            "{} has no monthly CMS section",
            template_path.display()
        ));
    }
    let weekly_sheet = template
        .sheets
        .iter()
        .position(|sheet| sheet.period == Period::Week);

    let mut app = App {
        template,
        monthly_sheet,
        weekly_sheet,
        journal: Journal::load(&journal_path),
        journal_path,
        history: History::load(),
        month: util::current_month(),
        summary: Summary::default(),
        items: Vec::new(),
        selected: 0,
        editing: None,
        setup: None,
        message: String::new(),
        service: None,
        quit: false,
        last_poll: Instant::now() - Duration::from_secs(10),
    };
    app.rebuild();
    app.poll_service();

    if app.journal.month(&app.month).is_none() {
        app.start_setup();
        app.message =
            "no journal block for this month — answer the setup prompts (Esc to skip)".into();
    } else {
        app.ensure_week(false);
        app.message = if app.service.is_some() {
            "journal ready".into()
        } else {
            "journal ready — start reminders with: finCAL -s".into()
        };
    }

    enable_raw_mode().map_err(|error| error.to_string())?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|error| error.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result.map_err(|error| error.to_string())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    let mut state = TableState::default();
    while !app.quit {
        terminal.draw(|frame| app.draw(frame, &mut state))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}

impl App {
    fn reload_journal(&mut self) {
        self.journal = Journal::load(&self.journal_path);
        self.history = History::load();
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.month = util::current_month();
        let journal_summary = self.journal.summary(&self.month);
        let has_month = self.journal.month(&self.month).is_some();
        self.summary = if has_month {
            journal_summary
        } else {
            let fallback = budget::compute(&self.template, self.history.left_before(&self.month));
            Summary {
                sal: fallback.monthly().and_then(|sheet| sheet.sal),
                month_total: fallback.monthly_total().unwrap_or(0.0),
                week_totals: Vec::new(),
                mtotal: fallback.monthly_total().unwrap_or(0.0),
                left: fallback.monthly_left(),
            }
        };
        self.items = self.build_items(has_month);
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
        if !matches!(
            self.items.get(self.selected),
            Some(ItemKind::Row { target: Some(_), .. })
        ) {
            self.selected = self.first_selectable().unwrap_or(0);
        }
    }

    fn build_items(&self, has_month: bool) -> Vec<ItemKind> {
        let mut items = Vec::new();
        if has_month {
            let Some(month_index) = self.journal.month_index(&self.month) else {
                return items;
            };
            let month = &self.journal.months[month_index];
            items.push(ItemKind::Section(month.name.clone()));
            items.push(ItemKind::ColumnHeader);
            for (row_index, row) in month.rows.iter().enumerate() {
                items.push(ItemKind::Row {
                    target: Some(RowRef::Month {
                        month: month_index,
                        row: row_index,
                    }),
                    label: row.label.clone(),
                    raw: row.raw.clone(),
                    value: row.value,
                    bold: false,
                    editable: self.is_editable(Period::Month, &row.label),
                });
            }
            items.push(ItemKind::Derived {
                label: "MTotal".into(),
                value: Some(self.summary.mtotal),
                kind: DerivedKind::Total,
            });
            items.push(ItemKind::Derived {
                label: "LEFT".into(),
                value: self.summary.left,
                kind: DerivedKind::Left,
            });

            for (week_index, week) in month.weeks.iter().enumerate() {
                items.push(ItemKind::Spacer);
                items.push(ItemKind::Section(format!("Week{}", week.number)));
                items.push(ItemKind::ColumnHeader);
                for (row_index, row) in week.rows.iter().enumerate() {
                    let total = row.label.trim().eq_ignore_ascii_case("wnt");
                    items.push(ItemKind::Row {
                        target: if total {
                            None
                        } else {
                            Some(RowRef::Week {
                                month: month_index,
                                week: week_index,
                                row: row_index,
                            })
                        },
                        label: row.label.clone(),
                        raw: row.raw.clone(),
                        value: row.value,
                        bold: total,
                        editable: !total && self.is_editable(Period::Week, &row.label),
                    });
                }
            }
        } else {
            let Some(sheet_index) = self.monthly_sheet else {
                return items;
            };
            let sheet = &self.template.sheets[sheet_index];
            items.push(ItemKind::Section(format!(
                "{} (template — press i to start the month)",
                sheet.name
            )));
            items.push(ItemKind::ColumnHeader);
            let report: Report = budget::compute(&self.template, self.history.left_before(&self.month));
            for (row_index, row) in sheet.rows.iter().enumerate() {
                if row.is_total {
                    continue;
                }
                let value = report
                    .sheets
                    .get(sheet_index)
                    .and_then(|sheet| sheet.rows.get(row_index))
                    .and_then(|row| row.value);
                items.push(ItemKind::Row {
                    target: None,
                    label: row.label.clone(),
                    raw: row.raw_cost.clone(),
                    value,
                    bold: false,
                    editable: false,
                });
            }
            items.push(ItemKind::Derived {
                label: "MTotal".into(),
                value: Some(self.summary.mtotal),
                kind: DerivedKind::Total,
            });
            items.push(ItemKind::Derived {
                label: "LEFT".into(),
                value: self.summary.left,
                kind: DerivedKind::Left,
            });
        }
        items
    }

    fn is_editable(&self, period: Period, label: &str) -> bool {
        let Some(index) = self
            .template
            .sheets
            .iter()
            .position(|sheet| sheet.period == period)
        else {
            return true;
        };
        match self.template.sheets[index]
            .rows
            .iter()
            .find(|row| row.label.eq_ignore_ascii_case(label))
        {
            Some(row) => row.editable,
            None => true,
        }
    }

    fn first_selectable(&self) -> Option<usize> {
        self.items
            .iter()
            .position(|item| matches!(item, ItemKind::Row { target: Some(_), .. }))
    }

    fn move_selection(&mut self, forward: bool) {
        if self.items.is_empty() {
            return;
        }
        let step = if forward { 1 } else { self.items.len() - 1 };
        let mut index = self.selected;
        for _ in 0..self.items.len() {
            index = (index + step) % self.items.len();
            if matches!(self.items[index], ItemKind::Row { target: Some(_), .. }) {
                self.selected = index;
                return;
            }
        }
    }

    fn poll_service(&mut self) {
        self.service = ipc::status();
    }

    fn tick(&mut self) {
        if self.last_poll.elapsed() >= Duration::from_secs(2) {
            self.last_poll = Instant::now();
            self.poll_service();
        }
    }

    fn reload(&mut self) {
        match Template::load(&self.template.path) {
            Ok(template) => {
                self.template = template;
                self.monthly_sheet = self
                    .template
                    .sheets
                    .iter()
                    .position(|sheet| sheet.period == Period::Month);
                self.weekly_sheet = self
                    .template
                    .sheets
                    .iter()
                    .position(|sheet| sheet.period == Period::Week);
                self.reload_journal();
                let synced = ipc::reload();
                self.message = format!(
                    "reloaded template{}",
                    if synced { " and notified the service" } else { "" }
                );
            }
            Err(error) => self.message = format!("reload failed: {error}"),
        }
    }

    /// Adds the current week's report block on Friday through Sunday.
    fn ensure_week(&mut self, announce: bool) {
        let Some(weekly) = self.weekly_sheet else {
            return;
        };
        if self.journal.month(&self.month).is_none() || !util::is_report_day(util::now_unix()) {
            return;
        }
        let week = util::week_of_month(util::now_unix());
        match self
            .journal
            .ensure_week(&self.template, weekly, &self.month, week)
        {
            Ok(true) => {
                self.reload_journal();
                if announce {
                    self.message = format!("added Week{} to {}", week, self.journal_path.display());
                }
            }
            Ok(false) => {}
            Err(error) => self.message = format!("cannot add week: {error}"),
        }
    }

    fn start_setup(&mut self) {
        if self.journal.month(&self.month).is_some() {
            self.message = format!("{} already exists in the journal", self.month);
            return;
        }
        let Some(monthly) = self.monthly_sheet else {
            return;
        };
        let default_sal = self
            .journal
            .sal
            .or_else(|| self.history.left_before(&self.month).map(|_| 0.0))
            .unwrap_or(0.0);
        let mut steps = vec![Step::Salary {
            default: default_sal,
        }];
        for row in &self.template.sheets[monthly].rows {
            if !row.editable {
                continue;
            }
            if let Some(default) = crate::expr::plain_amount(&row.raw_cost) {
                steps.push(Step::Cost {
                    label: row.label.clone(),
                    default,
                });
            }
        }
        let buffer = match &steps[0] {
            Step::Salary { default } => template::format_number(*default),
            _ => String::new(),
        };
        self.setup = Some(Setup {
            steps,
            index: 0,
            buffer,
            sal: 0.0,
            overrides: HashMap::new(),
        });
    }

    fn setup_input(&mut self, key: KeyEvent) {
        let Some(setup) = self.setup.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.setup = None;
                self.message = "setup cancelled — showing the template preview".into();
            }
            KeyCode::Backspace => {
                setup.buffer.pop();
            }
            KeyCode::Char(c)
                if c.is_ascii_digit() || c == '.' || c == '-' || c == ',' || c == '~' =>
            {
                setup.buffer.push(c);
            }
            KeyCode::Enter => {
                let cleaned: String = setup
                    .buffer
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != ',' && *c != '~')
                    .collect();
                let value = if cleaned.is_empty() {
                    None
                } else {
                    match cleaned.parse::<f64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            self.message = format!("'{}' is not a number", setup.buffer);
                            return;
                        }
                    }
                };
                let step = setup.steps[setup.index].clone();
                match step {
                    Step::Salary { default } => setup.sal = value.unwrap_or(default),
                    Step::Cost { label, default } => {
                        setup.overrides.insert(label, value.unwrap_or(default));
                    }
                }
                setup.index += 1;
                if setup.index >= setup.steps.len() {
                    self.finish_setup();
                } else {
                    let next = setup.steps[setup.index].clone();
                    setup.buffer = match next {
                        Step::Salary { default } => template::format_number(default),
                        Step::Cost { default, .. } => template::format_number(default),
                    };
                }
            }
            _ => {}
        }
    }

    fn finish_setup(&mut self) {
        let Some(setup) = self.setup.take() else {
            return;
        };
        let Some(monthly) = self.monthly_sheet else {
            return;
        };
        let name = util::month_name(&self.month);
        let left_prev = self.history.left_before(&self.month);
        match self.journal.create_month(
            &self.template,
            monthly,
            &name,
            &self.month,
            setup.sal,
            left_prev,
            &setup.overrides,
        ) {
            Ok(()) => {
                self.reload_journal();
                self.ensure_week(true);
                ipc::reload();
                self.message = format!(
                    "created {} in {} — edit costs with Enter",
                    name,
                    self.journal_path.display()
                );
            }
            Err(error) => self.message = format!("cannot create journal block: {error}"),
        }
    }

    fn start_edit(&mut self, row: Option<RowRef>) {
        if self.journal.month(&self.month).is_none() {
            self.start_setup();
            self.message = "create the month block first".into();
            return;
        }
        match row {
            None => {
                let current = self
                    .journal
                    .sal
                    .map(template::format_number)
                    .unwrap_or_default();
                self.editing = Some(Edit {
                    target: EditTarget::Sal,
                    buffer: current,
                });
            }
            Some(row_ref) => {
                let value = self
                    .row_value(row_ref)
                    .map(template::format_number)
                    .unwrap_or_default();
                self.editing = Some(Edit {
                    target: EditTarget::Row(row_ref),
                    buffer: value,
                });
            }
        }
    }

    fn row_value(&self, target: RowRef) -> Option<f64> {
        match target {
            RowRef::Month { month, row } => self
                .journal
                .months
                .get(month)
                .and_then(|block| block.rows.get(row))
                .and_then(|row| row.value),
            RowRef::Week { month, week, row } => self
                .journal
                .months
                .get(month)
                .and_then(|block| block.weeks.get(week))
                .and_then(|block| block.rows.get(row))
                .and_then(|row| row.value),
        }
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let cleaned: String = edit
            .buffer
            .chars()
            .filter(|c| !c.is_whitespace() && *c != ',' && *c != '~')
            .collect();
        let Ok(value) = cleaned.parse::<f64>() else {
            self.message = format!("'{}' is not a number", edit.buffer);
            return;
        };

        let applied = match edit.target {
            EditTarget::Sal => {
                let left_prev = self.history.left_before(&self.month);
                self.journal
                    .set_sal(value)
                    .and_then(|()| self.journal.recompute_month(&self.template, &self.month, left_prev))
            }
            EditTarget::Row(target) => self.journal.set_row_value(target, value).and_then(|()| {
                if let RowRef::Week { week, .. } = target {
                    let number = self.journal.months
                        .iter()
                        .find(|block| block.key == self.month)
                        .and_then(|block| block.weeks.get(week))
                        .map(|block| block.number)
                        .unwrap_or(0);
                    if number > 0 {
                        self.journal.refresh_week_total(&self.month, number)?;
                    }
                }
                self.journal.write()
            }),
        };

        match applied {
            Ok(()) => {
                self.reload_journal();
                ipc::reload();
                self.message = format!("saved {}", self.journal_path.display());
            }
            Err(error) => self.message = format!("save failed: {error}"),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.setup.is_some() {
            self.setup_input(key);
            return;
        }
        if self.editing.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.editing = None;
                    self.message = "edit cancelled".into();
                }
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Backspace => {
                    if let Some(edit) = self.editing.as_mut() {
                        edit.buffer.pop();
                    }
                }
                KeyCode::Char(c)
                    if c.is_ascii_digit() || c == '.' || c == '-' || c == ',' || c == '~' =>
                {
                    if let Some(edit) = self.editing.as_mut() {
                        edit.buffer.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Tab => self.jump_section(true),
            KeyCode::BackTab => self.jump_section(false),
            KeyCode::Char('e') => self.start_edit(None),
            KeyCode::Char('i') => self.start_setup(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Enter => {
                let target = match self.items.get(self.selected) {
                    Some(ItemKind::Row {
                        target: Some(target),
                        editable,
                        ..
                    }) => {
                        if *editable {
                            Some(*target)
                        } else {
                            self.message = "that row is formula driven and read-only".into();
                            None
                        }
                    }
                    _ => {
                        self.message = "select a cost row first".into();
                        None
                    }
                };
                if let Some(target) = target {
                    self.start_edit(Some(target));
                }
            }
            _ => {}
        }
    }

    fn jump_section(&mut self, forward: bool) {
        let sections: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, ItemKind::Section(_)))
            .map(|(index, _)| index)
            .collect();
        if sections.is_empty() {
            return;
        }
        let current = self.selected;
        let next = if forward {
            sections
                .iter()
                .copied()
                .find(|index| *index > current)
                .or_else(|| sections.first().copied())
        } else {
            sections
                .iter()
                .copied()
                .rev()
                .find(|index| *index < current)
                .or_else(|| sections.last().copied())
        };
        if let Some(section) = next {
            if let Some(row) = self.items[section..]
                .iter()
                .position(|item| matches!(item, ItemKind::Row { target: Some(_), .. }))
            {
                self.selected = section + row;
            }
        }
    }

    fn prompt_line(&self) -> Option<Line<'static>> {
        if let Some(setup) = &self.setup {
            let step = setup.steps.get(setup.index)?;
            let text = match step {
                Step::Salary { .. } => format!(
                    " Salary for {}: {}_  (Enter accept · Esc skip)",
                    util::month_name(&self.month),
                    setup.buffer
                ),
                Step::Cost { label, .. } => format!(
                    " {}: {}_  (Enter accept · Esc skip)",
                    label, setup.buffer
                ),
            };
            return Some(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Black).bg(Color::Yellow),
            )));
        }
        if let Some(edit) = &self.editing {
            let label = match edit.target {
                EditTarget::Sal => "SAL".to_string(),
                EditTarget::Row(RowRef::Month { month, row }) => self
                    .journal
                    .months
                    .get(month)
                    .and_then(|block| block.rows.get(row))
                    .map(|row| row.label.clone())
                    .unwrap_or_else(|| "cost".into()),
                EditTarget::Row(RowRef::Week { month, week, row }) => self
                    .journal
                    .months
                    .get(month)
                    .and_then(|block| block.weeks.get(week))
                    .and_then(|block| block.rows.get(row))
                    .map(|row| row.label.clone())
                    .unwrap_or_else(|| "cost".into()),
            };
            return Some(Line::from(vec![
                Span::styled(
                    format!(" {label} = "),
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                ),
                Span::raw(format!("{}_", edit.buffer)),
                Span::styled(
                    "   Enter save · Esc cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &mut TableState) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_summary(frame, chunks[1]);
        self.draw_table(frame, chunks[2], state);
        self.draw_message(frame, chunks[3]);
        self.draw_footer(frame, chunks[4]);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(42)])
            .split(area);
        let title = Line::from(vec![
            Span::styled(
                " finCAL ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(util::month_name(&self.month), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(
                self.journal_path.display().to_string(),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(title), halves[0]);

        let (text, color) = match &self.service {
            Some(status) if status.ok => (
                format!("service pid {} · {}", status.pid, util::timestamp(status.updated)),
                Color::Green,
            ),
            Some(status) => (
                status
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("service pid {} error", status.pid)),
                Color::Red,
            ),
            None => ("service stopped".to_string(), Color::Red),
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{text} "),
                Style::default().fg(color),
            )))
            .alignment(ratatui::layout::Alignment::Right),
            halves[1],
        );
    }

    fn draw_summary(&self, frame: &mut Frame, area: Rect) {
        let sal = self
            .summary
            .sal
            .map(util::money)
            .unwrap_or_else(|| "unset".into());
        let total = util::money(self.summary.mtotal);
        let left = self
            .summary
            .left
            .map(util::money)
            .unwrap_or_else(|| "—".into());
        let left_style = if self.summary.left.unwrap_or(0.0) < 0.0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![
            Span::styled(" SAL ", Style::default().fg(Color::DarkGray)),
            Span::styled(sal, Style::default().fg(Color::White)),
            Span::styled("   MTotal ", Style::default().fg(Color::DarkGray)),
            Span::styled(total, Style::default().fg(Color::White)),
            Span::styled("   LEFT ", Style::default().fg(Color::DarkGray)),
            Span::styled(left, left_style),
        ];
        if !self.summary.week_totals.is_empty() {
            let weeks = self
                .summary
                .week_totals
                .iter()
                .map(|(week, total)| format!("W{week} {}", util::money(*total)))
                .collect::<Vec<_>>()
                .join("  ");
            spans.push(Span::styled(
                format!("   {weeks}"),
                Style::default().fg(Color::Gray),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect, state: &mut TableState) {
        let mut rows: Vec<TuiRow> = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            let selected = index == self.selected
                && matches!(item, ItemKind::Row { target: Some(_), .. });
            let highlight = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            match item {
                ItemKind::Section(name) => rows.push(TuiRow::new(vec![
                    Cell::from(format!(" {name} ")).style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(""),
                    Cell::from(""),
                ])),
                ItemKind::ColumnHeader => rows.push(
                    TuiRow::new(vec!["  Sub", "Cost", "Value"])
                        .style(Style::default().fg(Color::DarkGray)),
                ),
                ItemKind::Row {
                    label,
                    raw,
                    value,
                    bold,
                    ..
                } => {
                    let mut style = highlight;
                    if *bold && !selected {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    rows.push(
                        TuiRow::new(vec![
                            Cell::from(format!("  {label}")),
                            Cell::from(raw.clone()),
                            Cell::from(
                                value
                                    .map(util::money)
                                    .unwrap_or_else(|| "—".into()),
                            ),
                        ])
                        .style(style),
                    );
                }
                ItemKind::Derived { label, value, kind } => {
                    let value = value.map(util::money).unwrap_or_else(|| "—".into());
                    let style = match kind {
                        DerivedKind::Total => Style::default().add_modifier(Modifier::BOLD),
                        DerivedKind::Left => {
                            if self.summary.left.unwrap_or(0.0) < 0.0 {
                                Style::default()
                                    .fg(Color::Red)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD)
                            }
                        }
                    };
                    rows.push(
                        TuiRow::new(vec![
                            Cell::from(format!("  {label}")),
                            Cell::from(""),
                            Cell::from(value),
                        ])
                        .style(style),
                    );
                }
                ItemKind::Spacer => rows.push(TuiRow::new(vec!["", "", ""])),
            }
        }

        state.select(Some(self.selected));
        let table = Table::new(
            rows,
            [
                Constraint::Percentage(44),
                Constraint::Percentage(30),
                Constraint::Percentage(26),
            ],
        )
        .block(Block::default().borders(Borders::ALL).title(" Budget "))
        .column_spacing(2);
        frame.render_stateful_widget(table, area, state);
    }

    fn draw_message(&self, frame: &mut Frame, area: Rect) {
        let line = if let Some(prompt) = self.prompt_line() {
            prompt
        } else {
            Line::from(Span::styled(
                format!(" {}", self.message),
                Style::default().fg(Color::Gray),
            ))
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let hints = Line::from(vec![
            Span::styled(" q", Style::default().fg(Color::Cyan)),
            Span::raw(" quit  "),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::raw(" section  "),
            Span::styled("↑↓/jk", Style::default().fg(Color::Cyan)),
            Span::raw(" select  "),
            Span::styled("e", Style::default().fg(Color::Cyan)),
            Span::raw(" SAL  "),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::raw(" edit  "),
            Span::styled("i", Style::default().fg(Color::Cyan)),
            Span::raw(" new month  "),
            Span::styled("r", Style::default().fg(Color::Cyan)),
            Span::raw(" reload"),
        ]);
        frame.render_widget(Paragraph::new(hints), area);
    }
}
