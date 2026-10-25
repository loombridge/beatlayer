//! The clips, media, scenes and lanes of an export (the DAWproject spec,
//! R70 to R74, R79): each clip with its places, markers, loop, fades and
//! content, the take lanes, and the Session View's slots.

use std::collections::{BTreeMap, BTreeSet};

use bl_model::{AudioData, Clip, ClipData, ClipId, MidiData, WarpMode};

use super::automation::beats;
use super::*;
use crate::dawproject::model::{
    Arrangement, Clip as DawClip, ClipSlot, Clips, Content, FileReference, Lanes, MediaFile, Notes,
    PointValue, Points, Scene, Target, TimeUnit, Timeline, Warp, Warps,
};

/// The crossfades among a track's clips: the clip that fades in over the
/// one before it, with the crossfade's whole length in seconds, and the
/// clips that fade out under one (R71).
#[derive(Default)]
struct Fades {
    later: BTreeMap<ClipId, f64>,
    earlier: BTreeSet<ClipId>,
}

impl Fades {
    fn of(track: &Track) -> Self {
        let mut fades = Self::default();
        for crossfade in bl_model::crossfades(track.clips()) {
            fades
                .later
                .insert(crossfade.later, crossfade.before + crossfade.after);
            fades.earlier.insert(crossfade.earlier);
        }
        fades
    }
}

/// A warp mode as the file names it (R71).
const fn algorithm(mode: WarpMode) -> &'static str {
    match mode {
        WarpMode::RePitch => "repitch",
        WarpMode::Beats => "beats",
        WarpMode::Tones => "tones",
        WarpMode::Texture => "texture",
        WarpMode::Complex => "complex",
        WarpMode::ComplexPro => "complex pro",
    }
}

/// The attributes the beat timelines inside a clip share.
fn beat_info() -> TimelineInfo {
    TimelineInfo {
        time_unit: Some(TimeUnit::Beats),
        ..TimelineInfo::default()
    }
}

impl Exporter<'_> {
    /// The Arrangement: each track's lanes, the markers and the tempo and
    /// signature automation (R72, R75, R76, R79).
    pub(super) fn arrangement(&mut self) -> Option<Arrangement> {
        let mut lanes = Vec::new();
        for id in self.order() {
            if let Some(lane) = self.track_lane(id) {
                lanes.push(lane);
            }
        }
        let markers = self.markers();
        let tempo_automation = self.tempo_automation();
        let signature_automation = self.signature_automation();
        if lanes.is_empty()
            && markers.is_none()
            && tempo_automation.is_none()
            && signature_automation.is_none()
        {
            return None;
        }
        Some(Arrangement {
            info: TimelineInfo::default(),
            lanes,
            tempo_automation,
            signature_automation,
            markers,
        })
    }

    /// A track's lanes: its Arrangement clips, its take lanes after them,
    /// and its envelopes (R72, R76).
    fn track_lane(&mut self, id: TrackId) -> Option<Timeline> {
        let track = self.document.track(id)?;
        let mut children = Vec::new();
        if track.kind().holds_clips() {
            let fades = Fades::of(track);
            let mut held: Vec<&Clip> = track.clips().collect();
            held.sort_by_key(|clip| (clip.start, clip.id));
            let clips: Vec<DawClip> = held
                .into_iter()
                .filter_map(|clip| self.clip(track, clip, &fades, None))
                .collect();
            let takes: Vec<(String, Vec<DawClip>)> = track
                .take_lanes()
                .filter_map(|lane| {
                    if track.kind() != TrackKind::Audio {
                        self.report.add(
                            "a take lane on a MIDI track",
                            &Self::place(track),
                            "not written",
                        );
                        return None;
                    }
                    let clips = lane
                        .clips
                        .iter()
                        .filter_map(|clip| self.clip(track, clip, &Fades::default(), None))
                        .collect();
                    Some((lane.name.clone(), clips))
                })
                .collect();
            if !clips.is_empty() || !takes.is_empty() {
                children.push(Timeline::Clips(Clips {
                    info: beat_info(),
                    clips,
                }));
            }
            for (name, clips) in takes {
                children.push(Timeline::Clips(Clips {
                    info: TimelineInfo {
                        name: Some(name),
                        ..beat_info()
                    },
                    clips,
                }));
            }
        }
        let automated: Vec<(AutomationTarget, Vec<bl_model::Breakpoint>)> = track
            .automation()
            .map(|(target, held)| (*target, held.envelope.points().collect()))
            .collect();
        for (target, envelope) in automated {
            if let Some(points) = self.parameter_points(track, target, &envelope) {
                children.push(points);
            }
        }
        (!children.is_empty()).then(|| {
            Timeline::Lanes(Lanes {
                info: TimelineInfo {
                    track: self.ids.tracks.get(&id).cloned(),
                    ..TimelineInfo::default()
                },
                children,
            })
        })
    }

    /// Where a clip is, for the report (R81).
    fn clip_place(&self, track: &Track, clip: &Clip, scene: Option<usize>) -> String {
        let at = match scene {
            Some(scene) => format!("in scene {scene}"),
            None => format!("at {}", self.signatures.to_bbs(clip.start)),
        };
        let track = Self::place(track);
        if clip.name.is_empty() {
            format!("a clip {at} on {track}")
        } else {
            format!("the clip \"{}\" {at} on {track}", clip.name)
        }
    }

    /// One clip, or `None` when the file leaves it out (R70, R71).
    fn clip(
        &mut self,
        track: &Track,
        clip: &Clip,
        fades: &Fades,
        scene: Option<usize>,
    ) -> Option<DawClip> {
        let place = self.clip_place(track, clip, scene);
        let own = Self::place(track);
        let note = |this: &mut Self, what: &str, held: bool| {
            if held {
                this.report.add(what, &own, "not written");
            }
        };
        note(
            self,
            "clips' launch settings and follow actions",
            clip.launch != bl_model::ClipLaunch::default(),
        );
        note(self, "clips' grooves", clip.groove.is_some());
        note(self, "clips' scales", !clip.scale.is_default());
        let colour = clip.colour.unwrap_or(track.colour());
        let mut out = DawClip {
            info: TimelineInfo {
                name: (!clip.name.is_empty()).then(|| clip.name.clone()),
                color: Some(self.colour(colour)),
                ..TimelineInfo::default()
            },
            time: Some(beats(clip.start)),
            duration: Some(beats(clip.length)),
            enable: clip.mute.then_some(false),
            ..DawClip::default()
        };
        let mut lanes = Vec::new();
        for held in &clip.envelopes {
            let envelope: Vec<bl_model::Breakpoint> = held.envelope.points().collect();
            let points = match (held.kind, held.target) {
                (
                    bl_model::EnvelopeKind::Modulation,
                    AutomationTarget::MidiCc { .. }
                    | AutomationTarget::MidiPitchBend
                    | AutomationTarget::MidiChannelPressure,
                ) => self.controller_points(held.target, &envelope),
                (bl_model::EnvelopeKind::Automation, target) if scene.is_some() => {
                    self.parameter_points(track, target, &envelope)
                }
                (bl_model::EnvelopeKind::Automation, _) => {
                    self.report.add(
                        "a clip's automation envelope in the Arrangement",
                        &own,
                        "not written",
                    );
                    None
                }
                (bl_model::EnvelopeKind::Modulation, _) => {
                    self.report
                        .add("clip modulation envelopes", &own, "not written");
                    None
                }
            };
            lanes.extend(points);
        }
        let primary = match &clip.data {
            ClipData::Midi(data) => self.midi_content(track, clip, data, &mut out),
            ClipData::Audio(data) => {
                let (timeline, expressions) =
                    self.audio_content(track, clip, data, fades, &place, &mut out)?;
                lanes.extend(expressions);
                timeline
            }
        };
        out.content = Some(Box::new(Content::Timeline(Box::new(if lanes.is_empty() {
            primary
        } else {
            let mut children = vec![primary];
            children.append(&mut lanes);
            Timeline::Lanes(Lanes {
                info: TimelineInfo::default(),
                children,
            })
        }))));
        Some(out)
    }

    /// A MIDI clip's notes, Start and End markers and loop (R70).
    fn midi_content(
        &mut self,
        track: &Track,
        clip: &Clip,
        data: &MidiData,
        out: &mut DawClip,
    ) -> Timeline {
        out.content_time_unit = Some(TimeUnit::Beats);
        out.play_start = Some(beats(data.start));
        out.play_stop = data.end.map(beats);
        if data.looping {
            out.loop_start = Some(beats(data.loop_start));
            // A brace with no end runs to the clip's end, a quarter note at
            // least, as the engine plays it.
            out.loop_end = Some(beats(data.loop_end.unwrap_or_else(|| {
                (data.loop_start + clip.length).max(data.loop_start + Ticks::QUARTER)
            })));
        }
        let (mut muted, mut chance) = (0usize, 0usize);
        let mut notes = Vec::new();
        for note in data.notes.iter() {
            if note.mute {
                muted += 1;
                continue;
            }
            if note.probability != 1.0 || note.velocity_deviation != 0 {
                chance += 1;
            }
            notes.push(crate::dawproject::model::Note {
                time: Some(beats(note.start)),
                duration: Some(beats(note.length)),
                channel: Some(0),
                key: Some(i64::from(note.key)),
                vel: Some(f64::from(note.velocity) / 127.0),
                rel: Some(f64::from(note.release_velocity) / 127.0),
                content: self
                    .note_expressions(note)
                    .map(|inner| Box::new(Content::Timeline(Box::new(inner)))),
            });
        }
        notes.sort_by(|a, b| {
            a.time
                .partial_cmp(&b.time)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.key.cmp(&b.key))
        });
        let place = Self::place(track);
        for (what, count) in [
            ("muted notes", muted),
            ("notes' chance and velocity deviation", chance),
        ] {
            if count > 0 {
                self.report.add_count(what, &place, "not written", count);
            }
        }
        Timeline::Notes(Notes {
            info: TimelineInfo::default(),
            notes,
        })
    }

    /// An audio clip's file, warping, markers, loop and fades (R71): its
    /// timeline, and the expressions beside it.
    fn audio_content(
        &mut self,
        track: &Track,
        clip: &Clip,
        data: &AudioData,
        fades: &Fades,
        place: &str,
        out: &mut DawClip,
    ) -> Option<(Timeline, Vec<Timeline>)> {
        let Some(medium) = self.medium(data.media) else {
            self.report
                .add("a clip whose file is missing", place, "left out");
            return None;
        };
        let own = Self::place(track);
        if data.warp_settings != bl_model::WarpSettings::default() {
            self.report
                .add("a warp mode's settings", &own, "not written");
        }
        if data.fade_in_curve != 0.0 || data.fade_out_curve != 0.0 {
            self.report.add("fade curves", &own, "not written");
        }
        let file = MediaFile {
            info: TimelineInfo::default(),
            duration: Some(medium.seconds),
            channels: Some(medium.channels),
            sample_rate: Some(medium.sample_rate),
            algorithm: None,
            file: Some(FileReference {
                path: medium.path.clone(),
                external: medium.external,
            }),
        };
        let timeline = if data.warp && !data.warp_markers.is_empty() {
            out.content_time_unit = Some(TimeUnit::Beats);
            let ticks = |value: f64| value / Ticks::QUARTER.get() as f64;
            out.play_start = Some(ticks(data.start));
            out.play_stop = data.end.map(ticks);
            if data.looping {
                out.loop_start = Some(ticks(data.loop_start));
                // A brace with no end runs to the end of the file, which the
                // clip's tempo carries on from the last warp marker.
                let end = data.warp_markers.last().map_or(data.loop_start, |last| {
                    let tempo = data
                        .tempo
                        .unwrap_or_else(|| self.tempo.bpm_at(clip.start).get());
                    last.beat.get() as f64
                        + (medium.seconds - last.time).max(0.0) * tempo / 60.0
                            * Ticks::QUARTER.get() as f64
                });
                out.loop_end = Some(ticks(data.loop_end.unwrap_or(end)));
            }
            let mut warps: Vec<Warp> = data
                .warp_markers
                .iter()
                .map(|marker| Warp {
                    time: Some(beats(marker.beat)),
                    content_time: Some(marker.time),
                })
                .collect();
            if let [only] = data.warp_markers.as_slice() {
                // One marker is no warp map: the clip's tempo gives a second
                // event a beat on.
                let tempo = data
                    .tempo
                    .unwrap_or_else(|| self.tempo.bpm_at(clip.start).get());
                warps.push(Warp {
                    time: Some(beats(only.beat) + 1.0),
                    content_time: Some(only.time + 60.0 / tempo),
                });
            }
            Timeline::Warps(Warps {
                info: TimelineInfo::default(),
                content_time_unit: Some(TimeUnit::Seconds),
                audio: Some(MediaFile {
                    algorithm: Some(algorithm(data.warp_mode).to_owned()),
                    ..file
                }),
                warps,
            })
        } else {
            out.content_time_unit = Some(TimeUnit::Seconds);
            out.play_start = Some(data.start);
            out.play_stop = data.end;
            if data.looping {
                out.loop_start = Some(data.loop_start);
                out.loop_end = Some(data.loop_end.unwrap_or(medium.seconds));
            }
            Timeline::Audio(file)
        };
        out.fade_time_unit = Some(TimeUnit::Seconds);
        if let Some(length) = fades.later.get(&clip.id) {
            // A crossfade with the clip before: a negative fade-in as long
            // as the crossfade (R71).
            out.fade_in = Some(-length);
        } else if data.fade_in > 0.0 {
            out.fade_in = Some(data.fade_in);
        }
        if !fades.earlier.contains(&clip.id) && data.fade_out > 0.0 {
            out.fade_out = Some(data.fade_out);
        }
        let mut expressions = Vec::new();
        let mut one = |expression, unit, value| {
            expressions.push(Timeline::Points(Points {
                info: beat_info(),
                unit: Some(unit),
                target: Target {
                    expression: Some(expression),
                    ..Target::default()
                },
                points: vec![PointValue::Real {
                    time: Some(0.0),
                    value: Some(value),
                    interpolation: None,
                }],
            }));
        };
        if data.gain != 1.0 {
            one(
                crate::dawproject::model::Expression::Gain,
                Unit::Linear,
                data.gain,
            );
        }
        if data.transpose != 0 || data.detune != 0.0 {
            one(
                crate::dawproject::model::Expression::Transpose,
                Unit::Semitones,
                f64::from(data.transpose) + data.detune / 100.0,
            );
        }
        Some((timeline, expressions))
    }

    /// The medium a hash names, embedded or referred to, found once (R73).
    fn medium(&mut self, hash: ContentHash) -> Option<Medium> {
        if let Some(found) = self.media.get(&hash) {
            return found.clone();
        }
        let found = self.find_medium(hash);
        self.media.insert(hash, found.clone());
        found
    }

    fn find_medium(&mut self, hash: ContentHash) -> Option<Medium> {
        let path = self.context.media.path(hash)?;
        let info = bl_media::probe(&path).ok()?;
        let seconds = info.frames.unwrap_or(0) as f64 / f64::from(info.sample_rate.max(1));
        let (channels, sample_rate) = (i64::from(info.channels), i64::from(info.sample_rate));
        if !self.context.embed_media {
            let absolute = std::fs::canonicalize(&path).unwrap_or(path);
            return Some(Medium {
                path: absolute.to_string_lossy().into_owned(),
                external: true,
                seconds,
                channels,
                sample_rate,
            });
        }
        let name: String = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let entry = self.free_name("audio", &name);
        self.audio.push(Entry {
            name: entry.clone(),
            data: Data::File(path),
            deflate: false,
        });
        Some(Medium {
            path: entry,
            external: false,
            seconds,
            channels,
            sample_rate,
        })
    }

    /// A name in `folder` no entry has, with " 2", " 3" and so on before the
    /// extension where it is taken (R73).
    pub(super) fn free_name(&mut self, folder: &str, name: &str) -> String {
        let (stem, extension) = match name.rfind('.') {
            Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
            _ => (name, ""),
        };
        let mut candidate = format!("{folder}/{name}");
        let mut number = 2;
        while !self.taken.insert(candidate.clone()) {
            candidate = format!("{folder}/{stem} {number}{extension}");
            number += 1;
        }
        candidate
    }

    /// The scenes with their slots (R74).
    pub(super) fn scenes(&mut self) -> Vec<Scene> {
        let document = self.document;
        let holders: Vec<TrackId> = self
            .order()
            .into_iter()
            .filter(|id| {
                document
                    .track(*id)
                    .is_some_and(|track| track.kind().holds_clips())
            })
            .collect();
        let mut scenes = Vec::new();
        for (index, scene) in document.scenes().enumerate() {
            let place = if scene.name.is_empty() {
                format!("scene {}", index + 1)
            } else {
                format!("the scene \"{}\"", scene.name)
            };
            let note = |this: &mut Self, what: &str, held: bool| {
                if held {
                    this.report.add(what, &place, "not written");
                }
            };
            note(self, "a scene's tempo", scene.tempo.is_some());
            note(self, "a scene's signature", scene.signature.is_some());
            note(
                self,
                "a scene's follow actions",
                scene.follow != bl_model::FollowActions::default(),
            );
            let mut slots = Vec::new();
            for id in &holders {
                let Some(track) = document.track(*id) else {
                    continue;
                };
                let clip = track
                    .slot(scene.id)
                    .and_then(|clip| self.clip(track, clip, &Fades::default(), Some(index + 1)));
                slots.push(Timeline::ClipSlot(ClipSlot {
                    info: TimelineInfo {
                        track: self.ids.tracks.get(id).cloned(),
                        ..TimelineInfo::default()
                    },
                    has_stop: Some(track.has_stop_button(scene.id)),
                    clip: clip.map(Box::new),
                }));
            }
            scenes.push(Scene {
                info: TimelineInfo {
                    name: Some(scene.name.clone()),
                    color: scene.colour.map(|swatch| self.colour(swatch)),
                    ..TimelineInfo::default()
                },
                content: Some(Box::new(Content::Timeline(Box::new(Timeline::Lanes(
                    Lanes {
                        info: TimelineInfo::default(),
                        children: slots,
                    },
                ))))),
            });
        }
        scenes
    }
}
