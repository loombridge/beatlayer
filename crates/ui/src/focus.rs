//! Keyboard focus in the main window (WP-1.14; the accessibility spec):
//! the controls Tab and Shift-Tab move between, the same control on the
//! next or previous track for Navigate ▸ Next Neighbor and Previous
//! Neighbor, and whether the focus is outlined.
//!
//! Each frame, every control a person can reach with the keyboard adds
//! itself to the frame's [`Order`] with [`register`], saying which area of
//! the window it's in and, for a track's control, which track and which
//! control it is. The main window reads the last frame's order to carry out
//! a focus move (`focus.next`, `focus.previous`, `focus.next_neighbor`,
//! `focus.previous_neighbor`), in the orders observed in Live 12.4.6:
//!
//! - Outside the main view (the Control Bar, the browser, the Status Bar),
//!   Tab moves between the area's controls in the order they're drawn.
//! - In the Arrangement, Tab runs through the view: the selected track's
//!   header (title, activator, Solo, Arm, volume, pan), then the controls
//!   at the view's bottom right (Optimize Height). Live has locator and
//!   automation buttons before the header, and the In/Out choosers, sends,
//!   and waveform zoom in it, which Beatlayer doesn't have yet.
//! - In a mixer, Tab stays in one track's strip: the column (standing for
//!   its title bar), the peak level, pan, the activator, Solo, Arm, and
//!   volume. From the Session View itself it goes into the selected
//!   track's strip after its title bar.
//! - At either end, Tab and Shift-Tab wrap round while Wrap Tab Navigation
//!   is on (as it is in a new install), and otherwise stay where they are.
//! - A neighbour move goes to the same control on the next or previous
//!   track that has it, bringing the track into view if it isn't, and
//!   leaves the selection as it was.

use bl_model::TrackId;
use bl_shell::Area;
use egui::{Context, Id};

use crate::mixer::Place;

/// Which of a track's controls a control is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The track's title in the Arrangement's headers.
    Title,
    /// A mixer's column, which Navigate ▸ Mixer focuses and which stands for
    /// the track's title bar in the strip's Tab order.
    Column,
    /// The peak level readout.
    Peak,
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

    /// A track's controls in `place`, in Tab's order (observed in Live
    /// 12.4.6).
    fn track_controls(&self, place: Place, track: TrackId) -> Vec<Id> {
        let mut controls: Vec<(u8, Id)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let c = entry.control?;
                (c.place == place && c.track == track).then(|| (c.slot.rank(place), entry.id))
            })
            .collect();
        controls.sort_by_key(|(rank, _)| *rank);
        controls.into_iter().map(|(_, id)| id).collect()
    }

    /// The Arrangement's Tab order: `track`'s header, then the view's other
    /// controls.
    fn arrangement(&self, track: Option<TrackId>) -> Vec<Id> {
        let mut scope =
            track.map_or_else(Vec::new, |track| self.track_controls(Place::Header, track));
        scope.extend(
            self.entries
                .iter()
                .filter(|entry| entry.area == Area::MainView && entry.control.is_none())
                .map(|entry| entry.id),
        );
        scope
    }

    /// An area's controls outside the tracks, in the order drawn.
    fn area(&self, area: Area) -> Vec<Id> {
        self.entries
            .iter()
            .filter(|entry| entry.area == area && entry.control.is_none())
            .map(|entry| entry.id)
            .collect()
    }

    /// Where Tab (or Shift-Tab, `backwards`) moves the focus from `from`
    /// (R3 to R5): within its area, through the Arrangement with `track`'s
    /// header in it, or round a mixer strip. With nothing focused it goes
    /// to the first (or last) control of `area`; at an end it wraps round
    /// if `wrap`, and otherwise stays. `None` when there's nowhere to go.
    #[must_use]
    pub fn tab(
        &self,
        from: Option<Id>,
        area: Area,
        track: Option<TrackId>,
        backwards: bool,
        wrap: bool,
    ) -> Option<Id> {
        let current = from.and_then(|id| self.find(id));
        let scope = match current {
            Some(Entry {
                control: Some(c), ..
            }) => match c.place {
                Place::Mixer(_) => self.track_controls(c.place, c.track),
                Place::Header => self.arrangement(Some(c.track)),
            },
            Some(entry) if entry.area == Area::MainView => self.arrangement(track),
            Some(entry) => self.area(entry.area),
            None if area == Area::MainView => self.arrangement(track),
            None => self.area(area),
        };
        Self::step(&scope, from, backwards, wrap)
    }

    /// Where Tab from the Session View goes in a track's strip: the first
    /// control after its title bar (or, `backwards`, the last), as the first
    /// Tab in Live's Session View goes past the title bar (R4).
    #[must_use]
    pub fn enter_track(&self, place: Place, track: TrackId, backwards: bool) -> Option<Id> {
        let scope = self.track_controls(place, track);
        if backwards {
            scope.last().copied()
        } else {
            scope.get(1).or(scope.first()).copied()
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

    /// Where a neighbour move from `current` goes: the same control on the
    /// next (or previous) track in `tracks` that has it, skipping drawn
    /// tracks without it and stopping at the ends (R4).
    #[must_use]
    pub fn neighbor(
        &self,
        current: TrackControl,
        tracks: &[TrackId],
        backwards: bool,
    ) -> Option<Neighbor> {
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

impl Slot {
    /// Where the control comes in its track's Tab order in `place`
    /// (observed in Live 12.4.6): an Arrangement header's title, activator,
    /// Solo, Arm, volume, and pan; a mixer strip's title bar (the column),
    /// peak level, pan, activator, Solo, Arm, and volume.
    #[must_use]
    pub const fn rank(self, place: Place) -> u8 {
        match place {
            Place::Header => match self {
                Self::Title | Self::Column => 0,
                Self::Activator => 1,
                Self::Solo => 2,
                Self::Arm => 3,
                Self::Volume => 4,
                Self::Pan => 5,
                Self::Peak => 6,
            },
            Place::Mixer(_) => match self {
                Self::Title | Self::Column => 0,
                Self::Peak => 1,
                Self::Pan => 2,
                Self::Activator => 3,
                Self::Solo => 4,
                Self::Arm => 5,
                Self::Volume => 6,
            },
        }
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
        let mixer = Place::Mixer(MixerOf::Session);
        // Drawn in the strip's own order, not Tab's.
        for (n, slots) in [
            (
                1,
                &[
                    Slot::Column,
                    Slot::Peak,
                    Slot::Pan,
                    Slot::Volume,
                    Slot::Activator,
                    Slot::Solo,
                ][..],
            ),
            (2, &[Slot::Column, Slot::Activator, Slot::Solo][..]),
            (
                3,
                &[
                    Slot::Column,
                    Slot::Peak,
                    Slot::Pan,
                    Slot::Volume,
                    Slot::Activator,
                    Slot::Solo,
                ][..],
            ),
        ] {
            for slot in slots {
                push(
                    &format!("m{n}{slot:?}"),
                    Area::MainView,
                    Some(TrackControl {
                        place: mixer,
                        track: track(n),
                        slot: *slot,
                    }),
                );
            }
        }
        for n in 1..=2 {
            for slot in [
                Slot::Title,
                Slot::Activator,
                Slot::Solo,
                Slot::Volume,
                Slot::Pan,
            ] {
                push(
                    &format!("h{n}{slot:?}"),
                    Area::MainView,
                    Some(TrackControl {
                        place: Place::Header,
                        track: track(n),
                        slot,
                    }),
                );
            }
        }
        push("optimize", Area::MainView, None);
        order
    }

    // Spec: accessibility R3, R5.
    #[test]
    fn tab_stays_in_its_area_and_wraps_only_when_asked() {
        let order = order();
        let id = Id::new;
        let tab = |from, backwards, wrap| order.tab(from, Area::ControlBar, None, backwards, wrap);
        assert_eq!(tab(Some(id("a")), false, false), Some(id("b")));
        assert_eq!(tab(Some(id("c")), false, false), None);
        assert_eq!(tab(Some(id("c")), false, true), Some(id("a")));
        assert_eq!(tab(Some(id("a")), true, true), Some(id("c")));
        assert_eq!(tab(None, false, false), Some(id("a")));
        assert_eq!(tab(None, true, false), Some(id("c")));
    }

    // Spec: accessibility R4.
    #[test]
    fn tab_goes_round_a_mixer_strip_in_live_s_order() {
        let order = order();
        let id = Id::new;
        let tab =
            |from: &'static str, wrap| order.tab(Some(id(from)), Area::MainView, None, false, wrap);
        assert_eq!(tab("m1Column", false), Some(id("m1Peak")));
        assert_eq!(tab("m1Pan", false), Some(id("m1Activator")));
        assert_eq!(tab("m1Solo", false), Some(id("m1Volume")));
        assert_eq!(tab("m1Volume", false), None, "the strip's last control");
        assert_eq!(
            tab("m1Volume", true),
            Some(id("m1Column")),
            "round to the title bar"
        );
        let mixer = Place::Mixer(MixerOf::Session);
        assert_eq!(
            order.enter_track(mixer, track(1), false),
            Some(id("m1Peak"))
        );
        assert_eq!(
            order.enter_track(mixer, track(2), false),
            Some(id("m2Activator"))
        );
        assert_eq!(
            order.enter_track(mixer, track(1), true),
            Some(id("m1Volume"))
        );
    }

    // Spec: accessibility R4.
    #[test]
    fn tab_runs_through_the_arrangement_with_the_selected_track_s_header() {
        let order = order();
        let id = Id::new;
        let tab = |from: Option<&'static str>, backwards, wrap| {
            order.tab(
                from.map(id),
                Area::MainView,
                Some(track(2)),
                backwards,
                wrap,
            )
        };
        assert_eq!(tab(None, false, false), Some(id("h2Title")));
        assert_eq!(tab(Some("h2Title"), false, false), Some(id("h2Activator")));
        assert_eq!(tab(Some("h2Pan"), false, false), Some(id("optimize")));
        assert_eq!(tab(Some("optimize"), false, false), None);
        assert_eq!(tab(Some("optimize"), false, true), Some(id("h2Title")));
        // From another track's header (after a neighbour move), its header.
        assert_eq!(tab(Some("h1Volume"), false, false), Some(id("h1Pan")));
        assert_eq!(tab(Some("h1Title"), true, true), Some(id("optimize")));
    }

    // Spec: accessibility R4.
    #[test]
    fn neighbours_skip_tracks_without_the_control_and_reveal_undrawn_ones() {
        let order = order();
        let id = Id::new;
        let tracks: Vec<TrackId> = (1..=4).map(track).collect();
        let mixer = Place::Mixer(MixerOf::Session);
        let at = |n, slot| TrackControl {
            place: mixer,
            track: track(n),
            slot,
        };
        assert_eq!(
            order.neighbor(at(1, Slot::Volume), &tracks, false),
            Some(Neighbor::Focus(id("m3Volume")))
        );
        assert_eq!(
            order.neighbor(at(3, Slot::Volume), &tracks, true),
            Some(Neighbor::Focus(id("m1Volume")))
        );
        assert_eq!(
            order.neighbor(at(3, Slot::Solo), &tracks, false),
            Some(Neighbor::Reveal(at(4, Slot::Solo)))
        );
        assert_eq!(order.neighbor(at(1, Slot::Solo), &tracks, true), None);
    }
}
