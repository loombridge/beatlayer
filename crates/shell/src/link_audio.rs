//! Link Audio in the app (the Link Audio spec, R1 to R11): the tracks offered
//! to the session as channels, the peers' channels the tracks take, and the
//! settings both read.
//!
//! The main thread owns what Link makes and frees: [`LinkAudioHost`] keeps a
//! [`bl_link::Sink`] for each offered track and a [`bl_link::Source`] for each
//! channel a track takes, and hands the audio thread a [`Plan`] of them each
//! time they change. The audio thread's [`Bank`] reads the plan: it stages
//! each offered track's blocks and sends them, and reads the rings the sources
//! fill from Link's own thread. Neither side frees what the other holds: the
//! plan is shared, and the engine lets go of a bank only on its drop thread.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::Relaxed};

use bl_engine::{LinkAudio, LinkChannel, frame_at};
use bl_link::{Channel, ChannelId, PeerId, QUANTUM, Received, Sink, Source};
use bl_model::{DeviceRole, Document, Input, TrackId, TrackKind};
use bl_shell_spec::link::{LinkAudioPeer, MAX_LATENCY_MS, PeerState};

/// What the audio thread and Link's thread read of the Link page: the
/// Latency and Sync to Incoming Audio (the Link Audio spec, R7, R8).
#[derive(Debug, Default)]
pub struct AudioSettings {
    latency_micros: AtomicI64,
    sync: AtomicBool,
}

impl AudioSettings {
    /// Sets the Latency, in milliseconds, brought into 0 to 200.
    pub fn set_latency_ms(&self, ms: f64) {
        let ms = if ms.is_finite() { ms } else { 0.0 };
        let micros = (ms.clamp(0.0, MAX_LATENCY_MS) * 1000.0).round() as i64;
        self.latency_micros.store(micros, Relaxed);
    }

    /// Switches Sync to Incoming Audio.
    pub fn set_sync(&self, on: bool) {
        self.sync.store(on, Relaxed);
    }

    /// The Latency, in microseconds.
    #[must_use]
    pub fn latency_micros(&self) -> i64 {
        self.latency_micros.load(Relaxed)
    }

    /// How many microseconds Beatlayer delays its own playback by: the
    /// Latency while Sync to Incoming Audio is on, else none (R8).
    #[must_use]
    pub fn delay_micros(&self) -> i64 {
        if self.sync.load(Relaxed) {
            self.latency_micros()
        } else {
            0
        }
    }
}

/// What the audio thread has of Link Audio: the offered tracks' sinks and the
/// channels the tracks take, in rings.
pub struct Plan {
    settings: Arc<AudioSettings>,
    /// The offered tracks, by id.
    sinks: Vec<(TrackId, Arc<Sink>)>,
    /// The channels taken, by the names a track's input keeps.
    inputs: Vec<(String, String, Arc<LinkChannel>)>,
}

/// The audio thread's Link Audio, over a [`Plan`] (R4, R5, R6, R11): it
/// stages the offered tracks' blocks and sends them with the session's beat
/// of their first frame, and hands the engine the rings the sources fill.
pub struct Bank {
    audio: bl_link::Audio,
    plan: Arc<Plan>,
    offered: Vec<TrackId>,
    /// What each offered track's block holds so far, left and right.
    staged: Vec<(Vec<f32>, Vec<f32>)>,
}

impl Bank {
    /// A bank over `plan` for blocks of up to `max_frames`, made off the
    /// audio thread: it makes the buffers the audio thread fills.
    #[must_use]
    pub fn new(handle: &bl_link::Handle, plan: Arc<Plan>, max_frames: usize) -> Self {
        Self {
            audio: handle.audio(),
            offered: plan.sinks.iter().map(|(track, _)| *track).collect(),
            staged: plan
                .sinks
                .iter()
                .map(|_| (vec![0.0; max_frames], vec![0.0; max_frames]))
                .collect(),
            plan,
        }
    }
}

impl LinkAudio for Bank {
    fn active(&self) -> bool {
        self.audio.is_enabled() && self.audio.audio_enabled()
    }

    fn channel(&self, peer: &str, channel: &str) -> Option<&LinkChannel> {
        self.plan
            .inputs
            .iter()
            .find(|(p, c, _)| p == peer && c == channel)
            .map(|(_, _, ring)| &**ring)
    }

    fn offered(&self) -> &[TrackId] {
        &self.offered
    }

    fn stage(&mut self, track: TrackId, at: usize, left: &[f32], right: &[f32]) {
        if let Ok(i) = self.offered.binary_search(&track) {
            let (l, r) = &mut self.staged[i];
            let end = (at + left.len()).min(l.len());
            l[at..end].copy_from_slice(&left[..end - at]);
            r[at..end].copy_from_slice(&right[..end - at]);
        }
    }

    fn send(&mut self, moment: i64, frames: usize, rate: u32) {
        // Spec: link-audio R8. The beat labels what the song plays, which is
        // the session's as it was the delay ago.
        self.audio.capture();
        let at = moment - self.plan.settings.delay_micros();
        let beats = self.audio.beat_at_time(at, QUANTUM);
        for ((_, sink), (left, right)) in self.plan.sinks.iter().zip(&mut self.staged) {
            let frames = frames.min(left.len());
            sink.send(&self.audio, beats, &left[..frames], &right[..frames], rate);
            left[..frames].fill(0.0);
            right[..frames].fill(0.0);
        }
    }
}

/// What a peer's buffers become, to put in a ring at the engine's rate: the
/// samples as `f32`, a mono channel on both sides, and another rate
/// interpolated linearly to the engine's.
///
/// shortcut: linear interpolation images a little where the rates differ
/// (peers at 44.1 and 48 kHz); upgrade to a polyphase converter if the images
/// are heard, which takes more latency than a network buffer has to spare.
fn convert(
    buffer: &Received<'_>,
    rate: u32,
    left: &mut Vec<f32>,
    right: &mut Vec<f32>,
) {
    left.clear();
    right.clear();
    let frames = buffer.frames;
    let at = |frame: usize, side: usize| {
        let side = side.min(buffer.channels - 1);
        f32::from(buffer.samples[frame * buffer.channels + side]) / 32767.0
    };
    if buffer.rate == rate {
        for frame in 0..frames {
            left.push(at(frame, 0));
            right.push(at(frame, 1));
        }
        return;
    }
    let out = (frames as u64 * u64::from(rate) + u64::from(buffer.rate) / 2) / u64::from(buffer.rate);
    let step = f64::from(buffer.rate) / f64::from(rate);
    for j in 0..out as usize {
        let p = j as f64 * step;
        let i = (p as usize).min(frames - 1);
        let (a, b) = (i, (i + 1).min(frames - 1));
        let t = (p - i as f64) as f32;
        for (to, side) in [(&mut *left, 0), (&mut *right, 1)] {
            to.push(at(a, side) * (1.0 - t) + at(b, side) * t);
        }
    }
}

/// A source's callback, on Link's thread: it puts each buffer in the ring on
/// the frame it plays on, which is its beat's moment plus the Latency (R6).
struct Feed {
    ring: Arc<LinkChannel>,
    rate: u32,
    settings: Arc<AudioSettings>,
    /// Where the last buffer ended: a buffer that begins within a frame or
    /// two of it goes on from there, so rounding leaves no gap or overlap.
    next: Option<i64>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Feed {
    fn receive(&mut self, buffer: &Received<'_>) {
        let Some(begin) = buffer.begin_micros else {
            return;
        };
        if buffer.frames == 0 {
            return;
        }
        let mut start = frame_at(begin + self.settings.latency_micros(), self.rate);
        if let Some(next) = self.next
            && start.abs_diff(next) <= 2
        {
            start = next;
        }
        convert(buffer, self.rate, &mut self.left, &mut self.right);
        self.next = Some(start + self.left.len() as i64);
        self.ring.push(start, &self.left, &self.right);
    }
}

/// A channel a track takes, and the source that fills its ring.
struct Taken {
    peer: String,
    channel: String,
    id: ChannelId,
    ring: Arc<LinkChannel>,
    _source: Source,
    /// The frames the ring had kept when last looked at, and whether more
    /// had come then.
    seen: u64,
    receiving: bool,
}

/// What the app wants Link Audio to be: the offered tracks and the channels
/// taken, from the set and the settings (R4, R5).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    /// Whether Link and Link Audio are on, and the engine runs.
    pub on: bool,
    /// The engine's rate: the rings are made at it.
    pub rate: u32,
    /// The offered tracks and the names they are offered by.
    pub tracks: Vec<(TrackId, String)>,
    /// The peer and channel names the tracks' inputs keep, without repeats.
    pub inputs: Vec<(String, String)>,
}

impl Wanted {
    /// What `document` asks for, with Link Audio `on` and the engine at
    /// `rate`: every track with an audio output but Main (R4), and every
    /// channel an audio track's input names (R5).
    #[must_use]
    pub fn of(document: &Document, on: bool, rate: u32) -> Self {
        let mut wanted = Self {
            on: on && rate > 0,
            rate,
            ..Self::default()
        };
        if !wanted.on {
            return wanted;
        }
        for id in document.track_order() {
            let Some(track) = document.track(id) else {
                continue;
            };
            if offers(track) {
                let name = track.name();
                let name = if name.is_empty() {
                    crate::routing::track_label(document, id)
                } else {
                    name.to_owned()
                };
                wanted.tracks.push((id, name));
            }
            if let Input::LinkAudio { peer, channel } = &track.routing().input
                && track.kind() == TrackKind::Audio
                && !wanted.inputs.iter().any(|(p, c)| p == peer && c == channel)
            {
                wanted.inputs.push((peer.clone(), channel.clone()));
            }
        }
        wanted.tracks.sort_by_key(|(id, _)| *id);
        wanted
    }
}

/// Whether a track has an audio output Link Audio offers: audio tracks, MIDI
/// tracks that hold an instrument, group tracks and return tracks (R4).
fn offers(track: &bl_model::Track) -> bool {
    match track.kind() {
        TrackKind::Audio | TrackKind::Group | TrackKind::Return => true,
        TrackKind::Midi => track.devices().any(|d| d.role == DeviceRole::Instrument),
        _ => false,
    }
}

/// A fingerprint of what [`Wanted::of`] reads, which costs no allocation, so
/// each poll can tell whether the set changed it.
#[must_use]
pub fn fingerprint(document: &Document, on: bool, rate: u32) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    (on, rate).hash(&mut hasher);
    if on {
        for id in document.track_order() {
            let Some(track) = document.track(id) else {
                continue;
            };
            id.hash(&mut hasher);
            offers(track).hash(&mut hasher);
            track.name().hash(&mut hasher);
            if let Input::LinkAudio { peer, channel } = &track.routing().input {
                (peer, channel).hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// What the main thread keeps of Link Audio: the sinks and sources it made,
/// the channels the peers offer, and the plan the audio thread was last
/// given.
#[derive(Default)]
pub struct LinkAudioHost {
    settings: Arc<AudioSettings>,
    channels: Vec<Channel>,
    sinks: BTreeMap<TrackId, (String, Arc<Sink>)>,
    taken: Vec<Taken>,
    rate: u32,
    /// What [`fingerprint`] said when the plan was last made.
    shown: Option<u64>,
    /// Whether Link said its channels changed since the plan was made.
    changed: bool,
}

impl LinkAudioHost {
    /// The Latency and Sync to Incoming Audio, which the audio thread and
    /// Link's thread read.
    #[must_use]
    pub fn settings(&self) -> &Arc<AudioSettings> {
        &self.settings
    }

    /// Link said the channels the peers offer changed.
    pub fn channels_changed(&mut self) {
        self.changed = true;
    }

    /// Whether the plan is out of date with what `document`, the switches
    /// and the engine's `rate` say.
    #[must_use]
    pub fn stale(&self, document: &Document, on: bool, rate: u32) -> bool {
        self.changed || self.shown != Some(fingerprint(document, on, rate))
    }

    /// Brings the sinks and sources to `wanted`, and returns the plan the
    /// audio thread should have now, if it changed. Made off the audio
    /// thread: Link allocates here.
    pub fn update(
        &mut self,
        link: &bl_link::Link,
        fingerprint: u64,
        wanted: &Wanted,
    ) -> Option<Arc<Plan>> {
        self.shown = Some(fingerprint);
        self.changed = false;
        self.channels = if wanted.on { link.channels() } else { Vec::new() };
        let mut moved = false;
        if !wanted.on {
            moved = !self.sinks.is_empty() || !self.taken.is_empty();
            self.sinks.clear();
            self.taken.clear();
        }
        // The offered tracks: a name the track changed is the channel's too.
        let before = self.sinks.len();
        self.sinks
            .retain(|id, _| wanted.tracks.iter().any(|(track, _)| track == id));
        moved |= self.sinks.len() != before;
        for (id, name) in &wanted.tracks {
            match self.sinks.get_mut(id) {
                Some((had, sink)) if had != name => {
                    sink.set_name(name);
                    name.clone_into(had);
                }
                Some(_) => {}
                None => {
                    self.sinks
                        .insert(*id, (name.clone(), Arc::new(link.sink(name))));
                    moved = true;
                }
            }
        }
        // The channels taken: one a peer offers by those names, as the
        // session lists it now; a peer that is gone leaves the track silent
        // and its names kept (R10).
        let rate = wanted.rate;
        let before = self.taken.len();
        let channels = &self.channels;
        let settings = &self.settings;
        let found = |peer: &str, name: &str| {
            channels
                .iter()
                .find(|c| c.peer_name == peer && c.name == name)
                .map(|c| c.id)
        };
        self.taken.retain(|t| {
            wanted.inputs.iter().any(|(p, c)| *p == t.peer && *c == t.channel)
                && found(&t.peer, &t.channel) == Some(t.id)
                && t.ring.rate() == rate
        });
        moved |= self.taken.len() != before;
        for (peer, channel) in &wanted.inputs {
            if self.taken.iter().any(|t| t.peer == *peer && t.channel == *channel) {
                continue;
            }
            let Some(id) = found(peer, channel) else {
                continue;
            };
            let ring = Arc::new(LinkChannel::new(rate));
            let mut feed = Feed {
                ring: Arc::clone(&ring),
                rate,
                settings: Arc::clone(settings),
                next: None,
                left: Vec::new(),
                right: Vec::new(),
            };
            self.taken.push(Taken {
                peer: peer.clone(),
                channel: channel.clone(),
                id,
                ring,
                _source: link.source(id, move |buffer| feed.receive(buffer)),
                seen: 0,
                receiving: false,
            });
            moved = true;
        }
        self.rate = rate;
        moved.then(|| self.plan())
    }

    /// The plan for what is made now.
    #[must_use]
    pub fn plan(&self) -> Arc<Plan> {
        Arc::new(Plan {
            settings: Arc::clone(&self.settings),
            sinks: self
                .sinks
                .iter()
                .map(|(id, (_, sink))| (*id, Arc::clone(sink)))
                .collect(),
            inputs: self
                .taken
                .iter()
                .map(|t| (t.peer.clone(), t.channel.clone(), Arc::clone(&t.ring)))
                .collect(),
        })
    }

    /// Looks at what the rings have kept since the last look, for each peer's
    /// state, and returns whether any ring started or stopped receiving.
    /// Called once a poll.
    pub fn poll(&mut self) -> bool {
        let mut flipped = false;
        for taken in &mut self.taken {
            let kept = taken.ring.received();
            let receiving = kept != taken.seen;
            flipped |= receiving != taken.receiving;
            taken.receiving = receiving;
            taken.seen = kept;
        }
        flipped
    }

    /// The peers with Link Audio on that offer a channel, in the order Link
    /// lists them, with their state and what is buffered from each (R9).
    #[must_use]
    pub fn peers(&self) -> Vec<LinkAudioPeer> {
        let mut peers: Vec<(PeerId, LinkAudioPeer)> = Vec::new();
        for channel in &self.channels {
            let at = match peers.iter().position(|(id, _)| *id == channel.peer) {
                Some(at) => at,
                None => {
                    peers.push((
                        channel.peer,
                        LinkAudioPeer {
                            name: channel.peer_name.clone(),
                            state: PeerState::Connected,
                            buffered_ms: 0.0,
                            channels: Vec::new(),
                        },
                    ));
                    peers.len() - 1
                }
            };
            peers[at].1.channels.push(channel.name.clone());
        }
        for taken in &self.taken {
            let Some((_, peer)) = peers.iter_mut().find(|(_, p)| p.name == taken.peer) else {
                continue;
            };
            if taken.receiving {
                peer.state = PeerState::Receiving;
            }
            peer.buffered_ms +=
                taken.ring.buffered() as f64 * 1000.0 / f64::from(taken.ring.rate());
        }
        peers.into_iter().map(|(_, peer)| peer).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received(samples: &[i16], channels: usize, rate: u32) -> Received<'_> {
        Received {
            samples,
            channels,
            frames: samples.len() / channels,
            rate,
            begin_micros: Some(0),
        }
    }

    // Spec: link-audio R5, R6
    #[test]
    fn a_mono_buffer_is_the_same_on_both_sides_and_a_stereo_one_keeps_them() {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        convert(&received(&[32767, 0, -32767], 1, 48_000), 48_000, &mut left, &mut right);
        assert_eq!(left, [1.0, 0.0, -1.0]);
        assert_eq!(left, right);
        convert(&received(&[32767, -32767, 0, 32767], 2, 48_000), 48_000, &mut left, &mut right);
        assert_eq!(left, [1.0, 0.0]);
        assert_eq!(right, [-1.0, 1.0]);
    }

    // Spec: link-audio R6
    #[test]
    fn a_peer_at_another_rate_is_converted_to_ours_keeping_its_length_in_time() {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        // 441 frames at 44.1 kHz of a constant are 480 at 48 kHz.
        let samples = vec![16384i16; 441];
        convert(&received(&samples, 1, 44_100), 48_000, &mut left, &mut right);
        assert_eq!(left.len(), 480);
        assert!(left.iter().all(|&s| (s - 16384.0 / 32767.0).abs() < 1e-6));
        // And a ramp stays a ramp at the same slope in time.
        let ramp: Vec<i16> = (0..441).map(|i| (i * 10) as i16).collect();
        convert(&received(&ramp, 1, 44_100), 48_000, &mut left, &mut right);
        let slope = (left[300] - left[200]) / 100.0;
        assert!((slope - 10.0 * 44_100.0 / 48_000.0 / 32767.0).abs() < 1e-7);
        // Down: 480 frames at 48 kHz are 441 at 44.1 kHz.
        convert(&received(&vec![0i16; 960], 2, 48_000), 44_100, &mut left, &mut right);
        assert_eq!((left.len(), right.len()), (441, 441));
    }

    #[test]
    fn the_latency_is_kept_in_range_and_sync_delays_only_when_on() {
        let settings = AudioSettings::default();
        settings.set_latency_ms(20.0);
        assert_eq!(settings.latency_micros(), 20_000);
        assert_eq!(settings.delay_micros(), 0);
        settings.set_sync(true);
        assert_eq!(settings.delay_micros(), 20_000);
        settings.set_latency_ms(500.0);
        assert_eq!(settings.latency_micros(), 200_000);
        settings.set_latency_ms(-3.0);
        assert_eq!(settings.latency_micros(), 0);
        settings.set_latency_ms(f64::NAN);
        assert_eq!(settings.latency_micros(), 0);
    }
}
