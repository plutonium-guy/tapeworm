use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tapeworm::app::AppState;
use tapeworm::feed;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at};

const SYMBOL: &str = "BTCUSDT";
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
    let (tx, mut rx) = mpsc::channel::<feed::FeedEvent>(2048);
    let _feed_handle = feed::spawn(SYMBOL.to_string(), tx);

    let mut events = EventStream::new();
    let mut ticker = interval_at(
        Instant::now() + Duration::from_millis(FRAME_INTERVAL_MS),
        Duration::from_millis(FRAME_INTERVAL_MS),
    );

    // Initial paint.
    terminal.draw(|f| tapeworm::ui::draw(f, &app))?;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = ticker.tick() => {
                terminal.draw(|f| tapeworm::ui::draw(f, &app))?;
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                // Drain available events so we don't fall behind under bursts.
                app.handle(ev);
                while let Ok(more) = rx.try_recv() {
                    app.handle(more);
                }
            }
            term_ev = events.next() => {
                let Some(Ok(Event::Key(k))) = term_ev else { continue };
                if k.kind != KeyEventKind::Press { continue; }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => app.reset_session(),
                    KeyCode::Char('f') => app.toggle_footprint(),
                    KeyCode::Char('c') => app.toggle_chart(),
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
