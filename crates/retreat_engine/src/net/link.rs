//! A simulated network link between two machines in one process, measured in ticks.
//!
//! Each message sent at tick `t` is delivered at `t + latency + jitter`, or dropped. Jitter and loss
//! come from a seeded [`RetreatRng`], so a test run is reproducible. Messages may arrive out of order
//! when jitter makes a later message overtake an earlier one (as on a real network).

use retreat_rng::RetreatRng;

/// How a link behaves. All times are fixed ticks (1 tick = 1/64 s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkConditions {
    /// Ticks every message takes (one way).
    pub latency_ticks: u32,
    /// Each message takes up to this many extra ticks (uniformly 0..=jitter_ticks).
    pub jitter_ticks: u32,
    /// Chance (0.0 to 1.0) that a message never arrives.
    pub loss: f32,
}

impl LinkConditions {
    /// No delay and no loss.
    pub const PERFECT: LinkConditions = LinkConditions {
        latency_ticks: 0,
        jitter_ticks: 0,
        loss: 0.0,
    };

    /// `Ok` when `loss` is a probability.
    pub fn validate(&self) -> Result<(), String> {
        if self.loss.is_finite() && (0.0..=1.0).contains(&self.loss) {
            Ok(())
        } else {
            Err(format!("link loss must be from 0 to 1, got {}", self.loss))
        }
    }
}

/// One direction of a link: messages go in with `send`, and come out of `deliver` when due.
#[derive(Debug)]
pub struct SimulatedLink<T> {
    conditions: LinkConditions,
    rng: RetreatRng,
    /// `(delivery tick, sequence number, message)`; the sequence number keeps equal ticks in send order.
    in_flight: Vec<(u64, u64, T)>,
    next_sequence: u64,
    sent: u64,
    dropped: u64,
}

impl<T> SimulatedLink<T> {
    pub fn new(conditions: LinkConditions, seed: u64) -> Self {
        Self {
            conditions,
            rng: RetreatRng::from_seed(seed),
            in_flight: Vec::new(),
            next_sequence: 0,
            sent: 0,
            dropped: 0,
        }
    }

    /// Changes the conditions for messages sent from now on.
    pub fn set_conditions(&mut self, conditions: LinkConditions) {
        self.conditions = conditions;
    }

    pub fn conditions(&self) -> LinkConditions {
        self.conditions
    }

    /// Sends `message` at `tick`.
    pub fn send(&mut self, tick: u64, message: T) {
        self.sent += 1;
        let lost = self.conditions.loss > 0.0 && self.rng.next_f32() < self.conditions.loss;
        if lost {
            self.dropped += 1;
            return;
        }
        let jitter = if self.conditions.jitter_ticks == 0 {
            0
        } else {
            self.rng.next_u64() % u64::from(self.conditions.jitter_ticks + 1)
        };
        let due = tick + u64::from(self.conditions.latency_ticks) + jitter;
        self.in_flight.push((due, self.next_sequence, message));
        self.next_sequence += 1;
    }

    /// Every message due at or before `tick`, oldest delivery first.
    pub fn deliver(&mut self, tick: u64) -> Vec<T> {
        let mut due: Vec<(u64, u64, T)> = Vec::new();
        let mut index = 0;
        while index < self.in_flight.len() {
            if self.in_flight[index].0 <= tick {
                due.push(self.in_flight.swap_remove(index));
            } else {
                index += 1;
            }
        }
        due.sort_by_key(|(due_tick, sequence, _)| (*due_tick, *sequence));
        due.into_iter().map(|(_, _, message)| message).collect()
    }

    /// Messages sent so far, and how many of them were dropped.
    pub fn counts(&self) -> (u64, u64) {
        (self.sent, self.dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> SimulatedLink<u32> {
        SimulatedLink::new(
            LinkConditions {
                latency_ticks,
                jitter_ticks,
                loss,
            },
            7,
        )
    }

    #[test]
    fn a_perfect_link_delivers_in_the_same_tick() {
        let mut link = SimulatedLink::new(LinkConditions::PERFECT, 1);
        link.send(5, 10u32);
        assert_eq!(link.deliver(5), vec![10]);
        assert_eq!(link.deliver(6), Vec::<u32>::new());
    }

    #[test]
    fn latency_delays_delivery_by_that_many_ticks() {
        let mut link = link(4, 0, 0.0);
        link.send(10, 1);
        assert!(link.deliver(13).is_empty());
        assert_eq!(link.deliver(14), vec![1]);
    }

    #[test]
    fn messages_keep_send_order_without_jitter() {
        let mut link = link(2, 0, 0.0);
        for message in 0..5 {
            link.send(0, message);
        }
        assert_eq!(link.deliver(2), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn jitter_stays_within_bounds_and_can_reorder() {
        let mut link = link(3, 5, 0.0);
        for message in 0..200u32 {
            link.send(u64::from(message), message);
        }
        let mut arrival: Vec<(u64, u32)> = Vec::new();
        for tick in 0..400u64 {
            for message in link.deliver(tick) {
                let sent_at = u64::from(message);
                assert!(
                    tick >= sent_at + 3 && tick <= sent_at + 3 + 5,
                    "{message} at {tick}"
                );
                arrival.push((tick, message));
            }
        }
        assert_eq!(arrival.len(), 200);
        let order: Vec<u32> = arrival.iter().map(|(_, message)| *message).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_ne!(
            order, sorted,
            "with 5 ticks of jitter some messages overtake others"
        );
    }

    #[test]
    fn loss_drops_about_that_share_and_is_reproducible() {
        let run = || {
            let mut link = link(0, 0, 0.25);
            for message in 0..2000u32 {
                link.send(0, message);
            }
            (link.deliver(0).len(), link.counts())
        };
        let (delivered, (sent, dropped)) = run();
        assert_eq!(sent, 2000);
        assert_eq!(delivered as u64, sent - dropped);
        assert!(
            (400..600).contains(&dropped),
            "dropped {dropped} of 2000 at 25%"
        );
        assert_eq!(run().0, delivered, "the same seed drops the same messages");
    }

    #[test]
    fn total_loss_delivers_nothing_and_loss_is_validated() {
        let mut link = link(0, 0, 1.0);
        link.send(0, 1);
        assert!(link.deliver(100).is_empty());
        assert!(
            LinkConditions {
                loss: 1.5,
                ..LinkConditions::PERFECT
            }
            .validate()
            .is_err()
        );
        assert!(
            LinkConditions {
                loss: f32::NAN,
                ..LinkConditions::PERFECT
            }
            .validate()
            .is_err()
        );
        assert!(LinkConditions::PERFECT.validate().is_ok());
    }
}
