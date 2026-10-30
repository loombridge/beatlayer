//! What the APC mini mk2 and the APC40 mkII profiles share
//! (`docs/specs/supported-controllers.md`, R35 to R50): the lights they keep
//! as note-on messages, the position of the controls that don't move by
//! themselves, and the version they introduce Beatlayer with.

use std::collections::BTreeMap;

use bl_model::{DeviceId, SceneId, TrackId};

use crate::surface::{Cx, ReadModel, Role};

/// The three bytes of a note-on: status, note and velocity.
pub(super) type Msg = [u8; 3];

/// A light's messages, in the order they are sent.
pub(super) type Lit = Vec<Msg>;

/// A light by the note it lights and the channel of the track whose strip it
/// is (0 for the controls with no strip).
pub(super) type Key = (u8, u8);

/// Beatlayer's version as the introduction gives it: major, minor and fix.
pub(super) fn version() -> [u8; 3] {
    let part = |n: &str| n.parse::<u8>().unwrap_or(0).min(0x7F);
    [
        part(env!("CARGO_PKG_VERSION_MAJOR")),
        part(env!("CARGO_PKG_VERSION_MINOR")),
        part(env!("CARGO_PKG_VERSION_PATCH")),
    ]
}

/// The lights the surface has sent, so only what changed is sent again.
#[derive(Default)]
pub(super) struct Leds {
    lit: BTreeMap<Key, Lit>,
}

impl Leds {
    /// Sends the lights of `want` that changed and puts out the ones no
    /// longer wanted.
    pub(super) fn show(&mut self, want: BTreeMap<Key, Lit>, cx: &mut Cx) {
        for (key, messages) in &want {
            let before = self.lit.get(key);
            if before != Some(messages) {
                // A light that alternated with a second colour starts afresh.
                if before.is_some_and(|before| before.len() > 1) {
                    cx.send_for(control_id(*key), off(*key));
                }
                for bytes in messages {
                    cx.send_for(control_id(*key), *bytes);
                }
            }
        }
        for key in self.lit.keys().filter(|key| !want.contains_key(key)) {
            cx.send_for(control_id(*key), off(*key));
        }
        self.lit = want;
    }

    /// Puts out every light it sent (R35).
    pub(super) fn out(&mut self, cx: &mut Cx) {
        for key in std::mem::take(&mut self.lit).into_keys() {
            cx.send(off(key));
        }
    }
}

/// The message that puts a light out: a note-on of velocity 0.
fn off((note, channel): Key) -> Msg {
    [0x90 | channel, note, 0]
}

fn control_id((note, channel): Key) -> u32 {
    u32::from(channel) << 8 | u32::from(note)
}

/// What the controls tell the shell about a role's place: the ring's first
/// track and scene, and the device in focus with its bank (R35).
pub(super) type Reach = (Option<TrackId>, Option<SceneId>, Option<(DeviceId, usize)>);

/// Where the controller's faders last were, which the shell needs for
/// Takeover Mode as soon as a fader reaches a parameter it didn't before
/// (R35): the introduction's reply gives them first, and each message
/// after.
pub(super) struct Rests<const N: usize> {
    at: [Option<u8>; N],
    told: Option<([Option<Role>; N], Reach)>,
}

impl<const N: usize> Rests<N> {
    pub(super) const fn new() -> Self {
        Self {
            at: [None; N],
            told: None,
        }
    }

    /// Takes fader `index` to be at `value`.
    pub(super) fn set(&mut self, index: usize, value: u8) {
        if let Some(slot) = self.at.get_mut(index) {
            *slot = Some(value);
        }
    }

    /// Takes the faders to be where the introduction's reply puts them, and
    /// tells the shell again on the next tick, which may have run before the
    /// reply arrived.
    pub(super) fn replied(&mut self, values: &[u8]) {
        for (index, value) in values.iter().enumerate() {
            self.set(index, *value);
        }
        self.told = None;
    }

    /// Tells the shell where each fader rests when the roles they reach or
    /// the places those roles take them to are not the ones it was last told.
    pub(super) fn tell(&mut self, roles: [Option<Role>; N], reach: Reach, cx: &mut Cx) {
        if self
            .told
            .as_ref()
            .is_some_and(|(told, at)| *told == roles && *at == reach)
        {
            return;
        }
        for (role, value) in roles.iter().zip(self.at) {
            if let (Some(role), Some(value)) = (role, value) {
                cx.rest(*role, value);
            }
        }
        self.told = Some((roles, reach));
    }
}

/// The place the roles of a row reach now, as the model gives it.
pub(super) fn reach(model: &ReadModel, row: usize) -> Reach {
    let row = model.row(row);
    let ring = row.and_then(|r| r.ring.as_ref());
    (
        ring.and_then(|r| r.first_track),
        ring.and_then(|r| r.first_scene),
        row.and_then(|r| r.focus.as_ref())
            .map(|focus| (focus.device, focus.bank)),
    )
}

/// The brightness and motion channel an RGB light is sent on (R37, R42): the
/// palette index is its velocity.
pub(super) mod channel {
    /// Solid at 50 % of the colour's brightness.
    pub const HALF: u8 = 2;
    /// Solid at 25 %.
    pub const QUARTER: u8 = 1;
    /// Solid at full brightness.
    pub const FULL: u8 = 6;
    /// Pulsing every quarter note.
    pub const PULSE_QUARTER: u8 = 9;
    /// Blinking every eighth note.
    pub const BLINK_EIGHTH: u8 = 13;
}
