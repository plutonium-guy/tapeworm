use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tapeworm::app::AppState;
use tapeworm::feed;
use tapeworm::multi;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at};

const SYMBOL: &str = "BTCUSDT";
const WATCHLIST: &[&str] = &[
    "BTCUSDT", "ETHUSDT", "SOLUSDT", "BNBUSDT",
    "XRPUSDT", "DOGEUSDT", "ADAUSDT", "AVAXUSDT",
];
const FRAME_INTERVAL_MS: u64 = 16; // ~60 fps

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    install_panic_hook();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    result
}

async fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = AppState::new(SYMBOL);
    // Best-effort: pick up previously-trained RL weights if present so the
    // agent isn't a blank slate every launch. Failure (file missing or
    // parse error) is silent — the agent stays freshly initialised.
    if let Err(e) = app.rl_load() {
        tracing::debug!(error = ?e, "no rl-weights.txt to load (fresh agent)");
    }
    // Tagged channel: (symbol, FeedEvent) — one full feed per symbol so
    // every watched symbol's heavy engines run concurrently.
    let (tx, mut rx) = mpsc::channel::<(String, feed::FeedEvent)>(8192);
    // Default-active symbol is the first one in the WATCHLIST.
    let _active_feed = feed::spawn_tagged(SYMBOL.to_string(), tx.clone());
    for s in WATCHLIST.iter().filter(|s| !s.eq_ignore_ascii_case(SYMBOL)) {
        app.add_symbol(*s);
        let _ = feed::spawn_tagged((*s).to_string(), tx.clone());
    }
    drop(tx);

    // Lightweight watchlist summaries (cheaper trade+bookTicker streams).
    let (wtx, mut wrx) = mpsc::channel::<multi::WatchEvent>(2048);
    for s in WATCHLIST {
        app.add_watchlist(*s);
        let _ = multi::spawn_watchlist_feed((*s).to_string(), wtx.clone());
    }
    drop(wtx);

    let mut events = EventStream::new();
    let mut ticker = interval_at(
        Instant::now() + Duration::from_millis(FRAME_INTERVAL_MS),
        Duration::from_millis(FRAME_INTERVAL_MS),
    );

    // Initial paint.
    terminal.draw(|f| tapeworm::ui::draw(f, &app))?;
    app.hit_map = tapeworm::ui::take_hits();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = ticker.tick() => {
                terminal.draw(|f| tapeworm::ui::draw(f, &app))?;
                app.hit_map = tapeworm::ui::take_hits();
            }
            ev = rx.recv() => {
                let Some((sym, ev)) = ev else { break };
                route_event(&mut app, &sym, ev);
                while let Ok((s, e)) = rx.try_recv() {
                    route_event(&mut app, &s, e);
                }
            }
            wev = wrx.recv() => {
                let Some(wev) = wev else { continue };
                multi::apply_event(&mut app.watchlist, &wev);
                while let Ok(more) = wrx.try_recv() {
                    multi::apply_event(&mut app.watchlist, &more);
                }
            }
            term_ev = events.next() => {
                let Some(Ok(ev)) = term_ev else { continue };
                if let Event::Mouse(m) = ev {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        let quit = app.handle_click(m.column, m.row);
                        if quit { return Ok(()); }
                    }
                    continue;
                }
                let Event::Key(k) = ev else { continue };
                if k.kind != KeyEventKind::Press { continue; }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => app.reset_session(),
                    KeyCode::Char('f') => app.toggle_footprint(),
                    KeyCode::Char('c') => app.toggle_chart(),
                    KeyCode::Char('s') => app.toggle_signals_log(),
                    KeyCode::Char('S') => app.cycle_signals_filter(),
                    KeyCode::Char('j') if app.show_signals_log => app.signals_select(1),
                    KeyCode::Char('k') if app.show_signals_log => app.signals_select(-1),
                    KeyCode::Char('g') if app.show_signals_log => app.jump_chart_to_selected_signal(),
                    KeyCode::Char('p') => app.toggle_paper(),
                    KeyCode::Char('z') => app.toggle_analytics(),
                    KeyCode::Char('w') => app.toggle_watchlist(),
                    KeyCode::Char('G') => app.toggle_graphs(),
                    KeyCode::Char('M') => app.toggle_rl_panel(),
                    KeyCode::Char('N') => app.toggle_rl_enabled(),
                    KeyCode::Char('Y') => app.toggle_rl_training(),
                    KeyCode::Char('Z') => app.toggle_rl_auto_trade(),
                    KeyCode::Char(']') if app.show_graphs => app.cycle_graphs_page(),
                    KeyCode::Tab => {
                        // Cycle to the next watched symbol.
                        if !app.others.is_empty() {
                            let next = app.others[0].symbol.clone();
                            app.switch_active(&next);
                        }
                    }
                    KeyCode::Char('b') => app.paper_market_buy(0),
                    KeyCode::Char('B') => app.paper_market_sell(0),
                    KeyCode::Char('F') => app.paper_flatten(0),
                    KeyCode::Char('X') => app.paper_cancel_all(),
                    KeyCode::Char('+') | KeyCode::Char('=') => app.paper_qty_inc(),
                    KeyCode::Char('_') => app.paper_qty_dec(),
                    KeyCode::Char('T') => app.toggle_trading_mode(),
                    KeyCode::Char('1') => app.chart.set_timeframe_index(0),
                    KeyCode::Char('2') => app.chart.set_timeframe_index(1),
                    KeyCode::Char('3') => app.chart.set_timeframe_index(2),
                    KeyCode::Char('4') => app.chart.set_timeframe_index(3),
                    KeyCode::Char('5') => app.chart.set_timeframe_index(4),
                    KeyCode::Char('t') => app.chart.cycle_candle_type(),
                    KeyCode::Char('v') => app.chart.cycle_volume_mode(),
                    KeyCode::Char('i') => app.chart.cycle_indicator(),
                    KeyCode::Char('V') => app.chart.toggle_vwap(),
                    KeyCode::Char('P') => app.chart.toggle_volume_profile(),
                    KeyCode::Char('D') => app.chart.toggle_cum_delta(),
                    KeyCode::Char('I') => app.chart.toggle_indicator_panel(),
                    KeyCode::Char('9') => app.chart.toggle_ema(0),
                    KeyCode::Char('0') => app.chart.toggle_ema(1),
                    KeyCode::Char('-') => app.chart.toggle_ema(2),
                    KeyCode::Char('a') => {
                        if let Some(p) = app.last_chart_price() { app.chart.add_alert(p); }
                    }
                    KeyCode::Delete | KeyCode::Char('A') => {
                        if let Some(p) = app.last_chart_price() { app.chart.remove_nearest_alert(p); }
                    }
                    KeyCode::Char('x') => app.chart.toggle_crosshair(),
                    KeyCode::Left => app.chart.scroll_left(1),
                    KeyCode::Right => app.chart.scroll_right(1),
                    KeyCode::Home => app.chart.scroll_home(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn route_event(app: &mut AppState, sym: &str, ev: feed::FeedEvent) {
    if app.active.symbol.eq_ignore_ascii_case(sym) {
        app.handle(ev);
    } else if let Some(idx) = app
        .others
        .iter()
        .position(|s| s.symbol.eq_ignore_ascii_case(sym))
    {
        app.handle_other(idx, ev);
    }
}

fn init_tracing() {
    // Logs go to stderr — UI uses stdout via alt screen.
    if std::env::var_os("RUST_LOG").is_none() {
        // Default quiet.
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        prev(info);
    }));
}
