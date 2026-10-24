//! The golden fixtures: songs built in code, through the model's commands.
//!
//! The first fixtures' sound sources are test tones, through tracks, groups,
//! returns, and their mixers, with commands scheduled at musical positions.
//! The transport's fixtures (WP-1.5) use the beat tone, which sounds only
//! while the song plays and whose every sample follows the song position in
//! ticks, so a render shows the frame each start, stop, and loop lands on.
//! The mixer's fixtures (WP-1.6) pan, send, solo, and move faders. The clip
//! fixtures (WP-1.4) play audio files made here, as 32-bit float WAV files
//! in memory: sine chirps, whose pitch moves all the time, so a render
//! shows exactly which frame of a file each frame of the song played, made
//! with `libm`'s pure-Rust sine, so they're the same to the bit on every
//! platform. One file is at 44.1 kHz, so it plays through sample-rate
//! conversion. Tones and files play at −24 dBFS or so, louder than the
//! tones' default of −60, so that a level error shows above the
//! cross-platform threshold. These renders are files for tests, never
//! played on a device.
//!
//! Changing a fixture changes its render: remake its reference with
//! `cargo xtask golden --update` and say why in the pull request.

use std::io::Cursor;
use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap};
use bl_engine::mixer::{gain_from_db, track as mixer};
use bl_engine::nodes::{BEAT_TONE, TEST_TONE, beat_tone, test_tone};
use bl_engine::{
    Action, MediaMap, NodeKey, ParamValue, ScheduledCommand, SessionCommand, TransportCommand,
};
use bl_model::{
    AudioData, AutomationTarget, BeatsUpdate, Clip, ClipData, ClipId, ClipParam, Command,
    ContentHash, CrossfadeAssign, Device, DeviceId, DeviceKind, Editor, EnvelopeKind, ParamId,
    Preserve, SendTap, TrackId, TrackKind, TransientLoopMode, WarpMarker, WarpMode,
};

use super::{Fixture, MediaFile, SAMPLE_RATE, Scene};
use crate::render::{Cue, Launch};

pub(super) const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "single-tone",
        about: "One test tone through a track and Main.",
        build: single_tone,
    },
    Fixture {
        name: "summed-tracks",
        about: "Four tracks summed into Main through their faders, at three volumes, one muted.",
        build: summed_tracks,
    },
    Fixture {
        name: "group-and-return",
        about: "A group holding two tracks and a group of its own, a return, and a track beside them, \
                each through its fader, then Main's.",
        build: group_and_return,
    },
    Fixture {
        name: "tempo-change",
        about: "A command on every eighth note through a tempo map that holds at 120 BPM, ramps \
                to 200, and jumps to 90, so where each one lands depends on the map.",
        build: tempo_change,
    },
    Fixture {
        name: "scheduled-commands",
        about: "Commands at bars, beats, and sixteenths of a song that changes from 4/4 to 7/8: \
                mute and unmute, a level, frequencies, and volumes, each on its exact frame.",
        build: scheduled_commands,
    },
    Fixture {
        name: "start-mid-bar",
        about: "The transport on frames no block starts on: Play from an insert marker in the \
                middle of bar 2, Stop, Continue Playback, Stop twice (back to 1.1.1), and Play \
                again, sounded by the beat tone.",
        build: start_mid_bar,
    },
    Fixture {
        name: "loop-wraps",
        about: "Playback from 1.1.1 into an Arrangement loop of a beat and a half at 130 BPM, \
                which wraps four times on its exact frames; the loop is in the set, so a saved \
                set renders the same.",
        build: loop_wraps,
    },
    Fixture {
        name: "tempo-ramp-loop",
        about: "A loop inside a tempo ramp from 90 to 150 BPM that runs on past the loop's \
                end: each pass speeds up, and the wrap lands where the tempo map puts the end.",
        build: tempo_ramp_loop,
    },
    Fixture {
        name: "scene-tempo",
        about: "A scene with its own tempo launched while the song plays at 120 BPM: the launch \
                lands on the next bar line, where the beat tone's beats lengthen to the \
                scene's 60 BPM and the playhead keeps its beat.",
        build: scene_tempo,
    },
    Fixture {
        name: "pan-and-sends",
        about: "The mixer: tones panned across the field by the constant-power law, a group \
                with its own volume and pan, a post-fader return and a pre-fader one, a \
                deactivated track heard only through its pre-fader send, and Main's volume and \
                pan.",
        build: pan_and_sends,
    },
    Fixture {
        name: "solo-in-place",
        about: "A track inside a group soloed with Solo in Place on: it plays through its \
                group and its send to the return, and the other tracks and their sends are \
                silent.",
        build: solo_in_place,
    },
    Fixture {
        name: "solo-a-return",
        about: "A return track soloed, with Solo in Place off: it plays every send it gets, \
                and no track is heard directly.",
        build: solo_a_return,
    },
    Fixture {
        name: "fader-gesture",
        about: "A fader dragged with the mouse in the Session mixer (WP-1.9): the volumes the \
                window's drag set on a 440 Hz tone, one 60 Hz frame apart, recorded in \
                tests/fixtures/ui/fader-gesture.json by bl-ui's mixer test, each gliding to its \
                value over 20 ms, so the render has no zipper noise.",
        build: fader_gesture,
    },
    Fixture {
        name: "fader-move",
        about: "A fader drag from 0 dB to -20 dB and back in 1 dB steps every 1/32 beat, a \
                mute and an unmute, and a pan sweep from left to right in steps, each gliding \
                to its value over 20 ms, so the render has no zipper noise or clicks.",
        build: fader_move,
    },
    Fixture {
        name: "volume-modulation",
        about: "A tone whose volume is automated down over two bars while its clip's volume \
                modulation envelope rises: the modulation acts on the automation, so the \
                result follows it down.",
        build: volume_modulation,
    },
    Fixture {
        name: "device-modulation",
        about: "A tone through a Utility whose gain a clip's modulation envelope lowers and \
                then hands back across two bars: the envelope moves the gain from where its \
                control is.",
        build: device_modulation,
    },
    Fixture {
        name: "crossfade",
        about: "Two tracks, one assigned to the crossfader's A side and one to B, swept from \
                the A end to the B end in 1/16 steps every 1/32 beat, closer than the 20 ms a \
                move takes so the glides run into each other, with a third track assigned to \
                neither playing through.",
        build: crossfade,
    },
    Fixture {
        name: "clip-mid-bar",
        about: "Audio clips starting mid-bar, on frames no block starts on: one plays a stretch \
                from the middle of its file, between its Start and End markers, and one is cut \
                off by its own end mid-file.",
        build: clip_mid_bar,
    },
    Fixture {
        name: "comp-from-two-takes",
        about: "A comp copied from two take lanes (WP-6.21): the first half of a bar from one \
                take of the chirp and the second from another take of it, started three \
                quarters of a second later, each part cut from its take as Copy Selection to \
                Main Lane cuts it, so the main lane plays the stretches of the takes they \
                came from.",
        build: comp_from_two_takes,
    },
    Fixture {
        name: "linked-comp",
        about: "A comp made on one of two linked tracks (WP-6.23): each track holds two takes \
                of the chirp, the second started three quarters of a second later in its file, \
                the right track's both a tenth of a second later than the left's. Copying the \
                first half of the bar from the first take and the second half from the second \
                on the left track comps the right track too, each from its own takes.",
        build: linked_comp,
    },
    Fixture {
        name: "looped-clip",
        about: "A looped audio clip at -6 dB: it plays from its Start marker into its loop \
                brace and repeats the brace nine times, each wrap on its exact frame.",
        build: looped_clip,
    },
    Fixture {
        name: "overlapping-clips",
        about: "Clips overlapping on two audio tracks, one a mono file at 44.1 kHz that plays \
                through sample-rate conversion, on a track panned right at -6 dB.",
        build: overlapping_clips,
    },
    Fixture {
        name: "clip-fades",
        about: "Clip edges with fades off (hard edges mid-waveform), with Create Fades on Clip \
                Edges' 4 ms fades, and with long fades on a clip above 0 dB.",
        build: clip_fades,
    },
    Fixture {
        name: "beat-repeat",
        about: "A drum loop of two bars through Beat Repeat, in \
                Insert mode: a capture of a sixteenth on beat three of each bar repeats for \
                half a bar, each repeat quieter and lower than the last, the grid varied by \
                the device's seeded source.",
        build: beat_repeat,
    },
    Fixture {
        name: "clip-tempo-change",
        about: "An unwarped clip across a tempo ramp from 120 to 90 BPM, whose audio runs on \
                unbroken while its end lands where the tempo map puts it, and a looped clip \
                placed after the ramp.",
        build: clip_tempo_change,
    },
    Fixture {
        name: "clip-crossfade",
        about: "Three audio clips of two sines on one track, each pair crossfaded where they \
                meet: a 200 ms crossfade, 80 ms before the boundary and 120 ms after it, with \
                a curve bowed towards full level, across a tempo ramp, and a 100 ms one with \
                a curve bowed towards silence.",
        build: clip_crossfade,
    },
    Fixture {
        name: "warped-clip-crossfade",
        about: "Two warped clips of the same sines in Re-Pitch, crossfaded across a tempo \
                ramp as the unwarped ones are.",
        build: warped_clip_crossfade,
    },
    Fixture {
        name: "clip-split-crossfade",
        about: "A chirp clip split in two across a tempo ramp, the parts crossfaded 2 ms \
                either side of the split as Create Fades on Clip Edges leaves them: they play \
                what the whole clip did.",
        build: clip_split_crossfade,
    },
    Fixture {
        name: "warped-loop-120",
        about: "A one-bar loop warped at 90 BPM, looping over three 2/4 bars of a 120 BPM song \
                in Re-Pitch through the HiQ interpolator: it plays at four thirds of its speed \
                and wraps on every bar line.",
        build: warped_loop_120,
    },
    Fixture {
        name: "warped-loop-140",
        about: "The same loop in a 140 BPM song, through the cheaper interpolator (HiQ off).",
        build: warped_loop_140,
    },
    Fixture {
        name: "leader-tempo",
        about: "A one-bar clip that leads the tempo (WP-4.11): its own tempo is 90 BPM in a \
                120 BPM song, so the set plays at the clip's speed over its span and the clip \
                plays as if unwarped, while a following clip after the span follows the song's \
                120.",
        build: leader_tempo,
    },
    Fixture {
        name: "beats-120",
        about: "A one-bar drum loop warped at 120 BPM in Beats at the clip's own tempo, Preserve \
                Transients and Loop Forward: at this tempo every Preserve and loop mode plays it \
                to the bit (warp-modes R4, R14).",
        build: beats_120,
    },
    Fixture {
        name: "beats-bar-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1 Bar, Loop Off.",
        build: beats_bar_off_60,
    },
    Fixture {
        name: "beats-bar-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1 Bar, Loop Forward.",
        build: beats_bar_forward_60,
    },
    Fixture {
        name: "beats-bar-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1 Bar, Loop Back-and-Forth.",
        build: beats_bar_backandforth_60,
    },
    Fixture {
        name: "beats-half-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/2, Loop Off.",
        build: beats_half_off_60,
    },
    Fixture {
        name: "beats-half-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/2, Loop Forward.",
        build: beats_half_forward_60,
    },
    Fixture {
        name: "beats-half-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/2, Loop Back-and-Forth.",
        build: beats_half_backandforth_60,
    },
    Fixture {
        name: "beats-quarter-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/4, Loop Off.",
        build: beats_quarter_off_60,
    },
    Fixture {
        name: "beats-quarter-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/4, Loop Forward.",
        build: beats_quarter_forward_60,
    },
    Fixture {
        name: "beats-quarter-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/4, Loop Back-and-Forth.",
        build: beats_quarter_backandforth_60,
    },
    Fixture {
        name: "beats-eighth-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/8, Loop Off.",
        build: beats_eighth_off_60,
    },
    Fixture {
        name: "beats-eighth-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/8, Loop Forward.",
        build: beats_eighth_forward_60,
    },
    Fixture {
        name: "beats-eighth-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/8, Loop Back-and-Forth.",
        build: beats_eighth_backandforth_60,
    },
    Fixture {
        name: "beats-sixteenth-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/16, Loop Off.",
        build: beats_sixteenth_off_60,
    },
    Fixture {
        name: "beats-sixteenth-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/16, Loop Forward.",
        build: beats_sixteenth_forward_60,
    },
    Fixture {
        name: "beats-sixteenth-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/16, Loop Back-and-Forth.",
        build: beats_sixteenth_backandforth_60,
    },
    Fixture {
        name: "beats-thirtysecond-off-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/32, Loop Off.",
        build: beats_thirtysecond_off_60,
    },
    Fixture {
        name: "beats-thirtysecond-forward-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/32, Loop Forward.",
        build: beats_thirtysecond_forward_60,
    },
    Fixture {
        name: "beats-thirtysecond-backandforth-60",
        about: "The drum loop in a 60 BPM song, Preserve 1/32, Loop Back-and-Forth.",
        build: beats_thirtysecond_backandforth_60,
    },
    Fixture {
        name: "complex-60",
        about: "A one-bar tone warped at 120 BPM in Complex, in a 60 BPM song: the frequency-domain stretcher reads the file at half its speed (warp-modes R21 to R24).",
        build: complex_60,
    },
    Fixture {
        name: "complex-90",
        about: "The same tone in a 90 BPM song, in Complex.",
        build: complex_90,
    },
    Fixture {
        name: "complex-180",
        about: "The same tone in a 180 BPM song, in Complex.",
        build: complex_180,
    },
    Fixture {
        name: "complex-240",
        about: "The same tone in a 240 BPM song, in Complex.",
        build: complex_240,
    },
    Fixture {
        name: "complex-pro-60",
        about: "The same tone in a 60 BPM song, in Complex Pro at Formants 100 % and Envelope 128.",
        build: complex_pro_60,
    },
    Fixture {
        name: "complex-pro-90",
        about: "The same tone in a 90 BPM song, in Complex Pro.",
        build: complex_pro_90,
    },
    Fixture {
        name: "complex-pro-180",
        about: "The same tone in a 180 BPM song, in Complex Pro.",
        build: complex_pro_180,
    },
    Fixture {
        name: "complex-pro-240",
        about: "The same tone in a 240 BPM song, in Complex Pro.",
        build: complex_pro_240,
    },
    Fixture {
        name: "transposition-envelope-beats",
        about: "The drum loop in Beats, a Transposition clip envelope rising from 0 to 12 \
                semitones over the bar, each segment read at the ratio it begins with \
                (warp-modes R17, R29).",
        build: transposition_envelope_beats,
    },
    Fixture {
        name: "transposition-envelope-tones",
        about: "The sweep in Tones, a Transposition clip envelope rising from 0 to 7 \
                semitones over the bar (warp-modes R20, R29).",
        build: transposition_envelope_tones,
    },
    Fixture {
        name: "warp-beats-60",
        about: "The first two beats of the drum loop warped at 120 BPM in Beats, in a 60 BPM song: it plays at 0.5 times its speed (warp-modes R34, R37).",
        build: warp_beats_60,
    },
    Fixture {
        name: "warp-beats-90",
        about: "The first two beats of the drum loop warped at 120 BPM in Beats, in a 90 BPM song: it plays at 0.75 times its speed (warp-modes R34, R37).",
        build: warp_beats_90,
    },
    Fixture {
        name: "warp-beats-120",
        about: "The first two beats of the drum loop warped at 120 BPM in Beats, in a 120 BPM song: it plays at 1 times its speed (warp-modes R34, R37).",
        build: warp_beats_120,
    },
    Fixture {
        name: "warp-beats-180",
        about: "The first two beats of the drum loop warped at 120 BPM in Beats, in a 180 BPM song: it plays at 1.5 times its speed (warp-modes R34, R37).",
        build: warp_beats_180,
    },
    Fixture {
        name: "warp-beats-240",
        about: "The first two beats of the drum loop warped at 120 BPM in Beats, in a 240 BPM song: it plays at 2 times its speed (warp-modes R34, R37).",
        build: warp_beats_240,
    },
    Fixture {
        name: "warp-tones-60",
        about: "The first two beats of the drum loop warped at 120 BPM in Tones, in a 60 BPM song: it plays at 0.5 times its speed (warp-modes R34, R37).",
        build: warp_tones_60,
    },
    Fixture {
        name: "warp-tones-90",
        about: "The first two beats of the drum loop warped at 120 BPM in Tones, in a 90 BPM song: it plays at 0.75 times its speed (warp-modes R34, R37).",
        build: warp_tones_90,
    },
    Fixture {
        name: "warp-tones-120",
        about: "The first two beats of the drum loop warped at 120 BPM in Tones, in a 120 BPM song: it plays at 1 times its speed (warp-modes R34, R37).",
        build: warp_tones_120,
    },
    Fixture {
        name: "warp-tones-180",
        about: "The first two beats of the drum loop warped at 120 BPM in Tones, in a 180 BPM song: it plays at 1.5 times its speed (warp-modes R34, R37).",
        build: warp_tones_180,
    },
    Fixture {
        name: "warp-tones-240",
        about: "The first two beats of the drum loop warped at 120 BPM in Tones, in a 240 BPM song: it plays at 2 times its speed (warp-modes R34, R37).",
        build: warp_tones_240,
    },
    Fixture {
        name: "warp-texture-60",
        about: "The first two beats of the drum loop warped at 120 BPM in Texture, in a 60 BPM song: it plays at 0.5 times its speed (warp-modes R34, R37).",
        build: warp_texture_60,
    },
    Fixture {
        name: "warp-texture-90",
        about: "The first two beats of the drum loop warped at 120 BPM in Texture, in a 90 BPM song: it plays at 0.75 times its speed (warp-modes R34, R37).",
        build: warp_texture_90,
    },
    Fixture {
        name: "warp-texture-120",
        about: "The first two beats of the drum loop warped at 120 BPM in Texture, in a 120 BPM song: it plays at 1 times its speed (warp-modes R34, R37).",
        build: warp_texture_120,
    },
    Fixture {
        name: "warp-texture-180",
        about: "The first two beats of the drum loop warped at 120 BPM in Texture, in a 180 BPM song: it plays at 1.5 times its speed (warp-modes R34, R37).",
        build: warp_texture_180,
    },
    Fixture {
        name: "warp-texture-240",
        about: "The first two beats of the drum loop warped at 120 BPM in Texture, in a 240 BPM song: it plays at 2 times its speed (warp-modes R34, R37).",
        build: warp_texture_240,
    },
    Fixture {
        name: "warp-re-pitch-60",
        about: "The first two beats of the drum loop warped at 120 BPM in Re-Pitch, in a 60 BPM song: it plays at 0.5 times its speed (warp-modes R34, R37).",
        build: warp_re_pitch_60,
    },
    Fixture {
        name: "warp-re-pitch-90",
        about: "The first two beats of the drum loop warped at 120 BPM in Re-Pitch, in a 90 BPM song: it plays at 0.75 times its speed (warp-modes R34, R37).",
        build: warp_re_pitch_90,
    },
    Fixture {
        name: "warp-re-pitch-120",
        about: "The first two beats of the drum loop warped at 120 BPM in Re-Pitch, in a 120 BPM song: it plays at 1 times its speed (warp-modes R34, R37).",
        build: warp_re_pitch_120,
    },
    Fixture {
        name: "warp-re-pitch-180",
        about: "The first two beats of the drum loop warped at 120 BPM in Re-Pitch, in a 180 BPM song: it plays at 1.5 times its speed (warp-modes R34, R37).",
        build: warp_re_pitch_180,
    },
    Fixture {
        name: "warp-re-pitch-240",
        about: "The first two beats of the drum loop warped at 120 BPM in Re-Pitch, in a 240 BPM song: it plays at 2 times its speed (warp-modes R34, R37).",
        build: warp_re_pitch_240,
    },
];

/// A song under construction.
struct Song {
    editor: Editor,
    files: Vec<MediaFile>,
    media: MediaMap,
    schedule: Vec<ScheduledCommand>,
    cues: Vec<Cue>,
    launches: Vec<Launch>,
}

impl Song {
    fn new() -> Self {
        Self {
            editor: Editor::new(),
            files: Vec::new(),
            media: MediaMap::new(),
            schedule: Vec::new(),
            cues: Vec::new(),
            launches: Vec::new(),
        }
    }

    /// Makes an audio file of `channels` at `rate` as a 32-bit float WAV
    /// file, and returns the content hash clips name it by. The engine gets
    /// it as the media store would give it: decoded from the file's bytes.
    fn file(&mut self, name: &str, rate: u32, channels: &[Vec<f32>]) -> ContentHash {
        let spec = hound::WavSpec {
            channels: u16::try_from(channels.len()).expect("a few channels"),
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("the WAV header writes");
        for frame in 0..channels[0].len() {
            for channel in channels {
                writer
                    .write_sample(channel[frame])
                    .expect("the WAV file writes");
            }
        }
        writer.finalize().expect("the WAV file finishes");
        let bytes = cursor.into_inner();
        let hash = ContentHash(*bl_media::ContentHash::of_bytes(&bytes).as_bytes());
        let decoded = bl_media::Decoder::from_reader(Cursor::new(bytes.clone()), Some("wav"))
            .and_then(bl_media::Decoder::decode_to_end)
            .expect("the WAV file decodes");
        self.media.insert(hash, Arc::new(decoded));
        self.files.push(MediaFile {
            name: name.to_owned(),
            hash,
            bytes,
        });
        hash
    }

    /// A stereo file at 48 kHz, 4.5 seconds long: a chirp up from 220 to
    /// 1,320 Hz on the left at -24 dBFS, and down from 990 to 330 Hz on the
    /// right, 2.5 dB quieter.
    fn chirp(&mut self) -> ContentHash {
        let frames = 216_000;
        let left = chirp(frames, 48_000, 220.0, 1_320.0, 0.0625);
        let right = chirp(frames, 48_000, 990.0, 330.0, 0.046_875);
        self.file("chirp.wav", 48_000, &[left, right])
    }

    /// A stereo file at 48 kHz, 4 seconds long: a steady sine of `hz` on both
    /// channels at -12 dBFS.
    fn sine(&mut self, hz: f64) -> ContentHash {
        let tone = chirp(192_000, 48_000, hz, hz, 0.25);
        self.file(&format!("sine-{hz}.wav"), 48_000, &[tone.clone(), tone])
    }

    /// A mono file at 44.1 kHz, 3 seconds long: a chirp up from 165 to
    /// 247.5 Hz at -24 dBFS.
    fn drone(&mut self) -> ContentHash {
        let mono = chirp(132_300, 44_100, 165.0, 247.5, 0.0625);
        self.file("drone.wav", 44_100, &[mono])
    }

    /// Puts an audio clip on `track` at `at` for `length`.
    fn audio_clip(&mut self, track: TrackId, at: Ticks, length: Ticks, audio: AudioData) -> ClipId {
        let id = self.editor.new_id();
        self.apply(Command::CreateClip {
            track,
            clip: Clip::new(id, at, length, ClipData::Audio(audio)),
        });
        id
    }

    /// How long the song plays to the end of its last clip, and a quarter of
    /// a second more, rounded up to a quarter of a second. The ticks are
    /// measured through the tempo map the set plays, a leading clip's tempo
    /// included (the warping spec, R48).
    fn to_last_clip(&self) -> f64 {
        let document = self.editor.document();
        let end = document
            .tracks()
            .flat_map(bl_model::Track::clips)
            .map(bl_model::Clip::end)
            .max()
            .expect("the song has clips");
        let seconds = document.tempo_map().ticks_to_seconds(end) + 0.25;
        (seconds * 4.0).ceil() / 4.0
    }

    /// Adds a beat tone at the end of a track's chain.
    fn beat_tone(&mut self, track: TrackId, cycles_per_beat: f64, dbfs: f64) -> DeviceId {
        let id = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: BEAT_TONE.to_owned(),
        };
        let mut device = Device::new(id, "Beat Tone", kind);
        device
            .params
            .insert(beat_tone::CYCLES_PER_BEAT, cycles_per_beat);
        device.params.insert(beat_tone::LEVEL, dbfs);
        self.apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        });
        id
    }

    /// Moves the transport on frame `at` of the render.
    fn cue(&mut self, at: u64, command: TransportCommand) {
        self.cues.push(Cue { at, command });
    }

    /// Asks the Session View for `command` on frame `at` of the render.
    fn launch(&mut self, at: u64, command: SessionCommand) {
        self.launches.push(Launch { at, command });
    }

    /// Puts the loop brace at `start` for `length`, and switches the loop
    /// on.
    fn loop_on(&mut self, start: Ticks, length: Ticks) {
        self.apply(Command::SetLoop { start, length });
        self.apply(Command::SetLoopEnabled { enabled: true });
    }

    fn apply(&mut self, command: Command) {
        self.editor
            .apply(command)
            .expect("a fixture's commands are valid");
    }

    /// Adds a track at the end of its list: the top level, a group, or the
    /// returns.
    fn track(&mut self, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let document = self.editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => document.returns().len(),
            (_, Some(group)) => document
                .track(group)
                .expect("the group exists")
                .children()
                .len(),
            (_, None) => document.top_level().len(),
        };
        let id = self.editor.new_id();
        self.apply(Command::CreateTrack {
            id,
            kind,
            name: name.to_owned(),
            parent,
            index,
        });
        id
    }

    /// Adds a test tone at the end of a track's chain.
    fn tone(&mut self, track: TrackId, hz: f64, dbfs: f64) -> DeviceId {
        let id = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        };
        let mut device = Device::new(id, "Test Tone", kind);
        device.params.insert(test_tone::FREQUENCY, hz);
        device.params.insert(test_tone::LEVEL, dbfs);
        let index = self
            .editor
            .document()
            .track(track)
            .expect("the track exists")
            .devices()
            .len();
        self.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        id
    }

    fn volume(&mut self, track: TrackId, volume: f64) {
        self.apply(Command::SetVolume { track, volume });
    }

    fn main_volume(&mut self, volume: f64) {
        let track = self.editor.document().main_id();
        self.volume(track, volume);
    }

    fn mute(&mut self, track: TrackId) {
        self.apply(Command::SetMute { track, mute: true });
    }

    fn pan(&mut self, track: TrackId, pan: f64) {
        self.apply(Command::SetPan { track, pan });
    }

    fn send(&mut self, track: TrackId, to: TrackId, level: f64) {
        self.apply(Command::SetSend { track, to, level });
    }

    fn solo(&mut self, track: TrackId) {
        self.apply(Command::SetSolo { track, solo: true });
    }

    /// A position given as bar, beat, and sixteenth (`"2.3.1"`), by the
    /// song's time signatures.
    fn at(&self, position: &str) -> Ticks {
        self.editor
            .document()
            .settings()
            .time_signatures
            .parse_position(position)
            .expect("a fixture's positions are valid")
    }

    /// Schedules a parameter change at `at`.
    fn schedule(&mut self, at: Ticks, key: NodeKey, param: ParamId, value: f64) {
        let id = self.schedule.len() as u64 + 1;
        self.schedule.push(ScheduledCommand {
            id,
            at,
            action: Action::Param {
                key,
                param,
                value: ParamValue::Set(value),
            },
        });
    }

    /// The finished song, `seconds` long. Without cues, it plays from its
    /// start.
    fn finish(self, seconds: f64) -> Scene {
        Scene {
            editor: self.editor,
            files: self.files,
            media: self.media,
            schedule: self.schedule,
            cues: if self.cues.is_empty() {
                vec![Cue::PLAY]
            } else {
                self.cues
            },
            launches: self.launches,
            frames: SAMPLE_RATE.seconds_to_frames(seconds).get() as u64,
        }
    }
}

fn bpm(bpm: f64) -> Bpm {
    Bpm::new(bpm).expect("a fixture's tempos are valid")
}

/// `frames` frames at `rate` of a sine at `level` for each of `hertz` in
/// turn, each for an equal share of them and starting afresh (so each beat
/// of a loop made of them is plain to hear, and the file takes little room as
/// a FLAC reference). Its sine is `libm`'s.
fn beat_tones(frames: usize, rate: u32, hertz: &[f64], level: f64) -> Vec<f32> {
    let each = frames / hertz.len();
    (0..frames)
        .map(|i| {
            let (beat, within) = ((i / each).min(hertz.len() - 1), i % each);
            let phase = std::f64::consts::TAU * hertz[beat] * within as f64 / f64::from(rate);
            (libm::sin(phase) * level) as f32
        })
        .collect()
}

/// A sine at `level` whose frequency moves in a straight line from `from` to
/// `to` hertz over `frames` frames at `rate`. Its sine is `libm`'s, so it's
/// the same to the bit on every platform.
fn chirp(frames: usize, rate: u32, from: f64, to: f64, level: f64) -> Vec<f32> {
    let mut phase = 0.0f64;
    let mut samples = Vec::with_capacity(frames);
    for i in 0..frames {
        samples.push((libm::sin(phase * std::f64::consts::TAU) * level) as f32);
        let hz = from + (to - from) * i as f64 / frames as f64;
        phase += hz / f64::from(rate);
        if phase >= 1.0 {
            phase -= 1.0;
        }
    }
    samples
}

/// A bar of drums at 120 BPM, mono at 48 kHz, made from `libm`'s sine and
/// exponential, so it is the same to the bit on every platform: a kick on
/// beats one and three, a snare (three inharmonic tones) on two and four,
/// and a hat (two high tones) on the eighth notes. There is no noise: the
/// reference's FLAC encoder expands on it.
fn drum_bar() -> Vec<f32> {
    let rate = 48_000.0;
    let tau = std::f64::consts::TAU;
    let mut bar = vec![0.0f32; 96_000];
    let mut add = |at: usize, length: usize, voice: &mut dyn FnMut(f64) -> f64| {
        for n in 0..length {
            bar[at + n] += voice(n as f64 / rate) as f32;
        }
    };
    for beat in [0, 2] {
        let mut phase = 0.0;
        add(beat * 24_000, 12_000, &mut |t| {
            phase += (45.0 + 75.0 * libm::exp(-t * 30.0)) / rate;
            libm::sin(phase * tau) * libm::exp(-t * 12.0) * 0.5
        });
    }
    for beat in [1, 3] {
        add(beat * 24_000, 9_000, &mut |t| {
            let tones = libm::sin(190.0 * t * tau)
                + 0.7 * libm::sin(327.0 * t * tau)
                + 0.5 * libm::sin(511.0 * t * tau);
            tones * 0.15 * libm::exp(-t * 20.0)
        });
    }
    for eighth in 0..8 {
        add(eighth * 12_000, 1_800, &mut |t| {
            let tones = libm::sin(5_200.0 * t * tau) + libm::sin(7_300.0 * t * tau);
            tones * 0.06 * libm::exp(-t * 90.0)
        });
    }
    bar
}

/// Spec: beat-repeat R1 to R10. A drum loop through Beat Repeat in Insert mode.
fn beat_repeat() -> Scene {
    use bl_devices::beat_repeat::{
        BEAT_REPEAT, DECAY, GATE, GRID, INTERVAL, MODE, OFFSET, PITCH_DECAY, VARIATION,
    };
    let mut song = Song::new();
    let bar = drum_bar();
    let two_bars: Vec<f32> = bar.iter().chain(&bar).copied().collect();
    let drums = song.file("drums.wav", 48_000, &[two_bars.clone(), two_bars]);
    let track = song.track(TrackKind::Audio, "Drums", None);
    let at = song.at("1.1.1");
    song.audio_clip(track, at, Ticks::QUARTER * 8, AudioData::new(drums));
    let mut device = Device::new(
        song.editor.new_id(),
        "Beat Repeat",
        DeviceKind::Native {
            device_type: BEAT_REPEAT.to_owned(),
        },
    );
    for (param, value) in [
        (INTERVAL, 5.0),
        (OFFSET, 8.0),
        (GRID, 4.0),
        (GATE, 8.0),
        (MODE, 1.0),
        (VARIATION, 50.0),
        (PITCH_DECAY, 30.0),
        (DECAY, 25.0),
    ] {
        device.params.insert(param, value);
    }
    song.apply(Command::InsertDevice {
        track,
        index: 0,
        device,
    });
    song.finish(4.25)
}

fn single_tone() -> Scene {
    let mut song = Song::new();
    let track = song.track(TrackKind::Midi, "Tone", None);
    song.tone(track, 440.0, -24.0);
    song.finish(2.0)
}

fn summed_tracks() -> Scene {
    let mut song = Song::new();
    for (name, hz, volume) in [
        ("Root", 220.0, 1.0),
        ("Fifth", 330.0, 0.5),
        ("Third", 550.0, 0.25),
    ] {
        let track = song.track(TrackKind::Midi, name, None);
        song.tone(track, hz, -24.0);
        song.volume(track, volume);
    }
    // Loud, and silent because it's muted.
    let muted = song.track(TrackKind::Audio, "Muted", None);
    song.tone(muted, 880.0, -12.0);
    song.mute(muted);
    song.finish(2.0)
}

fn group_and_return() -> Scene {
    let mut song = Song::new();
    let group = song.track(TrackKind::Group, "Group", None);
    song.volume(group, 0.5);
    let a = song.track(TrackKind::Midi, "A", Some(group));
    song.tone(a, 262.0, -24.0);
    let b = song.track(TrackKind::Midi, "B", Some(group));
    song.tone(b, 392.0, -24.0);
    song.volume(b, 0.7);
    let inner = song.track(TrackKind::Group, "Inner", Some(group));
    song.volume(inner, 0.8);
    let c = song.track(TrackKind::Audio, "C", Some(inner));
    song.tone(c, 523.25, -24.0);
    let beside = song.track(TrackKind::Midi, "Beside", None);
    song.tone(beside, 196.0, -30.0);
    // Returns have no sends yet (WP-2.19b), so this one has a tone of its own.
    let ret = song.track(TrackKind::Return, "Return", None);
    song.tone(ret, 660.0, -30.0);
    song.volume(ret, 0.5);
    song.main_volume(0.8);
    song.finish(2.0)
}

fn tempo_change() -> Scene {
    let mut song = Song::new();
    // 120 BPM for two beats, a ramp to 200 over four, then a jump to 90.
    let beats = |n: i64| Ticks::QUARTER * n;
    let tempo = TempoMap::from_breakpoints([
        (beats(0), bpm(120.0)),
        (beats(2), bpm(120.0)),
        (beats(6), bpm(200.0)),
        (beats(6), bpm(90.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let track = song.track(TrackKind::Midi, "Tone", None);
    let tone = song.tone(track, 440.0, -24.0);
    // Eight beats, which take about 3.9 seconds.
    for eighth in 0..16 {
        let hz = if eighth % 2 == 0 { 440.0 } else { 660.0 };
        let at = Ticks::QUARTER / 2 * eighth;
        song.schedule(at, NodeKey::Device(tone), test_tone::FREQUENCY, hz);
    }
    song.finish(4.0)
}

fn scheduled_commands() -> Scene {
    let mut song = Song::new();
    // A bar of 4/4, then 7/8, at 180 BPM: bars of 1⅓ seconds, then 1⅙.
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(180.0)),
    });
    let signature = |n, d| TimeSignature::new(n, d).expect("the time signature is valid");
    let time_signatures = TimeSignatureMap::new([
        TimeSignatureChange {
            at: Ticks::ZERO,
            signature: signature(4, 4),
        },
        TimeSignatureChange {
            at: Ticks::WHOLE,
            signature: signature(7, 8),
        },
    ])
    .expect("the time-signature map is valid");
    song.apply(Command::SetTimeSignatures { time_signatures });
    let lead = song.track(TrackKind::Midi, "Lead", None);
    let tone = song.tone(lead, 440.0, -24.0);
    let pad = song.track(TrackKind::Midi, "Pad", None);
    song.tone(pad, 220.0, -30.0);

    let lead_mixer = NodeKey::Mixer(lead);
    let pad_mixer = NodeKey::Mixer(pad);
    let tone = NodeKey::Device(tone);
    let commands = [
        ("1.3.1", lead_mixer, mixer::MUTE, 1.0),
        ("1.4.1", lead_mixer, mixer::MUTE, 0.0),
        ("1.4.3", pad_mixer, mixer::VOLUME, 0.5),
        ("2.1.1", tone, test_tone::FREQUENCY, 660.0),
        ("2.4.1", pad_mixer, mixer::VOLUME, 0.25),
        ("2.5.2", tone, test_tone::LEVEL, -36.0),
        ("3.1.1", tone, test_tone::FREQUENCY, 495.0),
        ("3.3.2", lead_mixer, mixer::VOLUME, 0.5),
        ("3.7.2", pad_mixer, mixer::MUTE, 1.0),
    ];
    for (position, key, param, value) in commands {
        let at = song.at(position);
        song.schedule(at, key, param, value);
    }
    // Three bars.
    song.finish(3.75)
}

/// Spec: transport R1 to R6, on exact frames.
fn start_mid_bar() -> Scene {
    let mut song = Song::new();
    let track = song.track(TrackKind::Midi, "Beat", None);
    // 440 Hz at 120 BPM.
    song.beat_tone(track, 220.0, -24.0);
    let marker = song.at("2.2.3");
    for (at, command) in [
        // The insert marker moves while the song is stopped, silently.
        (0, TransportCommand::Locate(marker)),
        (30_001, TransportCommand::Play),
        (90_017, TransportCommand::Stop),
        (100_003, TransportCommand::Continue),
        (140_009, TransportCommand::Stop),
        (140_500, TransportCommand::Stop),
        (150_011, TransportCommand::Play),
    ] {
        song.cue(at, command);
    }
    song.finish(3.75)
}

/// Spec: transport R13, R14, R16.
fn loop_wraps() -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(130.0)),
    });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    // From 1.2.3 to 1.4.1: frames 33,231 to 66,462.
    let start = song.at("1.2.3");
    song.loop_on(start, Ticks::QUARTER * 3 / 2);
    song.finish(4.0)
}

/// Spec: transport R13, R15.
fn tempo_ramp_loop() -> Scene {
    let mut song = Song::new();
    // 90 BPM for a bar, then a ramp to 150 over two bars.
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(90.0)),
        (Ticks::WHOLE, bpm(90.0)),
        (Ticks::WHOLE * 3, bpm(150.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    // From 2.2.1 to 2.3.3, inside the ramp.
    let start = song.at("2.2.1");
    song.loop_on(start, Ticks::QUARTER * 3 / 2);
    let bar_2 = song.at("2.1.1");
    song.cue(0, TransportCommand::Locate(bar_2));
    song.cue(0, TransportCommand::Play);
    song.finish(4.5)
}

/// Spec: session-view R22, R23.
fn scene_tempo() -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(120.0)),
    });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    let scene = song.editor.document().scenes().next().unwrap().id;
    song.apply(Command::SetSceneTempo {
        scene,
        tempo: Some(bpm(60.0)),
    });
    // Asked for a frame past 1.2.1: the 1 Bar grid puts the launch on
    // 2.1.1, frame 96,000, and the scene has no clips to launch.
    song.launch(24_001, SessionCommand::LaunchScene(scene));
    song.finish(4.0)
}

/// Spec: mixer R1 to R9.
fn pan_and_sends() -> Scene {
    let mut song = Song::new();
    let post = song.track(TrackKind::Return, "Post", None);
    song.volume(post, 0.5);
    let pre = song.track(TrackKind::Return, "Pre", None);
    song.apply(Command::SetSendTap {
        track: pre,
        tap: SendTap::Pre,
    });
    song.pan(pre, 0.6);

    // Hard left: +3 dB on the left, nothing on the right.
    let left = song.track(TrackKind::Midi, "Left", None);
    song.tone(left, 220.0, -24.0);
    song.pan(left, -1.0);
    song.send(left, post, 0.5);
    // Half right at -6 dB; its pre-fader send ignores both.
    let right = song.track(TrackKind::Midi, "Half right", None);
    song.tone(right, 330.0, -24.0);
    song.pan(right, 0.5);
    song.volume(right, gain_from_db(-6.0));
    song.send(right, pre, 0.25);
    // A group with its own volume and pan, holding a panned track.
    let bus = song.track(TrackKind::Group, "Bus", None);
    song.volume(bus, 0.8);
    song.pan(bus, -0.3);
    let inner = song.track(TrackKind::Midi, "Inner", Some(bus));
    song.tone(inner, 440.0, -30.0);
    song.pan(inner, 0.25);
    song.send(inner, post, 1.0);
    song.send(bus, post, 0.3);
    // Deactivated: silent on Main and through its post-fader send, heard
    // through its pre-fader send.
    let off = song.track(TrackKind::Audio, "Deactivated", None);
    song.tone(off, 550.0, -24.0);
    song.mute(off);
    song.send(off, pre, 0.7);
    song.send(off, post, 1.0);

    song.main_volume(0.9);
    let main = song.editor.document().main_id();
    song.pan(main, 0.1);
    song.finish(2.0)
}

/// A group of two tracks and a track beside it, each a tone sent to a
/// return panned to the right: (kick, hat, bass, return).
fn drums_and_bass(song: &mut Song) -> [TrackId; 4] {
    let verb = song.track(TrackKind::Return, "Verb", None);
    song.pan(verb, 0.8);
    song.volume(verb, 0.5);
    let drums = song.track(TrackKind::Group, "Drums", None);
    let kick = song.track(TrackKind::Audio, "Kick", Some(drums));
    song.tone(kick, 110.0, -24.0);
    song.send(kick, verb, 1.0);
    let hat = song.track(TrackKind::Audio, "Hat", Some(drums));
    song.tone(hat, 880.0, -30.0);
    song.send(hat, verb, 0.5);
    let bass = song.track(TrackKind::Midi, "Bass", None);
    song.tone(bass, 165.0, -24.0);
    song.send(bass, verb, 1.0);
    [kick, hat, bass, verb]
}

/// Spec: mixer R10, R11.
fn solo_in_place() -> Scene {
    let mut song = Song::new();
    let [kick, ..] = drums_and_bass(&mut song);
    song.solo(kick);
    song.finish(2.0)
}

/// Spec: mixer R11, R12.
fn solo_a_return() -> Scene {
    let mut song = Song::new();
    let [.., verb] = drums_and_bass(&mut song);
    song.apply(Command::SetSoloInPlace { enabled: false });
    song.solo(verb);
    song.finish(2.0)
}

/// Spec: mixer R18. Every move is a parameter change on its exact frame, as
/// a UI drag sends them; the steps are 1/32 beat (15.625 ms at 120 BPM)
/// apart, closer than the 20 ms a change takes, so glides run into each
/// other as they do under a hand on a fader.
fn fader_move() -> Scene {
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let key = NodeKey::Mixer(lead);
    let step = Ticks::QUARTER / 32;
    // From beat 1.5: down 1 dB a step to -20 dB, then back up to 0 dB.
    let drag_from = Ticks::QUARTER + Ticks::QUARTER / 2;
    for n in 1..=40i32 {
        let db = -f64::from(20 - (n - 20).abs());
        let at = drag_from + step * i64::from(n);
        song.schedule(at, key, mixer::VOLUME, gain_from_db(db));
    }
    // A mute on beat 4 and an unmute half a beat later.
    song.schedule(Ticks::QUARTER * 3, key, mixer::MUTE, 1.0);
    let unmute = Ticks::QUARTER * 3 + Ticks::QUARTER / 2;
    song.schedule(unmute, key, mixer::MUTE, 0.0);
    // From beat 5: across from left to right in 32 steps.
    for n in 0..=32i32 {
        let pan = -1.0 + f64::from(n) / 16.0;
        let at = Ticks::QUARTER * 4 + step * i64::from(n);
        song.schedule(at, key, mixer::PAN, pan);
    }
    song.finish(3.25)
}

/// A two-bar MIDI clip at the song's start on `track`, with `envelope`'s
/// breakpoints (a position in ticks and a value) for `target`, modulating it
/// while the clip plays.
fn modulated_clip(
    song: &mut Song,
    track: TrackId,
    target: bl_model::AutomationTarget,
    envelope: &[(Ticks, f64)],
) {
    let clip = song.editor.new_id();
    song.apply(Command::CreateClip {
        track,
        clip: Clip::new(
            clip,
            Ticks::ZERO,
            Ticks::WHOLE * 2,
            ClipData::Midi(bl_model::MidiData::default()),
        ),
    });
    for &(at, value) in envelope {
        song.apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target,
            at,
            value,
        });
    }
}

/// Spec: modulation R2, R4. The manual's example of modulation on top of
/// automation: the volume automation fades from 0 dB to a quarter of the
/// fader over two bars, and the clip's modulation envelope rises from 20 %
/// to 100 % of it, so the sound follows the fade down.
fn volume_modulation() -> Scene {
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let volume = bl_model::AutomationTarget::Mixer {
        param: bl_model::MixerParam::Volume,
    };
    for (at, value) in [(Ticks::ZERO, 1.0), (Ticks::WHOLE * 2, 0.25)] {
        song.apply(Command::SetAutomationBreakpoint {
            track: lead,
            target: volume,
            at,
            value,
        });
    }
    modulated_clip(
        &mut song,
        lead,
        volume,
        &[(Ticks::ZERO, 0.2), (Ticks::WHOLE * 2, 1.0)],
    );
    song.finish(4.25)
}

/// Spec: modulation R3, R4. A clip's modulation envelope moves a device
/// parameter relative to its control: the Utility's gain is at 0 dB, and the
/// envelope takes it a third of its travel down at the start and back to
/// where the control is at the end of the second bar.
fn device_modulation() -> Scene {
    use bl_devices::utility::{GAIN, UTILITY};
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let id = song.editor.new_id();
    song.apply(Command::InsertDevice {
        track: lead,
        index: 1,
        device: Device::new(
            id,
            "Utility",
            DeviceKind::Native {
                device_type: UTILITY.to_owned(),
            },
        ),
    });
    modulated_clip(
        &mut song,
        lead,
        bl_model::AutomationTarget::Device {
            device: id,
            param: GAIN,
        },
        &[(Ticks::ZERO, -0.33), (Ticks::WHOLE * 2, 0.0)],
    );
    song.finish(4.25)
}

/// Spec: mixer R23. The crossfader swept from the A end to the B end with
/// a track on each side and one on neither: every move is a parameter
/// change on its exact frame, as a UI drag sends one, and the steps are
/// 1/32 beat (15.625 ms at 120 BPM) apart, closer than the 20 ms a change
/// takes, so the glides run into each other. The position reaches every
/// track's mixer node, as the engine's does.
fn crossfade() -> Scene {
    let mut song = Song::new();
    let left = song.track(TrackKind::Midi, "Left", None);
    song.tone(left, 220.0, -24.0);
    song.apply(Command::SetCrossfadeAssign {
        track: left,
        assign: CrossfadeAssign::A,
    });
    let right = song.track(TrackKind::Midi, "Right", None);
    song.tone(right, 330.0, -24.0);
    song.apply(Command::SetCrossfadeAssign {
        track: right,
        assign: CrossfadeAssign::B,
    });
    let through = song.track(TrackKind::Midi, "Through", None);
    song.tone(through, 660.0, -30.0);
    let step = Ticks::QUARTER / 32;
    for n in 0..=32i32 {
        let position = f64::from(n) / 32.0;
        let at = Ticks::QUARTER + step * i64::from(n);
        for track in [left, right, through] {
            song.schedule(at, NodeKey::Mixer(track), mixer::CROSSFADER, position);
        }
    }
    song.finish(2.0)
}

/// The fader drag bl-ui's mixer test makes through the window and the
/// shell (`fader_gesture` in `crates/ui/tests/mixer.rs`): the track's volume
/// after each 60 Hz frame of the drag.
pub const FADER_GESTURE: &str = include_str!("../../../../tests/fixtures/ui/fader-gesture.json");

/// The frames a [`FADER_GESTURE`] volume lands on: one 60 Hz frame apart,
/// from half a second in.
#[must_use]
pub fn fader_gesture_frames() -> Vec<(u64, f64)> {
    let gesture: serde_json::Value =
        serde_json::from_str(FADER_GESTURE).expect("the recorded gesture is JSON");
    let rate = gesture["frame_rate"].as_u64().expect("a frame rate");
    let per_frame = u64::from(SAMPLE_RATE.hz()) / rate;
    gesture["volumes"]
        .as_array()
        .expect("the volumes")
        .iter()
        .enumerate()
        .map(|(i, volume)| {
            (
                u64::from(SAMPLE_RATE.hz()) / 2 + per_frame * i as u64,
                volume.as_f64().expect("a volume"),
            )
        })
        .collect()
}

/// Spec: mixer-view R18. The recorded drag's volumes, each as the
/// `mixer.set_volume` it was, scheduled as the engine's volume change on its
/// frame.
fn fader_gesture() -> Scene {
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let key = NodeKey::Mixer(lead);
    let tempo = song.editor.document().settings().tempo.clone();
    for (frame, volume) in fader_gesture_frames() {
        let at = tempo
            .seconds_to_ticks(SAMPLE_RATE.frames_to_seconds(bl_core::time::Frames(frame as i64)));
        song.schedule(at, key, mixer::VOLUME, volume);
    }
    song.finish(2.0)
}

/// Spec: audio-clips R1, R3. Clips start on 1.2.3 (frame 36,000) and 2.3.2
/// (frame 150,000), neither on a block's first frame.
fn clip_mid_bar() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Clips", None);
    // From 0.5 to 1.75 seconds into the file: 2.5 beats at 120 BPM, as long
    // as the clip.
    let middle = AudioData {
        start: 0.5,
        end: Some(1.75),
        ..AudioData::new(chirp)
    };
    let at = song.at("1.2.3");
    song.audio_clip(track, at, Ticks::QUARTER * 5 / 2, middle);
    // From two seconds in, cut off a beat later, mid-file.
    let late = AudioData {
        start: 2.0,
        ..AudioData::new(chirp)
    };
    let at = song.at("2.3.2");
    song.audio_clip(track, at, Ticks::QUARTER, late);
    song.finish(4.0)
}

/// Spec: comping R17, R18. Two takes, each on a take lane of its own; the main
/// lane holds a part of each, cut from the take at the edges of the stretch
/// and copied to the main lane under an ID of its own. The take lanes play
/// nothing, so the render is the two stretches, one after the other, each
/// from the place in its take that the comp took it from.
fn comp_from_two_takes() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Vocal", None);
    let tempo = TempoMap::default();
    let bar = Ticks::QUARTER * 4;
    let mut takes = Vec::new();
    for (name, audio) in [
        ("Take 1", AudioData::new(chirp)),
        (
            "Take 2",
            AudioData {
                start: 0.75,
                ..AudioData::new(chirp)
            },
        ),
    ] {
        let lane = song.editor.new_id();
        song.apply(Command::AddTakeLane {
            track,
            lane: bl_model::NewTakeLane {
                id: Some(lane),
                name: name.to_owned(),
                clips: Vec::new(),
            },
            index: None,
        });
        let clip = Clip::new(
            song.editor.new_id(),
            Ticks::ZERO,
            bar,
            ClipData::Audio(audio),
        );
        song.apply(Command::CreateTakeClip {
            lane,
            clip: clip.clone(),
        });
        takes.push(clip);
    }
    // The first two beats from the first take, the last two from the second.
    let half = Ticks::QUARTER * 2;
    for (take, from, to) in [(&takes[0], Ticks::ZERO, half), (&takes[1], half, bar)] {
        let part = Clip {
            id: song.editor.new_id(),
            ..take.part(from, to, &tempo)
        };
        song.apply(Command::CreateClip { track, clip: part });
    }
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: linked-track editing R12, comping R17, R18. Two linked tracks with
/// two takes each, comped as one: the main lanes hold the stretches of each
/// track's own takes, as Copy Selection to Main Lane makes them on a linked
/// pair (the shell's `linked_tracks` tests make the same document by the
/// command).
fn linked_comp() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let tempo = TempoMap::default();
    let bar = Ticks::QUARTER * 4;
    let half = Ticks::QUARTER * 2;
    let tracks = [
        song.track(TrackKind::Audio, "Overhead L", None),
        song.track(TrackKind::Audio, "Overhead R", None),
    ];
    let group = song.editor.new_id();
    song.apply(Command::SetTrackLinks {
        tracks: tracks.to_vec(),
        group: Some(group),
    });
    for (n, track) in tracks.into_iter().enumerate() {
        let late = 0.1 * n as f64;
        let mut takes = Vec::new();
        for (name, start) in [("Take 1", late), ("Take 2", 0.75 + late)] {
            let lane = song.editor.new_id();
            song.apply(Command::AddTakeLane {
                track,
                lane: bl_model::NewTakeLane {
                    id: Some(lane),
                    name: name.to_owned(),
                    clips: Vec::new(),
                },
                index: None,
            });
            let clip = Clip::new(
                song.editor.new_id(),
                Ticks::ZERO,
                bar,
                ClipData::Audio(AudioData {
                    start,
                    ..AudioData::new(chirp)
                }),
            );
            song.apply(Command::CreateTakeClip {
                lane,
                clip: clip.clone(),
            });
            takes.push(clip);
        }
        for (take, from, to) in [(&takes[0], Ticks::ZERO, half), (&takes[1], half, bar)] {
            let part = Clip {
                id: song.editor.new_id(),
                ..take.part(from, to, &tempo)
            };
            song.apply(Command::CreateClip { track, clip: part });
        }
    }
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R4, R5. From 1.1.3, the clip plays 0.25 seconds of
/// pre-roll from its Start marker, then its 0.3-second brace over and over
/// until it ends, three seconds later.
fn looped_clip() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Loop", None);
    let looped = AudioData {
        start: 0.25,
        looping: true,
        loop_start: 0.5,
        loop_end: Some(0.8),
        gain: 0.5,
        ..AudioData::new(chirp)
    };
    let at = song.at("1.1.3");
    song.audio_clip(track, at, Ticks::QUARTER * 6, looped);
    song.finish(3.5)
}

/// Spec: audio-clips R2, R9, and the mixer under them. The drone is mono at
/// 44.1 kHz: it plays on both channels, converted to 48 kHz.
fn overlapping_clips() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let drone = song.drone();
    let first = song.track(TrackKind::Audio, "Chirp", None);
    song.audio_clip(
        first,
        Ticks::ZERO,
        Ticks::QUARTER * 5,
        AudioData::new(chirp),
    );
    let second = song.track(TrackKind::Audio, "Drone", None);
    song.volume(second, gain_from_db(-6.0));
    song.pan(second, 0.5);
    let late = AudioData {
        start: 0.1,
        ..AudioData::new(drone)
    };
    let at = song.at("1.3.1");
    song.audio_clip(second, at, Ticks::QUARTER * 5, late);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R5, R6, R7. Each clip starts and ends mid-waveform.
fn clip_fades() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Fades", None);
    let hard = AudioData {
        start: 0.3,
        ..AudioData::new(chirp)
    };
    let at = song.at("1.1.2");
    song.audio_clip(track, at, Ticks::QUARTER, hard);
    let edges = AudioData {
        start: 1.0,
        ..AudioData::new(chirp).with_edge_fades()
    };
    let at = song.at("1.3.1");
    song.audio_clip(track, at, Ticks::QUARTER, edges);
    let long = AudioData {
        start: 1.5,
        gain: 1.5,
        fade_in: 0.25,
        fade_out: 0.5,
        ..AudioData::new(chirp)
    };
    let at = song.at("2.1.1");
    song.audio_clip(track, at, Ticks::QUARTER * 2, long);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Tempo for the crossfade fixtures: 120 BPM for a beat, a ramp to 90 BPM
/// over two, then 90 BPM, so the first boundary (on beat 3) falls in the ramp.
fn crossfade_tempo() -> Command {
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(120.0)),
        (Ticks::QUARTER, bpm(120.0)),
        (Ticks::QUARTER * 3, bpm(90.0)),
    ])
    .expect("the tempo map is valid");
    Command::SetTempo { tempo }
}

/// Spec: audio-clips R23, R24, R31. Each clip plays on past its edge across
/// a crossfade, from its file: the later one from before its start, so its
/// Start marker is a second in.
fn clip_crossfade() -> Scene {
    let mut song = Song::new();
    song.apply(crossfade_tempo());
    let low = song.sine(440.0);
    let high = song.sine(660.0);
    let track = song.track(TrackKind::Audio, "Sines", None);
    let beat = Ticks::QUARTER;
    song.audio_clip(track, Ticks::ZERO, beat * 2, AudioData::new(low));
    let later = |media| AudioData {
        start: 1.0,
        ..AudioData::new(media)
    };
    song.audio_clip(track, beat * 2, beat * 3, later(high));
    song.audio_clip(track, beat * 5, beat * 2, later(low));
    for (boundary, before, after, curve) in
        [(beat * 2, 0.08, 0.12, 0.25), (beat * 5, 0.05, 0.05, -0.5)]
    {
        song.apply(Command::SetCrossfade {
            track,
            boundary,
            before,
            after,
            curve,
        });
    }
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R24, R31, warping R12. The same crossfade between warped
/// clips, whose markers are ticks of their contents: the later starts a
/// second (two beats at the files' 120 BPM) in.
fn warped_clip_crossfade() -> Scene {
    let mut song = Song::new();
    song.apply(crossfade_tempo());
    let low = song.sine(440.0);
    let high = song.sine(660.0);
    let track = song.track(TrackKind::Audio, "Warped sines", None);
    let beat = Ticks::QUARTER;
    let warped = |media, start: Ticks| AudioData {
        warp: true,
        warp_markers: vec![
            bl_model::WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            bl_model::WarpMarker {
                time: 4.0,
                beat: beat * 8,
            },
        ],
        tempo: Some(120.0),
        warp_mode: bl_model::WarpMode::RePitch,
        high_quality: false,
        start: start.get() as f64,
        ..AudioData::new(media)
    };
    song.audio_clip(track, Ticks::ZERO, beat * 2, warped(low, Ticks::ZERO));
    song.audio_clip(track, beat * 2, beat * 3, warped(high, beat * 2));
    song.apply(Command::SetCrossfade {
        track,
        boundary: beat * 2,
        before: 0.08,
        after: 0.12,
        curve: 0.25,
    });
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R24, R26. A chirp clip from 1.1.3 split a hair off a
/// beat, the parts crossfaded as Create Fades on Clip Edges leaves them.
fn clip_split_crossfade() -> Scene {
    let mut song = Song::new();
    song.apply(crossfade_tempo());
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Parts", None);
    let start = song.at("1.1.3");
    let length = Ticks::QUARTER * 4;
    let whole = song.audio_clip(
        track,
        start,
        length,
        AudioData {
            start: 0.2,
            ..AudioData::new(chirp)
        },
    );
    let at = start + Ticks::QUARTER + Ticks(1_234);
    let tempo = song.editor.document().settings().tempo.clone();
    let whole = song
        .editor
        .document()
        .clip(whole)
        .expect("the clip")
        .clone();
    let right = Clip {
        id: song.editor.new_id(),
        ..whole.part(at, whole.end(), &tempo)
    };
    song.apply(Command::SetClipLength {
        clip: whole.id,
        length: at - start,
    });
    song.apply(Command::CreateClip { track, clip: right });
    song.apply(Command::SetCrossfade {
        track,
        boundary: at,
        before: 0.002,
        after: 0.002,
        curve: 0.0,
    });
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R2, R4. 120 BPM for two beats, then a ramp to 90 BPM
/// over two beats, then 90 BPM.
fn clip_tempo_change() -> Scene {
    let mut song = Song::new();
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(120.0)),
        (Ticks::QUARTER * 2, bpm(120.0)),
        (Ticks::QUARTER * 4, bpm(90.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let chirp = song.chirp();
    let across = song.track(TrackKind::Audio, "Across", None);
    let at = song.at("1.2.1");
    song.audio_clip(across, at, Ticks::QUARTER * 3, AudioData::new(chirp));
    let after = song.track(TrackKind::Audio, "After", None);
    let looped = AudioData {
        start: 2.0,
        looping: true,
        loop_start: 2.0,
        loop_end: Some(2.25),
        ..AudioData::new(chirp).with_edge_fades()
    };
    let at = song.at("2.1.1");
    song.audio_clip(after, at, Ticks::QUARTER * 2, looped);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: warping R13, warp-modes R9, R12. A loop of one 2/4 bar at 90 BPM
/// (a tone for each of its two beats, a different one in each channel),
/// warped with a marker at each end, looping its whole length over three
/// bars of a song at `song_bpm`: Re-Pitch reads it `song_bpm / 90` times as
/// fast, and every bar of the song starts it again.
fn warped_loop(song_bpm: f64, high_quality: bool) -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(song_bpm)),
    });
    let time_signatures = TimeSignatureMap::new([TimeSignatureChange {
        at: Ticks::ZERO,
        signature: TimeSignature::new(2, 4).expect("the time signature is valid"),
    }])
    .expect("the time-signature map is valid");
    song.apply(Command::SetTimeSignatures { time_signatures });
    // Two beats at 90 BPM are 4/3 of a second.
    let frames = 64_000;
    let file = song.file(
        "loop.wav",
        48_000,
        &[
            beat_tones(frames, 48_000, &[330.0, 440.0], 0.0625),
            beat_tones(frames, 48_000, &[660.0, 495.0], 0.046_875),
        ],
    );
    let track = song.track(TrackKind::Audio, "Loop", None);
    let bar = Ticks::QUARTER * 2;
    let audio = AudioData {
        warp: true,
        warp_markers: vec![
            bl_model::WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            bl_model::WarpMarker {
                time: frames as f64 / 48_000.0,
                beat: bar,
            },
        ],
        tempo: Some(90.0),
        warp_mode: bl_model::WarpMode::RePitch,
        high_quality,
        looping: true,
        loop_start: 0.0,
        loop_end: Some(bar.get() as f64),
        ..AudioData::new(file)
    };
    song.audio_clip(track, Ticks::ZERO, bar * 3, audio);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: warping R43 to R48. A one-bar clip that leads the tempo, its own
/// tempo 90 BPM in a song whose own tempo is 120, so the set plays at the
/// clip's speed over its span and the clip plays as if unwarped; a second,
/// following clip after the span follows the song's own 120.
fn leader_tempo() -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(120.0)),
    });
    let bar = Ticks::QUARTER * 4;
    // Four quarter notes over eight thirds of a second is 90 BPM.
    let leader_file = song.file(
        "lead.wav",
        48_000,
        &[chirp(128_000, 48_000, 220.0, 1_320.0, 0.0625)],
    );
    // Four quarter notes over four thirds of a second is 180 BPM, so the
    // clip reads at two thirds of the song's speed at 120.
    let follower_file = song.file(
        "follow.wav",
        48_000,
        &[chirp(64_000, 48_000, 330.0, 990.0, 0.046_875)],
    );
    let marker = |time: f64, beat: Ticks| bl_model::WarpMarker { time, beat };
    let leader = song.track(TrackKind::Audio, "Lead", None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![marker(0.0, Ticks::ZERO), marker(128_000.0 / 48_000.0, bar)],
        tempo: Some(90.0),
        tempo_leader: true,
        warp_mode: bl_model::WarpMode::RePitch,
        ..AudioData::new(leader_file)
    };
    song.audio_clip(leader, Ticks::ZERO, bar, audio);
    let follower = song.track(TrackKind::Audio, "Follow", None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![marker(0.0, Ticks::ZERO), marker(64_000.0 / 48_000.0, bar)],
        tempo: Some(180.0),
        warp_mode: bl_model::WarpMode::RePitch,
        ..AudioData::new(follower_file)
    };
    song.audio_clip(follower, bar, bar, audio);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

fn warped_loop_120() -> Scene {
    warped_loop(120.0, true)
}

fn warped_loop_140() -> Scene {
    warped_loop(140.0, false)
}

/// Spec: warp-modes R14 to R17. A Beats clip of the one-bar drum loop
/// (`drum_bar`, 4/4 at 120 BPM) warped at its own tempo in a song at
/// `song_bpm`, with `preserve` and `loop_mode`: at the clip's own tempo every
/// setting reads it to the bit (R4), and at half tempo each segment runs out
/// of its own audio and its loop mode fills the rest (R15), the transient
/// envelope shaping each segment (R16).
fn beats(song_bpm: f64, preserve: Preserve, loop_mode: TransientLoopMode) -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(song_bpm)),
    });
    let bar = drum_bar();
    let file = song.file("drums.wav", 48_000, &[bar.clone(), bar]);
    // The loop's hits, a transient every eighth: kick on 1 and 3, snare on 2
    // and 4, hats between.
    song.media
        .set_transients(file, (0..8).map(|n| f64::from(n) * 0.25).collect());
    let track = song.track(TrackKind::Audio, "Drums", None);
    let mut audio = AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 2.0,
                beat: Ticks::WHOLE,
            },
        ],
        tempo: Some(120.0),
        warp_mode: WarpMode::Beats,
        looping: true,
        loop_start: 0.0,
        loop_end: Some(Ticks::WHOLE.get() as f64),
        ..AudioData::new(file)
    };
    audio.warp_settings.beats.preserve = preserve;
    audio.warp_settings.beats.transient_loop_mode = loop_mode;
    audio.warp_settings.beats.transient_envelope = 100;
    song.audio_clip(track, Ticks::ZERO, Ticks::WHOLE, audio);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

fn beats_120() -> Scene {
    beats(120.0, Preserve::Transients, TransientLoopMode::LoopForward)
}

/// The half-tempo Beats goldens (warp-modes R15, R16): one per Preserve and
/// Transient Loop Mode, each a build over [`beats`].
macro_rules! beats_fixtures {
    ($($name:ident, $preserve:ident, $loop:ident;)*) => {
        $(
            fn $name() -> Scene {
                beats(60.0, Preserve::$preserve, TransientLoopMode::$loop)
            }
        )*
    };
}

beats_fixtures! {
    beats_bar_off_60, Bar, LoopOff;
    beats_bar_forward_60, Bar, LoopForward;
    beats_bar_backandforth_60, Bar, LoopBackAndForth;
    beats_half_off_60, Half, LoopOff;
    beats_half_forward_60, Half, LoopForward;
    beats_half_backandforth_60, Half, LoopBackAndForth;
    beats_quarter_off_60, Quarter, LoopOff;
    beats_quarter_forward_60, Quarter, LoopForward;
    beats_quarter_backandforth_60, Quarter, LoopBackAndForth;
    beats_eighth_off_60, Eighth, LoopOff;
    beats_eighth_forward_60, Eighth, LoopForward;
    beats_eighth_backandforth_60, Eighth, LoopBackAndForth;
    beats_sixteenth_off_60, Sixteenth, LoopOff;
    beats_sixteenth_forward_60, Sixteenth, LoopForward;
    beats_sixteenth_backandforth_60, Sixteenth, LoopBackAndForth;
    beats_thirtysecond_off_60, ThirtySecond, LoopOff;
    beats_thirtysecond_forward_60, ThirtySecond, LoopForward;
    beats_thirtysecond_backandforth_60, ThirtySecond, LoopBackAndForth;
}

/// Spec: warp-modes R21 to R24. A one-bar tone (a sweep from 220 to 880 Hz)
/// warped at 120 BPM in Complex or Complex Pro, in a song at `song_bpm`, at
/// Complex Pro's default Formants and Envelope.
fn complex(song_bpm: f64, pro: bool) -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(song_bpm)),
    });
    let bar = Ticks::WHOLE;
    // One bar at 120 BPM is two seconds.
    let frames = 96_000;
    let file = song.file(
        "tone.wav",
        48_000,
        &[chirp(frames, 48_000, 220.0, 880.0, 0.0625)],
    );
    let track = song.track(TrackKind::Audio, "Tone", None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: frames as f64 / 48_000.0,
                beat: bar,
            },
        ],
        tempo: Some(120.0),
        warp_mode: if pro {
            WarpMode::ComplexPro
        } else {
            WarpMode::Complex
        },
        ..AudioData::new(file)
    };
    song.audio_clip(track, Ticks::ZERO, bar, audio);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

fn complex_60() -> Scene {
    complex(60.0, false)
}

fn complex_90() -> Scene {
    complex(90.0, false)
}

fn complex_180() -> Scene {
    complex(180.0, false)
}

fn complex_240() -> Scene {
    complex(240.0, false)
}

fn complex_pro_60() -> Scene {
    complex(60.0, true)
}

fn complex_pro_90() -> Scene {
    complex(90.0, true)
}

fn complex_pro_180() -> Scene {
    complex(180.0, true)
}

fn complex_pro_240() -> Scene {
    complex(240.0, true)
}

/// A warped one-bar clip of `file` (`frames` long, at 120 BPM) in `mode`, at
/// 120 BPM, with the clip's id.
fn bar_clip(
    song: &mut Song,
    file: ContentHash,
    frames: usize,
    mode: WarpMode,
) -> (TrackId, ClipId) {
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(120.0)),
    });
    let track = song.track(TrackKind::Audio, "Audio", None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: frames as f64 / 48_000.0,
                beat: Ticks::WHOLE,
            },
        ],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(file)
    };
    let clip = song.audio_clip(track, Ticks::ZERO, Ticks::WHOLE, audio);
    (track, clip)
}

/// A Transposition clip envelope rising from 0 to `semitones` over the bar.
fn transposition_ramp(song: &mut Song, clip: ClipId, semitones: f64) {
    for (at, value) in [(Ticks::ZERO, 0.0), (Ticks::WHOLE, semitones)] {
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: AutomationTarget::Clip {
                param: ClipParam::Transpose,
            },
            kind: EnvelopeKind::Modulation,
            at,
            value,
        });
    }
}

/// Spec: warp-modes R17, R29. The drum loop in Beats under a Transposition
/// envelope: each segment is read at the ratio the envelope has where it
/// begins.
fn transposition_envelope_beats() -> Scene {
    let mut song = Song::new();
    let bar = drum_bar();
    let file = song.file("drums.wav", 48_000, &[bar]);
    let (_, clip) = bar_clip(&mut song, file, 96_000, WarpMode::Beats);
    // A segment at each eighth note: a saved set renders the same, with no
    // transients to find.
    song.apply(Command::SetClipWarpSettings {
        clip,
        beats: Some(BeatsUpdate {
            preserve: Some(Preserve::Eighth),
            ..BeatsUpdate::default()
        }),
        tones: None,
        texture: None,
        complex_pro: None,
    });
    transposition_ramp(&mut song, clip, 12.0);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: warp-modes R20, R29. A sweep in Tones under a Transposition
/// envelope.
fn transposition_envelope_tones() -> Scene {
    let mut song = Song::new();
    let frames = 96_000;
    let file = song.file(
        "tone.wav",
        48_000,
        &[chirp(frames, 48_000, 220.0, 880.0, 0.0625)],
    );
    let (_, clip) = bar_clip(&mut song, file, frames, WarpMode::Tones);
    transposition_ramp(&mut song, clip, 7.0);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: warp-modes R34, R37. The first two beats of the drum loop (`drum_bar`, 4/4 at 120
/// BPM) warped at 120 BPM in `mode` at its settings' defaults, played in a song
/// at `song_bpm`: the reference of each mode at each of R34's speeds.
fn warp_mode(mode: WarpMode, song_bpm: f64) -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(song_bpm)),
    });
    let bar = drum_bar();
    let file = song.file("drums.wav", 48_000, &[bar.clone(), bar]);
    // The loop's hits, a transient every eighth, which Beats cuts at.
    song.media
        .set_transients(file, (0..8).map(|n| f64::from(n) * 0.25).collect());
    let track = song.track(TrackKind::Audio, "Drums", None);
    let audio = AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 2.0,
                beat: Ticks::WHOLE,
            },
        ],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(file)
    };
    // The bar's first two beats: a kick, two hats and a snare, short enough
    // for a reference of the noisiest mode to stay small.
    song.audio_clip(track, Ticks::ZERO, Ticks::QUARTER * 2, audio);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// The goldens of Tones, Texture and Re-Pitch at every speed (R37), each a
/// build over [`warp_mode`]. Complex and Complex Pro have none of the drum
/// loop: their stretcher's render of it differs by up to -70 dBFS across
/// platforms, so the quieter chirp of [`complex`] stands for them.
macro_rules! warp_mode_fixtures {
    ($($name:ident, $mode:ident, $bpm:expr;)*) => {
        $(
            fn $name() -> Scene {
                warp_mode(WarpMode::$mode, $bpm)
            }
        )*
    };
}

/// Beats at its defaults, over [`beats`] (R14 to R17). The fixture hands the
/// clip its transients, which a saved set doesn't have until the analysis
/// reaches the engine, so the scene stops the transport on its last frame
/// to be one `bl-cli render` doesn't play as saved.
macro_rules! warp_beats_fixtures {
    ($($name:ident, $bpm:expr;)*) => {
        $(
            fn $name() -> Scene {
                let mut scene = beats($bpm, Preserve::Transients, TransientLoopMode::LoopForward);
                scene.cues.push(Cue {
                    at: scene.frames - 1,
                    command: TransportCommand::Stop,
                });
                scene
            }
        )*
    };
}

warp_beats_fixtures! {
    warp_beats_60, 60.0;
    warp_beats_90, 90.0;
    warp_beats_120, 120.0;
    warp_beats_180, 180.0;
    warp_beats_240, 240.0;
}

warp_mode_fixtures! {
    warp_tones_60, Tones, 60.0;
    warp_tones_90, Tones, 90.0;
    warp_tones_120, Tones, 120.0;
    warp_tones_180, Tones, 180.0;
    warp_tones_240, Tones, 240.0;
    warp_texture_60, Texture, 60.0;
    warp_texture_90, Texture, 90.0;
    warp_texture_120, Texture, 120.0;
    warp_texture_180, Texture, 180.0;
    warp_texture_240, Texture, 240.0;
    warp_re_pitch_60, RePitch, 60.0;
    warp_re_pitch_90, RePitch, 90.0;
    warp_re_pitch_120, RePitch, 120.0;
    warp_re_pitch_180, RePitch, 180.0;
    warp_re_pitch_240, RePitch, 240.0;
}
