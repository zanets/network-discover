use crate::portscan::{self, PortResult, PortScanEvent};
use crate::types::HostInfo;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};
use std::{
    io,
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

// ── Public event type sent by the host scan task ──────────────────────────────

pub enum ScanEvent {
    Host(HostInfo),
    Hostname(Ipv4Addr, String),
    Done,
}

// ── TUI mode ──────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Mode {
    HostList,
    PortScan,
}

// ── Port scan overlay state ───────────────────────────────────────────────────

struct PortScan {
    ip: Ipv4Addr,
    open_ports: Vec<PortResult>,
    done: usize,
    total: usize,
    complete: bool,
    rx: mpsc::Receiver<PortScanEvent>,
    task: tokio::task::JoinHandle<()>,
}

impl PortScan {
    fn new(ip: Ipv4Addr) -> Self {
        let (tx, rx) = mpsc::channel::<PortScanEvent>(512);
        let task = tokio::spawn(portscan::scan(ip, tx));
        Self {
            ip,
            open_ports: Vec::new(),
            done: 0,
            total: portscan::PORTS.len(),
            complete: false,
            rx,
            task,
        }
    }

    fn drain(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(PortScanEvent::Open(r)) => {
                    let pos = self.open_ports.partition_point(|p| p.port < r.port);
                    self.open_ports.insert(pos, r);
                }
                Ok(PortScanEvent::Progress { done, total }) => {
                    self.done = done;
                    self.total = total;
                }
                Ok(PortScanEvent::Done) => self.complete = true,
                Err(_) => break,
            }
        }
    }

    fn abort(self) {
        self.task.abort();
    }

    fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.done as f64 / self.total as f64
        }
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    hosts: Vec<HostInfo>,
    table_state: TableState,
    scan_done: bool,
    tick: u8,
    start: Instant,
    target: String,
    mode: Mode,
    port_scan: Option<PortScan>,
}

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl App {
    fn new(target: String) -> Self {
        Self {
            hosts: Vec::new(),
            table_state: TableState::default(),
            scan_done: false,
            tick: 0,
            start: Instant::now(),
            target,
            mode: Mode::HostList,
            port_scan: None,
        }
    }

    fn push_host(&mut self, host: HostInfo) {
        let pos = self.hosts.partition_point(|h| h.ip < host.ip);
        self.hosts.insert(pos, host);
        if self.table_state.selected().is_none() {
            self.table_state.select(Some(0));
        }
    }

    fn update_hostname(&mut self, ip: Ipv4Addr, name: String) {
        if let Ok(i) = self.hosts.binary_search_by_key(&ip, |h| h.ip) {
            self.hosts[i].hostname = Some(name);
        }
    }

    fn scroll_up(&mut self) {
        if let Some(i) = self.table_state.selected() {
            self.table_state.select(Some(i.saturating_sub(1)));
        }
    }

    fn scroll_down(&mut self) {
        if self.hosts.is_empty() {
            return;
        }
        let max = self.hosts.len() - 1;
        let i = self
            .table_state
            .selected()
            .map(|i| (i + 1).min(max))
            .unwrap_or(0);
        self.table_state.select(Some(i));
    }

    fn open_port_scan(&mut self) {
        if let Some(idx) = self.table_state.selected() {
            if idx < self.hosts.len() {
                let ip = self.hosts[idx].ip;
                self.port_scan = Some(PortScan::new(ip));
                self.mode = Mode::PortScan;
            }
        }
    }

    fn close_port_scan(&mut self) {
        if let Some(ps) = self.port_scan.take() {
            ps.abort();
        }
        self.mode = Mode::HostList;
    }

    fn spinner_char(&self) -> char {
        SPINNER[self.tick as usize % SPINNER.len()]
    }

    fn elapsed_str(&self) -> String {
        format!("{:.1}s", self.start.elapsed().as_secs_f64())
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(mut rx: mpsc::Receiver<ScanEvent>, target: String) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut rx, target).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    rx: &mut mpsc::Receiver<ScanEvent>,
    target: String,
) -> Result<()> {
    let mut app = App::new(target);
    let tick_rate = Duration::from_millis(80);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render(f, &mut app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => match app.mode {
                            Mode::PortScan => app.close_port_scan(),
                            Mode::HostList => break,
                        },
                        KeyCode::Enter => {
                            if app.mode == Mode::HostList {
                                app.open_port_scan();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if app.mode == Mode::HostList {
                                app.scroll_up();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.mode == Mode::HostList {
                                app.scroll_down();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Drain host scan events
        loop {
            match rx.try_recv() {
                Ok(ScanEvent::Host(h)) => app.push_host(h),
                Ok(ScanEvent::Hostname(ip, name)) => app.update_hostname(ip, name),
                Ok(ScanEvent::Done) => app.scan_done = true,
                Err(_) => break,
            }
        }

        // Drain port scan events
        if let Some(ps) = &mut app.port_scan {
            ps.drain();
        }

        if last_tick.elapsed() >= tick_rate {
            app.tick = app.tick.wrapping_add(1);
            last_tick = Instant::now();
        }
    }

    Ok(())
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // header (2 content + 2 borders)
            Constraint::Min(0),    // table
            Constraint::Length(1), // footer
        ])
        .split(area);

    render_header(frame, app, chunks[0]);
    render_host_table(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);

    if app.mode == Mode::PortScan {
        if let Some(ps) = &app.port_scan {
            let popup = centered_rect(72, 78, area);
            render_port_scan(frame, ps, popup, app.tick);
        }
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let inner = area.inner(Margin { horizontal: 1, vertical: 1 });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(Block::default().borders(Borders::ALL), area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "nd",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(app.target.clone(), Style::default().fg(Color::Yellow)),
        ])),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            if app.scan_done {
                Span::styled("✓  Complete", Style::default().fg(Color::Green))
            } else {
                Span::styled(
                    format!("{}  Scanning...", app.spinner_char()),
                    Style::default().fg(Color::Green),
                )
            },
            Span::raw("   "),
            Span::styled(
                format!(
                    "{} host{}",
                    app.hosts.len(),
                    if app.hosts.len() == 1 { "" } else { "s" }
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(app.elapsed_str(), Style::default().fg(Color::DarkGray)),
        ])),
        rows[1],
    );
}

fn render_host_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let col_header = Row::new(
        ["IP Address", "MAC Address", "Vendor", "Hostname"]
            .iter()
            .map(|h| {
                Cell::from(*h).style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            }),
    )
    .height(1);

    let rows = app.hosts.iter().map(|h| {
        Row::new([
            Cell::from(h.ip.to_string()),
            Cell::from(h.mac_display()),
            Cell::from(h.vendor.as_deref().unwrap_or("").to_string()),
            Cell::from(h.hostname.as_deref().unwrap_or("").to_string()),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(20),
            Constraint::Length(22),
            Constraint::Min(16),
        ],
    )
    .header(col_header)
    .block(Block::default().borders(Borders::ALL).title(" Hosts "))
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let spans = if app.mode == Mode::PortScan {
        vec![
            Span::raw(" "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" close scan   "),
            Span::styled("q", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ]
    } else {
        vec![
            Span::raw(" "),
            Span::styled("↑↓ / jk", Style::default().fg(Color::Yellow)),
            Span::raw(" scroll   "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" port scan   "),
            Span::styled("q / Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ]
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_port_scan(frame: &mut Frame, ps: &PortScan, area: Rect, tick: u8) {
    // Clear the background area so the popup renders cleanly over the table
    frame.render_widget(Clear, area);

    let title = format!(" Port Scan: {} ", ps.ip);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status text
            Constraint::Length(1), // progress bar
            Constraint::Length(1), // spacer
            Constraint::Min(0),    // port table
            Constraint::Length(1), // footer hint
        ])
        .split(inner);

    // Status line
    let status = if ps.complete {
        Line::from(vec![
            Span::styled("✓  Complete", Style::default().fg(Color::Green)),
            Span::raw("   "),
            Span::styled(
                format!(
                    "{} open port{}",
                    ps.open_ports.len(),
                    if ps.open_ports.len() == 1 { "" } else { "s" }
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                format!("{}  Scanning...", SPINNER[tick as usize % SPINNER.len()]),
                Style::default().fg(Color::Green),
            ),
            Span::raw("   "),
            Span::styled(
                format!("{}/{} ports", ps.done, ps.total),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("   "),
            Span::styled(
                format!(
                    "{} open",
                    ps.open_ports.len(),
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(status), chunks[0]);

    // Progress gauge
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
        .ratio(ps.ratio());
    frame.render_widget(gauge, chunks[1]);

    // Port table
    if ps.open_ports.is_empty() {
        let msg = if ps.complete {
            "No open ports found."
        } else {
            "Waiting for results..."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(Color::DarkGray))),
            chunks[3],
        );
    } else {
        let col_header = Row::new([
            Cell::from("PORT").style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("SERVICE").style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("STATE").style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .height(1);

        let rows = ps.open_ports.iter().map(|r| {
            Row::new([
                Cell::from(r.port.to_string()),
                Cell::from(r.service),
                Cell::from("open").style(Style::default().fg(Color::Green)),
            ])
        });

        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(18),
                Constraint::Min(0),
            ],
        )
        .header(col_header);

        frame.render_widget(table, chunks[3]);
    }

    // Footer hint
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" close"),
        ]))
        .alignment(Alignment::Right),
        chunks[4],
    );
}

// ── Layout helpers ─────────────────────────────────────────────────────────────

fn centered_rect(width_pct: u16, height_pct: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_pct) / 2),
            Constraint::Percentage(height_pct),
            Constraint::Percentage((100 - height_pct) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(vert[1])[1]
}
