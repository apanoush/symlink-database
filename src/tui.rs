//! Rendering and input handling for the interactive stats dashboard.
//!
//! Everything here is pure: [`draw`] only writes into a [`Frame`] and
//! [`handle_key`] only mutates [`App`]. There is no terminal I/O, so the whole
//! module is testable with `ratatui`'s [`TestBackend`](ratatui::backend::TestBackend).
//! The actual terminal lifecycle and event loop live in the binary
//! (`src/tui_run.rs`), keeping this library module free of `crossterm` setup.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, Block, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap};
use regex::{Regex, RegexBuilder};

use crate::stats::{Stats, format_epoch};

/// The dashboard views, in tab-bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Totals, broken gauge and a folder preview.
    Overview,
    /// Files with the most incoming links.
    Files,
    /// Folders by number of symlinks inside them (filterable with `/`).
    Folders,
    /// Target directories by incoming links (filterable with `/`).
    TargetDirs,
    /// Every broken symlink.
    Broken,
    /// How many targets receive 1, 2, ... links.
    Distribution,
    /// Symlinks ordered by first import time.
    Newest,
}

impl Tab {
    /// All tabs in display order.
    pub const ALL: [Tab; 7] = [
        Tab::Overview,
        Tab::Files,
        Tab::Folders,
        Tab::TargetDirs,
        Tab::Broken,
        Tab::Distribution,
        Tab::Newest,
    ];

    /// Short title shown in the tab bar.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Files => "Files",
            Tab::Folders => "Folders",
            Tab::TargetDirs => "Target dirs",
            Tab::Broken => "Broken",
            Tab::Distribution => "Distribution",
            Tab::Newest => "Newest",
        }
    }

    /// Position of this tab in [`Tab::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Whether the app is navigating or editing the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal navigation.
    Normal,
    /// Editing the `/` filter pattern.
    Filter,
}

/// Everything the UI needs: the view model plus navigation state.
pub struct App {
    /// Aggregated data, recomputed on refresh.
    pub stats: Stats,
    /// Scan root, shown in the header.
    pub root: String,
    /// Database path, shown in the header.
    pub database: String,
    /// Currently selected tab.
    pub tab: Tab,
    /// Folder grouping depth for the folder and target-dir views.
    pub depth: usize,
    /// Scroll offset for the active tab, in rows.
    pub scroll: usize,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Set when the user asks to quit.
    pub should_quit: bool,
    /// Input mode (navigation or filter editing).
    pub mode: Mode,
    /// The committed filter, shared by the filterable tabs.
    pub filter: Option<Regex>,
    /// The pattern being edited while in [`Mode::Filter`].
    pub filter_input: String,
    /// Compile error for the current `filter_input`, if any.
    pub filter_error: Option<String>,
}

/// Tabs whose rows can be filtered with `/`.
fn tab_is_filterable(tab: Tab) -> bool {
    matches!(tab, Tab::Files | Tab::Folders | Tab::TargetDirs)
}

impl App {
    /// Create an app from freshly collected [`Stats`].
    pub fn new(stats: Stats, root: String, database: String) -> Self {
        Self {
            depth: stats.depth,
            stats,
            root,
            database,
            tab: Tab::Overview,
            scroll: 0,
            show_help: false,
            should_quit: false,
            mode: Mode::Normal,
            filter: None,
            filter_input: String::new(),
            filter_error: None,
        }
    }

    /// Replace the view model (used on refresh) and keep the scroll in range.
    pub fn refresh(&mut self, stats: Stats) {
        self.depth = stats.depth;
        self.stats = stats;
        self.clamp_scroll();
    }

    /// Whether `text` passes the committed filter (always true when unset).
    fn matches(&self, text: &str) -> bool {
        self.filter.as_ref().is_none_or(|re| re.is_match(text))
    }

    /// Number of rows in the active tab after filtering.
    fn row_count(&self) -> usize {
        match self.tab {
            Tab::Overview => 0,
            Tab::Files => self
                .stats
                .top_targets
                .iter()
                .filter(|item| self.matches(&item.target))
                .count(),
            Tab::Folders => self
                .stats
                .folders
                .iter()
                .filter(|item| self.matches(&item.folder))
                .count(),
            Tab::TargetDirs => self
                .stats
                .target_dirs
                .iter()
                .filter(|item| self.matches(&item.folder))
                .count(),
            Tab::Broken => self.stats.broken.len(),
            Tab::Distribution => self.stats.links_per_target.len(),
            Tab::Newest => self.stats.newest.len(),
        }
    }

    fn max_scroll(&self) -> usize {
        self.row_count().saturating_sub(1)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// Filter the active tab's rows down to those matching the committed
    /// pattern. Non-filterable tabs return their rows unchanged.
    fn visible_top_targets(&self) -> Vec<&crate::stats::TargetStat> {
        self.stats
            .top_targets
            .iter()
            .filter(|item| self.matches(&item.target))
            .collect()
    }

    fn visible_folders(&self) -> Vec<&crate::stats::FolderStat> {
        self.stats
            .folders
            .iter()
            .filter(|item| self.matches(&item.folder))
            .collect()
    }

    fn visible_target_dirs(&self) -> Vec<&crate::stats::FolderStat> {
        self.stats
            .target_dirs
            .iter()
            .filter(|item| self.matches(&item.folder))
            .collect()
    }

    /// Compile `filter_input` (case-insensitive) into the committed filter.
    ///
    /// An empty pattern clears the filter. Returns the compile error, if any.
    fn apply_filter(&mut self) -> Option<String> {
        if self.filter_input.is_empty() {
            self.filter = None;
            self.filter_error = None;
            self.scroll = 0;
            return None;
        }
        match RegexBuilder::new(&self.filter_input)
            .case_insensitive(true)
            .build()
        {
            Ok(regex) => {
                self.filter = Some(regex);
                self.filter_error = None;
                self.scroll = 0;
                None
            }
            Err(error) => {
                self.filter_error = Some(error.to_string());
                Some(error.to_string())
            }
        }
    }

    /// Number of rows in the active tab matching the current `filter_input`
    /// (used for the live hint while editing). `None` when the input does not
    /// compile.
    fn live_match_count(&self) -> Option<usize> {
        let regex = RegexBuilder::new(&self.filter_input)
            .case_insensitive(true)
            .build()
            .ok()?;
        let count = match self.tab {
            Tab::Files => self
                .stats
                .top_targets
                .iter()
                .filter(|item| regex.is_match(&item.target))
                .count(),
            Tab::Folders => self
                .stats
                .folders
                .iter()
                .filter(|item| regex.is_match(&item.folder))
                .count(),
            Tab::TargetDirs => self
                .stats
                .target_dirs
                .iter()
                .filter(|item| regex.is_match(&item.folder))
                .count(),
            _ => 0,
        };
        Some(count)
    }
}

/// A side effect the event loop must apply after [`handle_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing beyond the state change made by `handle_key`.
    None,
    /// Re-run the stats queries.
    Refresh,
}

/// Apply a key press to the app, clamping scroll to the active tab.
///
/// Returns an [`Action`] the caller should perform (currently only refresh,
/// which needs the database).
pub fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if app.show_help {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            _ => app.show_help = false,
        }
        return Action::None;
    }

    if app.mode == Mode::Filter {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.filter_input = app
                    .filter
                    .as_ref()
                    .map_or_else(String::new, |re| re.as_str().to_string());
                app.filter_error = None;
            }
            KeyCode::Enter => {
                if app.apply_filter().is_none() {
                    app.mode = Mode::Normal;
                }
            }
            KeyCode::Backspace => {
                app.filter_input.pop();
                app.filter_error = None;
            }
            KeyCode::Char(c) => {
                app.filter_input.push(c);
                app.filter_error = None;
            }
            _ => {}
        }
        return Action::None;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('/') if tab_is_filterable(app.tab) => {
            app.mode = Mode::Filter;
            app.filter_input = app
                .filter
                .as_ref()
                .map_or_else(String::new, |re| re.as_str().to_string());
            app.filter_error = None;
        }
        KeyCode::Char('r') => return Action::Refresh,
        KeyCode::Char(']') => {
            app.depth += 1;
            return Action::Refresh;
        }
        KeyCode::Char('[') if app.depth > 1 => {
            app.depth -= 1;
            return Action::Refresh;
        }
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            app.tab = app.tab.next();
            app.scroll = 0;
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            app.tab = app.tab.prev();
            app.scroll = 0;
        }
        KeyCode::Char(digit @ '1'..='7') => {
            app.tab = Tab::ALL[digit as usize - '1' as usize];
            app.scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.scroll = (app.scroll + 1).min(app.max_scroll());
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.scroll = app.scroll.saturating_sub(1);
        }
        KeyCode::Home | KeyCode::Char('g') => app.scroll = 0,
        KeyCode::End | KeyCode::Char('G') => app.scroll = app.max_scroll(),
        _ => {}
    }
    Action::None
}

/// Render one full frame. Pure: no terminal I/O.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);
    draw_tabs(frame, app, tabs);
    draw_body(frame, app, body);
    draw_footer(frame, app, footer);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let totals = &app.stats.totals;
    let broken_style = if totals.broken > 0 {
        Style::new().fg(Color::Red)
    } else {
        Style::new().fg(Color::Green)
    };
    let lines = vec![
        Line::from(vec![
            label("symlinks "),
            value(totals.total.to_string(), Color::Cyan),
            Span::raw("   "),
            label("broken "),
            Span::styled(
                format!("{} ({:.1}%)", totals.broken, totals.broken_percent),
                broken_style.add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            label("unique targets "),
            value(totals.unique_targets.to_string(), Color::Cyan),
        ]),
        Line::from(vec![
            label("root "),
            Span::raw(app.root.clone()),
            Span::raw("   "),
            label("db "),
            Span::raw(app.database.clone()),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" lndb ")),
        area,
    );
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (index, tab) in Tab::ALL.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let active = *tab == app.tab;
        let style = if active {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(
            format!(" {} {} ", index + 1, tab.title()),
            style,
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::bordered().title(" views ")),
        area,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if app.mode == Mode::Filter {
        let mut spans = vec![
            Span::styled(
                "/ ",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.filter_input.clone()),
            Span::styled("_", Style::new().fg(Color::Cyan)),
        ];
        if let Some(error) = &app.filter_error {
            spans.push(Span::styled(
                format!("  {error}"),
                Style::new().fg(Color::Red),
            ));
        } else if let Some(count) = app.live_match_count() {
            spans.push(Span::styled(
                format!("  ({count} matches)"),
                Style::new().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            "  Enter apply  Esc cancel",
            Style::new().fg(Color::DarkGray),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let mut spans = vec![
        key_hint("q"),
        Span::raw(" quit  "),
        key_hint("Tab"),
        Span::raw("/"),
        key_hint("h l"),
        Span::raw(" switch  "),
        key_hint("1-7"),
        Span::raw(" jump  "),
        key_hint("j/k"),
        Span::raw(" scroll  "),
        key_hint("[ ]"),
        Span::raw(" depth  "),
        key_hint("/"),
        Span::raw(" filter  "),
        key_hint("?"),
        Span::raw(" help  "),
        key_hint("r"),
        Span::raw(" refresh"),
    ];
    if let Some(regex) = &app.filter {
        spans.push(Span::styled(
            format!("   filter /{}", regex.as_str()),
            Style::new().fg(Color::Yellow),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().fg(Color::DarkGray)),
        area,
    );
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    match app.tab {
        Tab::Overview => draw_overview(frame, app, area),
        Tab::Files => draw_files(frame, app, area),
        Tab::Folders => draw_folder_chart(frame, app, area, Chart::Folders),
        Tab::TargetDirs => draw_folder_chart(frame, app, area, Chart::TargetDirs),
        Tab::Broken => draw_broken(frame, app, area),
        Tab::Distribution => draw_distribution(frame, app, area),
        Tab::Newest => draw_newest(frame, app, area),
    }
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let [summary, gauge_area, chart] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let totals = &app.stats.totals;
    let lines = vec![
        stat_line("Total symlinks", totals.total.to_string(), Color::Cyan),
        stat_line(
            "Broken",
            format!("{} ({:.1}%)", totals.broken, totals.broken_percent),
            if totals.broken > 0 {
                Color::Red
            } else {
                Color::Green
            },
        ),
        stat_line(
            "Unique targets",
            totals.unique_targets.to_string(),
            Color::Cyan,
        ),
        stat_line(
            "Oldest import",
            totals
                .oldest_added_at
                .map_or_else(|| "n/a".to_string(), format_epoch),
            Color::Gray,
        ),
        stat_line(
            "Newest import",
            totals
                .newest_added_at
                .map_or_else(|| "n/a".to_string(), format_epoch),
            Color::Gray,
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" summary ")),
        summary,
    );

    let ratio = if totals.total > 0 {
        (totals.broken as f64 / totals.total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(Block::bordered().title(" broken coverage "))
        .gauge_style(Style::new().fg(Color::Red))
        .ratio(ratio)
        .label(format!(
            "{:.1}% broken ({} of {})",
            totals.broken_percent, totals.broken, totals.total
        ));
    frame.render_widget(gauge, gauge_area);

    let (start, end) = scroll_range(chart, app.scroll, app.stats.folders.len(), 2);
    let bars = app.stats.folders[start..end]
        .iter()
        .map(|folder| Bar::with_label(folder.folder.clone(), folder.links))
        .collect::<Vec<_>>();
    let max = app
        .stats
        .folders
        .first()
        .map_or(1, |folder| folder.links)
        .max(1);
    frame.render_widget(
        BarChart::horizontal(bars)
            .block(Block::bordered().title(range_title(
                &format!(" depth-{} folders by symlinks ", app.depth),
                start,
                end,
                app.stats.folders.len(),
            )))
            .bar_width(1)
            .bar_gap(0)
            .max(max),
        chart,
    );
}

/// Which folder aggregate a chart tab renders.
#[derive(Clone, Copy)]
enum Chart {
    Folders,
    TargetDirs,
}

fn draw_folder_chart(frame: &mut Frame, app: &App, area: Rect, chart: Chart) {
    let (data, label) = match chart {
        Chart::Folders => (app.visible_folders(), "folders by symlinks"),
        Chart::TargetDirs => (app.visible_target_dirs(), "target dirs by links"),
    };
    let title = format!(" depth-{} {} ", app.depth, label);
    let (start, end) = scroll_range(area, app.scroll, data.len(), 2);
    let bars = data[start..end]
        .iter()
        .map(|folder| Bar::with_label(folder.folder.clone(), folder.links))
        .collect::<Vec<_>>();
    let max = data.first().map_or(1, |folder| folder.links).max(1);
    frame.render_widget(
        BarChart::horizontal(bars)
            .block(Block::bordered().title(range_title(&title, start, end, data.len())))
            .bar_width(1)
            .bar_gap(0)
            .max(max),
        area,
    );
}

fn draw_files(frame: &mut Frame, app: &App, area: Rect) {
    let data = app.visible_top_targets();
    let (start, end) = scroll_range(area, app.scroll, data.len(), 3);
    let rows = data[start..end]
        .iter()
        .map(|target| {
            Row::new(vec![
                Cell::from(target.links.to_string()),
                Cell::from(target.broken.to_string()),
                Cell::from(target.target.clone()),
            ])
        })
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
    )
    .header(table_header(["links", "broken", "target"]))
    .block(Block::bordered().title(range_title(
        " files with most links ",
        start,
        end,
        data.len(),
    )));
    frame.render_widget(table, area);
}

fn draw_broken(frame: &mut Frame, app: &App, area: Rect) {
    let data = &app.stats.broken;
    let (start, end) = scroll_range(area, app.scroll, data.len(), 3);
    let rows = data[start..end]
        .iter()
        .map(|link| {
            Row::new(vec![
                Cell::from(link.path.clone()),
                Cell::from(link.target.clone()),
            ])
        })
        .collect::<Vec<_>>();
    let table = Table::new(rows, [Constraint::Min(20), Constraint::Min(20)])
        .header(table_header(["path", "missing target"]))
        .block(Block::bordered().title(range_title(" broken symlinks ", start, end, data.len())));
    frame.render_widget(table, area);
}

fn draw_distribution(frame: &mut Frame, app: &App, area: Rect) {
    let data = &app.stats.links_per_target;
    let (start, end) = scroll_range(area, app.scroll, data.len(), 2);
    let bars = data[start..end]
        .iter()
        .map(|bucket| Bar::with_label(format!("{} link(s)", bucket.links), bucket.targets))
        .collect::<Vec<_>>();
    let max = data
        .iter()
        .map(|bucket| bucket.targets)
        .max()
        .unwrap_or(1)
        .max(1);
    frame.render_widget(
        BarChart::horizontal(bars)
            .block(Block::bordered().title(range_title(
                " targets per link count ",
                start,
                end,
                data.len(),
            )))
            .bar_width(1)
            .bar_gap(0)
            .max(max),
        area,
    );
}

fn draw_newest(frame: &mut Frame, app: &App, area: Rect) {
    let data = &app.stats.newest;
    let (start, end) = scroll_range(area, app.scroll, data.len(), 3);
    let rows = data[start..end]
        .iter()
        .map(|link| {
            Row::new(vec![
                Cell::from(format_epoch(link.added_at)),
                Cell::from(if link.broken { "broken" } else { "" }),
                Cell::from(link.path.clone()),
                Cell::from(link.target.clone()),
            ])
        })
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [
            Constraint::Length(17),
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Min(12),
        ],
    )
    .header(table_header(["added", "state", "path", "target"]))
    .block(Block::bordered().title(range_title(" newest imports ", start, end, data.len())));
    frame.render_widget(table, area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from("Navigation").style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Line::from("  q / Esc      quit"),
        Line::from("  Tab / h l    next / previous view"),
        Line::from("  1 - 7        jump to a view"),
        Line::from("  j / k        scroll down / up"),
        Line::from("  g / G        scroll to top / bottom"),
        Line::from("  r            refresh from the database"),
        Line::from("  [ / ]        folder grouping depth"),
        Line::from("  /            filter tabs 2-4 by regex (Enter apply, Esc cancel)"),
        Line::from("  ?            toggle this help"),
        Line::from(""),
        Line::from("Views").style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Line::from("  Files         files with the most incoming links"),
        Line::from("  Folders       folders by symlink count (grouping depth applies)"),
        Line::from("  Target dirs   target folders by incoming links (depth applies)"),
        Line::from("  Broken        every broken symlink and its target"),
        Line::from("  Distribution  targets grouped by link count"),
        Line::from("  Newest        symlinks by first-import time"),
        Line::from(""),
        Line::from("Press any key to close").style(Style::new().fg(Color::DarkGray)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" help "))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// Rows available for content once `reserved` rows (borders, header) are
/// removed, then clamp `scroll` so overshoot shows the last page.
fn scroll_range(area: Rect, scroll: usize, len: usize, reserved: u16) -> (usize, usize) {
    let capacity = area.height.saturating_sub(reserved) as usize;
    if len == 0 || capacity == 0 {
        return (0, 0);
    }
    let max_start = len.saturating_sub(capacity);
    let start = scroll.min(max_start);
    let end = (start + capacity).min(len);
    (start, end)
}

fn range_title(title: &str, start: usize, end: usize, len: usize) -> String {
    if len == 0 {
        format!("{title} (empty) ")
    } else {
        format!("{title} [{}-{} of {}] ", start + 1, end, len)
    }
}

fn table_header<const N: usize>(labels: [&'static str; N]) -> Row<'static> {
    Row::new(labels.map(Cell::from))
        .style(Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))
}

fn stat_line(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<16}"), Style::new().fg(Color::Gray)),
        Span::styled(value, Style::new().fg(color).add_modifier(Modifier::BOLD)),
    ])
}

fn label(text: &'static str) -> Span<'static> {
    Span::styled(text, Style::new().fg(Color::Gray))
}

fn value(text: String, color: Color) -> Span<'static> {
    Span::styled(text, Style::new().fg(color).add_modifier(Modifier::BOLD))
}

fn key_hint(text: &'static str) -> Span<'static> {
    Span::styled(
        text,
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ]);
    let horizontal = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ]);
    let [_, middle, _] = vertical.areas(area);
    let [_, centered, _] = horizontal.areas(middle);
    centered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{FolderStat, LinkStat, TargetStat, Totals};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_stats() -> Stats {
        Stats {
            depth: 2,
            totals: Totals {
                total: 3,
                broken: 1,
                broken_percent: 100.0 / 3.0,
                unique_targets: 2,
                oldest_added_at: Some(0),
                newest_added_at: Some(1_700_000_000),
            },
            top_targets: vec![TargetStat {
                target: "docs/x".into(),
                links: 2,
                broken: 0,
            }],
            folders: vec![
                FolderStat {
                    folder: "a".into(),
                    links: 2,
                },
                FolderStat {
                    folder: "b".into(),
                    links: 1,
                },
            ],
            target_dirs: vec![FolderStat {
                folder: "docs".into(),
                links: 3,
            }],
            folder_health: vec![],
            links_per_target: vec![],
            newest: vec![LinkStat {
                path: "a/l1".into(),
                target: "docs/x".into(),
                broken: false,
                added_at: 1_700_000_000,
            }],
            broken: vec![LinkStat {
                path: "b/l2".into(),
                target: "docs/missing".into(),
                broken: true,
                added_at: 0,
            }],
        }
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn overview_shows_totals_and_header() {
        let app = App::new(sample_stats(), "/root".into(), "/db".into());
        let text = render(&app);
        assert!(text.contains("lndb"));
        assert!(text.contains("symlinks"));
        assert!(text.contains("Overview"));
        assert!(text.contains("a"));
        assert!(text.contains("b"));
    }

    #[test]
    fn files_tab_shows_targets() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;
        let text = render(&app);
        assert!(text.contains("docs/x"));
        assert!(text.contains("files with most links"));
    }

    #[test]
    fn broken_tab_lists_broken_links() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Broken;
        let text = render(&app);
        assert!(text.contains("b/l2"));
        assert!(text.contains("docs/missing"));
    }

    #[test]
    fn tabs_cycle_and_reset_scroll() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Broken;
        app.scroll = 0;

        handle_key(&mut app, KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Distribution);

        handle_key(&mut app, KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Newest);

        handle_key(&mut app, KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Overview);

        handle_key(&mut app, KeyEvent::from(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Newest);

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(app.tab, Tab::Folders);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn scroll_is_clamped_to_row_count() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Folders; // two folders -> max scroll 1
        app.scroll = 0;
        for _ in 0..5 {
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('j')));
        }
        assert_eq!(app.scroll, 1);
        for _ in 0..5 {
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('k')));
        }
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn refresh_action_and_quit() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        let action = handle_key(&mut app, KeyEvent::from(KeyCode::Char('r')));
        assert_eq!(action, Action::Refresh);

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('?')));
        assert!(app.show_help);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('x')));
        assert!(!app.show_help);

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn bracket_keys_change_depth_and_request_refresh() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        assert_eq!(app.depth, 2);

        assert_eq!(
            handle_key(&mut app, KeyEvent::from(KeyCode::Char(']'))),
            Action::Refresh
        );
        assert_eq!(app.depth, 3);

        assert_eq!(
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('['))),
            Action::Refresh
        );
        assert_eq!(app.depth, 2);

        // Clamp at depth 1: the key is consumed but no refresh is needed.
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('[')));
        assert_eq!(app.depth, 1);
        assert_eq!(
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('['))),
            Action::None
        );
        assert_eq!(app.depth, 1);
    }

    #[test]
    fn refresh_syncs_depth_and_resets_clamp() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.depth = 4;
        app.tab = Tab::Folders;
        app.scroll = 5;
        let mut stats = sample_stats();
        stats.depth = 1;
        stats.folders.truncate(1);
        app.refresh(stats);
        assert_eq!(app.depth, 1);
        assert_eq!(app.scroll, 0);
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            handle_key(app, KeyEvent::from(KeyCode::Char(c)));
        }
    }

    #[test]
    fn filter_opens_applies_and_clears() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Filter);
        type_str(&mut app, "docs");
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.filter.is_some());
        assert_eq!(app.row_count(), 1); // only "docs/x"

        // Reopening pre-fills the committed pattern.
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        assert_eq!(app.filter_input, "docs");

        // Emptying and pressing Enter clears the filter.
        for _ in 0..4 {
            handle_key(&mut app, KeyEvent::from(KeyCode::Backspace));
        }
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(app.filter.is_none());
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        type_str(&mut app, "DOCS/X");
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.row_count(), 1);
    }

    #[test]
    fn filter_esc_cancels_and_keeps_previous() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;
        app.filter = Some(Regex::new("docs").unwrap());

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        type_str(&mut app, "zzz");
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.filter.as_ref().unwrap().as_str(), "docs");
        assert_eq!(app.row_count(), 1);
    }

    #[test]
    fn invalid_regex_stays_in_editor() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        type_str(&mut app, "(");
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Filter);
        assert!(app.filter_error.is_some());
        assert!(app.filter.is_none());
    }

    #[test]
    fn filter_applies_to_folders_and_resets_scroll() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Folders;
        app.scroll = 5;

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        type_str(&mut app, "^a$");
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter));

        assert_eq!(app.row_count(), 1);
        assert_eq!(app.scroll, 0);
        let text = render(&app);
        assert!(text.contains("folders by symlinks"));
    }

    #[test]
    fn slash_is_ignored_on_unfilterable_tabs() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Broken;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn filter_mode_footer_shows_pattern_and_matches() {
        let mut app = App::new(sample_stats(), "/root".into(), "/db".into());
        app.tab = Tab::Files;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        type_str(&mut app, "doc");
        let text = render(&app);
        assert!(text.contains("/ doc"));
        assert!(text.contains("matches"));
        // The list is not filtered until Enter.
        assert!(text.contains("docs/x"));
    }
}
