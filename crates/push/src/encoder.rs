//! What an encoder's steps are worth (`docs/specs/push-2.md`, R9).

use std::time::{Duration, Instant};

/// A continuous parameter moves this share of its range a step: 1/200.
const COARSE: f64 = 200.0;
/// With Shift held: 1/2000.
const FINE: f64 = 2000.0;
/// A list or stepped parameter moves one value for this many steps.
pub const LIST_STEPS: i32 = 10;
/// A turn that rests this long starts again from where the document has the
/// parameter.
const REST: Duration = Duration::from_millis(500);

/// The relative value an encoder sends, as steps: 1 to 63 clockwise, 127
/// down to 64 anticlockwise (two's complement, R9).
#[must_use]
pub const fn steps(value: u8) -> i32 {
    match value {
        1..=63 => value as i32,
        64..=127 => value as i32 - 128,
        _ => 0,
    }
}

/// The share of its range a continuous parameter moves for `steps` (R9).
#[must_use]
pub fn fraction(steps: i32, shift: bool) -> f64 {
    f64::from(steps) / if shift { FINE } else { COARSE }
}

/// A continuous parameter an encoder is turning. Steps that arrive before
/// the document shows the last change add up from where the last one left it.
#[derive(Default)]
pub struct Moving {
    value: Option<f64>,
    last: Option<Instant>,
}

impl Moving {
    /// A finger came on the encoder: the next step starts from the
    /// document's value.
    pub fn touch(&mut self) {
        self.value = None;
    }

    /// The value after `steps`, from `current` (the parameter's position in
    /// 0 to 1 as the document has it), clamped to the range's ends (R9).
    pub fn turn(&mut self, now: Instant, current: f64, steps: i32, shift: bool) -> f64 {
        let rested = self
            .last
            .is_none_or(|at| now.saturating_duration_since(at) >= REST);
        let base = match self.value {
            Some(value) if !rested => value,
            _ => current,
        };
        let value = (base + fraction(steps, shift)).clamp(0.0, 1.0);
        self.value = Some(value);
        self.last = Some(now);
        value
    }
}

/// Steps counted towards the next value of a list or a stepped parameter,
/// or towards the next whole unit of anything coarser than its steps.
#[derive(Default)]
pub struct Counting {
    rest: i32,
}

impl Counting {
    /// Adds `steps` and returns the whole values they come to, one for each
    /// `per` steps, keeping what is left over.
    pub fn take(&mut self, steps: i32, per: i32) -> i32 {
        self.rest += steps;
        let whole = self.rest / per;
        self.rest -= whole * per;
        whole
    }

    /// Forgets what is left over: a new touch.
    pub fn reset(&mut self) {
        self.rest = 0;
    }
}

#[cfg(test)]
mod tests {
    use bl_core::clock::TestClock;

    use super::*;

    // Spec: push-2 R9. Two's complement: 1 to 63 clockwise, 127 down to 64
    // anticlockwise by 1 to 64.
    #[test]
    fn relative_values_read_as_two_s_complement() {
        assert_eq!(steps(1), 1);
        assert_eq!(steps(10), 10);
        assert_eq!(steps(63), 63);
        assert_eq!(steps(127), -1);
        assert_eq!(steps(124), -4);
        assert_eq!(steps(64), -64);
        assert_eq!(steps(0), 0);
    }

    // Spec: push-2 R9. 1/200 of the range a step, 1/2000 with Shift, stopping
    // at the ends; steps that arrive together add up.
    #[test]
    fn a_continuous_parameter_moves_a_two_hundredth_a_step_and_stops_at_the_ends() {
        let now = TestClock::new().now();
        let mut moving = Moving::default();
        assert!((moving.turn(now, 0.5, 1, false) - 0.505).abs() < 1e-12);
        assert!(
            (moving.turn(now, 0.5, 4, false) - 0.525).abs() < 1e-12,
            "the document is behind"
        );
        let mut fine = Moving::default();
        assert!((fine.turn(now, 0.5, 10, true) - 0.505).abs() < 1e-12);
        let mut top = Moving::default();
        assert_eq!(top.turn(now, 0.99, 20, false), 1.0);
        assert_eq!(top.turn(now, 0.99, -400, false), 0.0);
        // After a rest the document's value counts again.
        let later = now + Duration::from_millis(600);
        assert!((moving.turn(later, 0.2, 1, false) - 0.205).abs() < 1e-12);
    }

    // Spec: push-2 R9. One list value for every 10 steps in one direction.
    #[test]
    fn a_list_moves_one_value_for_every_ten_steps() {
        let mut list = Counting::default();
        assert_eq!(list.take(9, LIST_STEPS), 0);
        assert_eq!(list.take(1, LIST_STEPS), 1);
        assert_eq!(list.take(25, LIST_STEPS), 2);
        assert_eq!(list.take(5, LIST_STEPS), 1);
        assert_eq!(list.take(-30, LIST_STEPS), -3);
    }
}
