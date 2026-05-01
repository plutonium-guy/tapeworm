# TAPEWORM — Agent Build Prompt

---

## OPERATING RULES

You work in a strict loop. Every step must be verified before
moving to the next one.

Plan what you will build. Build it. Run it. Verify it works.
Fix any errors. Only then move to the next step.

Never write code across multiple components before verifying
the first one compiles. Never assume something works without
running it. If something fails three times, stop and explain
what is blocking you.

---

## WHAT IS TAPEWORM

Tapeworm is a real-time order flow terminal that runs in the
terminal (TUI). It is written in Rust.

It connects to a live market data feed, processes the raw
data stream into meaningful order flow signals, and displays
everything in a terminal UI that updates in real time.

The name comes from the reconstructed tape — the continuous
stream of every trade that executes in the market, revealing
whether buyers or sellers are in control moment to moment.

The product is inspired by Jigsaw Trading, a professional
order flow analysis tool used by futures day traders. Tapeworm
is the core of what Jigsaw does, without broker execution.

---

## DATA SOURCE

Use the Binance public WebSocket API. It is completely free,
requires no API key, no account and no agreements. It provides
two streams that Tapeworm needs: a depth of market stream and
a trade stream. Subscribe to both simultaneously over a single
connection. Use BTCUSDT as the default symbol.

---

## HOW THE MARKET DATA WORKS

The market produces two continuous data streams.

The first is the order book. At any moment the market has
thousands of resting limit orders at different prices. Buyers
place bids below the current price. Sellers place asks above
it. The collection of all these resting orders at every price
level is the order book. It changes constantly as orders are
added, modified and cancelled. Tapeworm must maintain a
correct and current snapshot of this in memory at all times.

The second is the trade feed. Every time a buyer and seller
match, a trade executes. Each trade has a price, a quantity,
and an aggressor side — either a buyer who hit an ask, or a
seller who hit a bid. Tapeworm consumes every trade and uses
it to track buying and selling pressure.

---

## CORE CONCEPTS TO IMPLEMENT

### Order Book State

The order book is the central data structure. It has two sides:
bids sorted by price descending and asks sorted by price
ascending. The application must maintain this state correctly
by applying every incoming update from the feed.

The most important correctness rule is that the book can fall
out of sync if a message is missed. When that happens the book
must be marked as stale and the display must reflect this
rather than showing incorrect data.

Prices must be handled with exact precision internally.
Floating point arithmetic is not reliable for equality checks
and map lookups. The implementation must account for this.

### Delta

Delta is the difference between buy-initiated volume and
sell-initiated volume over the current session.

Every trade is either buyer-initiated (a buyer hit the ask)
or seller-initiated (a seller hit the bid). By tracking these
separately and computing the running difference, delta reveals
whether buying or selling pressure is dominant right now.

Positive delta means buyers are more aggressive.
Negative delta means sellers are more aggressive.

The feed provides enough information to classify every trade
as buyer or seller initiated. Use it correctly.

### Reconstructed Tape

The tape is the chronological stream of every trade, each one
labelled as a buy or a sell. It is called reconstructed
because classifying each trade by aggressor side requires
interpretation of the raw feed data — it is not directly
labelled in most feeds.

A large trade is one that is significantly bigger than typical.
These deserve visual distinction in the tape because they often
indicate institutional activity.

---

## FEATURES TO BUILD

### Feature 1 — DOM Price Ladder

Display the live depth of market as a vertical price ladder.

Show the top price levels on both sides of the book. Asks
appear above the current price in red. Bids appear below in
green. The spread between best bid and best ask is shown as a
separator between the two sides.

Each price level shows the quantity available. A horizontal
bar proportional to that quantity gives an instant visual
sense of where size is concentrated relative to other levels.
The longest bar corresponds to the largest quantity visible.

The best bid and best ask rows are visually emphasized.

When the book is stale or disconnected the panel must clearly
show this status instead of displaying potentially incorrect
price levels.

### Feature 2 — Reconstructed Tape

Display a scrolling list of every trade as it happens.

Each entry shows the time it occurred, the price, the
quantity, and whether it was a buy or a sell. Buys are green
with an upward indicator. Sells are red with a downward
indicator.

The most recent trade always appears at the top. Older trades
scroll down and eventually fall off.

Large trades are visually distinguished from normal trades
so the trader's eye is drawn to them immediately.

### Feature 3 — Delta Panel

Display a summary of cumulative order flow for the session.

Show the total buy volume, total sell volume, total combined
volume, and the net delta (buy minus sell). The delta number
is green when positive and red when negative.

Show a visual ratio bar that represents the proportion of
buying versus selling activity. The bar fills proportionally
with buy volume on one side and sell volume on the other,
making it immediately obvious which side has been dominant.

### Feature 4 — Status Bar

A single line at the bottom of the terminal showing the
application name, the current symbol, the current spread,
the total number of market events received since launch,
and keyboard shortcut reminders.

### Feature 5 — Keyboard Controls

q or Escape — exit the application cleanly and restore
the terminal to its original state.

r — reset the delta counters to zero for a fresh session
without restarting the application.

### Feature 6 — Automatic Reconnection

If the connection to the feed drops for any reason the
application must attempt to reconnect automatically after
a short delay. The user should never need to restart the
binary. The status bar must reflect the connection state.

---

## QUALITY REQUIREMENTS

The UI must update smoothly at approximately 60 frames per
second. The market data feed runs on its own background task.
The UI rendering runs on its own cadence. They must not block
each other under any circumstances.

All internal buffers must have a maximum size. The tape, the
delta history, and any other rolling buffers must not grow
without bound. When they are full, old entries are dropped.

The application must never corrupt the terminal on exit or
crash. Even in the event of a panic, the terminal must be
restored to its normal state.

Unit tests must be written for the order book state logic
and the delta calculation logic. These are the two most
critical pieces of correctness in the entire application.

---

## BUILD ORDER

Build in this sequence. Do not proceed to the next phase
until the current phase is verified working.

Phase 1 — Order book engine with unit tests passing
Phase 2 — Delta engine with unit tests passing
Phase 3 — Feed adapter connecting to Binance and receiving live data
Phase 4 — Wire all three together into a single app state
Phase 5 — Build the TUI and render all panels with live data
Phase 6 — Polish: status bar, reconnection, edge cases

---

## OUT OF SCOPE

Do not build any of the following. They are future work:

Real broker connectivity or order execution of any kind.
Iceberg order detection. Absorption detection. Footprint
charts. Trade journaling or analytics. Historical data replay.
Any database. Multi-symbol view. Configuration files or CLI
arguments. Alerts or notifications.

---

## DEFINITION OF DONE

All unit tests pass with zero failures.
The binary connects to Binance and shows live data within
three seconds of launch.
All three panels update correctly with real market data.
Large trades are visually distinguished in the tape.
Pressing q restores the terminal completely.
Network disconnect and reconnect is handled without crashing.
The application runs stably for ten minutes without panic
or visible memory growth.
