//! Taps, holds, locks and modifiers on the clock the surface is given
//! (`docs/specs/push-2.md`, R7, R24 and R25).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::content::{MODIFIERS, Modifiers};
use crate::controls::cc;

/// A press released within this is a tap; one held longer is a hold (R7).
pub const HOLD: Duration = Duration::from_millis(300);
/// Mute, Solo and Stop Clip held alone this long lock on (R24).
pub const LOCK: Duration = Duration::from_secs(1);
/// The buttons that lock (R24).
pub const LOCKABLE: [u8; 3] = [cc::MUTE, cc::SOLO, cc::STOP_CLIP];

/// What a button did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Went down.
    Press(u8),
    /// Came up. `tap` is whether it was a tap with nothing used with it,
    /// `held` whether it had been down for 300 ms or more, and `used`
    /// whether another control was used while it was down.
    Release {
        /// The button.
        button: u8,
        /// A tap with nothing used with it.
        tap: bool,
        /// Down for 300 ms or more.
        held: bool,
        /// Another control was used while it was down.
        used: bool,
    },
    /// Has been down for 300 ms, and nothing was used with it.
    Hold(u8),
    /// Held alone for 1 s: locked on.
    Lock(u8),
    /// Pressed while locked: unlocked.
    Unlock(u8),
}

struct Down {
    since: Instant,
    used: bool,
    holding: bool,
    /// Its release does nothing: it unlocked, or it locked.
    swallow: bool,
}

/// Which buttons are down and which are locked.
#[derive(Default)]
pub struct Buttons {
    down: BTreeMap<u8, Down>,
    locked: BTreeSet<u8>,
}

impl Buttons {
    /// `button` went down at `at`.
    pub fn press(&mut self, button: u8, at: Instant) -> Vec<Event> {
        // A control pressed while a modifier is held is a combination: the
        // modifier doesn't do its own action when it comes up (R25).
        self.use_modifiers(button);
        // Pressed with a modifier held, it is part of that combination: it
        // neither taps nor locks (R24, R25).
        let combined = self
            .down
            .keys()
            .any(|b| *b != button && MODIFIERS.contains(b));
        let unlocked = self.locked.remove(&button);
        self.down.insert(
            button,
            Down {
                since: at,
                used: combined,
                holding: false,
                swallow: unlocked,
            },
        );
        if unlocked {
            vec![Event::Unlock(button)]
        } else {
            vec![Event::Press(button)]
        }
    }

    /// `button` came up at `at`.
    pub fn release(&mut self, button: u8, at: Instant) -> Vec<Event> {
        let Some(down) = self.down.remove(&button) else {
            return Vec::new();
        };
        if down.swallow {
            return Vec::new();
        }
        let held = at.saturating_duration_since(down.since) >= HOLD;
        vec![Event::Release {
            button,
            tap: !held && !down.used,
            held,
            used: down.used,
        }]
    }

    /// Something other than a button was used: a pad, an encoder, the
    /// strip. Every modifier down is used.
    pub fn use_modifiers(&mut self, except: u8) {
        for (button, down) in &mut self.down {
            if *button != except && MODIFIERS.contains(button) {
                down.used = true;
            }
        }
    }

    /// The holds and the locks that `now` brings.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        for (&button, down) in &mut self.down {
            if down.swallow {
                continue;
            }
            let held = now.saturating_duration_since(down.since);
            if !down.holding && held >= HOLD && !down.used {
                down.holding = true;
                events.push(Event::Hold(button));
            }
            if LOCKABLE.contains(&button) && held >= LOCK && !down.used {
                down.swallow = true;
                self.locked.insert(button);
                events.push(Event::Lock(button));
            }
        }
        events
    }

    /// Whether `button` is down.
    #[must_use]
    pub fn is_down(&self, button: u8) -> bool {
        self.down.contains_key(&button)
    }

    /// Whether another control was used while `button` is down.
    #[must_use]
    pub fn is_used(&self, button: u8) -> bool {
        self.down.get(&button).is_some_and(|down| down.used)
    }

    /// Marks `button` used, so its release is no tap.
    pub fn mark_used(&mut self, button: u8) {
        if let Some(down) = self.down.get_mut(&button) {
            down.used = true;
        }
    }

    /// The modifiers held now, and the buttons locked on.
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        Modifiers(
            self.down
                .keys()
                .copied()
                .filter(|button| MODIFIERS.contains(button))
                .chain(self.locked.iter().copied())
                .collect(),
        )
    }

    /// The buttons down, by controller number.
    pub fn held(&self) -> impl Iterator<Item = u8> + '_ {
        self.down.keys().copied()
    }

    /// The buttons locked on.
    pub fn locked(&self) -> impl Iterator<Item = u8> + '_ {
        self.locked.iter().copied()
    }

    /// Forgets everything: the surface left Live mode.
    pub fn clear(&mut self) {
        self.down.clear();
        self.locked.clear();
    }
}

#[cfg(test)]
mod tests {
    use bl_core::clock::TestClock;

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    // Spec: push-2 R7. Released within 300 ms is a tap, held longer a hold,
    // and the hold is noticed at 300 ms while the button is still down.
    #[test]
    fn a_tap_is_under_300_ms_and_a_hold_is_noticed_at_300_ms() {
        let clock = TestClock::new();
        let mut buttons = Buttons::default();
        let start = clock.now();
        buttons.press(cc::SESSION, start);
        assert!(buttons.tick(start + ms(299)).is_empty());
        assert_eq!(
            buttons.release(cc::SESSION, start + ms(299)),
            [Event::Release {
                button: cc::SESSION,
                tap: true,
                held: false,
                used: false
            }]
        );
        buttons.press(cc::SESSION, start);
        assert_eq!(buttons.tick(start + ms(300)), [Event::Hold(cc::SESSION)]);
        assert!(buttons.tick(start + ms(400)).is_empty(), "once");
        assert_eq!(
            buttons.release(cc::SESSION, start + ms(500)),
            [Event::Release {
                button: cc::SESSION,
                tap: false,
                held: true,
                used: false
            }]
        );
    }

    // Spec: push-2 R24. Held alone for 1 s locks on; pressed again unlocks,
    // and neither release is a tap.
    #[test]
    fn mute_held_alone_for_a_second_locks_and_a_press_unlocks() {
        let clock = TestClock::new();
        let mut buttons = Buttons::default();
        let start = clock.now();
        buttons.press(cc::MUTE, start);
        assert_eq!(buttons.tick(start + ms(300)), [Event::Hold(cc::MUTE)]);
        assert!(buttons.tick(start + ms(999)).is_empty());
        assert_eq!(buttons.tick(start + ms(1000)), [Event::Lock(cc::MUTE)]);
        assert!(buttons.release(cc::MUTE, start + ms(1200)).is_empty());
        assert!(buttons.modifiers().has(cc::MUTE), "locked counts as held");
        assert_eq!(
            buttons.press(cc::MUTE, start + ms(2000)),
            [Event::Unlock(cc::MUTE)]
        );
        assert!(buttons.release(cc::MUTE, start + ms(2050)).is_empty());
        assert!(!buttons.modifiers().has(cc::MUTE));
    }

    // Spec: push-2 R24, R25. Mute pressed while Shift is held is a
    // combination: it neither locks nor taps.
    #[test]
    fn mute_pressed_with_shift_held_neither_locks_nor_taps() {
        let clock = TestClock::new();
        let mut buttons = Buttons::default();
        let start = clock.now();
        buttons.press(cc::SHIFT, start);
        buttons.press(cc::MUTE, start);
        assert!(buttons.tick(start + ms(1500)).is_empty());
        buttons.release(cc::SHIFT, start + ms(1600));
        assert_eq!(
            buttons.release(cc::MUTE, start + ms(1700)),
            [Event::Release {
                button: cc::MUTE,
                tap: false,
                held: true,
                used: true,
            }]
        );
    }

    // Spec: push-2 R24, R25. A button used with Mute held doesn't lock it.
    #[test]
    fn a_control_used_while_mute_is_held_stops_the_lock_and_the_tap() {
        let clock = TestClock::new();
        let mut buttons = Buttons::default();
        let start = clock.now();
        buttons.press(cc::MUTE, start);
        buttons.use_modifiers(0);
        assert!(buttons.tick(start + ms(1500)).is_empty());
        assert_eq!(
            buttons.release(cc::MUTE, start + ms(100)),
            [Event::Release {
                button: cc::MUTE,
                tap: false,
                held: false,
                used: true
            }]
        );
    }
}
