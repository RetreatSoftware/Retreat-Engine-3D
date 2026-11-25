//! Keeping a client's tick number far enough ahead of the server, for the whole game.
//!
//! The handshake only guesses the lead. A client that ticks slower than the server (a slow
//! renderer: Bevy simulates at most 250 ms per frame), whose clock drifts, or whose round trip grows
//! would send inputs for ticks the server has already run. So:
//! - the **server** measures each client's *slack* (input tick minus the next tick it will run, when
//!   the input arrives; negative is late), keeps the smallest per window ([`InputWindow`]) and sends
//!   it every [`TIMING_INTERVAL_TICKS`] ticks (`ServerMessage::Timing`);
//! - the **client** ([`ClientPacing`]) runs extra ticks when it is late, slows its clock when it is
//!   far ahead, and jumps its tick number when it is hopelessly behind;
//! - the server spreads the inputs that still arrive late over the following ticks, one per tick,
//!   instead of running them all at once, where only the last would take effect
//!   ([`schedule_input`]).
//!
//! Every number is here; everything here is pure.

/// The server reports a client's slack this often.
pub const TIMING_INTERVAL_TICKS: u64 = 8;
/// Slack a late client aims for.
pub const TARGET_SLACK_TICKS: i64 = 2;
/// A client whose smallest slack is above this runs slower until it is not.
pub const MAX_SLACK_TICKS: i64 = 128;
/// Most extra lead a client that keeps falling behind aims for, on top of the target.
pub const MAX_CUSHION_TICKS: i64 = 64;
/// Most ticks a client runs at once to catch up (one report).
pub const MAX_CATCH_UP_TICKS: u64 = 64;
/// A client later than this does not replay the gap: it jumps its tick number.
pub const RESYNC_BEHIND_TICKS: i64 = 256;
/// Clock speed of a client that is too far ahead.
pub const SLOW_TIME_SCALE: f32 = 0.9;
/// Late inputs are spread over at most this many ticks after the server's next one.
pub const MAX_LATE_SPREAD_TICKS: u64 = 32;

/// What the server measured for one client since its last report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingReport {
    /// The smallest slack of the window.
    pub slack: i64,
    /// The oldest input tick of the window.
    pub oldest_tick: u64,
}

/// The server's collector for one client.
#[derive(Clone, Copy, Debug, Default)]
pub struct InputWindow(Option<TimingReport>);

impl InputWindow {
    /// Records an input for `tick` that arrived when `next` was the next tick to run.
    pub fn record(&mut self, next: u64, tick: u64) {
        let slack = i64::try_from(i128::from(tick) - i128::from(next)).unwrap_or(i64::MAX);
        let report = self.0.get_or_insert(TimingReport {
            slack,
            oldest_tick: tick,
        });
        report.slack = report.slack.min(slack);
        report.oldest_tick = report.oldest_tick.min(tick);
    }

    /// The window's report, if any input arrived, and starts a new window.
    pub fn take(&mut self) -> Option<TimingReport> {
        self.0.take()
    }
}

/// The tick the server runs an input on. `spread_end` is per client: the tick of the last late
/// input it scheduled (0 at first). On-time inputs run on their tick, but never before late ones
/// that came first.
pub fn schedule_input(next: u64, tick: u64, spread_end: &mut u64) -> u64 {
    if tick >= next {
        return tick.max(*spread_end);
    }
    let slot = if *spread_end >= next {
        (*spread_end + 1).min(next + MAX_LATE_SPREAD_TICKS)
    } else {
        next
    };
    *spread_end = slot;
    slot
}

/// The client's pacing state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientPacing {
    /// Speed of the client's clock (1, or [`SLOW_TIME_SCALE`]).
    pub time_scale: f32,
    /// Ticks to run now, on top of the clock's.
    pub catch_up: u64,
    /// Ticks to add to the tick number without simulating them.
    pub jump: u64,
    /// Extra lead kept because the client has been late: it grows with every late report (a client
    /// that keeps losing time needs room until the next correction) and shrinks when unused.
    cushion: i64,
    /// Reports about inputs up to this tick were measured before the last adjustment.
    ignore_through: u64,
}

impl Default for ClientPacing {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            catch_up: 0,
            jump: 0,
            cushion: 0,
            ignore_through: 0,
        }
    }
}

impl ClientPacing {
    /// Applies a server report.
    pub fn on_report(&mut self, report: TimingReport) {
        let waiting = self.catch_up > 0 || self.jump > 0;
        if waiting || report.oldest_tick <= self.ignore_through {
            return;
        }
        let slack = report.slack;
        self.time_scale = if slack > MAX_SLACK_TICKS {
            SLOW_TIME_SCALE
        } else {
            1.0
        };
        if slack < -RESYNC_BEHIND_TICKS {
            self.jump = TARGET_SLACK_TICKS.saturating_sub(slack).unsigned_abs();
            return;
        }
        let goal = TARGET_SLACK_TICKS + self.cushion;
        if slack < 0 {
            self.cushion = (self.cushion + (goal - slack) / 2).min(MAX_CUSHION_TICKS);
        } else if slack > goal * 2 + TIMING_INTERVAL_TICKS as i64 {
            self.cushion = (self.cushion - 1).max(0);
        }
        let goal = TARGET_SLACK_TICKS + self.cushion;
        // Late, or about to be late again.
        if slack < 0 || (self.cushion > 0 && slack < goal / 2) {
            self.catch_up = (goal - slack).unsigned_abs().min(MAX_CATCH_UP_TICKS);
        }
    }

    /// The client ran the ticks asked for and is now at `tick`.
    pub fn adjusted(&mut self, tick: u64) {
        self.catch_up = 0;
        self.jump = 0;
        self.ignore_through = tick;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(slack: i64, oldest_tick: u64) -> TimingReport {
        TimingReport { slack, oldest_tick }
    }

    #[test]
    fn the_window_keeps_the_smallest_slack_and_oldest_tick() {
        let mut window = InputWindow::default();
        assert_eq!(window.take(), None);
        window.record(100, 105);
        window.record(101, 99);
        window.record(102, 110);
        assert_eq!(window.take(), Some(report(-2, 99)));
        assert_eq!(window.take(), None);
    }

    #[test]
    fn late_inputs_are_spread_one_per_tick_and_stay_in_order() {
        let mut end = 0;
        // A burst of three late inputs arrives before tick 100.
        let slots: Vec<u64> = [90, 91, 92]
            .into_iter()
            .map(|tick| schedule_input(100, tick, &mut end))
            .collect();
        assert_eq!(slots, [100, 101, 102]);
        assert_eq!(schedule_input(100, 101, &mut end), 102, "not before 92");
        assert_eq!(schedule_input(100, 107, &mut end), 107);
        // Later, a single late input runs at once.
        assert_eq!(schedule_input(200, 150, &mut end), 200);
        let mut end = 0;
        let last = (0..100)
            .map(|tick| schedule_input(500, tick, &mut end))
            .max()
            .unwrap();
        assert_eq!(last, 500 + MAX_LATE_SPREAD_TICKS);
    }

    #[test]
    fn a_late_client_catches_up_once_per_measurement() {
        let mut pacing = ClientPacing::default();
        pacing.on_report(report(-10, 50));
        // The target, plus a cushion of half the shortfall.
        assert_eq!(pacing.catch_up, 18);
        pacing.on_report(report(-30, 55));
        assert_eq!(pacing.catch_up, 18, "one adjustment at a time");
        pacing.adjusted(78);
        pacing.on_report(report(-10, 70));
        assert_eq!(pacing.catch_up, 0, "measured before the adjustment");
        // Little room left: topped up before it is late.
        pacing.on_report(report(1, 80));
        assert_eq!(pacing.catch_up, 7);
    }

    #[test]
    fn a_client_on_time_is_left_alone() {
        let mut pacing = ClientPacing::default();
        for (tick, slack) in [(10, 0), (20, 1), (30, 12)] {
            pacing.on_report(report(slack, tick));
        }
        assert_eq!(pacing, ClientPacing::default());
    }

    #[test]
    fn a_client_far_ahead_slows_and_one_hopelessly_behind_jumps() {
        let mut pacing = ClientPacing::default();
        pacing.on_report(report(MAX_SLACK_TICKS + 1, 1));
        assert_eq!(pacing.time_scale, SLOW_TIME_SCALE);
        pacing.on_report(report(5, 9));
        assert_eq!((pacing.time_scale, pacing.catch_up), (1.0, 0));
        pacing.on_report(report(-1000, 20));
        assert_eq!((pacing.jump, pacing.catch_up), (1002, 0));
    }
}
