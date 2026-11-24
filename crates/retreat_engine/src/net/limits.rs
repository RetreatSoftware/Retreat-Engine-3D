//! The server's limits and the per-connection guard that enforces them (pure: no `World`, no transport).
//!
//! [`ServerLimits`] is the one source of truth for every number the server hardening uses; its defaults
//! suit the engine's 64 Hz fixed tick. A [`ConnectionGuard`] holds one connection's counters and decides,
//! datagram by datagram, whether to accept it. Every refusal is a [`Refusal`] the server counts in its
//! `ServerStats`; none panics.
//!
//! **Repeat offenders.** A refusal counts as a *violation*. When a connection makes
//! `violations_before_mute` violations inside one rate window it is *muted*: every datagram is dropped
//! (without being decoded) for `mute_ticks`. After `mutes_before_disconnect` mutes it is disconnected.
//! Violations are counted per window, so an honest client with one bad packet is never muted; mutes are
//! counted for the life of the connection.

/// Every number of the server hardening, with defaults for the 64 Hz fixed tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerLimits {
    /// Length of one rate window, in fixed ticks (64 = one second).
    pub rate_window_ticks: u64,
    /// Most datagrams of any kind one connection may send per window (a client sends about one per tick).
    pub max_datagrams_per_window: u32,
    /// Most `Input` messages one connection may send per window.
    pub max_inputs_per_window: u32,
    /// Largest datagram accepted, in bytes (what a transport can carry anyway).
    pub max_datagram_bytes: usize,
    /// Most command lines in one `Input`.
    pub max_lines_per_input: usize,
    /// Longest command line, in bytes.
    pub max_line_bytes: usize,
    /// Longest identity token in a `Hello`, in bytes.
    pub max_token_bytes: usize,
    /// How far ahead of the server's next tick an input's tick may be.
    pub max_ticks_ahead: u64,
    /// How far behind the server's next tick an input's tick may be. A newer late input still runs, late;
    /// an older one is dropped and counted but is *not* a violation: a client on a slow machine falls
    /// further behind real time without being abusive (a software-rendered browser client in CI lagged
    /// more than 512 ticks and was kicked when this was a violation).
    pub max_ticks_behind: u64,
    /// Violations inside one window that mute a connection.
    pub violations_before_mute: u32,
    /// How long a mute lasts, in ticks.
    pub mute_ticks: u64,
    /// Mutes after which the connection is disconnected.
    pub mutes_before_disconnect: u32,
    /// Most connections at once (joined or not); more are disconnected on arrival.
    pub max_connections: usize,
    /// A connection that has not joined (no accepted `Hello`) this many ticks after it connected is
    /// disconnected.
    pub join_timeout_ticks: u64,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            rate_window_ticks: 64,
            max_datagrams_per_window: 192,
            max_inputs_per_window: 128,
            max_datagram_bytes: 1200,
            max_lines_per_input: 16,
            max_line_bytes: 256,
            max_token_bytes: 512,
            max_ticks_ahead: super::MAX_HISTORY_TICKS,
            max_ticks_behind: 4096,
            violations_before_mute: 16,
            mute_ticks: 128,
            mutes_before_disconnect: 3,
            max_connections: 64,
            join_timeout_ticks: 3840,
        }
    }
}

/// Why the server refused a datagram or an input. Each variant has a counter in `ServerStats`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The connection is muted: the datagram was dropped unread.
    Muted,
    /// The datagram is larger than `max_datagram_bytes`.
    DatagramTooLarge,
    /// Too many datagrams this window.
    DatagramRate,
    /// Too many inputs this window.
    InputRate,
    /// The datagram could not be decoded.
    Undecodable,
    /// An input has more lines than `max_lines_per_input` or a line longer than `max_line_bytes`.
    InputShape,
    /// An input's tick is too far ahead of the server's tick (a violation).
    InputTick,
    /// An input's tick is too far behind the server's tick (dropped, not a violation).
    InputTooOld,
    /// A hello's token is longer than `max_token_bytes`.
    TokenTooLong,
}

/// What the server does with the datagram after the guard looked at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Drop it (the reason is counted).
    Refuse(Refusal),
    /// Drop it and disconnect the connection: it keeps offending.
    Disconnect(Refusal),
}

/// One connection's counters.
#[derive(Clone, Debug, Default)]
pub struct ConnectionGuard {
    window_start: u64,
    datagrams: u32,
    inputs: u32,
    violations: u32,
    muted_until: u64,
    mutes: u32,
}

impl ConnectionGuard {
    /// A new guard for a connection that appeared at `tick`.
    pub fn new(tick: u64) -> Self {
        Self {
            window_start: tick,
            ..Self::default()
        }
    }

    /// How many times this connection was muted.
    pub fn mutes(&self) -> u32 {
        self.mutes
    }

    /// True while the connection is muted.
    pub fn is_muted(&self, tick: u64) -> bool {
        tick < self.muted_until
    }

    fn roll_window(&mut self, limits: &ServerLimits, tick: u64) {
        if tick >= self.window_start + limits.rate_window_ticks.max(1) {
            self.window_start = tick;
            self.datagrams = 0;
            self.inputs = 0;
            self.violations = 0;
        }
    }

    /// Looks at a datagram of `len` bytes that arrived at `tick`, before decoding it.
    pub fn admit_datagram(&mut self, limits: &ServerLimits, tick: u64, len: usize) -> Verdict {
        self.roll_window(limits, tick);
        if self.is_muted(tick) {
            return Verdict::Refuse(Refusal::Muted);
        }
        if len > limits.max_datagram_bytes {
            return self.violation(limits, tick, Refusal::DatagramTooLarge);
        }
        self.datagrams = self.datagrams.saturating_add(1);
        if self.datagrams > limits.max_datagrams_per_window {
            return self.violation(limits, tick, Refusal::DatagramRate);
        }
        Verdict::Accept
    }

    /// Counts one decoded `Input` message against the per-window input limit.
    pub fn admit_input(&mut self, limits: &ServerLimits, tick: u64) -> Verdict {
        self.inputs = self.inputs.saturating_add(1);
        if self.inputs > limits.max_inputs_per_window {
            return self.violation(limits, tick, Refusal::InputRate);
        }
        Verdict::Accept
    }

    /// Records a violation found while handling an accepted datagram (bad content).
    pub fn violation(&mut self, limits: &ServerLimits, tick: u64, refusal: Refusal) -> Verdict {
        self.violations = self.violations.saturating_add(1);
        if self.violations >= limits.violations_before_mute {
            self.violations = 0;
            self.mutes += 1;
            self.muted_until = tick + limits.mute_ticks;
            if self.mutes >= limits.mutes_before_disconnect {
                return Verdict::Disconnect(refusal);
            }
        }
        Verdict::Refuse(refusal)
    }
}

/// Why an input's content is refused, or `None` when it is fine: the shape (line count and length) and
/// the tick (not further than the limits from `next_tick`, the tick the server simulates next).
pub fn check_input_content(
    limits: &ServerLimits,
    next_tick: u64,
    tick: u64,
    lines: &[String],
) -> Option<Refusal> {
    if lines.len() > limits.max_lines_per_input
        || lines.iter().any(|line| line.len() > limits.max_line_bytes)
    {
        return Some(Refusal::InputShape);
    }
    if tick > next_tick.saturating_add(limits.max_ticks_ahead) {
        return Some(Refusal::InputTick);
    }
    if tick.saturating_add(limits.max_ticks_behind) < next_tick {
        return Some(Refusal::InputTooOld);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ServerLimits {
        ServerLimits {
            rate_window_ticks: 10,
            max_datagrams_per_window: 5,
            max_inputs_per_window: 3,
            violations_before_mute: 4,
            mute_ticks: 20,
            mutes_before_disconnect: 2,
            ..ServerLimits::default()
        }
    }

    #[test]
    fn a_well_behaved_connection_is_never_refused() {
        // One datagram and one input per tick fits ten per ten-tick window.
        let limits = ServerLimits {
            max_datagrams_per_window: 10,
            max_inputs_per_window: 10,
            ..limits()
        };
        let mut guard = ConnectionGuard::new(0);
        for tick in 0..100 {
            assert_eq!(guard.admit_datagram(&limits, tick, 100), Verdict::Accept);
            assert_eq!(guard.admit_input(&limits, tick), Verdict::Accept);
        }
    }

    #[test]
    fn an_oversized_datagram_is_refused() {
        let limits = limits();
        let mut guard = ConnectionGuard::new(0);
        assert_eq!(
            guard.admit_datagram(&limits, 0, limits.max_datagram_bytes + 1),
            Verdict::Refuse(Refusal::DatagramTooLarge)
        );
        assert_eq!(
            guard.admit_datagram(&limits, 0, limits.max_datagram_bytes),
            Verdict::Accept
        );
    }

    #[test]
    fn datagrams_over_the_window_limit_are_refused_and_the_next_window_starts_clean() {
        let limits = limits();
        let mut guard = ConnectionGuard::new(0);
        for _ in 0..5 {
            assert_eq!(guard.admit_datagram(&limits, 1, 10), Verdict::Accept);
        }
        assert_eq!(
            guard.admit_datagram(&limits, 1, 10),
            Verdict::Refuse(Refusal::DatagramRate)
        );
        assert_eq!(guard.admit_datagram(&limits, 10, 10), Verdict::Accept);
    }

    #[test]
    fn inputs_over_the_window_limit_are_refused() {
        let limits = limits();
        let mut guard = ConnectionGuard::new(0);
        for _ in 0..3 {
            assert_eq!(guard.admit_input(&limits, 1), Verdict::Accept);
        }
        assert_eq!(
            guard.admit_input(&limits, 1),
            Verdict::Refuse(Refusal::InputRate)
        );
    }

    #[test]
    fn repeat_offenders_are_muted_then_disconnected() {
        let limits = limits();
        let mut guard = ConnectionGuard::new(0);
        for _ in 0..3 {
            assert!(matches!(
                guard.violation(&limits, 1, Refusal::Undecodable),
                Verdict::Refuse(Refusal::Undecodable)
            ));
        }
        // The fourth violation in the window mutes the connection.
        assert_eq!(
            guard.violation(&limits, 1, Refusal::Undecodable),
            Verdict::Refuse(Refusal::Undecodable)
        );
        assert_eq!(guard.mutes(), 1);
        assert!(guard.is_muted(2));
        assert_eq!(
            guard.admit_datagram(&limits, 2, 10),
            Verdict::Refuse(Refusal::Muted)
        );
        // The mute ends; a second round of violations ends in a disconnect.
        assert_eq!(guard.admit_datagram(&limits, 21, 10), Verdict::Accept);
        for _ in 0..3 {
            guard.violation(&limits, 22, Refusal::InputShape);
        }
        assert_eq!(
            guard.violation(&limits, 22, Refusal::InputShape),
            Verdict::Disconnect(Refusal::InputShape)
        );
    }

    #[test]
    fn violations_spread_over_many_windows_never_mute() {
        let limits = limits();
        let mut guard = ConnectionGuard::new(0);
        for window in 0..20 {
            let tick = window * 10;
            guard.admit_datagram(&limits, tick, 10);
            guard.violation(&limits, tick, Refusal::Undecodable);
        }
        assert_eq!(guard.mutes(), 0);
    }

    #[test]
    fn input_content_is_checked_for_shape_and_tick() {
        let limits = ServerLimits::default();
        let ok = vec!["physics.push a 1 0 0".to_string()];
        assert_eq!(check_input_content(&limits, 100, 110, &ok), None);
        assert_eq!(check_input_content(&limits, 100, 50, &ok), None);
        let many = vec!["wait 1".to_string(); limits.max_lines_per_input + 1];
        assert_eq!(
            check_input_content(&limits, 100, 100, &many),
            Some(Refusal::InputShape)
        );
        let long = vec!["x".repeat(limits.max_line_bytes + 1)];
        assert_eq!(
            check_input_content(&limits, 100, 100, &long),
            Some(Refusal::InputShape)
        );
        assert_eq!(
            check_input_content(&limits, 100, 100 + limits.max_ticks_ahead + 1, &ok),
            Some(Refusal::InputTick)
        );
        assert_eq!(
            check_input_content(&limits, 10_000, 10_000 - limits.max_ticks_behind - 1, &ok),
            Some(Refusal::InputTooOld)
        );
        assert_eq!(
            check_input_content(&limits, 100, u64::MAX, &ok),
            Some(Refusal::InputTick)
        );
    }
}
