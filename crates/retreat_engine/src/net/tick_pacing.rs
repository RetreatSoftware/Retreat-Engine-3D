//! Keeping a client's tick number just far enough ahead of the server, for the whole game.
//!
//! **The lead is delay.** An input for tick N waits in the server's buffer until tick N, so every
//! tick of lead a client does not need is a tick later that other players see it act. The handshake
//! only guesses the lead (a first slow frame inflates it), and catching up after a stall adds to it.
//! A client that ticks slower than the server (a slow renderer: Bevy simulates at most 250 ms per
//! frame), whose clock drifts, or whose round trip grows would instead send inputs for ticks the
//! server has already run. So the lead is corrected in **both directions**:
//! - the **server** measures each client's *slack* (input tick minus the next tick it will run, when
//!   the input arrives; negative is late), keeps the smallest per window ([`InputWindow`]) and sends
//!   it every [`TIMING_INTERVAL_TICKS`] ticks (`ServerMessage::Timing`);
//! - the **client** ([`ClientPacing`]) runs extra ticks when it is late, slows its clock in
//!   proportion to the lead it does not need ([`drain_time_scale`]), and jumps its tick number when
//!   it is hopelessly behind;
//! - the server spreads the inputs that still arrive late over the following ticks, one per tick,
//!   instead of running them all at once, where only the last would take effect
//!   ([`schedule_input`]).
//!
//! **Latency budget** (localhost or a quiet LAN, steady play): an input waits at most
//! [`TARGET_SLACK_TICKS`] + [`DRAIN_DEADBAND_TICKS`] ticks in the server (about 60 ms), and another
//! player sees the action within [`LOCAL_LATENCY_BUDGET_MS`] of the player seeing it (server wait,
//! the broadcast interval, `VIEW_DELAY_TICKS` and a frame on each side). After a stall or a slow
//! join the lead returns to that within a few seconds. Checked by `tests/engine/net_latency.rs`.
//!
//! Every number is here; everything here is pure.

/// The server reports a client's slack this often.
pub const TIMING_INTERVAL_TICKS: u64 = 8;
/// Slack a late client aims for.
pub const TARGET_SLACK_TICKS: i64 = 2;
/// Slack above the goal that is left alone: frames and the server's own tick make it jitter.
pub const DRAIN_DEADBAND_TICKS: i64 = 2;
/// Lead above the goal and the deadband slows the clock by this fraction per tick: 1/64 drains most
/// of an excess in about a second.
pub const DRAIN_PER_EXCESS_TICK: f32 = 1.0 / 64.0;
/// The clock never slows by more than this fraction (the player's own character stays in motion).
pub const MAX_SLOWDOWN: f32 = 0.5;
/// Most extra lead a client that keeps falling behind aims for, on top of the target.
pub const MAX_CUSHION_TICKS: i64 = 64;
/// One late report adds at most this much cushion: a single stall must not buy lasting delay, while
/// a client that is late again and again still gets there quickly.
pub const MAX_CUSHION_STEP_TICKS: i64 = 6;
/// After this many reports in a row with the room it aims for still there (at most
/// [`DRAIN_DEADBAND_TICKS`] under the goal) the cushion shrinks by a quarter. A client that keeps
/// losing time dips below that again and again and keeps its cushion.
pub const CALM_REPORTS_PER_CUSHION_DECAY: u32 = 4;
/// The clock slows only for lead that this many reports in a row all had to spare, counted from the
/// last time it was late: a client whose inputs arrive in bursts is judged by its tightest one.
pub const DRAIN_WINDOW_REPORTS: usize = 4;
/// Longest time from a player seeing their own action to another player seeing it, on localhost in
/// steady play (see the module docs).
pub const LOCAL_LATENCY_BUDGET_MS: u64 = 220;
/// Most ticks a client runs at once to catch up (one report).
pub const MAX_CATCH_UP_TICKS: u64 = 64;
/// A client later than this does not replay the gap: it jumps its tick number.
pub const RESYNC_BEHIND_TICKS: i64 = 256;
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

/// The clock speed that drains the lead a client does not need: 1 within the deadband above
/// `goal`, slower the further `slack` is beyond it.
pub fn drain_time_scale(slack: i64, goal: i64) -> f32 {
    let excess = slack
        .saturating_sub(goal)
        .saturating_sub(DRAIN_DEADBAND_TICKS)
        .max(0);
    1.0 - (excess as f32 * DRAIN_PER_EXCESS_TICK).min(MAX_SLOWDOWN)
}

/// The client's pacing state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientPacing {
    /// Speed of the client's clock: 1, or below while it drains lead.
    pub time_scale: f32,
    /// Ticks to run now, on top of the clock's.
    pub catch_up: u64,
    /// Ticks to add to the tick number without simulating them.
    pub jump: u64,
    /// Extra lead kept because the client has been late: it grows with every late report (a client
    /// that keeps losing time needs room until the next correction) and shrinks while it is on
    /// time.
    cushion: i64,
    /// Reports in a row with enough room since the cushion last changed.
    calm_reports: u32,
    /// The slack of the latest reports since the client was last late, newest first.
    recent_slack: [Option<i64>; DRAIN_WINDOW_REPORTS],
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
            calm_reports: 0,
            recent_slack: [None; DRAIN_WINDOW_REPORTS],
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
        if slack < -RESYNC_BEHIND_TICKS {
            self.time_scale = 1.0;
            self.recent_slack = [None; DRAIN_WINDOW_REPORTS];
            self.jump = TARGET_SLACK_TICKS.saturating_sub(slack).unsigned_abs();
            return;
        }
        let goal = TARGET_SLACK_TICKS + self.cushion;
        // Late, or about to be late again.
        let late = slack < 0 || (self.cushion > 0 && slack < goal / 2);
        if late {
            // Losing time: keep more room.
            let step = ((goal - slack) / 2).min(MAX_CUSHION_STEP_TICKS);
            self.cushion = (self.cushion + step).min(MAX_CUSHION_TICKS);
        }
        if late || slack < goal - DRAIN_DEADBAND_TICKS {
            self.calm_reports = 0;
        } else {
            self.calm_reports += 1;
            if self.calm_reports >= CALM_REPORTS_PER_CUSHION_DECAY {
                self.calm_reports = 0;
                // A quarter at a time (at least one tick), so a large cushion is gone in seconds.
                self.cushion -= (self.cushion + 3) / 4;
            }
        }
        let goal = TARGET_SLACK_TICKS + self.cushion;
        if late {
            self.recent_slack = [None; DRAIN_WINDOW_REPORTS];
        } else {
            self.recent_slack.rotate_right(1);
            self.recent_slack[0] = Some(slack);
        }
        let spare = self
            .recent_slack
            .iter()
            .try_fold(i64::MAX, |least, slack| slack.map(|slack| least.min(slack)));
        self.time_scale = spare.map_or(1.0, |spare| drain_time_scale(spare, goal));
        if late {
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
        // Back to the target, plus one step of cushion.
        assert_eq!(pacing.catch_up, 18);
        pacing.on_report(report(-30, 55));
        assert_eq!(pacing.catch_up, 18, "one adjustment at a time");
        pacing.adjusted(78);
        pacing.on_report(report(-10, 70));
        assert_eq!(pacing.catch_up, 0, "measured before the adjustment");
        // Little room left: topped up before it is late.
        pacing.on_report(report(1, 80));
        assert_eq!(pacing.catch_up, 10);
    }

    #[test]
    fn a_client_on_time_is_left_alone() {
        let mut pacing = ClientPacing::default();
        for (tick, slack) in [(10, 0), (20, 1), (30, 4)] {
            pacing.on_report(report(slack, tick));
        }
        assert_eq!(
            (pacing.time_scale, pacing.catch_up, pacing.jump),
            (1.0, 0, 0)
        );
    }

    #[test]
    fn lead_beyond_the_deadband_slows_the_clock_in_proportion() {
        assert_eq!(drain_time_scale(-5, 2), 1.0);
        assert_eq!(drain_time_scale(4, 2), 1.0);
        assert_eq!(drain_time_scale(12, 2), 1.0 - 8.0 / 64.0);
        assert_eq!(drain_time_scale(1000, 2), 0.5);
        let mut pacing = ClientPacing::default();
        pacing.on_report(report(26, 1));
        assert_eq!(pacing.time_scale, 1.0, "one report is not enough");
        for tick in 2..=DRAIN_WINDOW_REPORTS as u64 {
            pacing.on_report(report(26, tick));
        }
        assert!(pacing.time_scale < 0.75, "{}", pacing.time_scale);
        pacing.on_report(report(3, 9));
        assert_eq!((pacing.time_scale, pacing.catch_up), (1.0, 0));
    }

    #[test]
    fn a_single_stall_buys_little_lasting_lead_and_it_wears_off() {
        let mut pacing = ClientPacing::default();
        pacing.on_report(report(-100, 10));
        pacing.adjusted(200);
        let steps = MAX_CUSHION_STEP_TICKS;
        assert_eq!(pacing.cushion, steps);
        for index in 0..u64::from(CALM_REPORTS_PER_CUSHION_DECAY) * steps as u64 {
            pacing.on_report(report(TARGET_SLACK_TICKS + steps, 300 + index));
        }
        assert_eq!(pacing.cushion, 0);
        // Late again and again: the room grows up to its limit.
        for index in 0..40u64 {
            pacing.on_report(report(-20, 1000 + index * 100));
            pacing.adjusted(1050 + index * 100);
        }
        assert_eq!(pacing.cushion, MAX_CUSHION_TICKS);
    }

    #[test]
    fn a_client_hopelessly_behind_jumps() {
        let mut pacing = ClientPacing::default();
        pacing.on_report(report(-1000, 20));
        assert_eq!((pacing.jump, pacing.catch_up), (1002, 0));
    }
}
