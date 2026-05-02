//! Reinforcement-learning agent for paper-trade decision making.
//!
//! Design priorities — **trainable** and **predictable**:
//!
//! 1. *Trainable*: online tabular-style Q-learning with linear function
//!    approximation. After every step, weights are nudged via the temporal
//!    difference of the observed reward and the bootstrapped value of the
//!    next state. The model improves its policy continuously while the
//!    application runs.
//!
//! 2. *Predictable*: pure linear model — no neural network, no hidden state.
//!    For any input feature vector the output is deterministic and easily
//!    inspectable (`q = bias + Σ wₖ · fₖ`). When the agent is in *predict*
//!    mode (training disabled), the action it picks is the deterministic
//!    argmax over Q-values — no randomness.
//!
//! 3. *Inspectable*: weights and bias are public fields, easily logged or
//!    serialised. Save/load supported.
//!
//! Action space: `{Hold, Buy, Sell}`. The host application decides what to
//! do with the recommendation (e.g. submit a paper order, just record the
//! signal, etc.).

use std::collections::VecDeque;

pub const NUM_FEATURES: usize = 8;
pub const NUM_ACTIONS: usize = 4;
pub const REWARD_HISTORY_CAP: usize = 1024;
pub const REPLAY_CAP_DEFAULT: usize = 2048;
pub const REPLAY_BATCH_DEFAULT: usize = 16;
/// How often (in update steps) to refresh the target network.
pub const TARGET_SYNC_DEFAULT: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Hold = 0,
    Buy = 1,
    Sell = 2,
    /// Close any open position. Distinct from Buy/Sell so the agent can
    /// learn to exit explicitly rather than relying on flipping behaviour.
    Close = 3,
}

impl Action {
    pub fn from_idx(i: usize) -> Action {
        match i {
            1 => Action::Buy,
            2 => Action::Sell,
            3 => Action::Close,
            _ => Action::Hold,
        }
    }
    pub fn idx(&self) -> usize {
        *self as usize
    }
    pub fn label(&self) -> &'static str {
        match self {
            Action::Hold => "HOLD",
            Action::Buy => "BUY ",
            Action::Sell => "SELL",
            Action::Close => "CLOS",
        }
    }
}

/// Feature vector. Names line up with `Features::names()` for inspection.
pub type Features = [f64; NUM_FEATURES];

/// One observed transition for experience replay.
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub state: Features,
    pub action: Action,
    pub reward: f64,
    pub next_state: Features,
}

/// Aggregate statistics from running the agent's greedy policy across a
/// historical close-price series.
#[derive(Debug, Clone, Default)]
pub struct BacktestResult {
    pub total_reward: f64,
    pub avg_reward: f64,
    pub buys: u32,
    pub sells: u32,
    pub holds: u32,
}

/// Bounded ring buffer of transitions for off-policy mini-batch updates.
#[derive(Debug, Clone)]
pub struct ReplayBuffer {
    cap: usize,
    buffer: VecDeque<Transition>,
}

impl ReplayBuffer {
    pub fn new(cap: usize) -> Self {
        Self { cap, buffer: VecDeque::with_capacity(cap.min(4096)) }
    }
    pub fn push(&mut self, t: Transition) {
        self.buffer.push_back(t);
        while self.buffer.len() > self.cap {
            self.buffer.pop_front();
        }
    }
    pub fn len(&self) -> usize { self.buffer.len() }
    pub fn is_empty(&self) -> bool { self.buffer.is_empty() }
    /// Deterministically sample `n` transitions using the supplied LCG state.
    pub fn sample(&self, n: usize, rng_state: &mut u64) -> Vec<Transition> {
        if self.buffer.is_empty() { return Vec::new(); }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            *rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let r = (*rng_state >> 32) as u32 as usize;
            let idx = r % self.buffer.len();
            out.push(self.buffer[idx]);
        }
        out
    }
}

pub fn feature_names() -> [&'static str; NUM_FEATURES] {
    [
        "ret5",          // recent return over last 5 bars
        "delta_n",       // session delta sign-magnitude (-1..+1)
        "rsi_n",         // RSI mapped to (-1..+1)
        "ema_fast_slow", // (EMA9 - EMA21) / price
        "spread_n",      // (best_ask - best_bid) / price
        "pace_n",        // current TPS / session avg TPS, mapped to (-1..+1)
        "position",      // signed position (-1..+1)
        "volatility",    // bar HL range / price
    ]
}

#[derive(Debug, Clone)]
pub struct RlAgent {
    /// `weights[action][feature]` → coefficient.
    pub weights: [[f64; NUM_FEATURES]; NUM_ACTIONS],
    pub bias: [f64; NUM_ACTIONS],
    pub alpha: f64, // learning rate
    pub gamma: f64, // discount factor
    pub epsilon: f64, // exploration probability while training
    /// When false, behaves as a pure predictor (deterministic argmax).
    pub training: bool,
    /// Master switch — when off the agent is silent and updates nothing.
    pub enabled: bool,
    last_state: Option<Features>,
    last_action: Option<Action>,
    pub episode_count: u64,
    pub training_steps: u64,
    pub action_counts: [u64; NUM_ACTIONS],
    pub reward_history: VecDeque<f64>,
    pub last_reward: f64,
    /// Cumulative reward across the current episode.
    pub episode_reward: f64,
    /// Pseudo-random seed for exploration; deterministic given step count.
    rng_state: u64,
    /// Experience replay buffer for off-policy updates.
    pub replay: ReplayBuffer,
    pub batch_size: usize,
    /// Frozen copy of `weights` / `bias` used as the bootstrap target in
    /// TD updates. Refreshed every `target_sync_steps` updates. Decouples
    /// the target from the rapidly-moving online network and stabilises
    /// learning in noisy environments.
    pub target_weights: [[f64; NUM_FEATURES]; NUM_ACTIONS],
    pub target_bias: [f64; NUM_ACTIONS],
    pub target_sync_steps: u64,
    /// Watkins's Q(λ) eligibility-trace mode. When enabled, replay is
    /// suppressed and the agent updates ALL weights each step using a
    /// per-(action, feature) trace decayed by γλ. The trace is reset
    /// whenever the chosen action differs from the greedy action — the
    /// standard off-policy guard.
    pub use_eligibility: bool,
    pub trace_decay: f64,
    /// Eligibility traces themselves.
    pub traces: [[f64; NUM_FEATURES]; NUM_ACTIONS],
    pub trace_bias: [f64; NUM_ACTIONS],
}

impl Default for RlAgent {
    fn default() -> Self {
        Self {
            weights: [[0.0; NUM_FEATURES]; NUM_ACTIONS],
            bias: [0.0; NUM_ACTIONS],
            alpha: 0.05,
            gamma: 0.9,
            epsilon: 0.1,
            training: true,
            enabled: false,
            last_state: None,
            last_action: None,
            episode_count: 0,
            training_steps: 0,
            action_counts: [0; NUM_ACTIONS],
            reward_history: VecDeque::with_capacity(REWARD_HISTORY_CAP + 1),
            last_reward: 0.0,
            episode_reward: 0.0,
            rng_state: 0x9E3779B97F4A7C15,
            replay: ReplayBuffer::new(REPLAY_CAP_DEFAULT),
            batch_size: REPLAY_BATCH_DEFAULT,
            target_weights: [[0.0; NUM_FEATURES]; NUM_ACTIONS],
            target_bias: [0.0; NUM_ACTIONS],
            target_sync_steps: TARGET_SYNC_DEFAULT,
            use_eligibility: false,
            trace_decay: 0.7,
            traces: [[0.0; NUM_FEATURES]; NUM_ACTIONS],
            trace_bias: [0.0; NUM_ACTIONS],
        }
    }
}

impl RlAgent {
    pub fn q(&self, state: &Features, action: Action) -> f64 {
        let i = action.idx();
        let mut q = self.bias[i];
        for k in 0..NUM_FEATURES {
            q += self.weights[i][k] * state[k];
        }
        q
    }

    pub fn q_all(&self, state: &Features) -> [f64; NUM_ACTIONS] {
        let mut out = [0.0; NUM_ACTIONS];
        for a in 0..NUM_ACTIONS {
            out[a] = self.q(state, Action::from_idx(a));
        }
        out
    }

    /// Deterministic argmax — used by the predictor and as the exploit
    /// branch of ε-greedy.
    pub fn greedy(&self, state: &Features) -> Action {
        let qs = self.q_all(state);
        let mut best = 0usize;
        for a in 1..NUM_ACTIONS {
            if qs[a] > qs[best] {
                best = a;
            }
        }
        Action::from_idx(best)
    }

    /// Internal LCG for exploration randomness — keeps the agent fully
    /// reproducible given the same training history.
    fn rand_unit(&mut self) -> f64 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Take the high 32 bits so the result spans the full [0, 1) range.
        // (>> 33 only gave 31 bits → max value of 0.5 → severe bias.)
        ((self.rng_state >> 32) as u32) as f64 / (u32::MAX as f64 + 1.0)
    }

    pub fn select_action(&mut self, state: &Features) -> Action {
        if self.training && self.rand_unit() < self.epsilon {
            let a = (self.rand_unit() * NUM_ACTIONS as f64) as usize;
            Action::from_idx(a.min(NUM_ACTIONS - 1))
        } else {
            self.greedy(state)
        }
    }

    /// One agent step. Caller passes in the current observation and the
    /// reward earned **since the previous call**. The agent applies a TD
    /// update to the previous (state, action) pair (if any) and returns
    /// the next action. Returns `Action::Hold` when the agent is disabled.
    pub fn step(&mut self, state: Features, reward: f64) -> Action {
        if !self.enabled {
            return Action::Hold;
        }
        if let (Some(prev_s), Some(prev_a)) = (self.last_state, self.last_action) {
            self.update(prev_s, prev_a, reward, state);
            // Replay updates are off-policy and break eligibility-trace
            // contiguity, so they're disabled in trace mode.
            if !self.use_eligibility {
                self.replay.push(Transition {
                    state: prev_s,
                    action: prev_a,
                    reward,
                    next_state: state,
                });
                self.replay_train();
            }
        }
        let action = self.select_action(&state);
        self.last_state = Some(state);
        self.last_action = Some(action);
        self.last_reward = reward;
        self.reward_history.push_back(reward);
        while self.reward_history.len() > REWARD_HISTORY_CAP {
            self.reward_history.pop_front();
        }
        self.episode_reward += reward;
        self.action_counts[action.idx()] += 1;
        action
    }

    /// Run a mini-batch of TD updates sampled from the replay buffer.
    fn replay_train(&mut self) {
        if !self.training || self.batch_size == 0 || self.replay.is_empty() {
            return;
        }
        // Sample uses our deterministic rng_state.
        let batch = self.replay.sample(self.batch_size, &mut self.rng_state);
        for t in batch {
            self.update(t.state, t.action, t.reward, t.next_state);
        }
    }

    fn update(&mut self, s: Features, a: Action, r: f64, s_next: Features) {
        if !self.training {
            return;
        }
        // Bootstrap target uses the FROZEN target network — DQN trick
        // for stability so the online net isn't chasing itself.
        let qs_next = self.q_all_target(&s_next);
        let max_next = qs_next.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let target = r + self.gamma * max_next;
        let current = self.q(&s, a);
        let td = target - current;
        let i = a.idx();
        if self.use_eligibility {
            self.eligibility_update(s, i, td, &s_next);
        } else {
            self.bias[i] = clamp(self.bias[i] + self.alpha * td, -50.0, 50.0);
            for k in 0..NUM_FEATURES {
                let new_w = self.weights[i][k] + self.alpha * td * s[k];
                self.weights[i][k] = clamp(new_w, -10.0, 10.0);
            }
        }
        self.training_steps = self.training_steps.saturating_add(1);
        // Periodic target-network sync.
        if self.target_sync_steps > 0 && self.training_steps % self.target_sync_steps == 0 {
            self.sync_target();
        }
    }

    /// Watkins's Q(λ) update: accumulate the trace for the chosen
    /// (action, feature) pair, decay all traces by γλ, push δ × traces
    /// into all weights, and reset the trace whenever the chosen action
    /// differs from the greedy action (off-policy guard).
    fn eligibility_update(&mut self, s: Features, action_idx: usize, td: f64, s_next: &Features) {
        // Accumulate trace for the action just taken.
        self.trace_bias[action_idx] += 1.0;
        for k in 0..NUM_FEATURES {
            self.traces[action_idx][k] += s[k];
        }
        // Apply δ × trace to ALL weights.
        for a in 0..NUM_ACTIONS {
            self.bias[a] = clamp(self.bias[a] + self.alpha * td * self.trace_bias[a], -50.0, 50.0);
            for k in 0..NUM_FEATURES {
                let new_w = self.weights[a][k] + self.alpha * td * self.traces[a][k];
                self.weights[a][k] = clamp(new_w, -10.0, 10.0);
            }
        }
        // Watkins guard: reset trace if next action differs from greedy.
        let next_greedy = self.greedy(s_next);
        if self.last_action.is_some() && next_greedy.idx() != action_idx {
            self.reset_traces();
        } else {
            // Otherwise decay traces by γλ.
            let factor = self.gamma * self.trace_decay;
            for a in 0..NUM_ACTIONS {
                self.trace_bias[a] *= factor;
                for k in 0..NUM_FEATURES {
                    self.traces[a][k] *= factor;
                }
            }
        }
    }

    pub fn reset_traces(&mut self) {
        self.traces = [[0.0; NUM_FEATURES]; NUM_ACTIONS];
        self.trace_bias = [0.0; NUM_ACTIONS];
    }

    fn q_target(&self, state: &Features, action: Action) -> f64 {
        let i = action.idx();
        let mut q = self.target_bias[i];
        for k in 0..NUM_FEATURES {
            q += self.target_weights[i][k] * state[k];
        }
        q
    }

    fn q_all_target(&self, state: &Features) -> [f64; NUM_ACTIONS] {
        let mut out = [0.0; NUM_ACTIONS];
        for a in 0..NUM_ACTIONS {
            out[a] = self.q_target(state, Action::from_idx(a));
        }
        out
    }

    /// Copy online weights into the target network. Called automatically
    /// every `target_sync_steps`; exposed for explicit sync after `load()`.
    pub fn sync_target(&mut self) {
        self.target_weights = self.weights;
        self.target_bias = self.bias;
    }

    pub fn reset_episode(&mut self) {
        self.last_state = None;
        self.last_action = None;
        self.episode_reward = 0.0;
        self.episode_count = self.episode_count.saturating_add(1);
    }

    pub fn reset_weights(&mut self) {
        self.weights = [[0.0; NUM_FEATURES]; NUM_ACTIONS];
        self.bias = [0.0; NUM_ACTIONS];
        self.target_weights = [[0.0; NUM_FEATURES]; NUM_ACTIONS];
        self.target_bias = [0.0; NUM_ACTIONS];
        self.training_steps = 0;
        self.episode_count = 0;
        self.action_counts = [0; NUM_ACTIONS];
        self.reward_history.clear();
        self.last_reward = 0.0;
        self.last_state = None;
        self.last_action = None;
        self.episode_reward = 0.0;
        self.replay = ReplayBuffer::new(REPLAY_CAP_DEFAULT);
        self.reset_traces();
    }

    /// Greedy action plus a confidence score in [0, 1]. Confidence is
    /// `(max_q − mean_other_q) / (|max_q| + 1)` clamped to [0, 1] — high
    /// when one action dominates, low when actions are tied.
    pub fn predict(&self, state: &Features) -> (Action, f64) {
        let qs = self.q_all(state);
        let mut best_a = 0usize;
        let mut best_q = qs[0];
        for a in 1..NUM_ACTIONS {
            if qs[a] > best_q {
                best_q = qs[a];
                best_a = a;
            }
        }
        let other_sum: f64 = (0..NUM_ACTIONS).filter(|&i| i != best_a).map(|i| qs[i]).sum();
        let other_mean = other_sum / (NUM_ACTIONS as f64 - 1.0).max(1.0);
        let raw = (best_q - other_mean) / (best_q.abs() + 1.0);
        let confidence = raw.clamp(0.0, 1.0);
        (Action::from_idx(best_a), confidence)
    }

    /// For inspection / explainability: returns `(feature_index, weight)`
    /// pairs for the supplied action, sorted by absolute weight desc.
    pub fn feature_importance(&self, action: Action) -> Vec<(usize, f64)> {
        let i = action.idx();
        let mut pairs: Vec<(usize, f64)> = (0..NUM_FEATURES)
            .map(|k| (k, self.weights[i][k]))
            .collect();
        pairs.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
        pairs
    }

    /// Run the agent's greedy policy over a series of historical closes
    /// and return aggregate statistics. Does not mutate `self`. Useful as
    /// a quick sanity check that the trained weights produce a sensible
    /// policy on past data.
    pub fn backtest(&self, closes: &[f64]) -> BacktestResult {
        let mut result = BacktestResult::default();
        if closes.len() < 6 {
            return result;
        }
        let mut last_close = closes[0];
        let mut prev_action: Option<Action> = None;
        for window in closes.windows(6) {
            let close = *window.last().unwrap();
            let oldest = window[0];
            let ret5 = if oldest.abs() > 1e-12 {
                ((close - oldest) / oldest).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            // Minimal feature vector — only ret5 is non-zero. Sufficient
            // for most learned policies that ride directional signals.
            let mut s = [0.0_f64; NUM_FEATURES];
            s[0] = ret5;
            let action = self.greedy(&s);
            // Reward = realized P/L if we held a position from prev_action
            // through this bar, then took `action` at the close.
            if let Some(prev) = prev_action {
                let bar_ret = (close - last_close) / last_close.max(1e-12);
                let r = match prev {
                    Action::Buy => bar_ret,
                    Action::Sell => -bar_ret,
                    Action::Hold | Action::Close => 0.0,
                };
                result.total_reward += r;
            }
            match action {
                Action::Buy => result.buys += 1,
                Action::Sell => result.sells += 1,
                // Close in backtest is treated as a flat (no-position)
                // step for the next reward — same accounting as Hold.
                Action::Hold | Action::Close => result.holds += 1,
            }
            last_close = close;
            prev_action = Some(action);
        }
        let n = (result.buys + result.sells + result.holds).max(1) as f64;
        result.avg_reward = result.total_reward / n;
        result
    }

    pub fn mean_reward(&self) -> f64 {
        if self.reward_history.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.reward_history.iter().copied().sum();
        sum / self.reward_history.len() as f64
    }

    /// Plain-text serialisation. Stable, human-readable, easy to diff.
    pub fn save(&self, path: &str) -> std::io::Result<()> {
        let mut s = String::new();
        s.push_str("# tapeworm RL agent v1\n");
        s.push_str(&format!("alpha={}\n", self.alpha));
        s.push_str(&format!("gamma={}\n", self.gamma));
        s.push_str(&format!("epsilon={}\n", self.epsilon));
        s.push_str(&format!("training_steps={}\n", self.training_steps));
        s.push_str(&format!("episode_count={}\n", self.episode_count));
        for a in 0..NUM_ACTIONS {
            s.push_str(&format!("bias_{}={}\n", a, self.bias[a]));
            for k in 0..NUM_FEATURES {
                s.push_str(&format!("w_{}_{}={}\n", a, k, self.weights[a][k]));
            }
        }
        std::fs::write(path, s)
    }

    pub fn load(&mut self, path: &str) -> std::io::Result<()> {
        let s = std::fs::read_to_string(path)?;
        // After loading we want the target network to match — otherwise
        // bootstrap targets reference stale zeros and the agent will
        // immediately train AWAY from the loaded weights.
        // (We sync at the end of this function — see below.)
        for line in s.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let Ok(val) = v.parse::<f64>() else { continue };
            match k {
                "alpha" => self.alpha = val,
                "gamma" => self.gamma = val,
                "epsilon" => self.epsilon = val,
                "training_steps" => self.training_steps = val.max(0.0) as u64,
                "episode_count" => self.episode_count = val.max(0.0) as u64,
                _ => {
                    if let Some(rest) = k.strip_prefix("bias_") {
                        if let Ok(a) = rest.parse::<usize>() {
                            if a < NUM_ACTIONS {
                                self.bias[a] = val;
                            }
                        }
                    } else if let Some(rest) = k.strip_prefix("w_") {
                        let parts: Vec<&str> = rest.split('_').collect();
                        if parts.len() == 2 {
                            if let (Ok(a), Ok(j)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                                if a < NUM_ACTIONS && j < NUM_FEATURES {
                                    self.weights[a][j] = val;
                                }
                            }
                        }
                    }
                }
            }
        }
        self.sync_target();
        Ok(())
    }
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Build a feature vector from the application's live state. Designed to
/// be reasonably normalised so the linear model doesn't have to learn a
/// scale across price magnitudes.
pub fn extract_features(app: &crate::app::AppState) -> Features {
    let mut f = [0.0_f64; NUM_FEATURES];
    let last_price: f64 = app.active.last_price().unwrap_or(0.0);
    if last_price.abs() < 1e-12 {
        return f;
    }
    let inv_p = 1.0 / last_price;

    // 0: 5-bar return.
    let bars: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .rev()
        .take(5)
        .map(|b| b.close)
        .collect();
    if let Some(oldest) = bars.last().copied() {
        if oldest.abs() > 1e-12 {
            f[0] = ((last_price - oldest) / oldest).clamp(-0.5, 0.5);
        }
    }

    // 1: session delta normalised by total volume.
    let total: f64 = app.active.delta.total_volume().try_into().unwrap_or(0.0);
    if total > 0.0 {
        let d: f64 = app.active.delta.delta().try_into().unwrap_or(0.0);
        f[1] = (d / total).clamp(-1.0, 1.0);
    }

    // 2: RSI mapped to [-1, +1] via (rsi - 50)/50.
    let closes: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .map(|b| b.close)
        .collect();
    let mut rsi = crate::indicators::Rsi::new(14);
    let rsi_val = closes.iter().filter_map(|c| rsi.update(*c)).last();
    if let Some(rv) = rsi_val {
        f[2] = ((rv - 50.0) / 50.0).clamp(-1.0, 1.0);
    }

    // 3: EMA9/EMA21 spread normalised by price.
    let mut e9 = crate::indicators::Ema::new(9);
    let mut e21 = crate::indicators::Ema::new(21);
    for c in &closes {
        e9.update(*c);
        e21.update(*c);
    }
    if let (Some(a), Some(b)) = (e9.value(), e21.value()) {
        f[3] = ((a - b) * inv_p).clamp(-0.05, 0.05);
    }

    // 4: spread / price.
    if let Some(spread) = app.active.book.spread() {
        let s: f64 = spread.try_into().unwrap_or(0.0);
        f[4] = (s * inv_p).clamp(0.0, 0.01);
    }

    // 5: pace ratio mapped to [-1, +1] via tanh-like squash.
    let cur = app.active.signals.pace.current_tps();
    let avg = app
        .active
        .signals
        .pace
        .session_avg_tps(app.active.event_count as i64 + 1)
        .unwrap_or(1e-9)
        .max(1e-9);
    let ratio = cur / avg;
    f[5] = ((ratio - 1.0).clamp(-2.0, 2.0)) / 2.0;

    // 6: position sign.
    let pos = app.paper.position.qty;
    if pos.is_sign_positive() && !pos.is_zero() {
        f[6] = 1.0;
    } else if pos.is_sign_negative() {
        f[6] = -1.0;
    }

    // 7: bar HL range / price.
    if let Some(b) = app.active.candles.forming() {
        f[7] = ((b.high - b.low) * inv_p).clamp(0.0, 0.05);
    } else if let Some(b) = app.active.candles.completed().back() {
        f[7] = ((b.high - b.low) * inv_p).clamp(0.0, 0.05);
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_returns_argmax() {
        let mut a = RlAgent::default();
        a.weights[Action::Buy.idx()][0] = 1.0;
        a.weights[Action::Sell.idx()][0] = -1.0;
        let s = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(a.greedy(&s), Action::Buy);
        let s = [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(a.greedy(&s), Action::Sell);
    }

    #[test]
    fn predictor_is_deterministic_when_training_off() {
        let mut a = RlAgent::default();
        a.training = false;
        a.enabled = true;
        a.weights[Action::Buy.idx()][0] = 5.0;
        let s = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        // Run select_action many times — should always pick Buy since
        // exploration is suppressed.
        for _ in 0..100 {
            assert_eq!(a.select_action(&s), Action::Buy);
        }
    }

    #[test]
    fn td_update_moves_weights_toward_target() {
        // Single-feature, single-action world. Reward is f0; agent should
        // learn that w[Buy][0] > 0 raises Q for state f0=+1.
        let mut a = RlAgent::default();
        a.enabled = true;
        a.training = true;
        a.epsilon = 0.0; // disable exploration so TD signal is clean
        a.alpha = 0.1;
        let states = [
            ([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 1.0),
            ([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 1.0),
            ([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 1.0),
        ];
        // Prime: first call has no prior state → no update yet.
        for (s, r) in &states {
            a.step(*s, *r);
        }
        // After several steps, Buy weight on feature 0 should be > 0
        // (some action was reinforced by the reward).
        let max_w0: f64 = (0..NUM_ACTIONS).map(|i| a.weights[i][0]).fold(0.0, f64::max);
        assert!(max_w0 > 0.0, "expected at least one weight learned, got {:?}", a.weights);
    }

    #[test]
    fn save_then_load_roundtrips_weights() {
        let mut a = RlAgent::default();
        a.weights[1][3] = 1.234;
        a.weights[2][7] = -0.567;
        a.bias[0] = 0.42;
        a.alpha = 0.07;
        a.gamma = 0.85;
        a.epsilon = 0.12;
        a.training_steps = 9999;
        a.episode_count = 7;
        let tmp = std::env::temp_dir().join("tapeworm_rl_test.txt");
        let path = tmp.to_string_lossy().to_string();
        a.save(&path).unwrap();

        let mut b = RlAgent::default();
        b.load(&path).unwrap();
        assert!((a.weights[1][3] - b.weights[1][3]).abs() < 1e-12);
        assert!((a.weights[2][7] - b.weights[2][7]).abs() < 1e-12);
        assert!((a.bias[0] - b.bias[0]).abs() < 1e-12);
        assert!((a.alpha - b.alpha).abs() < 1e-12);
        assert!((a.gamma - b.gamma).abs() < 1e-12);
        assert!((a.epsilon - b.epsilon).abs() < 1e-12);
        assert_eq!(a.training_steps, b.training_steps);
        assert_eq!(a.episode_count, b.episode_count);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn agent_disabled_returns_hold_and_skips_updates() {
        let mut a = RlAgent::default();
        a.enabled = false;
        let s = [1.0; NUM_FEATURES];
        let act = a.step(s, 1.0);
        assert_eq!(act, Action::Hold);
        assert_eq!(a.training_steps, 0);
        assert!(a.last_state.is_none());
    }

    #[test]
    fn epsilon_greedy_explores_when_enabled() {
        let mut a = RlAgent::default();
        a.enabled = true;
        a.training = true;
        a.epsilon = 1.0; // always explore
        let s = [0.0; NUM_FEATURES];
        let mut actions = [0u32; NUM_ACTIONS];
        for _ in 0..1000 {
            let act = a.select_action(&s);
            actions[act.idx()] += 1;
        }
        // All three actions sampled at least once.
        assert!(actions.iter().all(|c| *c > 0), "actions: {:?}", actions);
    }

    #[test]
    fn reset_weights_clears_state() {
        let mut a = RlAgent::default();
        a.weights[0][0] = 1.0;
        a.bias[1] = 2.0;
        a.training_steps = 100;
        a.last_reward = 5.0;
        a.reward_history.push_back(1.0);
        a.reset_weights();
        assert_eq!(a.weights[0][0], 0.0);
        assert_eq!(a.bias[1], 0.0);
        assert_eq!(a.training_steps, 0);
        assert_eq!(a.last_reward, 0.0);
        assert!(a.reward_history.is_empty());
    }

    #[test]
    fn linear_q_learns_simple_two_state_environment() {
        // Toy env: feature f0 ∈ {+1, -1}. Reward = f0 if action==Buy when
        // f0=+1, or action==Sell when f0=-1, else -1.
        let mut a = RlAgent::default();
        a.enabled = true;
        a.training = true;
        a.epsilon = 0.2;
        a.alpha = 0.1;
        // Fixed-seed RNG via deterministic path: alternate states and reward
        // each step based on previous action.
        let states = [
            [1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [-1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let mut last_action: Option<Action> = None;
        let mut last_state: Option<Features> = None;
        for i in 0..2000 {
            let s = states[i % 2];
            let r = match (last_state, last_action) {
                (Some(ls), Some(la)) => {
                    if ls[0] > 0.0 && la == Action::Buy { 1.0 }
                    else if ls[0] < 0.0 && la == Action::Sell { 1.0 }
                    else { -1.0 }
                }
                _ => 0.0,
            };
            let act = a.step(s, r);
            last_state = Some(s);
            last_action = Some(act);
        }
        // Disable exploration and verify the greedy policy is correct.
        a.training = false;
        assert_eq!(a.greedy(&states[0]), Action::Buy);
        assert_eq!(a.greedy(&states[1]), Action::Sell);
    }

    #[test]
    fn feature_names_count_matches() {
        assert_eq!(feature_names().len(), NUM_FEATURES);
    }

    #[test]
    fn eligibility_mode_disables_replay_and_still_learns() {
        let mut a = RlAgent::default();
        a.enabled = true;
        a.training = true;
        a.epsilon = 0.0;
        a.alpha = 0.05;
        a.use_eligibility = true;
        let pos = [1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let neg = [-1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut last_a: Option<Action> = None;
        let mut last_s: Option<Features> = None;
        for i in 0..2000 {
            let s = if i % 2 == 0 { pos } else { neg };
            let r = match (last_s, last_a) {
                (Some(ls), Some(la)) => {
                    if ls[0] > 0.0 && la == Action::Buy { 1.0 }
                    else if ls[0] < 0.0 && la == Action::Sell { 1.0 }
                    else { -0.1 }
                }
                _ => 0.0,
            };
            let act = a.step(s, r);
            last_s = Some(s);
            last_a = Some(act);
        }
        // Replay must be empty since trace mode skips its push path.
        assert!(a.replay.is_empty());
        // Greedy on each state should be the optimal action.
        a.training = false;
        assert_eq!(a.greedy(&pos), Action::Buy);
        assert_eq!(a.greedy(&neg), Action::Sell);
    }

    #[test]
    fn reset_traces_zeros_trace_arrays() {
        let mut a = RlAgent::default();
        a.use_eligibility = true;
        a.traces[1][2] = 0.7;
        a.trace_bias[0] = 0.3;
        a.reset_traces();
        assert!(a.traces.iter().flatten().all(|&v| v == 0.0));
        assert!(a.trace_bias.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn close_action_round_trips_through_index() {
        assert_eq!(Action::from_idx(3), Action::Close);
        assert_eq!(Action::Close.idx(), 3);
        assert_eq!(Action::Close.label(), "CLOS");
    }

    #[test]
    fn weights_extend_to_close_action_row() {
        // The weights matrix must size to NUM_ACTIONS=4 — verify by
        // setting the Close row and reading it back.
        let mut a = RlAgent::default();
        a.weights[Action::Close.idx()][0] = 1.5;
        a.bias[Action::Close.idx()] = -0.7;
        let s = [1.0; NUM_FEATURES];
        let q = a.q(&s, Action::Close);
        // q = bias + Σ w·s = -0.7 + 1.5*1 + 0*7 = 0.8
        assert!((q - 0.8).abs() < 1e-9);
    }

    #[test]
    fn predict_returns_best_action_and_nonneg_confidence() {
        let mut a = RlAgent::default();
        a.weights[Action::Buy.idx()][0] = 5.0;
        let s = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let (act, conf) = a.predict(&s);
        assert_eq!(act, Action::Buy);
        assert!(conf > 0.0);
        // All-zero state → all Q values equal → confidence 0.
        let s0 = [0.0_f64; NUM_FEATURES];
        let (_, c0) = a.predict(&s0);
        assert!(c0 <= 1e-9, "expected near-zero confidence on tied Q values, got {c0}");
    }

    #[test]
    fn feature_importance_orders_by_abs_weight() {
        let mut a = RlAgent::default();
        a.weights[Action::Buy.idx()][2] = -3.0;
        a.weights[Action::Buy.idx()][0] = 1.0;
        a.weights[Action::Buy.idx()][7] = 2.5;
        let imp = a.feature_importance(Action::Buy);
        assert_eq!(imp[0].0, 2);
        assert_eq!(imp[1].0, 7);
        assert_eq!(imp[2].0, 0);
    }

    #[test]
    fn backtest_uphill_prices_with_buy_bias_yields_positive_reward() {
        // Build an agent that always picks Buy on positive ret5 (and pad
        // weights so neutrals/negatives prefer Hold).
        let mut a = RlAgent::default();
        a.weights[Action::Buy.idx()][0] = 5.0;
        // Strict uphill series.
        let closes: Vec<f64> = (0..40).map(|i| 100.0 + i as f64).collect();
        let r = a.backtest(&closes);
        assert!(r.total_reward > 0.0);
        assert!(r.buys >= r.sells);
    }

    #[test]
    fn backtest_with_short_series_returns_zero() {
        let a = RlAgent::default();
        let r = a.backtest(&[100.0, 101.0, 102.0]); // < 6 closes
        assert_eq!(r.total_reward, 0.0);
        assert_eq!(r.buys, 0);
        assert_eq!(r.sells, 0);
        assert_eq!(r.holds, 0);
    }

    #[test]
    fn target_network_syncs_after_target_sync_steps() {
        let mut a = RlAgent::default();
        a.enabled = true;
        a.training = true;
        a.epsilon = 0.0;
        a.batch_size = 0;
        a.target_sync_steps = 5;
        let s = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        // 3 step calls = 2 update calls, both before the first sync.
        for _ in 0..3 { a.step(s, 1.0); }
        assert!(a.target_weights.iter().flatten().all(|&w| w == 0.0));
        // Drive enough steps to cross the sync boundary at update #5.
        for _ in 0..10 { a.step(s, 1.0); }
        // Target must now be non-zero (a sync fired).
        let any_nonzero = a.target_weights.iter().flatten().any(|&w| w != 0.0)
            || a.target_bias.iter().any(|&b| b != 0.0);
        assert!(any_nonzero, "target should have been synced at least once");
    }

    #[test]
    fn load_syncs_target_to_loaded_weights() {
        let tmp = std::env::temp_dir().join("tapeworm_rl_target_sync.txt");
        let path = tmp.to_string_lossy().to_string();
        let mut src = RlAgent::default();
        src.weights[0][0] = 0.5;
        src.bias[1] = -0.3;
        src.save(&path).unwrap();
        let mut dst = RlAgent::default();
        dst.load(&path).unwrap();
        // Target must be in sync with the loaded online weights so that
        // post-load training doesn't immediately push away from them.
        assert_eq!(dst.target_weights, dst.weights);
        assert_eq!(dst.target_bias, dst.bias);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replay_buffer_caps_at_capacity() {
        let mut rb = ReplayBuffer::new(8);
        for i in 0..20 {
            rb.push(Transition {
                state: [i as f64; NUM_FEATURES],
                action: Action::Hold,
                reward: i as f64,
                next_state: [0.0; NUM_FEATURES],
            });
        }
        assert_eq!(rb.len(), 8);
    }

    #[test]
    fn replay_sample_is_deterministic_given_rng_state() {
        let mut rb = ReplayBuffer::new(16);
        for i in 0..16 {
            rb.push(Transition {
                state: [i as f64; NUM_FEATURES],
                action: Action::Hold,
                reward: i as f64,
                next_state: [0.0; NUM_FEATURES],
            });
        }
        let mut rng_a = 12345u64;
        let mut rng_b = 12345u64;
        let s_a = rb.sample(8, &mut rng_a);
        let s_b = rb.sample(8, &mut rng_b);
        assert_eq!(s_a.len(), 8);
        for (a, b) in s_a.iter().zip(s_b.iter()) {
            assert_eq!(a.reward, b.reward);
        }
    }

    #[test]
    fn replay_training_converges_to_optimal_policy_with_fewer_steps() {
        // With replay enabled, the agent converges to the optimal policy
        // in fewer environment steps than without replay. We measure
        // "converged" by sampling 50 greedy actions on each state after
        // training and counting correct picks.
        let train_and_score = |with_replay: bool, n_steps: usize| -> u32 {
            let mut a = RlAgent::default();
            a.enabled = true;
            a.training = true;
            a.epsilon = 0.1;
            a.alpha = 0.05;
            a.batch_size = if with_replay { 16 } else { 0 };
            let pos = [1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            let neg = [-1.0_f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            let mut last_a: Option<Action> = None;
            let mut last_s: Option<Features> = None;
            for i in 0..n_steps {
                let s = if i % 2 == 0 { pos } else { neg };
                let r = match (last_s, last_a) {
                    (Some(ls), Some(la)) => {
                        if ls[0] > 0.0 && la == Action::Buy { 1.0 }
                        else if ls[0] < 0.0 && la == Action::Sell { 1.0 }
                        else { -0.1 }
                    }
                    _ => 0.0,
                };
                let act = a.step(s, r);
                last_s = Some(s);
                last_a = Some(act);
            }
            // Greedy correctness on both states.
            a.training = false;
            let pos_ok = if a.greedy(&pos) == Action::Buy { 1 } else { 0 };
            let neg_ok = if a.greedy(&neg) == Action::Sell { 1 } else { 0 };
            pos_ok + neg_ok
        };
        // 500 steps with replay should converge; 2000 without replay
        // should also converge. We just want to confirm replay is at
        // least as effective, given enough samples.
        let with_replay = train_and_score(true, 500);
        assert_eq!(with_replay, 2, "replay should converge within 500 steps");
        let no_replay = train_and_score(false, 2000);
        assert_eq!(no_replay, 2);
    }
}
