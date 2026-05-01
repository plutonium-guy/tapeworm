# TAPEWORM — Phase 3 Agent Build Prompt
## Advanced Candlestick Chart

---

## OPERATING RULES

Same as all previous phases. Plan, build, run, verify, fix,
then move on. Never skip the verification step. Never assume
code compiles without running it. Fix errors immediately
before touching anything else.

Phases 1 and 2 are complete. Do not modify any existing
behaviour. Phase 3 adds an advanced candlestick chart as a
new panel. Everything from Phases 1 and 2 continues to work
unchanged.

---

## WHAT IS BEING BUILT

A professional-grade candlestick chart rendered entirely
inside the terminal. This is not a basic OHLCV chart. It
integrates order flow data, volume analysis, session
statistics, and multiple technical overlays into a single
coherent visualization that updates in real time.

The chart draws from data the application already has —
the trade feed, the footprint engine, and the delta engine
from previous phases. No new external data source is
required.

The design philosophy is the same as the rest of Tapeworm:
show what is actually happening in the market at the level
of order flow, not just price and time.

---

## THE TERMINAL RENDERING CHALLENGE

A terminal is a fixed grid of character cells. Each cell
holds one character and one color pair. There are no pixels,
no subpixel rendering, and no diagonal lines.

To achieve high visual resolution within these constraints
the chart must use Unicode half-block and braille characters.
Half-block characters divide a cell into a top half and a
bottom half, each independently colored. This effectively
doubles the vertical resolution of the chart without
increasing the number of rows. Braille characters divide
a cell into an eight-dot grid and can represent up to eight
independent on-off values per cell, enabling smooth line
rendering for overlays.

Every rendering decision must account for this constraint.
Smooth curves do not exist. Lines are approximated. The
goal is maximum information density within the character
grid.

---

## CANDLE CONSTRUCTION

All candles are constructed from raw trade data the
application already receives. The footprint engine from
Phase 2 already computes open, high, low, close, buy volume,
sell volume, and delta per time bar. The candle chart engine
uses this as its data source and extends it with additional
computed values.

Each completed candle stores everything needed for all
overlays and indicators: the four prices, both volume
components, delta, timestamp, and all derived values
computed at close time such as VWAP contribution,
typical price, and indicator values.

The current forming candle updates on every trade. All
overlays and indicators that depend on the current bar
recalculate in real time as each trade arrives.

---

## SUPPORTED TIMEFRAMES

Support five timeframes selectable by keyboard shortcut.
One minute, three minutes, five minutes, fifteen minutes,
and one hour.

When the user switches timeframes the chart reconstructs
its history by re-aggregating the stored one-minute bars
from the footprint engine. This means no historical data
is lost when switching — the full available history
re-renders at the new timeframe instantly.

The current timeframe is shown clearly in the chart panel
header at all times.

---

## CANDLE TYPES

Support three candle rendering modes selectable by keyboard.

Standard candlesticks show open, high, low, close with
the body colored by price direction. Bullish candles where
close is above open are green. Bearish candles where close
is below open are red. Doji where open and close are within
one tick of each other render in a neutral color with a
distinct shape.

Heikin Ashi candles are a smoothed variant. Each Heikin
Ashi candle's open is the average of the previous candle's
open and close. Its close is the average of the current
candle's open, high, low, and close. Its high and low are
the actual high and low. This smoothing filters out noise
and makes trends easier to read. The engine must compute
Heikin Ashi values correctly from the underlying raw data
and must never confuse Heikin Ashi values with actual
market prices.

Delta candles use the same OHLC structure as standard
candles but color each candle by its delta rather than
its price direction. A candle with positive delta is
green regardless of whether price went up or down. A
candle with negative delta is red regardless of price
direction. This reveals divergences between price movement
and order flow that are invisible on standard charts.

The current candle type is shown in the chart panel header.

---

## VOLUME VISUALIZATION

### Standard Volume Bars

Below the candle area render a volume histogram. Each bar
represents total volume for that candle period. Bar height
is proportional to volume relative to the maximum volume
bar in the current visible window.

Color each volume bar to match its candle. Standard mode
colors by price direction. Delta mode colors by delta
direction.

### Delta Volume Stacked Bars

In an advanced volume mode, split each volume bar into
two segments: the buy volume portion and the sell volume
portion. Stack them within the same bar height. The buy
portion is always green and grows from the bottom. The
sell portion is always red and grows from the top. This
gives an instant per-bar picture of the buying and selling
composition without needing to read the footprint.

### Volume Moving Average

Overlay a line on the volume histogram showing the average
volume over the last twenty bars. Bars that are
significantly above this average are highlighted, because
volume spikes at key price levels are important signals.

---

## PRICE OVERLAYS

### Session VWAP

The Volume Weighted Average Price resets at the start of
each trading session. It represents the average price at
which all volume has transacted during the session,
weighted by how much volume occurred at each price.

Render VWAP as a continuous line overlaid on the candles.
The line updates in real time with each new trade. VWAP
is considered the fairest price of the day and acts as a
dynamic support and resistance level that institutional
traders reference constantly.

Compute VWAP correctly using the running sum of price
multiplied by volume divided by running total volume.
Use the typical price (high plus low plus close divided
by three) for each bar's price contribution.

### VWAP Standard Deviation Bands

Compute one and two standard deviation bands above and
below VWAP. These bands represent statistically unusual
prices relative to the session's volume distribution.
Price beyond two standard deviations is considered
extended and often reverts toward VWAP.

Render the first standard deviation band as a subtle
shaded region. Render the second standard deviation band
as a more prominent line. Label both clearly on the
price axis.

### Exponential Moving Averages

Support three configurable EMA lines rendered as overlays
on the candle chart. Default values are nine period,
twenty-one period, and fifty period EMAs.

Each EMA line is a different color and labeled on the
right side of the chart at its current value. EMA lines
use braille dot rendering to appear smooth despite the
terminal grid constraint.

The user can toggle individual EMA lines on and off with
keyboard shortcuts.

### Cumulative Delta Line

Render the running cumulative delta as a line overlaid
on the candle chart, scaled to a secondary axis on the
left side of the chart. This line shows the cumulative
buying versus selling pressure throughout the session
and makes divergences between price and delta
immediately visible.

When price makes a new high but cumulative delta does not
make a new high, this is a bearish delta divergence.
When price makes a new low but cumulative delta does not
make a new low, this is a bullish delta divergence.
These divergences are automatically detected and marked
on the chart with a small indicator.

---

## VOLUME PROFILE

The volume profile is a horizontal histogram rendered on
the right side of the chart showing how much total volume
traded at each price level across the entire visible
session.

Each horizontal bar extends rightward from the price axis.
Its length represents the relative volume at that price
compared to the maximum volume price level.

### Point of Control

The price level with the highest volume in the session is
the Point of Control. It is the fairest price the market
has found and acts as a magnet. Render it as a visually
distinct horizontal line extending across the entire
chart with a label.

### Value Area

The Value Area is the range of prices containing seventy
percent of the session's total volume. The upper boundary
is the Value Area High and the lower boundary is the
Value Area Low. These levels are widely watched by
professional traders.

Shade the value area subtly on the volume profile. Render
the Value Area High and Value Area Low as horizontal
lines extending across the chart and label them on the
price axis.

### High Volume Nodes and Low Volume Nodes

Price levels with significantly above-average volume are
High Volume Nodes. They act as areas of support and
resistance because they represent prices where the market
found strong two-sided activity. Render them prominently
in the volume profile.

Price levels with significantly below-average volume are
Low Volume Nodes. Price tends to move quickly through
these areas because there was little agreement at these
prices. Render them in a subdued color.

---

## MOMENTUM INDICATORS

Render a separate sub-panel below the volume histogram
for momentum indicators. The user toggles which indicator
is shown in this panel.

### Relative Strength Index

RSI measures the speed and magnitude of recent price
changes on a scale from zero to one hundred. Values above
seventy indicate overbought conditions. Values below
thirty indicate oversold conditions.

Render RSI as a line chart in the indicator panel with
horizontal reference lines at thirty and seventy. Color
the line red when above seventy and green when below
thirty, neutral otherwise.

Compute RSI correctly using Wilder's smoothing method
with a default period of fourteen.

### Delta Momentum Oscillator

This is a Tapeworm-specific indicator not found in
standard charting tools. It measures the rate of change
of cumulative delta rather than price. A rising delta
momentum line means buying pressure is accelerating. A
falling line means selling pressure is accelerating.

When delta momentum diverges from price momentum the
signal is particularly significant. A price making new
highs with falling delta momentum suggests the move is
weakening. Detect and mark these divergences on the
oscillator panel.

### MACD

The Moving Average Convergence Divergence indicator
shows the relationship between two EMAs. Render the MACD
line, signal line, and histogram in the indicator panel.
The histogram bars are green when MACD is above the
signal line and red when below.

Use standard default values of twelve, twenty-six, and
nine periods.

---

## PRICE AXIS AND GRID

The price axis on the right shows price labels at
meaningful intervals that adapt to the current visible
price range. Labels always fall on round numbers
appropriate for the instrument.

The current last trade price is highlighted on the axis
and shown with a dotted horizontal line extending across
the entire chart.

Render a subtle grid of horizontal lines at each labeled
price level and vertical lines at each labeled time
position. The grid uses a subdued color that does not
visually compete with the candles and overlays.

---

## TIME AXIS

The time axis at the bottom shows timestamps at regular
intervals. The interval adapts to the current timeframe
so labels are always meaningful and never overlap.

Mark session open and close times with a distinct vertical
line if they fall within the visible window. The session
boundary is a significant reference for VWAP and volume
profile calculations.

---

## CROSSHAIR AND DATA PANEL

When the user navigates with arrow keys a crosshair
follows the selected candle. The crosshair renders as
a full horizontal line at the cursor price and a full
vertical line at the cursor time, both in a high-contrast
color.

A data panel appears showing all values for the selected
candle: open, high, low, close, volume, buy volume, sell
volume, delta, VWAP at that time, all active EMA values,
RSI value, and timestamp.

The data panel is positioned to never obscure the candles
being examined. If the crosshair is in the right half of
the chart the panel appears on the left and vice versa.

---

## ALERT LEVELS

The user can set horizontal price alert levels using a
keyboard shortcut. When the current price crosses an
alert level the status bar flashes and the level is
highlighted on the chart.

Alert levels persist until manually removed. They render
as dashed horizontal lines across the chart with a small
label showing the price.

---

## KEYBOARD CONTROLS

All controls from Phases 1 and 2 remain unchanged.

New controls for the chart panel:

Timeframe selection — one key per timeframe to switch
between one minute, three minute, five minute, fifteen
minute, and one hour bars.

Candle type cycling — one key to cycle through standard,
Heikin Ashi, and delta candle modes.

Volume mode toggle — one key to toggle between standard
volume bars and delta-split stacked volume bars.

Indicator panel cycling — one key to cycle through RSI,
delta momentum oscillator, and MACD in the indicator panel.

EMA toggles — individual keys to show and hide each of
the three EMA lines independently.

VWAP toggle — one key to show and hide VWAP and its bands.

Volume profile toggle — one key to show and hide the
volume profile on the right side of the chart.

Cumulative delta overlay toggle — one key to show and
hide the cumulative delta line and secondary axis.

Scroll left and right — arrow keys to pan through candle
history. Home key to return to the live edge.

Alert level placement — one key to place an alert at
the current crosshair price. Delete key to remove the
nearest alert level.

---

## LAYOUT

The chart occupies the lower portion of the terminal.
The Phase 1 panels remain at the top. The footprint chart
from Phase 2 and the candle chart from Phase 3 share the
lower portion, toggled by a keyboard shortcut.

Within the candle chart area the vertical space is divided
as follows. The majority of the height goes to the candle
and overlay area. Below that is the volume histogram
including the delta stack view. Below that is the momentum
indicator sub-panel. The volume profile occupies a fixed
width column on the right side of the candle area only.
The price axis is on the right edge. The time axis is at
the bottom of the candle area.

All sub-panels resize proportionally when the terminal
is resized. The chart must handle terminal resize events
cleanly without corrupting the display.

---

## CORRECTNESS REQUIREMENTS

VWAP must be computed as a running session calculation
starting from the first trade of the session. Switching
timeframes must not change the VWAP value because it is
computed from individual trades, not from candles.

EMA values must be computed using correct exponential
smoothing. The smoothing factor is two divided by the
period plus one. The seed value for the first EMA
calculation uses a simple average of the first period's
closes.

RSI must use Wilder's smoothing, not simple averages.
The first RSI value requires at least fourteen complete
bars. Before that the RSI panel shows a waiting indicator.

Volume profile must recompute whenever the visible window
changes, whether by scrolling or by timeframe switching.
The Point of Control, Value Area High, and Value Area Low
are properties of the visible session data, not global
constants.

Heikin Ashi values must be computed from actual OHLC data.
They must never be used as inputs to VWAP, RSI, or any
other indicator. Indicators always compute from actual
market prices, not smoothed values.

---

## BUILD ORDER

Build in this sequence. Verify each sub-phase before
proceeding to the next.

Phase 3a — Extend the candle engine to compute all derived
values per bar: VWAP contribution, EMA inputs, RSI inputs,
delta momentum. Unit tests must verify all calculations
against known reference values.

Phase 3b — Basic candle rendering with half-block
characters. Bodies, wicks, price axis, time axis, grid.
No overlays yet. Verify visually with live data.

Phase 3c — Volume histogram with delta stack mode and
volume moving average line.

Phase 3d — VWAP line and standard deviation bands.
EMA lines using braille dot rendering.

Phase 3e — Volume profile with Point of Control, Value
Area, High Volume Nodes, and Low Volume Nodes.

Phase 3f — Cumulative delta overlay with secondary axis
and divergence detection and marking.

Phase 3g — Momentum indicator sub-panel with RSI, delta
momentum oscillator, and MACD.

Phase 3h — Crosshair with data panel, alert levels,
Heikin Ashi mode, delta candle mode, keyboard controls,
and layout integration with Phase 2.

---

## OUT OF SCOPE FOR THIS PHASE

Drawing tools for trend lines or channels. Pattern
recognition or automated trade signals. Exporting chart
images or data. Mouse interaction. More than five
timeframes. Tick bars, volume bars, or range bars.
Connection to any data source other than the existing
Binance feed. Saving alert levels between sessions.

---

## DEFINITION OF DONE

All unit tests for candle construction and indicator
calculations pass with zero failures.

VWAP values match manual calculation from raw trades.

EMA values match reference values for known input sequences.

RSI values match Wilder's method reference values.

All five timeframes render correctly and switch instantly.

All three candle types render correctly.

Volume profile correctly identifies Point of Control and
Value Area for the visible session.

Cumulative delta overlay correctly marks divergences.

All three momentum indicators render correctly in the
indicator sub-panel.

Crosshair navigation and data panel work across the full
candle history.

Alert levels place and remove correctly and flash on cross.

All keyboard controls work as specified.

Terminal resize is handled cleanly with no display
corruption.

All Phase 1 and Phase 2 features continue to work without
any regression.

The application runs stably for thirty minutes with all
overlays active and no memory growth.
