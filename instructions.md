# TAPEWORM — Master Build Prompt
## Real-Time Order Flow Terminal

---

## OPERATING RULES

You work in a strict loop on every phase and every step
within each phase.

Plan what you will build. Build it. Run it. Verify it works.
Fix any errors. Only then move to the next step.

Never write code across multiple components before verifying
the first one compiles. Never assume something works without
running it. If something fails three times, stop and explain
what is blocking you.

Do not begin a new phase until the current phase fully
passes its definition of done. Every phase builds on the
one before it. Skipping ahead will cause failures that are
harder to debug than the original problem.

---

## WHAT IS TAPEWORM

Tapeworm is a real-time order flow terminal that runs
entirely in the terminal. It is written in Rust.

It connects to live market data, processes the raw stream
into meaningful order flow signals, and displays everything
in a terminal UI that updates in real time. It is built for
traders who want to understand what is actually happening
in the market at the level of order flow, not just price.

The name comes from the reconstructed tape — the continuous
stream of every trade that executes, revealing whether
buyers or sellers are in control moment to moment.

It is inspired by Jigsaw Trading, a professional order flow
analysis tool used by futures day traders. Tapeworm builds
everything Jigsaw does and more, running entirely in a
terminal window as a single Rust binary.

---

## DATA SOURCE

Use the Binance public WebSocket API. It is free, requires
no API key, no account, and no agreements. It provides a
depth of market stream and a trade stream. Subscribe to
both simultaneously over a single connection. Use BTCUSDT
as the default symbol.

---

## HOW THE MARKET DATA WORKS

The market produces two continuous streams.

The order book contains all resting limit orders at every
price level. Buyers place bids below the current price.
Sellers place asks above it. It changes constantly as
orders are added, modified, and cancelled. Tapeworm
maintains a correct snapshot in memory at all times.

The trade feed delivers every executed trade with a price,
quantity, and aggressor side — either a buyer who hit an
ask or a seller who hit a bid. Tapeworm uses this to track
buying and selling pressure continuously.

---

## DEPENDENCIES TO USE

tokio for the async runtime. tokio-tungstenite for
WebSocket. futures-util for async streams. serde and
serde_json for JSON parsing. ratatui for the TUI framework.
crossterm for the terminal backend. anyhow for error
handling. No other dependencies unless a phase explicitly
requires one.

---

---

# PHASE 1 — Core Terminal

## What gets built

The foundation of the entire application. A live connection
to Binance, an in-memory order book, and a three-panel
terminal UI showing the DOM price ladder, reconstructed
tape, and delta summary.

## Core concepts

The order book has two sides — bids sorted descending and
asks sorted ascending. Prices must be stored as integer
ticks internally, never as raw floating point. This is
mandatory. Floating point equality is unreliable for map
lookups and will cause silent correctness bugs.

Delta is buy-initiated volume minus sell-initiated volume.
Binance provides a field per trade indicating whether the
buyer was the maker. If the buyer was the maker the seller
was the aggressor — classify it as a sell trade. Otherwise
classify it as a buy trade.

The order book can fall out of sync if a message is missed.
When that happens mark the book as stale and show this in
the UI rather than displaying potentially incorrect data.

The feed runs on a background async task. The UI renders
on its own cadence. They communicate only through a
buffered channel. The UI never blocks waiting for market
data. All rolling buffers have a maximum size and drop
old entries when full.

## Features

DOM price ladder showing top price levels on both sides.
Asks in red above the spread, bids in green below. Each
level shows quantity and a proportional horizontal bar.
Best bid and ask are visually emphasized. Stale state
shown clearly when the book loses sync.

Reconstructed tape showing every trade as it happens.
Most recent at the top. Each entry shows time, price,
quantity, and direction. Buys in green with an upward
indicator. Sells in red with a downward indicator. Large
trades visually distinguished. Older trades scroll off.

Delta panel showing session buy volume, sell volume,
total volume, net delta, and a ratio bar. Delta number
green when positive, red when negative.

Status bar at the bottom showing application name,
symbol, current spread, total events received, and
keyboard shortcut reminders.

Keyboard controls: q or Escape to exit cleanly, r to
reset delta counters.

Automatic reconnection when the feed drops. No manual
restart required. Status bar reflects connection state.

## Build order

Connect to Binance and verify live data arrives before
building anything else. Then build the order book engine
with unit tests. Then the delta engine with unit tests.
Then wire them together. Then build the UI panels. Then
the main event loop.

## Definition of done

All unit tests pass. Binary connects within three seconds.
All three panels show correct live data. Large trades
distinguished in tape. q restores the terminal. Reconnection
works. Ten minutes stable with no panic or memory growth.

---

---

# PHASE 2 — Footprint Chart

## What gets built

A footprint chart panel showing buy and sell volume at
every price level within each time bar. The most
information-dense visualization in order flow trading.

## Core concept

A standard candle shows open, high, low, close, and total
volume for a period. A footprint chart shows all of that
plus, for every price level the market visited inside the
bar, exactly how much volume traded as a buyer aggressor
and how much as a seller aggressor.

No new data source is needed. Every trade from the Phase 1
feed has a price, quantity, and side. The footprint engine
organises these trades into time buckets and price buckets
simultaneously.

## How bars are built

Each bar covers a fixed time period. One minute bars are
the default. Within each bar every price level that saw
trading gets a cell containing two numbers: sell-initiated
volume and buy-initiated volume at that price. When the
period ends the bar is sealed as complete and a new one
begins. Maintain a rolling history of twenty completed
bars plus the current forming bar.

Per bar compute: open price, close price, high price, low
price, total buy volume, total sell volume, total delta,
and the point of control which is the price level with the
highest combined volume in the bar.

Bars must close on clean clock minute boundaries, not one
minute after the first trade. Use wall clock time aligned
to the minute.

## Imbalance signal

When the buy volume at a price level is three times or
more the sell volume, or vice versa, it is an imbalance.
Imbalances are visually highlighted because they indicate
one side overwhelmed the other at a specific price.

## Features

Footprint chart panel showing completed bars side by side
with the current forming bar on the right. Price levels
aligned vertically across all bars. Buy-heavy levels in
green, sell-heavy in red, balanced in neutral. Imbalanced
levels in a brighter style. Point of control visually
distinct in each bar.

Bar summary showing total delta and total volume per bar
at the top or bottom of each column. Positive delta green,
negative red.

Time labels at the bottom of each completed bar. The
forming bar shows a countdown to close in seconds.

Keyboard shortcut to toggle the footprint panel on and off.
The Phase 1 panels remain fully functional regardless.

Volume totals in the footprint must match the delta engine
totals exactly for the same period. They draw from the
same raw trades and must agree.

## Build order

Build the footprint engine with unit tests first. Wire it
into app state alongside the delta engine. Render completed
bars before worrying about the forming bar animation. Then
add imbalance highlighting, point of control, countdown,
and toggle.

## Definition of done

Unit tests pass. Volume totals match delta engine for the
same periods. Completed bars render correctly. Forming bar
updates with each trade. Imbalances visually distinct.
Point of control identifiable. Bar boundaries on clean
clock minutes. Toggle works. Phase 1 unaffected. Ten
minutes stable.

---

---

# PHASE 3 — Advanced Candlestick Chart

## What gets built

A professional-grade candlestick chart with order flow
overlays, volume analysis, technical indicators, and
a volume profile. All data comes from what the application
already has — no new data source required.

## Terminal rendering

Use Unicode half-block characters to double vertical
resolution. Half-block characters divide each cell into
a top and bottom half independently colored. Use braille
characters for smooth line overlays where straight
character lines would look too coarse.

## Candle construction

All candles come from the footprint engine's per-bar data.
The current forming candle updates on every trade.

Support five timeframes switchable by keyboard: one minute,
three minutes, five minutes, fifteen minutes, and one hour.
Switching timeframe reconstructs history by re-aggregating
stored one-minute bars instantly without losing data.

## Three candle types

Standard candles colored by price direction. Bullish close
above open is green, bearish is red, doji is neutral.

Heikin Ashi candles smoothed to filter noise. Each candle's
open is the average of the previous candle's open and close.
Its close is the average of its own open, high, low, close.
Never use Heikin Ashi values as inputs to any indicator.
Indicators always compute from actual market prices.

Delta candles colored by delta direction rather than price.
Positive delta is green regardless of price direction.
Negative delta is red regardless of price direction. This
reveals divergences between price and order flow.

## Volume visualization

Standard volume bars below the candle area proportional
to the maximum visible bar. Colored to match their candle.

Delta split stacked bars as an alternative mode. Buy volume
grows from the bottom in green, sell volume grows from the
top in red within the same bar height.

Volume moving average line overlay on the histogram.
Bars significantly above average are highlighted.

## Price overlays

Session VWAP as a continuous line updating in real time.
Compute using typical price (high plus low plus close
divided by three) multiplied by volume, running sum divided
by running total volume. VWAP resets at session open.

One and two standard deviation bands above and below VWAP.
First band subtly shaded. Second band as a prominent line.

Three configurable EMA lines defaulting to nine, twenty-one,
and fifty periods. Rendered using braille dots for smoothness.
Each toggleable independently. Labels at current values on
the right axis.

Cumulative delta line overlaid on candles with a secondary
axis on the left. Auto-detect and mark bearish divergence
(price new high but delta not) and bullish divergence
(price new low but delta not).

## Volume profile

Horizontal histogram on the right side showing volume at
each price level across the visible session.

Point of control is the highest volume level. Render as a
distinct horizontal line across the full chart with a label.

Value area contains seventy percent of session volume.
Shade it in the profile. Render Value Area High and Value
Area Low as horizontal lines with axis labels.

High volume nodes are significantly above average volume
levels. Prominent in the profile. Low volume nodes are
significantly below average. Subdued color.

## Momentum indicators

A sub-panel below the volume histogram toggled by keyboard.

RSI using Wilder's smoothing with a default period of
fourteen. Reference lines at thirty and seventy. Red above
seventy, green below thirty. Show a waiting indicator until
fourteen complete bars exist.

Delta momentum oscillator measuring rate of change of
cumulative delta. Rising means buying pressure is
accelerating. Falling means selling is accelerating.
Detect and mark divergences from price momentum.

MACD with standard twelve, twenty-six, nine defaults.
MACD line, signal line, and histogram. Histogram green
above signal, red below.

## Navigation and interaction

Price axis on the right with adaptive labels. Current price
highlighted with a dotted horizontal line across the chart.
Time axis at the bottom with session boundary markers.
Subtle grid at labeled price and time positions.

Crosshair navigation with arrow keys. Full horizontal and
vertical lines at cursor position. Data panel showing all
values for the selected candle, positioned to not obscure
the candles being examined.

Alert levels placed by keyboard shortcut at the current
crosshair price. Rendered as dashed horizontal lines with
price labels. Flash status bar on cross. Removable by
keyboard.

Keyboard toggles for: timeframe, candle type, volume mode,
indicator panel, each EMA line, VWAP, volume profile,
delta overlay, scroll, alert placement, and layout mode.

## Layout

Candle chart occupies the lower portion. Phase 1 panels
remain at the top. Footprint and candle chart share the
lower portion, toggled by keyboard. A mode that shows
both in reduced size when the terminal is tall enough.

## Build order

Extend the candle engine first with all derived values and
unit tests. Basic rendering with bodies and wicks. Volume
histogram. VWAP and EMAs. Volume profile. Cumulative delta
and divergence. Momentum indicators. Crosshair, alerts, and
full layout integration.

## Definition of done

All unit tests pass. VWAP matches manual calculation. EMA
and RSI match reference values. All five timeframes work.
All three candle types correct. Volume profile correctly
identifies POC and value area. Divergences marked correctly.
All three indicators render correctly. Crosshair and alerts
work. Terminal resize handled cleanly. Phase 1 and 2
unaffected. Thirty minutes stable with all overlays active.

---

---

# PHASE 4 — Advanced Order Flow Signals

## What gets built

A signals engine that detects five advanced order flow
patterns in real time and annotates existing panels with
alerts. No new data source required.

## Signal 1 — Iceberg detection

A large participant hides their full order size by placing
a small visible order that refreshes repeatedly as it fills.
The price level keeps trading but does not disappear.

Detect by tracking the ratio of total volume traded at a
price level to the maximum visible quantity at that level.
When this ratio exceeds a significant threshold the level
has traded far more than its visible size suggested.
Confirmation signals: price holds despite repeated
aggression, refresh timing is consistent, refresh quantity
is consistent.

Surface by highlighting the level in the DOM with a
distinct color, adding to the signals log, annotating
the footprint chart, and marking the candle chart.

## Signal 2 — Absorption detection

Aggressive orders on one side are consumed by large
passive orders on the other side without price moving.
Sell aggression hits the bid but price does not fall.

Detect by correlating the trade feed with order book
changes. When sell-initiated trades repeatedly hit the
same bid level and that level does not disappear,
absorption is occurring. Quantify by comparing volume
of aggressive trades to change in visible quantity.

Distinguish from iceberg: iceberg is a refreshing
resting order, absorption is a participant actively
adding size as it is consumed. The order book signature
differs.

Surface identically to iceberg with a different visual
indicator and log entry.

## Signal 3 — Pace of tape

Measures trade execution speed relative to session norms.
Sudden acceleration indicates urgency. Slow tape at key
levels may indicate quiet accumulation.

Compute a rolling average of trades per second. Compare
current rate to average. Track buy pace and sell pace
separately. Acceleration in one side without the other
is more significant than overall acceleration.

Render a dynamic pace gauge in the tape panel. Fill
with color intensity proportional to pace relative to
average. Green for buy acceleration, red for sell, neutral
for balanced. Flash and log when pace exceeds three times
the session average.

## Signal 4 — Stop run detection

Price quickly penetrates a key level with high volume then
immediately reverses. Delta during the run opposes price
direction — a downward run shows dominant buy delta as
the large participant absorbs triggered sells.

Detect by monitoring rapid moves through previously
high-volume or multiply-tested levels, combined with delta
divergence where delta does not confirm the price direction.
Reversal must follow within a small number of bars to
confirm the pattern.

Mark retrospectively on the candle chart after the reversal
confirms. Log with the level, direction, and divergence score.

## Signal 5 — Exhaustion detection

A trending market where pace is high and price is making
new extremes but delta is diverging — the dominant side
is weakening as price extends.

Detect by combining pace, delta, and price into a composite
signal. Look for pace acceleration at price extremes with
delta divergence from the prior extreme. High volume at
the extreme where the aggressive side achieves a
disproportionately small price move is additional
confirmation.

Mark on the candle chart at the triggering bar. These are
high-confidence signals and must be visually more prominent
than lower-confidence alerts.

## Signal strength scoring

Each detection scores one to five based on how many
confirmation criteria are met. Score shown in the log
and visual intensity reflects score on chart annotations.

## Signals log panel

Chronological list of all detected signals this session.
Most recent at top. Each entry shows time, type, price,
description, and score. Color coded by type. Filterable
by signal type via keyboard. Full session history retained.

Selecting a log entry navigates the candle chart and
footprint chart to that time for review.

## Configuration

Each signal type has adjustable sensitivity thresholds
accessible via a settings panel. Defaults are conservative.
Changes take effect immediately without restart.

## Build order

Signals engine foundation and scoring system with unit tests.
Iceberg detection with unit tests against synthetic sequences.
Absorption detection distinguishing it from iceberg. Pace
of tape gauge. Stop run and exhaustion detection. Signals
log panel and chart annotation. Configuration panel.

## Definition of done

Unit tests correctly identify and reject patterns for all
signal types. Pace gauge updates correctly. Stop runs appear
retrospectively after confirmed reversals. Exhaustion at
correct extremes with delta confirmation. Log shows all
signals with correct timestamps and scores. Log navigation
jumps chart to correct time. Filtering works. Configuration
changes take effect immediately. Phase 1 through 3 unaffected.
Thirty minutes stable.

---

---

# PHASE 5 — Simulated Paper Trading

## What gets built

A complete paper trading system using live Binance data for
realistic fill simulation. No real broker. No real money.

## Simulated fills

Market orders fill immediately walking the book at current
prices. Each partial fill records price and quantity
separately giving a realistic average fill price with
slippage.

Limit orders rest in the simulation and fill when the live
tape trades through their price. Same price orders fill
in placement order — first in first out.

Stop orders trigger at the stop price and become market
orders. If the market gaps through the stop the fill may
be worse than the stop price. The simulation replicates
this slippage.

Stop limit orders trigger like stops but become limit
orders. If the limit is not reached after trigger the
order does not fill.

## Order lifecycle

Orders move through states: pending on submission, working
when live, partial fill as fills arrive, filled on
completion, cancelled by user, rejected on validation
failure. Every state transition is logged with a timestamp.

Position limits are enforced. Orders that would exceed the
configured maximum position size are rejected with a clear
reason.

## Position tracking

Position expressed as signed quantity. Positive is long,
negative is short, zero is flat. Track average entry price,
total cost basis, unrealised PnL marked to last trade
price, and realised PnL from closed portions. Unrealised
PnL updates on every incoming trade.

## Bracket orders

Three linked orders submitted simultaneously: entry, stop
loss, and profit target. Entry fills activate both stop
and target. When one fills the other is automatically
cancelled — one cancels other. Quantities must match the
filled portion for partial entries.

The user defines a bracket by entry price, stop ticks, and
target ticks. Preview shows absolute prices, risk to reward
ratio, and dollar risk before confirmation.

## Trailing stop

Follows price in the user's favour but never moves against
them. For a long position trailing by ten ticks the stop
starts ten ticks below entry and follows each new high.
On reversal the stop holds at its highest point. Tracks
the extreme price since entry and adjusts on every trade.

## Simulated account

Configurable starting balance defaulting to ten thousand
dollars. Balance adjusts with realised PnL. Unrealised PnL
shown separately. Configurable maximum daily loss limit —
when breached no new orders can be submitted.

Account panel shows balance, session realised PnL, open
position unrealised PnL, total exposure, and remaining
daily loss buffer.

## DOM integration

The Phase 1 DOM gains trading interactivity. Orders placed
from the DOM appear as highlighted rows at their limit
price. Working order quantity visible alongside market
quantity. Cancel individual orders from the DOM by cursor
and key. One-click flatten closes position and cancels
all working orders simultaneously.

## Order entry panel

Full panel for complex orders navigated by keyboard. Fields
for type, side, quantity, price, stop price, and bracket
attachment. Preview shows estimated fill, bracket levels,
risk in ticks, risk in dollars, and resulting position.
Confirm with enter, cancel with escape.

## Slippage tracking

Every fill records intended price versus actual fill price.
Session slippage totals tracked and displayed in analytics
so the user can see the real cost of market orders.

## Keyboard controls

Market buy and sell. Flatten. Cancel all. Increase and
decrease quantity. Open order entry panel. Toggle trading
mode to prevent accidental orders during analysis.

## Build order

Order model and state machine with unit tests for every
state transition. Fill simulation engine with unit tests
against synthetic order books for all order types. Position
tracker with PnL unit tests. Bracket and trailing stop
logic. Account and daily loss limit. DOM integration.
Order entry panel. Full integration and performance
verification.

## Definition of done

All order state transition tests pass. Fill simulation
produces correct prices and slippage for all order types.
PnL correct across complex partial fill sequences. Bracket
OCO cancellation correct. Trailing stop tracks correctly.
Daily loss limit prevents orders when breached. DOM shows
and clears working orders correctly. Flatten closes
position and cancels orders simultaneously. Trading does
not degrade chart or feed performance. Phase 1 through 4
unaffected. Thirty minutes of active simulated trading
with no memory growth.

---

---

# PHASE 6 — Session Analytics

## What gets built

An analytics system that processes the Phase 5 trade
history and produces performance statistics, behavioural
insights, and interactive charts — all within the terminal.

## Outcome metrics

Win rate computed separately for long and short trades.
Average winner and average loser in ticks and dollars.
Profit factor as gross profit divided by gross loss.
Expectancy as win rate times average winner minus loss
rate times average loser — the single most important
number for any trader to understand their edge.

Maximum drawdown as the largest peak to trough decline
in the cumulative PnL curve. Largest winner and largest
loser. Longest winning and losing streaks.

Return on risk as total PnL divided by maximum risk
per trade. Average risk to reward achieved comparing
actual exit to originally intended target and stop.

## Timing metrics

Time of day analysis grouping trades by entry hour.
Win rate, average PnL, and count per hour rendered
as a bar chart. Reveals when the user's edge is
strongest.

Trade duration analysis tracking hold times from
entry to exit. Distribution of hold times revealing
whether the user holds winners shorter than losers —
one of the most common and damaging trading behaviours.

Entry timing within the candle: early, mid, or late
in the bar. Performance statistics by timing group.

## Behavioural metrics

Stop adjustment tracking. Every post-entry stop
movement recorded as widening or tightening. Win rate
for widened stops computed separately. This is the
most important behavioural metric — widening stops
after entry is a sign of poor discipline.

Target adjustment tracking. Ratio of early exits to
target hits. Average ticks left on the table for
early exits. Quantifies how much profit poor exit
discipline costs.

Overtrading detection. Trade frequency per hour
plotted against cumulative PnL versus trade number.
Reveals degradation in performance as a session
progresses.

Revenge trading detection. Entries within a short
window after a loss that exceed typical entry spacing
are flagged as potential revenge trades. Win rate
computed separately for flagged trades.

## Market context metrics

VWAP context grouping trades by whether entry was
above, at, or below VWAP. Win rate and average PnL
per group.

Delta context grouping trades by whether entry
aligned with or opposed the dominant delta direction.

Signal context grouping trades by which Phase 4
signals were active at entry. Performance per signal
type reveals whether signal-based entries outperform
entries with no signals present.

Volatility context grouping trades by pace of tape
and spread at entry time.

## Cumulative PnL chart

Running cumulative PnL as a line chart across all
trades. Zero line overlay. Maximum drawdown shaded
between peak and trough. Selecting a point navigates
the candle chart to that trade.

## Trade log

Complete chronological log of every trade. Columns
for all trade data plus behavioural flags. Sortable
by any column. Selecting a trade navigates the candle
chart to entry time.

## Summary dashboard

Single screen showing all key metrics. Four sections:
performance summary, timing analysis, behavioural
summary, and market context summary. Uses full
terminal space for maximum information density.

## Navigation

Analytics view replaces the live trading panels.
Toggle in and out with one keyboard shortcut. Navigate
between analytics sections with arrow keys or dedicated
shortcuts.

## Build order

Analytics engine with unit tests for all metric
calculations against known trade sequences with
manually computed expected values. Timing metrics.
Behavioural metrics using order history from Phase 5.
Market context metrics. PnL chart and trade log
rendering. Summary dashboard. View switching.

## Definition of done

All metric calculation tests pass with correct values.
Win rate, profit factor, and expectancy correct against
manual computation. Hold time distribution correctly
identifies losers held longer than winners. Stop
widening correctly flagged and win rate computed
separately. Overtrading pattern visible when present.
Revenge trade candidates correctly flagged. All context
groupings correct. PnL chart navigation works. Trade log
sort works. Summary dashboard correct. View switching
clean. Phase 1 through 5 unaffected.

---

---

# PHASE 7 — Multi-Symbol View

## What gets built

The ability to monitor up to eight symbols simultaneously
with a watchlist, correlation analysis, and relative
strength view.

## Architectural change

Phase 7 makes symbol a first-class dimension. The
architecture shifts from one feed and one set of engines
to a feed manager running multiple feeds and an engine
registry holding one instance of every engine per symbol.

The engines themselves do not change. What changes is
that there are now N instances running concurrently. All
existing panels continue to show data for the currently
selected active symbol. Switching the active symbol
instantly updates all panels. No reconnection delay —
all engines have been running continuously.

This refactor must be done carefully. Verify that single
symbol behaviour is completely identical to Phase 6
before adding multi-symbol support.

## Feed manager

Coordinates multiple WebSocket connections. Packs symbol
subscriptions efficiently onto as few connections as
possible respecting Binance's per-connection stream limit.
Handles connection lifecycle per symbol independently.
A symbol can be live, connecting, or stale independent
of all other symbols.

## Symbol registry

One complete engine set per symbol. Instantiated when
the user adds a symbol, destroyed when removed. Stores
a current state summary per symbol for watchlist display:
last price, spread, session delta, session volume,
dominant side, active signal count, connection status.

## Watchlist panel

One row per symbol showing: symbol name, last price,
price change from session open, spread in ticks, delta
direction and magnitude, session volume, active signal
count badge, and an inline sparkline.

The sparkline is a miniature price chart using braille
characters spanning approximately twenty characters wide.
Shows the last thirty minutes of one-minute closes scaled
to fit in one character row. Gives instant directional
and volatility context for every symbol at a glance.

Rows color coded by delta direction. Active signal count
badge draws the eye to symbols needing attention.

Navigate with arrow keys. Enter selects the active symbol
and updates all analysis panels instantly.

## Correlation view

Price correlation matrix showing rolling twenty-minute
correlation between every pair of monitored symbols.
Deep red for strong negative, deep green for strong
positive, neutral for zero. Rows and columns labeled
with symbol names.

Delta correlation matrix — same structure but using
delta rather than price. Price-correlated but
delta-divergent pairs are significant signals.

Lead-lag analysis computing which symbols tend to move
before others. The lag at maximum cross-correlation is
shown as a signed number of seconds per pair.

## Relative strength view

All monitored symbols on one chart normalised to zero
at session open and expressed as percentage change.
Each symbol a distinct colored line. Labels at the
right edge with symbol name and current percentage.
Most recently touched line renders on top.

## Symbol management

Symbol search panel for adding symbols. Up to eight
simultaneously — ninth attempt rejected with a clear
message. Remove by navigating to a watchlist row and
pressing delete, with a confirmation prompt. Reorder
by selecting and moving up or down with keyboard.

## Performance requirements

Adding symbols must not degrade existing symbol
processing. Non-active symbol engines update state
but do not trigger UI redraws. Watchlist updates for
all symbols at approximately two per second. UI
maintains sixty frames per second regardless of symbol
count. No shared mutable state between symbol engines.
Memory scales linearly with symbol count.

## Build order

Refactor engine registry to be symbol-aware and verify
single symbol works identically. Feed manager for
multiple symbols with unit tests for event routing.
Watchlist panel with two symbols active. Active symbol
switching with all panels updating correctly. Correlation
and lead-lag with unit tests. Relative strength view.
Symbol management. Performance validation with eight
symbols for thirty minutes.

## Definition of done

Single-symbol behaviour identical to Phase 6 after
refactor. Eight symbols run simultaneously with
independent feeds and engines. Watchlist renders correct
summaries and updates twice per second. Sparklines
correct. Active symbol switch instantaneous. Correlation
matrix values correct against manual reference. Lead-lag
correctly identifies known leading symbol in synthetic
series. Relative strength normalised correctly. Symbol
add, remove, and reorder work. Eight symbol limit
enforced. Memory less than eight times single-symbol
usage plus reasonable overhead. Sixty fps maintained
with eight active symbols. Phase 1 through 6 unaffected.
Thirty minutes stable with eight symbols.

---

---

# OVERALL DEFINITION OF DONE

Tapeworm is complete when all seven phases have passed
their individual definitions of done and the following
hold simultaneously:

All unit tests across all phases pass with zero failures.

The application connects to Binance, populates all panels,
and is fully interactive within five seconds of launch.

All seven phases of functionality operate correctly and
concurrently without any phase interfering with another.

Eight symbols can be monitored simultaneously with all
overlays, signals, and paper trading active.

The application runs stably for one hour under full load
with no panic, no memory growth, and consistent sixty
frames per second rendering.

q exits cleanly and restores the terminal to its exact
original state regardless of which panels are active.
