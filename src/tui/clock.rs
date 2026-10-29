//! Time as the sidebar sees it. Injected into `App` so time-driven behaviour
//! (sweep dates, multi-click windows, auto-close debounce) is testable with
//! a fake clock the test advances by hand.

use std::time::Instant;

pub trait Clock {
    /// Monotonic now, for measuring intervals.
    // First consumers (multi-click, auto-close debounce) land later in spec #22.
    #[allow(dead_code)]
    fn now(&self) -> Instant;
    /// The local calendar date, for stamping swept items.
    fn today(&self) -> chrono::NaiveDate;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn today(&self) -> chrono::NaiveDate {
        chrono::Local::now().date_naive()
    }
}

/// A clock that only moves when told to. Clones share the same time, so a
/// test keeps one handle and boxes another into the app.
#[cfg(test)]
#[derive(Clone)]
pub struct FakeClock {
    base: Instant,
    elapsed: std::rc::Rc<std::cell::Cell<std::time::Duration>>,
    today: chrono::NaiveDate,
}

#[cfg(test)]
impl FakeClock {
    pub fn new(today: chrono::NaiveDate) -> Self {
        FakeClock {
            base: Instant::now(),
            elapsed: Default::default(),
            today,
        }
    }

    pub fn advance(&self, by: std::time::Duration) {
        self.elapsed.set(self.elapsed.get() + by);
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + self.elapsed.get()
    }
    fn today(&self) -> chrono::NaiveDate {
        self.today
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fake_clock_moves_only_when_advanced_and_clones_share_time() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let clock = FakeClock::new(date);
        let boxed: Box<dyn Clock> = Box::new(clock.clone());
        let t0 = boxed.now();
        assert_eq!(boxed.now(), t0);
        clock.advance(Duration::from_millis(350));
        assert_eq!(boxed.now() - t0, Duration::from_millis(350));
        assert_eq!(boxed.today(), date);
    }
}
