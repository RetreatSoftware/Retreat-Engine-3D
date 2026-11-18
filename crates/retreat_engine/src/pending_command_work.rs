//! Work that a command started but that finishes on a later frame (for example a snapshot,
//! which the GPU captures and the engine saves a frame or two after the command ran).

use bevy::prelude::*;

/// A command script does not run its next line while any work is in flight, and fails
/// when a piece of work reports a failure. Commands call [`start`](Self::start) and the
/// code that completes the work calls [`finish`](Self::finish) or [`fail`](Self::fail).
#[derive(Resource, Debug, Default)]
pub struct PendingCommandWork {
    in_flight: u32,
    failures: Vec<String>,
}

impl PendingCommandWork {
    /// Records that one piece of work has started.
    pub fn start(&mut self) {
        self.in_flight += 1;
    }

    /// Records that one piece of work finished successfully.
    pub fn finish(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Records that one piece of work finished with an error. The script fails with `message`.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.finish();
        self.failures.push(message.into());
    }

    /// True while some work has started and not finished.
    pub fn is_busy(&self) -> bool {
        self.in_flight > 0
    }

    /// Removes and returns the oldest reported failure.
    pub fn take_failure(&mut self) -> Option<String> {
        if self.failures.is_empty() {
            None
        } else {
            Some(self.failures.remove(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_until_every_started_piece_finishes() {
        let mut work = PendingCommandWork::default();
        work.start();
        work.start();
        work.finish();
        assert!(work.is_busy());
        work.finish();
        assert!(!work.is_busy());
    }

    #[test]
    fn a_failure_finishes_the_work_and_is_reported_once() {
        let mut work = PendingCommandWork::default();
        work.start();
        work.fail("disk full");
        assert!(!work.is_busy());
        assert_eq!(work.take_failure().as_deref(), Some("disk full"));
        assert_eq!(work.take_failure(), None);
    }
}
