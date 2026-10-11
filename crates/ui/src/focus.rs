//! Keyboard focus in the main window (WP-1.14; the accessibility spec):
//! the controls Tab and Shift-Tab move between, the same control on the
//! next or previous track for Navigate ▸ Next Neighbor and Previous
//! Neighbor, and whether the focus is outlined.
//!
//! Each frame, every control a person can reach with the keyboard adds
//! itself to the frame's [`Order`] with [`register`], in the order it's
//! drawn, saying which area of the window it's in and, for a track's
//! control, which track and which control it is. The main window reads the
//! last frame's order to carry out a focus move (`focus.next`,
//! `focus.previous`, `focus.next_neighbor`, `focus.previous_neighbor`):
//!
//! - Outside the tracks (the Control Bar, the browser, the Status Bar), Tab
//!   moves between the area's controls (I-FOCUS-004).
//! - In the main view, Tab moves between one track's controls, in the
//!   headers or in a mixer, as Live moves through a track's mixer controls
//!   (manual 41.4.1.2, 41.4.1.3); from the view itself it goes into the
//!   selected track's.
//! - At either end, Tab and Shift-Tab wrap round while Wrap Tab Navigation
//!   is on, and otherwise stay where they are.
//! - A neighbour move goes to the same control on the next or previous
//!   track that has it, bringing the track into view if it isn't.

use bl_model::TrackId;
use bl_shell::Area;
use egui::{Context, Id};

use crate::mixer::Place;

/// Which of a track's controls a control is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The track's title in the Arrangement's headers. It isn't in the Tab
    /// order (Esc and the arrow keys reach title bars), but marks the track
    /// as drawn.
    Title,
    /// A mixer's column, which Navigate ▸ Mixer focuses.
    Column,
    /// The activator.
    Activator,
    /// Solo.
    Solo,
    /// Arm.
    Arm,
    /// The volume fader, or a header's volume field.
    Volume,
    /// Pan.
    Pan,
}

/// A track's control: where it is, whose it is, and which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackControl {
    /// The headers or a mixer.
    pub place: Place,
    /// The track.
    pub track: TrackId,
    /// Which control.
    pub slot: Slot,
}

/// One control in the focus order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// Its widget's ID.
    pub id: Id,
    /// The area of the main window it's in.
    pub area: Area,
    /// For a track's control, which one it is.
    pub control: Option<TrackControl>,
}

/// The controls a frame drew that the keyboard reaches, in the order drawn.
#[derive(Clone, Debug, Default)]
pub struct Order {
    entries: Vec<Entry>,
}

/// Where a neighbour move goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Neighbor {
    /// To this control, drawn last frame.
    Focus(Id),
    /// To this control, on a track that wasn't drawn: bring it into view,
    /// then focus the control once it's drawn.
    Reveal(TrackControl),
}

fn order_id() -> Id {
    Id::new("focus-order")
}

fn outline_id() -> Id {
    Id::new("focus-outline")
}

/// Adds a control the keyboard reaches to this frame's order.
pub fn register(ctx: &Context, id: Id, area: Area, control: Option<TrackControl>) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Order>(order_id())
            .entries
            .push(Entry { id, area, control });
    });
}

/// Takes the order the last frame drew, leaving an empty one for this
/// frame's controls to fill.
#[must_use]
pub fn take(ctx: &Context) -> Order {
    ctx.data_mut(|d| d.remove_temp::<Order>(order_id()))
        .unwrap_or_default()
}

/// The controls this frame has drawn so far.
#[must_use]
pub fn drawn(ctx: &Context) -> Order {
    ctx.data(|d| d.get_temp::<Order>(order_id()))
        .unwrap_or_default()
}

/// Says whether the focused control is outlined in this window this frame:
/// the main window follows Outline View in Focus (the accessibility spec,
/// R7).
pub fn set_outlined(ctx: &Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(outline_id(), on));
}

/// Whether to draw the focused control's outline. A window that doesn't
/// say (the Settings window) draws it.
#[must_use]
pub fn outlined(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(outline_id()))
        .unwrap_or(true)
}

impl Order {
    /// The controls, in the order drawn.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry for a widget, if the frame drew it.
    #[must_use]
    pub fn find(&self, id: Id) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The widget of a track's control, if the frame drew it.
    #[must_use]
    pub fn control(&self, control: TrackControl) -> Option<Id> {
        self.entries
            .iter()
            .find(|entry| entry.control == Some(control))
            .map(|entry| entry.id)
    }

    /// Whether the frame drew any of a track's controls in `place`.
    #[must_use]
    pub fn drew_track(&self, place: Place, track: TrackId) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .control
                .is_some_and(|c| c.place == place && c.track == track)
        })
    }

    /// The controls Tab moves between with `entry` focused: the area's
    /// controls outside the tracks, or one track's in one place.
    fn scope(&self, area: Area, track: Option<(Place, TrackId)>) -> Vec<Id> {
        self.entries
            .iter()
            .filter(|entry| match (track, entry.control) {
                (None, None) => entry.area == area,
                (Some((place, track)), Some(c)) => {
                    c.place == place && c.track == track && c.slot != Slot::Title
                }
                _ => false,
            })
            .map(|entry| entry.id)
            .collect()
    }

    /// Where Tab (or Shift-Tab, `backwards`) moves the focus from `from`,
    /// within its area or its track (R3, R4). With nothing focused it goes
    /// to the first (or last) control of `area`; at an end it wraps round
    /// if `wrap`, and otherwise stays. `None` when there's nowhere to go.
    #[must_use]
    pub fn tab(&self, from: Option<Id>, area: Area, backwards: bool, wrap: bool) -> Option<Id> {
        let current = from.and_then(|id| self.find(id));
        let scope = match current {
            Some(entry) => self.scope(entry.area, entry.control.map(|c| (c.place, c.track))),
            None => self.scope(area, None),
        };
        Self::step(&scope, from, backwards, wrap)
    }

    /// The first (or, `backwards`, the last) of a track's controls in
    /// `place`, for Tab from the main view into the track (R4).
    #[must_use]
    pub fn enter_track(&self, place: Place, track: TrackId, backwards: bool) -> Option<Id> {
        let scope = self.scope(Area::MainView, Some((place, track)));
        if backwards {
            scope.last().copied()
        } else {
            scope.first().copied()
        }
    }

    fn step(scope: &[Id], from: Option<Id>, backwards: bool, wrap: bool) -> Option<Id> {
        let at = from.and_then(|id| scope.iter().position(|s| *s == id));
        let len = scope.len();
        if len == 0 {
            return None;
        }
        let next = match (at, backwards) {
            (None, false) => Some(0),
            (None, true) => Some(len - 1),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            (Some(i), true) if i > 0 => Some(i - 1),
            (Some(_), false) => wrap.then_some(0),
            (Some(_), true) => wrap.then_some(len - 1),
        };
        next.map(|i| scope[i])
    }

    /// Where a neighbour move from `from` goes: the same control on the
    /// next (or previous) track in `tracks` that has it, skipping drawn
    /// tracks without it and stopping at the ends (R4).
    #[must_use]
    pub fn neighbor(&self, from: Id, tracks: &[TrackId], backwards: bool) -> Option<Neighbor> {
        let current = self.find(from)?.control?;
        let at = tracks.iter().position(|t| *t == current.track)?;
        let others: Box<dyn Iterator<Item = &TrackId>> = if backwards {
            Box::new(tracks[..at].iter().rev())
        } else {
            Box::new(tracks[at + 1..].iter())
        };
        for track in others {
            let wanted = TrackControl {
                track: *track,
                ..current
            };
            if let Some(id) = self.control(wanted) {
                return Some(Neighbor::Focus(id));
            }
            if !self.drew_track(current.place, *track) {
                return Some(Neighbor::Reveal(wanted));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_shell::mixer_view::MixerOf;

    fn track(n: u64) -> TrackId {
        TrackId(bl_core::id::Id::new(n).expect("not zero"))
    }

    fn order() -> Order {
        let mut order = Order::default();
        let mut push = |id: &str, area, control| {
            order.entries.push(Entry {
                id: Id::new(id),
                area,
                control,
            });
        };
        push("a", Area::ControlBar, None);
        push("b", Area::ControlBar, None);
        push("c", Area::ControlBar, None);
        push("pane", Area::MainView, None);
        let place = Place::Mixer(MixerOf::Session);
        for (n, slots) in [
            (1, &[Slot::Column, Slot::Volume, Slot::Solo][..]),
            (2, &[Slot::Column, Slot::Solo][..]),
            (3, &[Slot::Column, Slot::Volume, Slot::Solo][..]),
        ] {
            for slot in slots {
                push(
                    &format!("{n}{slot:?}"),
                    Area::MainView,
                    Some(TrackControl {
                        place,
                        track: track(n),
                        slot: *slot,
                    }),
                );
            }
        }
        order
    }

    // Spec: accessibility R3, R5.
    #[test]
    fn tab_stays_in_its_area_and_wraps_only_when_asked() {
        let order = order();
        let id = Id::new;
        assert_eq!(
            order.tab(Some(id("a")), Area::ControlBar, false, false),
            Some(id("b"))
        );
        assert_eq!(
            order.tab(Some(id("c")), Area::ControlBar, false, false),
            None
        );
        assert_eq!(
            order.tab(Some(id("c")), Area::ControlBar, false, true),
            Some(id("a"))
        );
        assert_eq!(
            order.tab(Some(id("a")), Area::ControlBar, true, true),
            Some(id("c"))
        );
        assert_eq!(
            order.tab(None, Area::ControlBar, false, false),
            Some(id("a"))
        );
        assert_eq!(
            order.tab(None, Area::ControlBar, true, false),
            Some(id("c"))
        );
    }

    // Spec: accessibility R4.
    #[test]
    fn tab_moves_through_one_track_s_controls() {
        let order = order();
        let id = Id::new;
        assert_eq!(
            order.tab(Some(id("1Volume")), Area::MainView, false, false),
            Some(id("1Solo"))
        );
        assert_eq!(
            order.tab(Some(id("1Solo")), Area::MainView, false, true),
            Some(id("1Column"))
        );
        let place = Place::Mixer(MixerOf::Session);
        assert_eq!(
            order.enter_track(place, track(2), false),
            Some(id("2Column"))
        );
        assert_eq!(order.enter_track(place, track(2), true), Some(id("2Solo")));
    }

    // Spec: accessibility R4.
    #[test]
    fn neighbours_skip_tracks_without_the_control_and_reveal_undrawn_ones() {
        let order = order();
        let id = Id::new;
        let tracks: Vec<TrackId> = (1..=4).map(track).collect();
        assert_eq!(
            order.neighbor(id("1Volume"), &tracks, false),
            Some(Neighbor::Focus(id("3Volume")))
        );
        assert_eq!(
            order.neighbor(id("3Volume"), &tracks, true),
            Some(Neighbor::Focus(id("1Volume")))
        );
        assert_eq!(
            order.neighbor(id("3Solo"), &tracks, false),
            Some(Neighbor::Reveal(TrackControl {
                place: Place::Mixer(MixerOf::Session),
                track: track(4),
                slot: Slot::Solo,
            }))
        );
        assert_eq!(order.neighbor(id("1Solo"), &tracks, true), None);
        assert_eq!(order.neighbor(id("a"), &tracks, false), None);
    }
}
