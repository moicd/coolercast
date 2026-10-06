//! Small animations for the settings window: values that glide to a new target.
//!
//! The window asks for the value at paint time and keeps repainting only while something still
//! moves, so nothing ticks while it is idle. With Windows animation effects turned off, every
//! change jumps to its target.

use std::time::Instant;

/// How long the edge ahead of a moving pill takes, and the one behind it.
const LEAD_MS: f32 = 220.0;
const TRAIL_MS: f32 = 360.0;
/// Overshoot of [`ease`]: a few percent past the target before settling.
const OVERSHOOT: f32 = 0.9;

/// Ease-out with a slight overshoot, from 0 at `t = 0` to 1 at `t = 1`.
pub fn ease(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + (OVERSHOOT + 1.0) * u * u * u + OVERSHOOT * u * u
}

/// A value heading for a target.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    ms: f32,
}

impl Tween {
    /// A value at rest.
    pub fn new(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            ms: 0.0,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        if self.ms <= 0.0 {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32() * 1000.0;
        (elapsed / self.ms).min(1.0)
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease(self.progress(now))
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn running(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    /// Heads for `to` from wherever the value is now, in `ms` (0 jumps).
    pub fn go(&mut self, to: f32, ms: f32, now: Instant) {
        if to == self.to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now;
        self.ms = ms;
    }
}

/// An interval along one axis, such as the extent of a selection pill. The edge ahead moves faster
/// than the one behind, so the pill stretches as it travels and settles back into shape.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    start: Tween,
    end: Tween,
}

impl Span {
    pub fn new((start, end): (f32, f32), now: Instant) -> Self {
        Self {
            start: Tween::new(start, now),
            end: Tween::new(end, now),
        }
    }

    pub fn value(&self, now: Instant) -> (f32, f32) {
        let (a, b) = (self.start.value(now), self.end.value(now));
        (a.min(b), a.max(b))
    }

    pub fn running(&self, now: Instant) -> bool {
        self.start.running(now) || self.end.running(now)
    }

    /// Moves to a new extent; `motion` false jumps there.
    pub fn go(&mut self, (start, end): (f32, f32), motion: bool, now: Instant) {
        let (lead, trail) = if motion {
            (LEAD_MS, TRAIL_MS)
        } else {
            (0.0, 0.0)
        };
        let forward = start > self.start.target();
        let (start_ms, end_ms) = if forward {
            (trail, lead)
        } else {
            (lead, trail)
        };
        self.start.go(start, start_ms, now);
        self.end.go(end, end_ms, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn ease_starts_at_zero_ends_at_one_and_barely_overshoots() {
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(1.0) - 1.0).abs() < 1e-6);
        let peak = (0..=100)
            .map(|i| ease(i as f32 / 100.0))
            .fold(0.0, f32::max);
        assert!(peak > 1.0 && peak < 1.06, "{peak}");
    }

    #[test]
    fn tween_glides_and_settles() {
        let t0 = Instant::now();
        let mut t = Tween::new(0.0, t0);
        t.go(100.0, 200.0, t0);
        assert_eq!(t.value(t0), 0.0);
        let mid = t.value(at(t0, 100));
        assert!(mid > 50.0 && mid < 100.0, "{mid}");
        assert!(t.running(at(t0, 100)));
        assert_eq!(t.value(at(t0, 200)), 100.0);
        assert!(!t.running(at(t0, 200)));
    }

    #[test]
    fn retargeting_continues_from_the_current_value() {
        let t0 = Instant::now();
        let mut t = Tween::new(0.0, t0);
        t.go(100.0, 200.0, t0);
        let now = at(t0, 50);
        let before = t.value(now);
        t.go(-100.0, 200.0, now);
        assert_eq!(t.value(now), before);
        assert_eq!(t.value(at(t0, 250)), -100.0);
    }

    #[test]
    fn without_motion_it_jumps() {
        let t0 = Instant::now();
        let mut span = Span::new((300.0, 340.0), t0);
        span.go((100.0, 140.0), false, t0);
        assert_eq!(span.value(t0), (100.0, 140.0));
        assert!(!span.running(t0));
    }

    #[test]
    fn pill_stretches_while_it_travels_up() {
        let t0 = Instant::now();
        let mut span = Span::new((300.0, 340.0), t0);
        span.go((100.0, 140.0), true, t0);
        let (top, bottom) = span.value(at(t0, 120));
        assert!(bottom - top > 40.0, "{top}..{bottom}");
        assert!(span.running(at(t0, 300)));
        assert_eq!(span.value(at(t0, 400)), (100.0, 140.0));
        assert!(!span.running(at(t0, 400)));
    }

    #[test]
    fn pill_moving_down_leads_with_its_bottom_edge() {
        let t0 = Instant::now();
        let mut span = Span::new((100.0, 140.0), t0);
        span.go((300.0, 340.0), true, t0);
        let (top, bottom) = span.value(at(t0, 120));
        assert!(bottom - 140.0 > top - 100.0, "{top}..{bottom}");
    }
}
