//! An envelope follower for the effects (the device framework spec, R28):
//! the level of a signal with an attack, a release and an optional hold.

/// Follows the level of a signal. Attack and release are time constants:
/// the time the output takes to cover 63 % (1 − 1/e) of a step up or down.
/// It is a plain value: nothing in it allocates.
#[derive(Clone, Debug)]
pub struct EnvelopeFollower {
    sample_rate: f64,
    level: f64,
    attack: f64,
    release: f64,
    hold: bool,
    /// While Hold is on: whether an attack is under way, and the peak it is
    /// heading for.
    attacking: bool,
    peak: f64,
}

/// How close to its peak an attack must come, with Hold on, before the
/// release may begin.
const ATTACK_DONE: f64 = 0.99;

impl EnvelopeFollower {
    /// A follower at `sample_rate`, instant and silent until
    /// [`set`](Self::set).
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            level: 0.0,
            attack: 1.0,
            release: 1.0,
            hold: false,
            attacking: false,
            peak: 0.0,
        }
    }

    /// Sets the attack and release time constants, in seconds (0 is
    /// instant), and Hold.
    pub fn set(&mut self, attack: f64, release: f64, hold: bool) {
        self.attack = self.coefficient(attack);
        self.release = self.coefficient(release);
        self.hold = hold;
    }

    fn coefficient(&self, seconds: f64) -> f64 {
        if seconds <= 0.0 {
            1.0
        } else {
            1.0 - (-1.0 / (seconds * self.sample_rate)).exp()
        }
    }

    /// The level now.
    #[must_use]
    pub fn level(&self) -> f64 {
        self.level
    }

    /// Takes one sample and gives the level, which is never negative.
    pub fn process(&mut self, sample: f32) -> f64 {
        // A NaN or infinity from upstream must not stick in the level.
        let input = Some(f64::from(sample).abs())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        if self.hold {
            // The attack finishes before the release begins, even if the
            // signal has already dropped.
            if input > self.level {
                self.attacking = true;
            }
            if self.attacking {
                self.peak = self.peak.max(input);
                self.level += (self.peak - self.level) * self.attack;
                if self.level >= self.peak * ATTACK_DONE {
                    self.attacking = false;
                    self.peak = 0.0;
                }
                return self.level;
            }
        } else if input > self.level {
            self.level += (input - self.level) * self.attack;
            return self.level;
        }
        self.level += (input - self.level) * self.release;
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Frames a step of `input` takes to move the follower past `fraction`
    /// of the way from where it began to `input`.
    fn frames_to(follower: &mut EnvelopeFollower, input: f32, fraction: f64) -> usize {
        let from = follower.level();
        let goal = from + (f64::from(input) - from) * fraction;
        let rising = f64::from(input) > from;
        (1..200_000)
            .find(|_| {
                let level = follower.process(input);
                if rising { level >= goal } else { level <= goal }
            })
            .expect("it gets there")
    }

    // Spec: device-framework R28
    #[test]
    fn attack_and_release_are_time_constants() {
        let mut follower = EnvelopeFollower::new(RATE);
        follower.set(0.020, 0.100, false);
        let attack = frames_to(&mut follower, 1.0, 1.0 - (-1.0f64).exp());
        assert!(
            (attack as f64 / RATE - 0.020).abs() < 0.020 * 0.05,
            "attack {attack}"
        );
        for _ in 0..48_000 {
            follower.process(1.0);
        }
        let release = frames_to(&mut follower, 0.0, 1.0 - (-1.0f64).exp());
        assert!(
            (release as f64 / RATE - 0.100).abs() < 0.100 * 0.05,
            "release {release}"
        );
    }

    // Spec: device-framework R28
    #[test]
    fn a_bad_sample_doesnt_stick_in_the_level() {
        let mut follower = EnvelopeFollower::new(RATE);
        follower.set(0.001, 0.010, false);
        follower.process(f32::NAN);
        follower.process(f32::INFINITY);
        for _ in 0..4_800 {
            follower.process(0.5);
        }
        assert!((follower.level() - 0.5).abs() < 1e-3);
    }

    // Spec: device-framework R28
    #[test]
    fn hold_finishes_the_attack_before_the_release() {
        let run = |hold| {
            let mut follower = EnvelopeFollower::new(RATE);
            follower.set(0.050, 0.050, hold);
            let mut top = 0.0f64;
            for i in 0..12_000 {
                // A pulse a hundredth of the attack long, then silence.
                top = top.max(follower.process(if i < 24 { 1.0 } else { 0.0 }));
            }
            top
        };
        assert!(run(false) < 0.02, "without Hold the release cuts it short");
        assert!(run(true) > 0.9, "with Hold the attack reaches the pulse");
    }
}
