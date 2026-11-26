//! "Show or play this once, however often the simulation replays it": the shared rule of one-shot
//! effects (`visual_effects.rs`) and sounds (`sound_events.rs`). Pure.
//!
//! A request is remembered by id for [`EFFECT_MEMORY_TICKS`] live ticks (longer than any rewind).
//! It becomes due at once, or when the client's view of the past reaches the tick that asked for it.

use std::collections::BTreeMap;

use retreat_engine_api::{EFFECT_MEMORY_TICKS, EffectTime};

/// Requests waiting to become due, and the ids already taken.
#[derive(Debug)]
pub(crate) struct OnceQueue<T> {
    /// Ids seen, with the live tick they came at.
    seen: BTreeMap<String, u64>,
    waiting: Vec<(u64, T)>,
    live_ticks: u64,
}

impl<T> Default for OnceQueue<T> {
    fn default() -> Self {
        Self {
            seen: BTreeMap::new(),
            waiting: Vec::new(),
            live_ticks: 0,
        }
    }
}

impl<T> OnceQueue<T> {
    /// Takes `request` unless its id was taken before. `tick` is the tick being simulated.
    pub fn add(&mut self, id: &str, time: EffectTime, tick: u64, request: T) {
        if self.seen.contains_key(id) {
            return;
        }
        self.seen.insert(id.to_string(), self.live_ticks);
        let due = match time {
            EffectTime::Present => 0,
            EffectTime::RemoteView => tick,
        };
        self.waiting.push((due, request));
    }

    /// The requests due at view tick `view` (`None`: no view of the past, everything is due).
    pub fn take_due(&mut self, view: Option<f64>) -> Vec<T> {
        let view = view.unwrap_or(f64::MAX);
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(tick, _)| *tick as f64 <= view);
        self.waiting = waiting;
        due.into_iter().map(|(_, request)| request).collect()
    }

    /// One live (not replayed) tick: old ids are forgotten.
    pub fn live_tick(&mut self) {
        self.live_ticks += 1;
        let now = self.live_ticks;
        self.seen
            .retain(|_, seen| now - *seen <= EFFECT_MEMORY_TICKS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_taken_once_until_it_is_forgotten() {
        let mut queue = OnceQueue::default();
        queue.add("a", EffectTime::Present, 5, 1);
        queue.add("a", EffectTime::Present, 5, 2);
        assert_eq!(queue.take_due(None), [1]);
        for _ in 0..EFFECT_MEMORY_TICKS {
            queue.live_tick();
        }
        queue.add("a", EffectTime::Present, 9, 3);
        assert!(queue.take_due(None).is_empty(), "still remembered");
        queue.live_tick();
        queue.add("a", EffectTime::Present, 9, 4);
        assert_eq!(queue.take_due(None), [4]);
    }

    #[test]
    fn a_remote_view_request_waits_for_the_view() {
        let mut queue = OnceQueue::default();
        queue.add("late", EffectTime::RemoteView, 20, "late");
        queue.add("now", EffectTime::Present, 20, "now");
        assert_eq!(queue.take_due(Some(10.0)), ["now"]);
        assert!(queue.take_due(Some(19.5)).is_empty());
        assert_eq!(queue.take_due(Some(20.0)), ["late"]);
    }
}
