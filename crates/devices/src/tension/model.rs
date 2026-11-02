//! Tension's exciters and damper (spec: tension R3 to R6): each a small model
//! that meets the string at a position, reads the string's velocity there,
//! and gives back a force, as the bowed and struck string models of the
//! acoustics literature do (McIntyre, Schumacher and Woodhouse, 1983; Smith,
//! 1986). The string's own waveguide is [`bl_dsp::physical`]'s; these are
//! its [`Interaction`], one lane per voice and unison copy.
//!
//! Every law and constant is Beatlayer's own reading of the manual's
//! controls on those models. Forces are in the string's own units: the
//! change of its velocity at the point per frame.

use bl_dsp::physical::Interaction;

/// What the exciter is, in the chooser's order after Off (R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Off,
    Bow,
    Hammer,
    Bounce,
    Pluck,
}

impl Kind {
    /// The exciter the chooser's `index` names.
    #[must_use]
    pub fn of(index: usize) -> Self {
        [Self::Bow, Self::Hammer, Self::Bounce, Self::Pluck][index.min(3)]
    }
}

/// Spec: tension R3. The friction curve's reference speed, as a share of the
/// bow's speed, and the most a frame's force may be of the relative speed it
/// acts on: a bit of force more than that would turn the string round and
/// make the explicit solution chatter, so the solution is clamped.
const BOW_V0: f32 = 0.1;
const BOW_CLAMP: f32 = 1.5;
/// Spec: tension R3. A bow's speed at Velocity 100 %, in the string's units.
const BOW_SPEED: f32 = 0.008;
/// Spec: tension R3. Force 0 and 100 % press with this share of the bow's
/// speed per frame, evenly in octaves.
const BOW_FORCE: (f32, f32) = (1.0, 100.0);
/// The force, in the bow's speed per frame, that plays at the output's
/// nominal level: a harder bow drives the string louder, so the output is
/// scaled by this over the force to a power, which keeps the level where it
/// was while the tone changes.
const BOW_NOMINAL: f32 = 10.0;
const BOW_LEVELLING: f32 = 0.9;
/// Frames the bow takes to lift once the key is let go.
const BOW_LIFT: f32 = 0.9993;
/// The friction ratio μs / μd at Friction 0 and 100 % (R3).
const GRIP: (f32, f32) = (1.0, 4.0);

/// A hammer's contact force is k × c^2.5 for a compression c (R4); its
/// stiffness constant k at Stiffness 0 and 100 %, in
/// string units per frame, evenly in octaves; a hard hammer touches the
/// string for a fifth of a millisecond, a soft one for four.
const HAMMER_K: (f32, f32) = (2e-6, 2e-3);
/// The hammer's mass at Mass 0 and 100 %, against the string's own, and its
/// speed at Velocity 100 %.
const HAMMER_MASS: (f32, f32) = (0.2, 20.0);
const HAMMER_SPEED: f32 = 0.5;
/// The largest force a frame of contact gives, which keeps a hard hit stable.
const CONTACT_MAX: f32 = 1.0;
/// How quickly a stringed displacement leaks away, so reading it as the
/// integral of the string's velocity doesn't drift.
const LEAK: f32 = 0.9998;
/// A bouncing hammer's return spring, per unit of its mass, and its loss
/// at Damping 0 and 100 % as a share of critical.
const RETURN_SPRING: f32 = 2.5e-3;
const BOUNCE_LOSS: (f32, f32) = (0.02, 0.3);
/// A bouncing hammer rests this share of the string's swing from it, and one
/// that has come to rest there is done.
const BOUNCE_GAP: f32 = 0.05;
const BOUNCE_REST: f32 = 1e-5;

/// Spec: tension R4. The plectrum's pull takes this many periods of the
/// note; the contact spring's stiffness at Stiffness 0 and 100 %; and how
/// quickly it lets go, in periods of the note, at 100 % and 0 %.
const PLUCK_PERIODS: f32 = 0.75;
const PLUCK_K: (f32, f32) = (1e-2, 3e-1);
const PLUCK_LET_GO: (f32, f32) = (0.02, 0.5);
/// A plucked string's speed at Protrusion 100 % and full velocity, in
/// string units, which sets how far the plectrum pulls it.
const PLUCK_SPEED: f32 = 0.12;

/// Spec: tension R4. How much of the string's velocity a touching exciter
/// takes out per frame at Damping 100 %.
const TOUCH_LOSS: f32 = 0.2;

/// Spec: tension R6. The damper's loss per frame at Damping 100 %, which
/// grows as its square and is as much as the explicit solution takes before
/// it turns round and rings, and the kick it gives the string as it moves, per unit of press moved.
const DAMPER_LOSS: f32 = 0.5;
const DAMPER_KICK: f32 = 0.5;
/// A damper lands and lifts in this many seconds at Velocity 100 % and 0 %,
/// and at a fixed speed when it isn't gated.
const DAMPER_SECONDS: (f32, f32) = (0.002, 0.06);
const DAMPER_UNGATED: f32 = 0.02;

/// `0 .. 1` mapped to `lo .. hi` evenly in octaves.
fn octaves(x: f32, (lo, hi): (f32, f32)) -> f32 {
    lo * (hi / lo).powf(x.clamp(0.0, 1.0))
}

/// `0 .. 1` mapped to `lo .. hi` evenly.
fn between(x: f32, (lo, hi): (f32, f32)) -> f32 {
    lo + (hi - lo) * x.clamp(0.0, 1.0)
}

/// What an exciter is set to at a note's start: every share is 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct Strike {
    pub kind: Kind,
    pub velocity: f32,
    pub force: f32,
    pub friction: f32,
    pub mass: f32,
    pub stiffness: f32,
    pub protrusion: f32,
    pub damping: f32,
    /// The note's frequency in cycles per frame, which the plectrum's pull
    /// takes its time from.
    pub cycles: f32,
}

/// What a damper is set to at a note's start: every share is 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct Damper {
    pub on: bool,
    pub gated: bool,
    pub mass: f32,
    pub stiffness: f32,
    pub velocity: f32,
    pub damping: f32,
    /// The seconds in a frame.
    pub frame: f32,
}

/// One lane's exciter and damper.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lane {
    live: bool,
    /// Frames before the exciter starts: a unison copy's delay.
    wait: u32,
    kind: Kind,
    // The bow.
    speed: f32,
    force: f32,
    level: f32,
    grip: f32,
    lift: f32,
    // The hammers and the plectrum: the string's displacement where they
    // meet it, the exciter's own, its speed, and its contact.
    y: f32,
    x: f32,
    v: f32,
    k: f32,
    mass: f32,
    touch: f32,
    touched: bool,
    /// How many times the exciter has met the string.
    strikes: u32,
    in_contact: bool,
    // A bouncing hammer's return spring and loss, and where it rests: a gap
    // below the string's own rest.
    spring: f32,
    loss: f32,
    rest: f32,
    // The plectrum's pull: where it is, how far it goes, how fast, and how
    // quickly its force falls once it lets go.
    reach: f32,
    pull: f32,
    letting_go: bool,
    relax: f32,
    // The damper: how hard it presses, where it is heading, and how fast.
    damper_on: bool,
    press: f32,
    goal: f32,
    step: f32,
    drag: f32,
    kick: f32,
}

impl Lane {
    /// A note starts on the lane: `strike`'s exciter after `wait` frames,
    /// `damper` lifting.
    pub fn start(&mut self, strike: &Strike, damper: &Damper, wait: u32) {
        *self = Self {
            live: true,
            wait,
            kind: strike.kind,
            grip: 1.0,
            lift: 1.0,
            level: 1.0,
            damper_on: damper.on,
            press: 1.0,
            goal: 0.0,
            ..Self::default()
        };
        self.set_bow(strike);
        match strike.kind {
            Kind::Hammer | Kind::Bounce => {
                self.k = octaves(strike.stiffness, HAMMER_K);
                self.mass = octaves(strike.mass, HAMMER_MASS);
                self.v = HAMMER_SPEED * strike.velocity;
                self.touch = strike.damping * TOUCH_LOSS;
                if strike.kind == Kind::Bounce {
                    self.spring = RETURN_SPRING * self.mass;
                    let omega = (RETURN_SPRING).sqrt();
                    self.loss = 2.0 * between(strike.damping, BOUNCE_LOSS) * omega * self.mass;
                    // A gap, as a share of the swing the strike gives the string.
                    let swing = HAMMER_SPEED * strike.velocity
                        / (std::f32::consts::TAU * strike.cycles).max(1e-4);
                    self.rest = -BOUNCE_GAP * swing;
                }
            }
            Kind::Pluck => {
                // The string's velocity it leaves is about its speed; its
                // displacement is that over the note's angular frequency.
                let omega = (std::f32::consts::TAU * strike.cycles).max(1e-4);
                self.reach = PLUCK_SPEED * strike.velocity * strike.protrusion.max(0.02) / omega;
                self.pull = self.reach / (PLUCK_PERIODS / strike.cycles.max(1e-6));
                self.k = octaves(strike.stiffness, PLUCK_K);
                self.touch = strike.damping * TOUCH_LOSS;
                self.relax = (-1.0
                    / (between(1.0 - strike.stiffness, PLUCK_LET_GO) / strike.cycles.max(1e-6)))
                .exp();
            }
            Kind::Bow | Kind::Off => {}
        }
        let seconds = if damper.gated {
            between(1.0 - damper.velocity, DAMPER_SECONDS)
        } else {
            DAMPER_UNGATED
        } * (0.5 + 1.5 * damper.mass);
        self.step = (damper.frame / seconds).clamp(1e-4, 1.0);
        self.drag = DAMPER_LOSS * damper.damping * damper.damping * (1.0 - 0.9 * damper.stiffness);
        self.kick = DAMPER_KICK * (0.2 + damper.mass);
    }

    /// The bow's speed, force and grip as they are now (R3).
    pub fn set_bow(&mut self, strike: &Strike) {
        if strike.kind == Kind::Bow {
            self.speed = BOW_SPEED * strike.velocity;
            let force = octaves(strike.force, BOW_FORCE);
            self.force = self.speed * force;
            self.level = (BOW_NOMINAL / force).powf(BOW_LEVELLING);
            self.grip = between(strike.friction, GRIP);
        }
    }

    /// What the lane's sound is scaled by: the bow's levelling.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// The key is let go: the bow lifts and the damper lands.
    pub fn release(&mut self) {
        self.goal = 1.0;
    }

    /// The lane's note is over.
    pub fn stop(&mut self) {
        self.live = false;
    }

    fn bow(&mut self, velocity: f32) -> f32 {
        if self.goal > 0.0 {
            self.lift *= BOW_LIFT;
        }
        let slip = self.speed - velocity;
        let mu = 1.0 + (self.grip - 1.0) * BOW_V0 * self.speed / (BOW_V0 * self.speed + slip.abs());
        let sign = slip / (slip.abs() + 0.02 * self.speed + 1e-9);
        let force = self.force * self.lift * mu * sign;
        let most = BOW_CLAMP * slip.abs();
        force.clamp(-most, most)
    }

    fn contact(&mut self, velocity: f32) -> f32 {
        self.y = self.y * LEAK + velocity;
        let c = self.x - self.y;
        let mut force = 0.0;
        if c > 0.0 {
            self.touched = true;
            if !self.in_contact {
                self.strikes += 1;
            }
            // The contact force, and its loss while it touches (R4).
            force = (self.k * c * c * c.sqrt() - self.touch * velocity).clamp(0.0, CONTACT_MAX);
            self.v -= force / self.mass;
        } else if self.kind == Kind::Bounce {
            self.v -=
                (self.spring * (self.x - self.rest) + self.loss * self.v) / self.mass.max(1e-3);
        } else if self.touched {
            // A hammer that has left the string is gone (R4).
            self.kind = Kind::Off;
        }
        self.in_contact = c > 0.0;
        self.x += self.v;
        if self.kind == Kind::Bounce
            && self.touched
            && c <= 0.0
            && self.v.abs() < BOUNCE_REST
            && (self.x - self.rest).abs() < BOUNCE_REST
        {
            self.kind = Kind::Off;
        }
        force
    }

    fn pluck(&mut self, velocity: f32) -> f32 {
        self.y = self.y * LEAK + velocity;
        if !self.letting_go {
            self.x += self.pull;
            self.letting_go = self.x >= self.reach;
        }
        if self.letting_go {
            self.k *= self.relax;
            if self.k < 1e-7 {
                self.kind = Kind::Off;
            }
        }
        (self.k * (self.x - self.y) - self.touch * velocity).clamp(-CONTACT_MAX, CONTACT_MAX)
    }
}

/// Every lane's exciter and damper, as the string's interaction.
#[derive(Clone, Debug)]
pub struct Strings {
    pub lanes: Vec<Lane>,
}

impl Strings {
    /// `lanes` lanes with nothing sounding.
    #[must_use]
    pub fn new(lanes: usize) -> Self {
        Self {
            lanes: vec![Lane::default(); lanes],
        }
    }
}

impl Interaction for Strings {
    const DAMPED: bool = true;

    fn needs(&mut self, lane: usize) -> (bool, bool) {
        let l = &mut self.lanes[lane];
        if !l.live {
            return (false, false);
        }
        l.wait = l.wait.saturating_sub(1);
        (
            l.wait == 0 && l.kind != Kind::Off,
            l.damper_on && (l.press != 0.0 || l.press != l.goal),
        )
    }

    fn force(&mut self, lane: usize, velocity: f32) -> f32 {
        let l = &mut self.lanes[lane];
        match l.kind {
            Kind::Off => 0.0,
            Kind::Bow => l.bow(velocity),
            Kind::Hammer | Kind::Bounce => l.contact(velocity),
            Kind::Pluck => l.pluck(velocity),
        }
    }

    fn damp(&mut self, lane: usize, velocity: f32) -> f32 {
        let l = &mut self.lanes[lane];
        let before = l.press;
        l.press += (l.goal - l.press).clamp(-l.step, l.step);
        -l.drag * l.press * velocity - l.kick * (l.press - before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_dsp::physical::{Kind as Guide, WaveguideBank, WaveguideParams};

    fn strike(kind: Kind) -> Strike {
        Strike {
            kind,
            velocity: 1.0,
            force: 0.5,
            friction: 0.5,
            mass: 0.5,
            stiffness: 0.5,
            protrusion: 0.5,
            damping: 0.2,
            cycles: 220.0 / 48_000.0,
        }
    }

    /// A string played by `kind` for `frames`: the lane, and its output.
    fn play(kind: Kind, damping: f32, frames: usize) -> (Lane, Vec<f32>) {
        let mut bank = WaveguideBank::new(48_000.0, 1, Guide::String, 20.0);
        let params = WaveguideParams {
            hz: 220.0,
            decay: 3.0,
            material: 0.6,
            excite: 0.2,
            pickup_l: 0.3,
            pickup_r: 0.3,
            ..WaveguideParams::default()
        };
        bank.reset_voice(0, &params);
        let mut strings = Strings::new(1);
        let damper = Damper {
            on: false,
            gated: true,
            mass: 0.0,
            stiffness: 0.0,
            velocity: 0.0,
            damping: 0.0,
            frame: 1.0 / 48_000.0,
        };
        strings.lanes[0].start(
            &Strike {
                damping,
                ..strike(kind)
            },
            &damper,
            0,
        );
        let stride = bank.stride();
        let mut out = Vec::new();
        for _ in 0..frames / 32 {
            let mut heard = vec![0.0; 32 * stride * 2];
            bank.process_with(&vec![0.0; 32 * stride], &mut heard, &mut strings);
            out.extend((0..32).map(|f| heard[f * 2 * stride]));
        }
        (strings.lanes[0], out)
    }

    // Spec: tension R4
    #[test]
    fn a_hammer_strikes_once_a_bouncing_one_again_and_a_plectrum_lets_go_once() {
        let (hammer, _) = play(Kind::Hammer, 0.2, 48_000);
        assert_eq!(hammer.strikes, 1);
        assert_eq!(hammer.kind, Kind::Off, "the hammer has left");
        let (bouncing, _) = play(Kind::Bounce, 0.2, 96_000);
        assert!((2..40).contains(&bouncing.strikes), "{}", bouncing.strikes);
        assert_eq!(bouncing.kind, Kind::Off, "the bounces have died away");
        let (plectrum, out) = play(Kind::Pluck, 0.2, 48_000);
        assert_eq!(plectrum.strikes, 0);
        assert!(plectrum.letting_go && plectrum.kind == Kind::Off);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    // Spec: tension R3
    #[test]
    fn a_bow_that_cannot_stick_is_silent_and_one_that_can_stays_bounded() {
        for force in [0.0, 0.5, 1.0] {
            let mut bank = WaveguideBank::new(48_000.0, 1, Guide::String, 20.0);
            bank.reset_voice(0, &WaveguideParams::default());
            let mut strings = Strings::new(1);
            let mut bow = strike(Kind::Bow);
            bow.force = force;
            let damper = Damper {
                on: false,
                gated: true,
                mass: 0.0,
                stiffness: 0.0,
                velocity: 0.0,
                damping: 0.0,
                frame: 1.0 / 48_000.0,
            };
            strings.lanes[0].start(&bow, &damper, 0);
            let stride = bank.stride();
            let mut loudest = 0.0f32;
            for _ in 0..96_000 / 32 {
                let mut heard = vec![0.0; 32 * stride * 2];
                bank.process_with(&vec![0.0; 32 * stride], &mut heard, &mut strings);
                loudest = (0..32).fold(loudest, |m, f| m.max(heard[f * 2 * stride].abs()));
            }
            assert!(
                loudest.is_finite() && loudest < 4.0,
                "force {force}: {loudest}"
            );
        }
    }
}
