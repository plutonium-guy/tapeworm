# TAPEWORM — Phase 2 Agent Build Prompt
## Footprint Chart

---

## OPERATING RULES

Same as Phase 1. You work in a strict loop.

Plan what you will build. Build it. Run it. Verify it works.
Fix any errors. Only then move to the next step.

Never write across multiple components before verifying the
first one compiles. Never assume something works without
running it. If something fails three times, stop and explain
what is blocking you.

Phase 1 is already complete. The application has a working
DOM ladder, reconstructed tape, and delta panel fed by a live
Binance WebSocket. Do not modify any Phase 1 behaviour.
Phase 2 adds the footprint chart as a new panel. Everything
built in Phase 1 continues to work unchanged.

---

## WHAT IS A FOOTPRINT CHART

A footprint chart is the most information-dense visualization
in order flow trading. It combines everything a standard
candlestick chart shows with the full breakdown of buying and
selling activity at every single price level within each bar.

A standard candlestick tells you open, high, low, close and
total volume for a time period. A footprint chart tells you
all of that plus, for every price level the market visited
inside that bar, exactly how much volume traded as a buyer
aggressor and how much traded as a seller aggressor.

This means a trader can see not just that a candle moved up,
but which price levels had strong buying, which had strong
selling, and where the balance shifted. It turns a single bar
into a complete story of the auction that happened inside it.

Jigsaw Trading calls this Auction Vista. In the wider industry
it is known as a footprint chart, bid/ask footprint, or
order flow chart. It is the feature that most separates
professional order flow tools from standard charting software.

---

## HOW A FOOTPRINT CHART IS BUILT

### The raw material

Everything needed to build footprint charts already exists in
the Phase 1 trade feed. No new data source is required. Every
trade that arrives has a price, a quantity, and a side
(buyer-initiated or seller-initiated). The footprint engine
simply organizes these trades into time buckets and price
buckets simultaneously.

### The two dimensions

Time dimension — each bar covers a fixed time period.
Start with one minute bars. Each bar has a start time and
an end time. When the period ends, the bar is closed and
a new one begins.

Price dimension — within each bar, every price level that
saw any trading activity gets its own row. The price
granularity matches the tick size of the instrument.

### What each cell contains

At the intersection of a bar and a price level there are
two numbers: the volume that traded as buyer-initiated at
that price, and the volume that traded as seller-initiated
at that price. These are written as a pair, conventionally
shown as sell volume on the left and buy volume on the right.

### What gets computed per bar

For each completed or in-progress bar, compute:

The open price — the first trade price in the bar.
The close price — the most recent trade price in the bar.
The high price — the highest price any trade occurred at.
The low price — the lowest price any trade occurred at.
Total buy volume — sum of all buyer-initiated trade volume.
Total sell volume — sum of all seller-initiated trade volume.
Total delta — buy volume minus sell volume for the whole bar.
Point of control — the price level with the highest total
volume (buy plus sell combined) within the bar.

### The imbalance signal

At each price level within a bar, compare the buy volume to
the sell volume. When one side is significantly larger than
the other — for example three times larger or more — this is
called an imbalance. Imbalances are visually highlighted
because they indicate one side overwhelmed the other at a
specific price, which often acts as a magnet for future price
activity or a barrier to movement.

---

## FEATURES TO BUILD

### Feature 1 — Footprint Engine

Build a new engine that ingests the same trade events that
the delta engine already receives. This engine organizes
trades into a rolling window of time bars.

Each bar accumulates volume at every price level it visits.
When a bar's time period expires it is sealed as complete
and a new bar begins. Maintain a rolling history of the
last twenty completed bars plus the current forming bar.

The engine must correctly handle the transition between bars.
A trade whose timestamp falls in the next period must not
contaminate the previous bar.

### Feature 2 — Footprint Chart Panel

Add a new panel to the TUI that displays the footprint chart.
This panel shows the most recent bars side by side, with the
current forming bar on the right and completed bars to its
left, scrolling leftward as new bars complete.

Each bar is rendered as a vertical column. Within each column,
every price level that saw trading activity shows the sell
volume and buy volume as a pair of numbers side by side.
Price levels are aligned vertically across bars so the same
price always appears at the same vertical position.

The current price level (where the last trade occurred) is
visually highlighted so the trader always knows where the
market is within the footprint.

### Feature 3 — Visual Encoding

Buy-heavy levels show in green. Sell-heavy levels show in red.
Balanced levels show in the default color.

Imbalanced levels — where one side is three times or more
larger than the other — are shown in a brighter or bolder
style to draw the eye immediately.

The point of control for each bar is visually distinguished.
It is the most important level in the bar and should be
immediately identifiable.

The current forming bar looks visually different from
completed bars. Completed bars are static. The forming bar
updates with every new trade.

### Feature 4 — Bar Summary Row

At the top or bottom of each bar column, show a compact
summary: the total delta for that bar (positive in green,
negative in red) and the total volume. This gives a quick
read of each bar's character without needing to read every
level.

### Feature 5 — Time Scale

Show a time label at the bottom of each completed bar
indicating when that bar closed. The current forming bar
shows how much time remains until it closes, counting down
in seconds.

### Feature 6 — Layout Integration

The footprint chart panel takes the full width of the
terminal below the existing three panels from Phase 1, or
replaces one of them if the terminal is too narrow to fit
four panels comfortably. The Phase 1 panels must remain
fully functional regardless of layout choice.

Add a keyboard shortcut to toggle the footprint panel
visibility on and off so the user can choose between the
compact three-panel view from Phase 1 and the expanded
view with the footprint chart.

---

## CORRECTNESS REQUIREMENTS

Bars must close on clean time boundaries. A one-minute bar
must close at exactly the start of the next minute, not one
minute after the first trade arrived. Use wall clock time
aligned to the minute, not elapsed time since start.

Volume totals in the footprint must match the delta engine
totals exactly. If the delta engine says buy volume is X for
a given bar period, the footprint engine must show X when
that bar's levels are summed. They draw from the same raw
trades and must agree.

Price levels within a bar must be sorted so that higher
prices appear higher on screen. The vertical alignment of
prices must be consistent across all visible bars so a
horizontal read across bars at the same vertical position
always represents the same price.

---

## BUILD ORDER

Build in this sequence. Verify each phase before proceeding.

Phase 2a — Footprint engine with unit tests. Tests must
verify that trades are bucketed into the correct bar, that
bar transitions work correctly, that volume totals are
accurate, and that point of control is identified correctly.

Phase 2b — Wire the footprint engine into the existing app
state alongside the delta engine. Both engines receive the
same trade events. Verify with a test that after a set of
known trades the footprint state matches expectations.

Phase 2c — Build the footprint chart panel and integrate
it into the TUI layout. Start by rendering completed bars
correctly before worrying about the forming bar animation.

Phase 2d — Add the forming bar live updates, the countdown
timer, the imbalance highlighting, and the keyboard toggle.

---

## OUT OF SCOPE FOR THIS PHASE

Variable bar sizes (tick bars, volume bars, range bars).
Zoom or scroll through historical bars beyond the rolling
twenty bar window. Saving or exporting footprint data.
Any kind of pattern detection or alerting on footprint
structure. Mouse interaction.

---

## DEFINITION OF DONE

Unit tests for the footprint engine pass with zero failures.
Volume totals in the footprint match the delta engine exactly
for the same time period.
Completed bars render correctly with buy and sell volumes
at each price level.
The forming bar updates in real time with each new trade.
Imbalanced levels are visually distinct from balanced ones.
The point of control is clearly identifiable in each bar.
Bar boundaries fall on clean clock minute boundaries.
The keyboard toggle shows and hides the footprint panel.
All Phase 1 features continue to work without any regression.
The application runs stably for ten minutes with the
footprint panel active and no memory growth.
