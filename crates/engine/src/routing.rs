//! The signal graph a document's routing makes (the routing spec): which
//! track feeds which, through what, and the routings the engine plays as
//! silence because they would close a loop.
//!
//! Every track runs once a block, after every track that feeds it, so the
//! graph must have no loop. Live accepts a routing that makes one
//! (observed), and so does the document; the compiler breaks it here:
//!
//! - Among audio, MIDI, and group tracks, outputs into groups come first,
//!   then outputs into other tracks, then inputs taken from other tracks,
//!   each in display order. A routing that would close a loop with those
//!   already taken carries nothing.
//! - A track that hears Main, because it resamples or takes Main or a
//!   track that hears Main as its input, sends nothing anywhere: its output
//!   and its sends are silent (Live suppresses a resampling track's output,
//!   so it can't feed back).
//! - A track that hears a return track, because it takes one as its input
//!   or a return's output reaches it, has silent sends: a return can't
//!   hear itself.
//! - Sidechains come last (WP-2.19b): each device's sidechain Source, in
//!   display order. One whose source the device's track already feeds, by
//!   any route, carries nothing. A device listening to its own track
//!   before its devices (Pre FX) hears it, as the signal is there before
//!   the chain runs; after them it would hear itself, and carries nothing.
//! - MIDI sidechains come after those (WP-5.27): each device's MIDI source,
//!   in display order, under the same rule. A device listening to its own
//!   track hears the notes its clips and input play, which are there
//!   before the chain runs.

use std::collections::{BTreeMap, BTreeSet};

use bl_model::{ChainId, DeviceId, Document, Input, SendTap, Tap, TrackId, TrackKind};

/// How a signal travels from one track into another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The source's output (Audio To): into its group, Main, or another
    /// track's input (Track In).
    Output,
    /// A send into a return track.
    Send,
    /// The destination's own input (Audio From), Resampling included.
    Input,
    /// A device's sidechain Source, on the destination track (WP-2.19b).
    Sidechain(DeviceId),
}

impl Via {
    /// The device whose sidechain the route feeds, if it feeds one.
    #[must_use]
    pub fn sidechain(self) -> Option<DeviceId> {
        match self {
            Self::Sidechain(device) => Some(device),
            _ => None,
        }
    }
}

/// A MIDI track's notes and channel messages reaching a device's MIDI
/// sidechain (WP-5.27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiLink {
    /// The MIDI track whose notes the device hears.
    pub from: TrackId,
    /// The track of the device.
    pub to: TrackId,
    /// The device.
    pub device: DeviceId,
}

/// One route in the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The track whose signal it carries.
    pub from: TrackId,
    /// The track it feeds.
    pub to: TrackId,
    /// How.
    pub via: Via,
    /// Where in the source the signal is taken.
    pub tap: Tap,
    /// The chain of a rack in the source whose signal at `tap` it carries,
    /// instead of the source's own (the racks spec, R38).
    pub chain: Option<ChainId>,
    /// Whether the destination's Monitor switch decides if it's heard: a
    /// track's own input, or another track's output into its Track In.
    pub monitored: bool,
}

/// The routes a document's routing makes, with every loop broken.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// Every route the engine plays, sends included.
    pub edges: Vec<Edge>,
    /// The MIDI sidechains the engine plays.
    pub midi_links: Vec<MidiLink>,
    /// Routings the document asks for that the engine plays as silence,
    /// because they would close a loop: (from, to).
    pub broken: Vec<(TrackId, TrackId)>,
    /// Tracks that hear Main: their output and sends are silent.
    pub hears_main: BTreeSet<TrackId>,
    /// Tracks that hear a return track: their sends are silent.
    pub hears_return: BTreeSet<TrackId>,
}

impl Graph {
    /// The graph for `document`.
    #[must_use]
    pub fn of(document: &Document) -> Self {
        let main = document.main_id();
        let order = document.track_order();
        let kind = |id: TrackId| document.track(id).map(bl_model::Track::kind);
        let inner = |id: TrackId| {
            matches!(
                kind(id),
                Some(TrackKind::Audio | TrackKind::Midi | TrackKind::Group)
            )
        };
        let monitored_into = |to: TrackId| kind(to) == Some(TrackKind::Audio);

        // The routes among audio, MIDI, and group tracks, in priority
        // order.
        let mut wanted: Vec<Edge> = Vec::new();
        let output_edge = |from: TrackId, to: TrackId| Edge {
            from,
            to,
            via: Via::Output,
            tap: Tap::PostMixer,
            chain: None,
            monitored: monitored_into(to),
        };
        for &id in &order {
            if !inner(id) {
                continue;
            }
            if let Some(to) = document.output_of(id).filter(|&to| inner(to))
                && kind(to) == Some(TrackKind::Group)
            {
                wanted.push(output_edge(id, to));
            }
        }
        for &id in &order {
            if !inner(id) {
                continue;
            }
            if let Some(to) = document.output_of(id).filter(|&to| inner(to))
                && kind(to) != Some(TrackKind::Group)
            {
                wanted.push(output_edge(id, to));
            }
        }
        for &id in &order {
            if let Some(Input::Track {
                track: from,
                tap,
                chain,
            }) = input_of(document, id)
                && inner(from)
                && inner(id)
            {
                wanted.push(Edge {
                    from,
                    to: id,
                    via: Via::Input,
                    tap,
                    chain,
                    monitored: true,
                });
            }
        }
        let mut graph = Self::default();
        let mut kept: BTreeMap<TrackId, Vec<TrackId>> = BTreeMap::new();
        let mut inner_edges = Vec::new();
        for edge in wanted {
            if reaches(&kept, edge.to, edge.from) {
                graph.broken.push((edge.from, edge.to));
            } else {
                kept.entry(edge.from).or_default().push(edge.to);
                inner_edges.push(edge);
            }
        }

        // Who hears Main, and who hears a return.
        let mut seeds_main = Vec::new();
        let mut seeds_return = Vec::new();
        for &id in &order {
            match input_of(document, id) {
                Some(Input::Resampling) => seeds_main.push(id),
                Some(Input::Track { track, .. }) if track == main => seeds_main.push(id),
                Some(Input::Track { track, .. }) if kind(track) == Some(TrackKind::Return) => {
                    seeds_return.push(id);
                }
                _ => {}
            }
            if kind(id) == Some(TrackKind::Return)
                && let Some(to) = document.output_of(id).filter(|&to| to != main)
            {
                seeds_return.push(to);
            }
        }
        graph.hears_main = spread(&kept, seeds_main);
        graph.hears_return = spread(&kept, seeds_return);

        // The routes the engine plays, in display order of their
        // destination's inputs.
        for edge in inner_edges {
            if !graph.hears_main.contains(&edge.from) || edge.via == Via::Input {
                graph.edges.push(edge);
            }
        }
        for &id in &order {
            let Some(track) = document.track(id) else {
                continue;
            };
            if id != main && !graph.hears_main.contains(&id) {
                // Outputs into Main, and a return's into a track.
                match document.output_of(id) {
                    Some(to) if to == main => graph.edges.push(output_edge(id, main)),
                    Some(to) if track.kind() == TrackKind::Return => {
                        graph.edges.push(output_edge(id, to));
                    }
                    _ => {}
                }
                if track.kind().has_sends() && !graph.hears_return.contains(&id) {
                    for to in document.returns() {
                        let tap = match document.track(to).map(|r| r.mixer().tap) {
                            Some(SendTap::Pre) => Tap::PostFx,
                            _ => Tap::PostMixer,
                        };
                        graph.edges.push(Edge {
                            from: id,
                            to,
                            via: Via::Send,
                            tap,
                            chain: None,
                            monitored: false,
                        });
                    }
                }
            }
            match input_of(document, id) {
                Some(Input::Resampling) => graph.edges.push(Edge {
                    from: main,
                    to: id,
                    via: Via::Input,
                    tap: Tap::PostMixer,
                    chain: None,
                    monitored: true,
                }),
                Some(Input::Track {
                    track: from,
                    tap,
                    chain,
                }) if !inner(from) => {
                    graph.edges.push(Edge {
                        from,
                        to: id,
                        via: Via::Input,
                        tap,
                        chain,
                        monitored: true,
                    });
                }
                _ => {}
            }
        }
        graph.add_sidechains(document, &order);
        graph.add_midi_links(document, &order);
        graph
    }

    /// Adds the routes into devices' sidechains, after every other route,
    /// leaving out each that would close a loop.
    fn add_sidechains(&mut self, document: &Document, order: &[TrackId]) {
        let mut wanted = Vec::new();
        for &id in order {
            let Some(track) = document.track(id) else {
                continue;
            };
            for device in track.devices_deep() {
                if let Some(Input::Track {
                    track: from, tap, ..
                }) = device.sidechain.as_ref().map(|s| &s.input)
                {
                    wanted.push(Edge {
                        from: *from,
                        to: id,
                        via: Via::Sidechain(device.id),
                        tap: *tap,
                        chain: None,
                        monitored: false,
                    });
                }
            }
        }
        if wanted.is_empty() {
            return;
        }
        let mut all: BTreeMap<TrackId, Vec<TrackId>> = BTreeMap::new();
        for edge in &self.edges {
            all.entry(edge.from).or_default().push(edge.to);
        }
        for edge in wanted {
            if edge.from == edge.to {
                // Its own track before its devices is there already.
                if edge.tap == Tap::PreFx {
                    self.edges.push(edge);
                } else {
                    self.broken.push((edge.from, edge.to));
                }
            } else if reaches(&all, edge.to, edge.from) {
                self.broken.push((edge.from, edge.to));
            } else {
                all.entry(edge.from).or_default().push(edge.to);
                self.edges.push(edge);
            }
        }
    }

    /// Adds the MIDI sidechains' links, after every audio route, leaving
    /// out each that would close a loop.
    fn add_midi_links(&mut self, document: &Document, order: &[TrackId]) {
        let mut all: BTreeMap<TrackId, Vec<TrackId>> = BTreeMap::new();
        for edge in &self.edges {
            all.entry(edge.from).or_default().push(edge.to);
        }
        for &id in order {
            let Some(track) = document.track(id) else {
                continue;
            };
            for device in track.devices_deep() {
                let Some(from) = device.midi_sidechain.as_ref().and_then(|s| s.source) else {
                    continue;
                };
                if document.track(from).is_none() {
                    continue;
                }
                // Its own track's notes are there before its chain runs.
                if from == id {
                    self.midi_links.push(MidiLink {
                        from,
                        to: id,
                        device: device.id,
                    });
                } else if reaches(&all, id, from) {
                    self.broken.push((from, id));
                } else {
                    all.entry(from).or_default().push(id);
                    self.midi_links.push(MidiLink {
                        from,
                        to: id,
                        device: device.id,
                    });
                }
            }
        }
    }

    /// The tracks whose output reaches `track`'s own input or mix, through
    /// outputs: its group's members, and tracks routed to its Track In.
    pub fn output_sources(&self, track: TrackId) -> impl Iterator<Item = TrackId> + '_ {
        self.edges
            .iter()
            .filter(move |e| e.to == track && e.via == Via::Output)
            .map(|e| e.from)
    }

    /// Where `track`'s output goes, if the engine plays it.
    #[must_use]
    pub fn output_target(&self, track: TrackId) -> Option<TrackId> {
        self.edges
            .iter()
            .find(|e| e.from == track && e.via == Via::Output)
            .map(|e| e.to)
    }
}

/// An audio or MIDI track's input; `None` for other kinds.
fn input_of(document: &Document, id: TrackId) -> Option<Input> {
    let track = document.track(id)?;
    track
        .kind()
        .holds_clips()
        .then(|| track.routing().input.clone())
}

/// Whether `to` can be reached from `from` along `edges`.
fn reaches(edges: &BTreeMap<TrackId, Vec<TrackId>>, from: TrackId, to: TrackId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(at) = stack.pop() {
        if at == to {
            return true;
        }
        if seen.insert(at) {
            stack.extend(edges.get(&at).into_iter().flatten().copied());
        }
    }
    false
}

/// Every track reachable from `seeds` along `edges`, the seeds included.
fn spread(edges: &BTreeMap<TrackId, Vec<TrackId>>, seeds: Vec<TrackId>) -> BTreeSet<TrackId> {
    let mut reached = BTreeSet::new();
    let mut stack = seeds;
    while let Some(at) = stack.pop() {
        if reached.insert(at) {
            stack.extend(edges.get(&at).into_iter().flatten().copied());
        }
    }
    reached
}
