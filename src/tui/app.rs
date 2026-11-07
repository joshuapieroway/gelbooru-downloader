use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::downloader::engine::human_bytes;
use crate::downloader::progress::{self, ProgressSnapshot};
use crate::utils::truncate_chars;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Gauge, Paragraph, Row, Table, TableState, Tabs,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::Duration as TokioDuration;

const TABS: &[&str] = &["Search", "Queue", "History", "Stats"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    SearchInput,
}

/// A single row in the Queue tab showing active download jobs.
#[derive(Clone)]
struct QueueJob {
    snapshot: ProgressSnapshot,
    speed_bps: f64,
    eta_secs: Option<u64>,
}

/// The bytes and finished-file count seen for a job at the previous poll,
/// used to derive per-job speed and ETA without mixing concurrent jobs.
#[derive(Clone, Copy)]
struct JobSample {
    bytes: u64,
    finished: usize,
    at: Instant,
}

struct AppState {
    tab: usize,
    mode: Mode,
    query: String,
    scroll: usize,
    tick: u64,
    db: Option<Arc<HistoryDb>>,
    client: Option<GelbooruClient>,
    completed: i64,
    history: Vec<crate::database::history::DownloadRecord>,
    last_poll: Instant,
    results: Vec<Post>,
    searching: bool,
    search_error: Option<String>,
    search_page: usize,
    search_per_page: usize,
    search_total: i64,
    /// Monotonic id for the latest search request. Out-of-order responses are
    /// ignored so a slow first page cannot overwrite a newer page or query.
    search_seq: u64,
    jobs: Vec<QueueJob>,
    prog_bytes: u64,
    prog_failed: usize,
    prog_skipped: usize,
    prog_total: usize,
    prog_completed: usize,
    speed_bps: f64,
    job_samples: HashMap<String, JobSample>,
}

impl AppState {
    fn new(db: Option<Arc<HistoryDb>>, client: Option<GelbooruClient>) -> Self {
        Self {
            tab: 0,
            mode: Mode::Normal,
            query: String::new(),
            scroll: 0,
            tick: 0,
            db,
            client,
            completed: 0,
            history: Vec::new(),
            last_poll: Instant::now(),
            results: Vec::new(),
            searching: false,
            search_error: None,
            search_page: 1,
            search_per_page: 50,
            search_total: 0,
            search_seq: 0,
            jobs: Vec::new(),
            prog_bytes: 0,
            prog_failed: 0,
            prog_skipped: 0,
            prog_total: 0,
            prog_completed: 0,
            speed_bps: 0.0,
            job_samples: HashMap::new(),
        }
    }

    /// Rebuild the Queue tab from the live progress records.
    ///
    /// Speed and ETA are derived from the previous poll for the same
    /// `job_id`, so two downloads of the same tag never share numbers.
    fn update_jobs(&mut self, snapshots: Vec<ProgressSnapshot>, now: Instant) {
        let mut seen = HashSet::new();
        let mut jobs = Vec::with_capacity(snapshots.len());
        let mut total_bytes = 0u64;
        let mut total_planned = 0usize;
        let mut total_completed = 0usize;
        let mut total_failed = 0usize;
        let mut total_skipped = 0usize;
        let mut total_speed = 0.0f64;

        for snapshot in snapshots {
            seen.insert(snapshot.job_id.clone());
            let finished = snapshot.completed + snapshot.failed + snapshot.skipped;
            let (speed_bps, eta_secs) = match self.job_samples.get(&snapshot.job_id) {
                Some(prev) => {
                    let elapsed = now.saturating_duration_since(prev.at).as_secs_f64();
                    let transferred = snapshot.bytes.saturating_sub(prev.bytes);
                    let done = finished.saturating_sub(prev.finished);
                    let speed = if elapsed > 0.0 {
                        transferred as f64 / elapsed
                    } else {
                        0.0
                    };
                    let rate = if elapsed > 0.0 {
                        done as f64 / elapsed
                    } else {
                        0.0
                    };
                    let remaining = snapshot.total.saturating_sub(finished);
                    let eta = if rate > 0.0 && remaining > 0 {
                        Some((remaining as f64 / rate).ceil() as u64)
                    } else {
                        None
                    };
                    (speed, eta)
                }
                None => (0.0, None),
            };

            self.job_samples.insert(
                snapshot.job_id.clone(),
                JobSample {
                    bytes: snapshot.bytes,
                    finished,
                    at: now,
                },
            );

            total_bytes += snapshot.bytes;
            total_planned += snapshot.total;
            total_completed += snapshot.completed;
            total_failed += snapshot.failed;
            total_skipped += snapshot.skipped;
            total_speed += speed_bps;

            jobs.push(QueueJob {
                snapshot,
                speed_bps,
                eta_secs,
            });
        }

        self.job_samples.retain(|id, _| seen.contains(id));
        self.jobs = jobs;
        self.prog_bytes = total_bytes;
        self.prog_total = total_planned;
        self.prog_completed = total_completed;
        self.prog_failed = total_failed;
        self.prog_skipped = total_skipped;
        self.speed_bps = total_speed;
    }

    /// Keep the search selection within the current page.  `usize` arithmetic
    /// previously let the cursor drift past the final result, which made the
    /// viewer open the last post while the highlight had already disappeared.
    fn clamp_search_scroll(&mut self) {
        if self.tab == 0 {
            self.scroll = self.scroll.min(self.results.len().saturating_sub(1));
        }
    }
}

/// Restore the terminal even when the TUI exits through an error path.
struct ScreenGuard;

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        disable_raw_mode().ok();
        execute!(
            io::stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        )
        .ok();
    }
}

/// Start (or restart) a search for the current query on `page`. Responses are
/// tagged with a sequence number so stale requests cannot overwrite newer
/// results when the user pages quickly or changes the page size.
fn begin_search(state: &Arc<Mutex<AppState>>, page: usize) {
    let page = page.max(1);
    let (client, query, per_page, seq) = {
        let mut st = state.lock().unwrap();
        st.search_seq = st.search_seq.wrapping_add(1);
        st.searching = true;
        st.search_error = None;
        st.results.clear();
        st.scroll = 0;
        st.search_page = page;
        (
            st.client.clone(),
            st.query.trim().to_string(),
            st.search_per_page,
            st.search_seq,
        )
    };

    let Some(client) = client else {
        let mut st = state.lock().unwrap();
        if st.search_seq == seq {
            st.searching = false;
            st.search_error = Some("No API client".to_string());
        }
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let result = client
            .search_page(&query, per_page as u32, page as u32, None, None)
            .await;
        let mut st = state.lock().unwrap();
        if st.search_seq != seq {
            return;
        }
        st.searching = false;
        match result {
            Ok((posts, total)) => {
                st.results = posts;
                st.search_total = total;
                st.search_page = page;
                st.clamp_search_scroll();
                st.search_error = if st.results.is_empty() {
                    Some("No results".to_string())
                } else {
                    None
                };
            }
            Err(e) => {
                st.results.clear();
                st.search_total = 0;
                st.search_error = Some(e.to_string());
            }
        }
    });
}

fn total_search_pages(total: i64, per_page: usize) -> usize {
    ((total.max(0) as usize + per_page.max(1) - 1) / per_page.max(1)).max(1)
}

/// Work that must happen after the state mutex is released.
enum UiAction {
    None,
    Quit,
    Search(usize),
    View(crate::tui::viewer::ViewerRequest),
}

/// Apply a key press to the UI state. Any network work is returned as an
/// action so the caller can await it without holding the state mutex.
fn handle_key(st: &mut AppState, code: KeyCode) -> UiAction {
    match st.mode {
        Mode::SearchInput => {
            match code {
                KeyCode::Esc => st.mode = Mode::Normal,
                KeyCode::Enter => {
                    st.mode = Mode::Normal;
                    if !st.query.trim().is_empty() {
                        return UiAction::Search(1);
                    }
                }
                KeyCode::Backspace => {
                    st.query.pop();
                }
                KeyCode::Char(c) => st.query.push(c),
                _ => {}
            }
            UiAction::None
        }
        Mode::Normal => match code {
            KeyCode::Char('q') | KeyCode::Esc => UiAction::Quit,
            KeyCode::Tab | KeyCode::Right => {
                st.tab = (st.tab + 1) % TABS.len();
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::BackTab | KeyCode::Left => {
                st.tab = (st.tab + TABS.len() - 1) % TABS.len();
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if st.tab == 0 {
                    st.scroll = st
                        .scroll
                        .saturating_add(1)
                        .min(st.results.len().saturating_sub(1));
                } else {
                    st.scroll = st.scroll.saturating_add(1);
                }
                UiAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                st.scroll = st.scroll.saturating_sub(1);
                UiAction::None
            }
            KeyCode::Char('/') => {
                if st.tab == 0 {
                    st.mode = Mode::SearchInput;
                }
                UiAction::None
            }
            KeyCode::Char('1') => {
                st.tab = 0;
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::Char('2') => {
                st.tab = 1;
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::Char('3') => {
                st.tab = 2;
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::Char('4') => {
                st.tab = 3;
                st.scroll = 0;
                UiAction::None
            }
            KeyCode::Char('n') => {
                if st.tab == 0 && !st.results.is_empty() {
                    let pages = total_search_pages(st.search_total, st.search_per_page);
                    if st.search_page < pages {
                        return UiAction::Search(st.search_page + 1);
                    }
                }
                UiAction::None
            }
            KeyCode::Char('p') => {
                if st.tab == 0 && st.search_page > 1 {
                    return UiAction::Search(st.search_page - 1);
                }
                UiAction::None
            }
            KeyCode::Char('+') | KeyCode::Char('=') if st.tab == 0 => {
                let next = match st.search_per_page {
                    25 => 50,
                    50 => 100,
                    _ => 100,
                };
                if next != st.search_per_page {
                    st.search_per_page = next;
                    if !st.query.trim().is_empty() {
                        return UiAction::Search(1);
                    }
                }
                UiAction::None
            }
            KeyCode::Char('-') if st.tab == 0 => {
                let next = match st.search_per_page {
                    100 => 50,
                    50 => 25,
                    _ => 25,
                };
                if next != st.search_per_page {
                    st.search_per_page = next;
                    if !st.query.trim().is_empty() {
                        return UiAction::Search(1);
                    }
                }
                UiAction::None
            }
            KeyCode::Char('v') | KeyCode::Enter if st.tab == 0 && !st.results.is_empty() => {
                UiAction::View(crate::tui::viewer::ViewerRequest {
                    posts: st.results.clone(),
                    start: st.scroll.min(st.results.len().saturating_sub(1)),
                    db: st.db.clone(),
                    client: st.client.clone(),
                    query: st.query.clone(),
                    per_page: st.search_per_page,
                    page: st.search_page,
                    total: st.search_total,
                })
            }
            _ => UiAction::None,
        },
    }
}

pub async fn run_tui(cfg: Config) -> anyhow::Result<()> {
    let db = if cfg.database.enabled {
        HistoryDb::open(&cfg.db_path()).ok().map(Arc::new)
    } else {
        None
    };
    let client = GelbooruClient::new(&cfg).ok();
    // Pick up downloads interrupted by a previous run without delaying the
    // first frame; progress shows up in the Queue tab like any other job.
    if let Some(db) = db.clone() {
        let cfg = cfg.clone();
        tokio::spawn(async move {
            match crate::queue::resume(&db, &cfg).await {
                Ok(report) if report.resumed > 0 => {
                    tracing::info!(
                        "resumed {} queued download(s): completed={} skipped={} failed={}",
                        report.resumed,
                        report.completed,
                        report.skipped,
                        report.failed
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("could not resume download queue: {e}"),
            }
        });
    }
    let state = Arc::new(Mutex::new(AppState::new(db, client)));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let _guard = ScreenGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        let poll_due = {
            let st = state.lock().unwrap();
            st.last_poll.elapsed() >= TokioDuration::from_secs(1)
        };
        if poll_due {
            // Never hold the state mutex across an await: DB work runs on a
            // blocking thread and search callbacks need the lock to publish
            // their results.
            let db = state.lock().unwrap().db.clone();
            let mut completed = None;
            let mut history = None;
            if let Some(db) = db {
                if let Ok(c) = db.count().await {
                    completed = Some(c);
                }
                if let Ok(h) = db.list(20, None).await {
                    history = Some(h);
                }
            }
            let snapshots = progress::read_snapshots();
            let now = Instant::now();
            let mut st = state.lock().unwrap();
            if let Some(c) = completed {
                st.completed = c;
            }
            if let Some(h) = history {
                st.history = h;
            }
            st.update_jobs(snapshots, now);
            st.last_poll = now;
            st.tick = st.tick.wrapping_add(1);
        } else {
            let mut st = state.lock().unwrap();
            st.tick = st.tick.wrapping_add(1);
        }

        terminal.draw(|f| {
            let st = state.lock().unwrap();
            draw(f, &st);
        })?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(k.code, KeyCode::Char('c'))
                    {
                        return Ok(());
                    }
                    let action = {
                        let mut st = state.lock().unwrap();
                        handle_key(&mut st, k.code)
                    };
                    match action {
                        UiAction::Quit => return Ok(()),
                        UiAction::Search(page) => begin_search(&state, page),
                        UiAction::View(req) => {
                            crate::tui::viewer::show(&mut terminal, &cfg, req).await;
                        }
                        UiAction::None => {}
                    }
                }
                Event::Mouse(m) => {
                    let mut st = state.lock().unwrap();
                    match m.kind {
                        MouseEventKind::ScrollUp if st.tab == 0 => {
                            st.scroll = st.scroll.saturating_sub(3);
                        }
                        MouseEventKind::ScrollDown if st.tab == 0 => {
                            st.scroll = st
                                .scroll
                                .saturating_add(3)
                                .min(st.results.len().saturating_sub(1));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn draw(f: &mut ratatui::Frame, app: &AppState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "Gelbooru Downloader",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            concat!("v", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("   "),
        Span::styled(
            "High-performance async image downloader",
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header, chunks[0]);

    let tabs = Tabs::new(TABS.iter().map(|t| Line::from(*t)).collect::<Vec<_>>())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Views")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .select(app.tab)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "));
    f.render_widget(tabs, chunks[1]);

    match app.tab {
        0 => draw_search(f, app, chunks[2]),
        1 => draw_queue(f, app, chunks[2]),
        2 => draw_history(f, app, chunks[2]),
        3 => draw_stats(f, app, chunks[2]),
        _ => {}
    }

    let help = match app.mode {
        Mode::Normal => {
            "Tab/←/→ views  1-4 jump  ↑/↓ scroll  / search  n/p page  +/- size  v/Enter view  mouse wheel scroll  q quit"
        }
        Mode::SearchInput => "Type query  Enter search  Esc cancel",
    };
    let footer = Paragraph::new(Span::styled(help, Style::default().fg(Color::DarkGray))).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    f.render_widget(footer, chunks[3]);
}

fn draw_search(f: &mut ratatui::Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let header = ["ID", "Rating", "Size", "Score", "Tags"];
    let widths = [
        Constraint::Percentage(10),
        Constraint::Percentage(12),
        Constraint::Percentage(15),
        Constraint::Percentage(8),
        Constraint::Percentage(55),
    ];
    let selected = if app.results.is_empty() {
        None
    } else {
        Some(app.scroll.min(app.results.len() - 1))
    };

    let rows: Vec<Row> = if app.searching {
        vec![Row::new(vec![
            Cell::from("Searching...").style(Style::default().fg(Color::Yellow)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else if let Some(err) = &app.search_error {
        vec![Row::new(vec![
            Cell::from(format!("Error: {}", err)).style(Style::default().fg(Color::Red)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else if app.results.is_empty() {
        vec![Row::new(vec![
            Cell::from("No results. Press / to search.")
                .style(Style::default().fg(Color::DarkGray)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else {
        app.results
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let style = if Some(i) == selected {
                    Style::default()
                        .bg(Color::Rgb(60, 60, 90))
                        .add_modifier(Modifier::BOLD)
                } else if i % 2 == 0 {
                    Style::default().bg(Color::Rgb(24, 24, 32))
                } else {
                    Style::default()
                };
                Row::new(vec![
                    Cell::from(p.id.to_string()),
                    Cell::from(p.rating.clone())
                        .style(Style::default().fg(rating_color(&p.rating))),
                    Cell::from(format!("{}x{}", p.width, p.height)),
                    Cell::from(p.score.to_string()),
                    Cell::from(truncate_chars(&p.tags, 60)),
                ])
                .style(style)
            })
            .collect()
    };

    let table = Table::new(rows, widths)
        .header(Row::new(
            header
                .iter()
                .map(|h| {
                    Cell::from(*h).style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!(
                    "Search Results ({})  Page {}/{}  Per page {}",
                    app.results.len(),
                    app.search_page,
                    total_search_pages(app.search_total, app.search_per_page),
                    app.search_per_page
                ))
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    // Scroll the table viewport so the selected row stays visible, and clamp
    // the offset at the final result so the table cannot scroll past the end
    // of the current page.
    let visible_rows = usize::from(chunks[0].height.saturating_sub(3)).max(1);
    let offset = selected
        .map(|s| s.saturating_sub(visible_rows.saturating_sub(1)))
        .unwrap_or(0);
    let mut table_state = TableState::default()
        .with_selected(selected)
        .with_offset(offset);
    f.render_stateful_widget(table, chunks[0], &mut table_state);

    let input_style = if app.mode == Mode::SearchInput {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let input = Paragraph::new(Line::from(vec![
        Span::styled(
            "Query: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.query.clone(), input_style),
        Span::styled(
            if app.mode == Mode::SearchInput {
                "█"
            } else {
                ""
            },
            Style::default().fg(Color::Yellow),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title("Search")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(input, chunks[1]);
}

fn queue_label(snapshot: &ProgressSnapshot) -> String {
    if let Some(out_name) = snapshot.output_name() {
        format!("{} ({})", snapshot.base_label(), out_name)
    } else {
        snapshot.base_label()
    }
}

fn format_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

fn draw_queue(f: &mut ratatui::Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let header = ["Tag", "Status", "Progress", "Speed", "ETA"];
    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(12),
        Constraint::Percentage(28),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
    ];

    let rows: Vec<Row> = if app.jobs.is_empty() {
        vec![Row::new(vec![
            Cell::from("idle").style(Style::default().fg(Color::DarkGray)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else {
        app.jobs
            .iter()
            .map(|job| {
                let s = &job.snapshot;
                let tag_label = queue_label(s);
                let status_str = if s.completed + s.failed + s.skipped >= s.total && s.total > 0 {
                    "completed"
                } else {
                    "downloading"
                };
                let pct = if s.total > 0 {
                    s.completed * 100 / s.total.max(1)
                } else {
                    0
                };
                Row::new(vec![
                    Cell::from(tag_label),
                    Cell::from(status_str),
                    Cell::from(format!("{}/{} ({pct}%)", s.completed, s.total)),
                    Cell::from(if job.speed_bps > 0.0 {
                        format!("{:.1} MB/s", job.speed_bps / 1_048_576.0)
                    } else {
                        "n/a".to_string()
                    }),
                    Cell::from(
                        job.eta_secs
                            .map(format_eta)
                            .unwrap_or_else(|| "n/a".to_string()),
                    ),
                ])
            })
            .collect()
    };

    let table = Table::new(rows, widths)
        .header(Row::new(
            header
                .iter()
                .map(|h| {
                    Cell::from(*h).style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!("Download Queue ({})", app.jobs.len()))
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(table, chunks[0]);

    let total_planned: usize = app.jobs.iter().map(|j| j.snapshot.total).sum();
    let total_finished: usize = app.jobs.iter().map(|j| j.snapshot.completed).sum();
    let percent = if total_planned > 0 {
        (total_finished * 100 / total_planned.max(1)) as u16
    } else {
        0
    };
    let label = if total_planned > 0 {
        format!("{}/{total_planned}", total_finished)
    } else {
        "idle".to_string()
    };
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Overall")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
        .percent(percent)
        .label(label);
    f.render_widget(gauge, chunks[1]);
}

fn draw_history(f: &mut ratatui::Frame, app: &AppState, area: Rect) {
    let header = ["ID", "Status", "Rating", "Size", "Path"];
    let widths = [
        Constraint::Percentage(10),
        Constraint::Percentage(12),
        Constraint::Percentage(12),
        Constraint::Percentage(10),
        Constraint::Percentage(56),
    ];
    let rows: Vec<Row> = if app.history.is_empty() {
        vec![Row::new(vec![
            Cell::from("no history yet").style(Style::default().fg(Color::DarkGray)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else {
        app.history
            .iter()
            .map(|r| {
                Row::new(vec![
                    Cell::from(r.post_id.to_string()),
                    Cell::from(r.status.clone()),
                    Cell::from(r.rating.clone())
                        .style(Style::default().fg(rating_color(&r.rating))),
                    Cell::from(human_bytes(r.file_size.max(0) as u64)),
                    Cell::from(r.local_path.clone()),
                ])
            })
            .collect()
    };

    let table = Table::new(rows, widths)
        .header(Row::new(
            header
                .iter()
                .map(|h| {
                    Cell::from(*h).style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!("Download History ({})", app.completed))
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(table, area);
}

fn draw_stats(f: &mut ratatui::Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(4)])
        .split(area);

    let bytes_str = human_bytes(app.prog_bytes);
    let stats_text = vec![
        Line::from(vec![
            Span::styled(
                "Completed: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.completed.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Failed:     ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.prog_failed.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Skipped:    ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.prog_skipped.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Bytes:      ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(bytes_str),
        ]),
        Line::from(vec![
            Span::styled(
                "Speed:      ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(if app.speed_bps > 0.0 {
                format!("{:.2} MB/s", app.speed_bps / 1_048_576.0)
            } else {
                "n/a".to_string()
            }),
        ]),
        Line::from(vec![
            Span::styled(
                "Live:       ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(if app.prog_total > 0 {
                format!("{}/{}", app.prog_completed, app.prog_total)
            } else {
                "idle".to_string()
            }),
        ]),
    ];
    let stats = Paragraph::new(stats_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title("Statistics")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(stats, chunks[0]);

    let percent = if app.prog_total > 0 {
        (app.prog_completed * 100 / app.prog_total.max(1)) as u16
    } else {
        0
    };
    let label = if app.prog_total > 0 {
        format!("{}/{}", app.prog_completed, app.prog_total)
    } else {
        "idle".to_string()
    };
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Session Progress")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
        .percent(percent)
        .label(label);
    f.render_widget(gauge, chunks[1]);
}

fn rating_color(r: &str) -> Color {
    match r.to_lowercase().as_str() {
        "general" | "safe" | "s" => Color::Green,
        "sensitive" => Color::Yellow,
        "questionable" | "q" => Color::LightYellow,
        "explicit" | "e" | "nsfw" => Color::Red,
        _ => Color::Gray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        job_id: &str,
        rating: &str,
        output: &str,
        total: usize,
        completed: usize,
        bytes: u64,
    ) -> ProgressSnapshot {
        ProgressSnapshot {
            job_id: job_id.to_string(),
            label: "gilberta_(arknights)".to_string(),
            rating: Some(rating.to_string()),
            output: Some(output.to_string()),
            total,
            completed,
            bytes,
            ..Default::default()
        }
    }

    #[test]
    fn same_tag_with_different_ratings_stays_separate_in_queue() {
        let mut app = AppState::new(None, None);
        app.update_jobs(
            vec![
                snapshot("a", "general,sensitive", "/out/safe", 10, 2, 100),
                snapshot("b", "explicit,questionable", "/out/nsfw", 20, 5, 200),
            ],
            Instant::now(),
        );

        assert_eq!(app.jobs.len(), 2);
        assert_ne!(
            queue_label(&app.jobs[0].snapshot),
            queue_label(&app.jobs[1].snapshot)
        );
        assert!(queue_label(&app.jobs[0].snapshot).contains("general,sensitive"));
        assert!(queue_label(&app.jobs[1].snapshot).contains("explicit,questionable"));
        assert_eq!(app.prog_total, 30);
        assert_eq!(app.prog_completed, 7);
        assert_eq!(app.prog_bytes, 300);
    }

    #[test]
    fn same_tag_and_rating_with_different_outputs_stays_separate_in_queue() {
        let mut app = AppState::new(None, None);
        app.update_jobs(
            vec![
                snapshot("a", "general", "/out/safe", 10, 1, 0),
                snapshot("b", "general", "/out/backup", 10, 1, 0),
            ],
            Instant::now(),
        );

        assert_eq!(app.jobs.len(), 2);
        assert_ne!(
            queue_label(&app.jobs[0].snapshot),
            queue_label(&app.jobs[1].snapshot)
        );
    }

    #[test]
    fn per_job_speed_and_eta_come_from_the_previous_poll() {
        let mut app = AppState::new(None, None);
        let start = Instant::now();
        app.update_jobs(vec![snapshot("a", "general", "/out", 10, 0, 0)], start);

        assert_eq!(app.jobs[0].speed_bps, 0.0);
        assert!(app.jobs[0].eta_secs.is_none());

        app.update_jobs(
            vec![snapshot("a", "general", "/out", 10, 2, 2048)],
            start + Duration::from_secs(2),
        );

        assert!((app.jobs[0].speed_bps - 1024.0).abs() < f64::EPSILON);
        assert_eq!(app.jobs[0].eta_secs, Some(8));
        assert!((app.speed_bps - 1024.0).abs() < f64::EPSILON);
    }

    #[test]
    fn finished_jobs_are_removed_from_the_queue() {
        let mut app = AppState::new(None, None);
        let now = Instant::now();
        app.update_jobs(
            vec![
                snapshot("a", "general", "/out/a", 10, 0, 0),
                snapshot("b", "explicit", "/out/b", 10, 0, 0),
            ],
            now,
        );
        assert_eq!(app.jobs.len(), 2);
        assert_eq!(app.job_samples.len(), 2);

        app.update_jobs(
            vec![snapshot("a", "general", "/out/a", 10, 4, 4096)],
            now + Duration::from_secs(1),
        );

        assert_eq!(app.jobs.len(), 1);
        assert_eq!(app.jobs[0].snapshot.job_id, "a");
        assert_eq!(app.job_samples.len(), 1);
        assert!(app.job_samples.contains_key("a"));
    }

    #[test]
    fn eta_formatting_is_human_readable() {
        assert_eq!(format_eta(0), "0s");
        assert_eq!(format_eta(45), "45s");
        assert_eq!(format_eta(90), "1m 30s");
        assert_eq!(format_eta(3_600), "1h 0m");
        assert_eq!(format_eta(7_320), "2h 2m");
    }

    #[test]
    fn queue_renders_one_row_per_concurrent_job() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = AppState::new(None, None);
        app.update_jobs(
            vec![
                snapshot("a", "general,sensitive", "/out/safe", 10, 2, 100),
                snapshot("b", "explicit,questionable", "/out/nsfw", 20, 5, 200),
            ],
            Instant::now(),
        );

        let backend = TestBackend::new(300, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                draw_queue(f, &app, area);
            })
            .unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("Download Queue (2)"));
        assert!(
            rendered.contains("gilberta_(arknights) [general,sensitive]"),
            "{rendered}"
        );
        assert!(rendered.contains("gilberta_(arknights) [explicit,questionable]"));
        assert!(rendered.contains("(safe)"));
        assert!(rendered.contains("(nsfw)"));
    }

    #[test]
    fn search_scroll_cannot_pass_the_last_result() {
        let mut app = AppState::new(None, None);
        app.results = vec![Post::default(), Post::default(), Post::default()];
        app.scroll = 99;

        app.clamp_search_scroll();

        assert_eq!(app.scroll, 2);
    }

    #[test]
    fn search_scroll_is_zero_when_there_are_no_results() {
        let mut app = AppState::new(None, None);
        app.scroll = 7;

        app.clamp_search_scroll();

        assert_eq!(app.scroll, 0);
    }
}
