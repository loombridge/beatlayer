//! What the devices' tests share: a node of a device type, run on its own,
//! each block a real-time section under the allocation trap.

#![allow(dead_code, reason = "each test file uses some")]

pub mod chain;
pub mod response;
pub mod synth;

use bl_core::id::Id;
use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_engine::{
    Block, DeviceType, Node, NodeConfig, NodeEvent, ParamValue, Registry, TransportInfo,
};
use bl_model::{Device, DeviceId, ParamId};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const CHANNELS: usize = 2;

pub fn config(max_frames: usize) -> NodeConfig {
    NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames,
        channels: CHANNELS,
    }
}

/// A device of `device_type` with `params` set.
pub fn device(device_type: &DeviceType, params: &[(ParamId, f64)]) -> Device {
    let mut device = device_type.device(DeviceId(Id::new(1).expect("1 is an ID")));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    device
}

/// The node `registry` makes for `device`.
pub fn node(registry: &Registry, device: &Device, max_frames: usize) -> Box<dyn Node> {
    registry
        .create(device, &config(max_frames))
        .expect("the registry makes the device")
}

pub fn set(offset: u32, param: ParamId, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param,
        value: ParamValue::Set(value),
    }
}

/// Runs one block of `input` (the same on every channel) through `node`
/// with `events`, as a real-time section, and returns each channel.
pub fn run(node: &mut dyn Node, input: &[f32], events: &[NodeEvent]) -> Vec<Vec<f32>> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Runs one block of per-channel `input` through `node` at `tempo` and
/// `transport`, as a real-time section, and returns each channel.
pub fn run_at(
    node: &mut dyn Node,
    input: &[Vec<f32>],
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: TransportInfo,
) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut data: Vec<f32> = input.iter().flat_map(|c| c.iter().copied()).collect();
    {
        let mut block = Block::new(&mut data, CHANNELS, frames, frames, events, transport, RATE)
            .with_tempo(tempo);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Runs `input` through `node` `block` frames at a time at `tempo`, the song
/// playing from its start, each block a real-time section. `events` (offsets
/// from the run's start) land in the block they fall in, on their frame.
pub fn run_blocks(
    node: &mut dyn Node,
    input: &[Vec<f32>],
    block: usize,
    events: &[NodeEvent],
    tempo: &TempoMap,
) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut out = vec![Vec::with_capacity(frames); input.len()];
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let part: Vec<Vec<f32>> = input
            .iter()
            .map(|c| c[done..done + length].to_vec())
            .collect();
        let transport = TransportInfo {
            playing: true,
            position: Frames(done as i64),
            ..TransportInfo::default()
        };
        for (all, chunk) in out
            .iter_mut()
            .zip(run_at(node, &part, &here, tempo, transport))
        {
            all.extend(chunk);
        }
        done += length;
    }
    out
}

/// A noise signal, the same every run.
pub fn noise(frames: usize) -> Vec<f32> {
    let mut state = 0x1234_5678u32;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 1.6 - 0.8
        })
        .collect()
}

/// Where `signal` peaks above `level` in size, one place for each peak: the
/// first frame of each run of frames more than 8 apart.
pub fn peaks(signal: &[f32], level: f32) -> Vec<usize> {
    let mut found: Vec<usize> = Vec::new();
    for (n, s) in signal.iter().enumerate() {
        if s.abs() > level && found.last().is_none_or(|last| n - last > 8) {
            found.push(n);
        }
    }
    found
}

/// The rows a modulator wrote over a run (WP-5.10): one `Vec` of values per
/// row, for `frames` frames.
pub struct Rows(pub Vec<Vec<f32>>);

impl Rows {
    /// Slot `slot`'s contribution row.
    pub fn contribution(&self, slot: usize) -> &[f32] {
        &self.0[slot * 2]
    }

    /// Slot `slot`'s hold row.
    pub fn hold(&self, slot: usize) -> &[f32] {
        &self.0[slot * 2 + 1]
    }
}

/// Runs a modulator's [`Node::control`] over `frames` frames in blocks of
/// `block`, each a real-time section, with `events` (offsets from the run's
/// start) landing in the block they fall in, at `tempo` and under
/// `transport` (whose position moves on with the run while it plays).
pub fn control(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: TransportInfo,
) -> Rows {
    control_with_cv(node, frames, block, events, tempo, transport, None)
}

/// As [`control`], for a modulator with a CV input that reads `cv_in`, one
/// sample value per frame of the run.
pub fn control_with_cv(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: TransportInfo,
    cv_in: Option<&[f32]>,
) -> Rows {
    use bl_engine::modbus::{Bus, Control, ROWS};

    let mut rows: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let mut out = vec![0.0f32; ROWS * length];
        {
            let mut ctl = Control {
                frames: length,
                events: &here,
                transport: TransportInfo {
                    playing: transport.playing,
                    position: Frames(
                        transport.position.get() + if transport.playing { done as i64 } else { 0 },
                    ),
                    speed: transport.speed,
                    ..TransportInfo::default()
                },
                sample_rate: RATE,
                tempo,
                cv_in: cv_in.map(|data| &data[done..done + length]),
                earlier: Bus::EMPTY,
                out: &mut out,
                stride: length,
            };
            let node = &mut *node;
            bl_rt::rt_test!({
                assert!(node.control(&mut ctl), "the node is a modulator");
            });
        }
        for (row, values) in rows.iter_mut().zip(out.chunks(length)) {
            row.extend_from_slice(values);
        }
        done += length;
    }
    Rows(rows)
}

/// A registry holding Beatlayer's native devices.
pub fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// A note-on of `key` on frame `offset`, with `id`.
pub fn note_on(offset: u32, key: u8, velocity: f32, id: Option<u32>) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        offset,
        kind: bl_engine::NoteKind::On,
        channel: 0,
        key,
        velocity,
        note_id: id,
    }
}

/// The note-off that ends `on`, on frame `offset`.
pub fn note_off(offset: u32, on: &bl_engine::NoteEvent, velocity: f32) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        offset,
        kind: bl_engine::NoteKind::Off,
        velocity,
        ..*on
    }
}

/// Runs one block of `frames` frames through `node` with `notes` to play, as
/// a real-time section, and returns the notes it sends on.
pub fn notes_through(
    node: &mut dyn Node,
    notes: &[bl_engine::NoteEvent],
    frames: usize,
) -> Vec<bl_engine::NoteEvent> {
    notes_through_events(node, notes, &[], frames)
}

/// The same, with `events` (parameter changes) landing in the block on their
/// own frames.
pub fn notes_through_events(
    node: &mut dyn Node,
    notes: &[bl_engine::NoteEvent],
    events: &[NodeEvent],
    frames: usize,
) -> Vec<bl_engine::NoteEvent> {
    let mut out = bl_engine::NoteBuffer::with_capacity(8192);
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut block = Block::new(
        &mut data,
        CHANNELS,
        frames,
        frames,
        events,
        TransportInfo::default(),
        RATE,
    )
    .with_notes(notes, &mut out);
    bl_rt::rt_test!({ node.process(&mut block) });
    out.notes().to_vec()
}

/// What a signal modulator wrote over a run (WP-5.11): the rows it wrote for
/// its own frames, the rows of what it wrote 32 frames before, which
/// [`Node::control`] gives before each part, and the notes it sent on.
pub struct Signal {
    pub rows: Rows,
    pub late: Rows,
    pub notes: Vec<bl_engine::NoteEvent>,
    /// The audio, each channel.
    pub audio: Vec<Vec<f32>>,
}

/// Runs a modulator driven by the signal over `frames` frames in parts of
/// `block` (at most 32, where a late row is read): each part is `control`
/// then `process`, each a real-time section. `input` is each channel's
/// audio (silence past its end), `sidechain` what its detector hears if it
/// has one, `notes` what it plays (offsets from the run's start) and
/// `events` its parameter changes.
#[allow(clippy::too_many_arguments)]
pub fn signal(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    input: &[Vec<f32>],
    sidechain: Option<&[Vec<f32>]>,
    notes: &[bl_engine::NoteEvent],
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: impl Fn(usize) -> TransportInfo,
) -> Signal {
    use bl_engine::modbus::{Bus, Control, ROWS};

    let mut rows: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut late: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut sent = Vec::new();
    let mut audio: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let at = transport(done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let played: Vec<bl_engine::NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= done && (n.offset as usize) < done + length)
            .map(|n| bl_engine::NoteEvent {
                offset: n.offset - done as u32,
                ..*n
            })
            .collect();
        // The part's late rows, before it runs.
        let mut out = vec![0.0f32; ROWS * length];
        {
            let mut ctl = Control {
                frames: length,
                events: &[],
                transport: at,
                sample_rate: RATE,
                tempo,
                cv_in: None,
                earlier: Bus::EMPTY,
                out: &mut out,
                stride: length,
            };
            let node = &mut *node;
            bl_rt::rt_test!({
                assert!(node.control(&mut ctl), "the node writes late rows");
            });
        }
        for (row, values) in late.iter_mut().zip(out.chunks(length)) {
            row.extend_from_slice(values);
        }
        let slice = |source: &[Vec<f32>], c: usize| -> Vec<f32> {
            (done..done + length)
                .map(|i| {
                    source
                        .get(c)
                        .and_then(|ch| ch.get(i))
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect()
        };
        let mut data: Vec<f32> = (0..CHANNELS).flat_map(|c| slice(input, c)).collect();
        let key: Option<Vec<f32>> =
            sidechain.map(|s| (0..CHANNELS).flat_map(|c| slice(s, c)).collect());
        let mut rows_out = vec![0.0f32; ROWS * length];
        let mut notes_out = bl_engine::NoteBuffer::with_capacity(8192);
        {
            let mut block = Block::new(&mut data, CHANNELS, length, length, &here, at, RATE)
                .with_tempo(tempo)
                .with_notes(&played, &mut notes_out)
                .with_rows(&mut rows_out);
            if let Some(key) = &key {
                block = block.with_sidechain(key);
            }
            let node = &mut *node;
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (row, values) in rows.iter_mut().zip(rows_out.chunks(length)) {
            row.extend_from_slice(values);
        }
        sent.extend(notes_out.notes().iter().map(|n| bl_engine::NoteEvent {
            offset: n.offset + done as u32,
            ..*n
        }));
        for (channel, values) in audio.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(values);
        }
        done += length;
    }
    Signal {
        rows: Rows(rows),
        late: Rows(late),
        notes: sent,
        audio,
    }
}

/// Runs per-channel `input` through `node` in blocks of `block` frames,
/// each a real-time section, and returns each channel's whole output.
pub fn stream(node: &mut dyn Node, input: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut out = vec![Vec::with_capacity(frames); input.len()];
    for start in (0..frames).step_by(block) {
        let end = (start + block).min(frames);
        let mut data: Vec<f32> = input.iter().flat_map(|c| c[start..end].to_vec()).collect();
        {
            let mut block = Block::new(
                &mut data,
                input.len(),
                end - start,
                end - start,
                &[],
                TransportInfo::default(),
                RATE,
            );
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(end - start)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

/// A sine of `hz` and `amplitude`, `frames` long, starting at phase 0.
pub fn sine(hz: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

/// `signal`'s amplitude at each frequency from 0 up to Nyquist, in steps of
/// `RATE / signal.len()` hertz (1 Hz for a second of audio), through a Hann
/// window: a sine of amplitude A on a bin reads A there.
pub fn spectrum(signal: &[f32]) -> Vec<f64> {
    use realfft::RealFftPlanner;

    let n = signal.len();
    let weights: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mut input: Vec<f64> = signal
        .iter()
        .zip(&weights)
        .map(|(s, w)| f64::from(*s) * w)
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output)
        .expect("the lengths match");
    let sum: f64 = weights.iter().sum();
    output.iter().map(|c| c.norm() * 2.0 / sum).collect()
}

/// The complex amplitude of `signal` at `hz` (a whole number of cycles in
/// it, for an exact reading): its real and imaginary parts.
pub fn dft_at(signal: &[f32], hz: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in signal.iter().enumerate() {
        let angle = std::f64::consts::TAU * hz * i as f64 / RATE.as_f64();
        re += f64::from(*s) * angle.cos();
        im -= f64::from(*s) * angle.sin();
    }
    let n = signal.len() as f64;
    (2.0 * re / n, 2.0 * im / n)
}

/// A signal's level in decibels relative to 1.
pub fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}

/// Runs `notes` through `node` in one block of `frames` frames, a real-time
/// section, with `scale` as its track's scale over the block, and returns the
/// notes it sent.
pub fn scaled_through(
    node: &mut dyn Node,
    notes: &[bl_engine::NoteEvent],
    frames: usize,
    scale: bl_engine::scales::ScaleRuns,
) -> Vec<bl_engine::NoteEvent> {
    let mut out = bl_engine::NoteBuffer::with_capacity(256);
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut block = Block::new(
        &mut data,
        CHANNELS,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    )
    .with_scale(scale)
    .with_notes(notes, &mut out);
    bl_rt::rt_test!({ node.process(&mut block) });
    out.notes().to_vec()
}

/// A note-on of `key` at `offset` with note ID `id`.
pub fn on(offset: u32, key: u8, id: u32) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        offset,
        kind: bl_engine::NoteKind::On,
        channel: 0,
        key,
        velocity: 0.8,
        note_id: Some(id),
    }
}

/// The note-off of the note `on` makes, at `offset`.
pub fn off(offset: u32, key: u8, id: u32) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        kind: bl_engine::NoteKind::Off,
        ..on(offset, key, id)
    }
}

/// A per-note bend of `value` for the note `on` makes, at `offset`.
pub fn bend(offset: u32, key: u8, id: u32, value: f32) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        kind: bl_engine::NoteKind::Expression {
            expression: bl_engine::NoteExpression::Bend,
            value,
        },
        ..on(offset, key, id)
    }
}

/// The keys of the note-ons among `sent`.
pub fn keys_on(sent: &[bl_engine::NoteEvent]) -> Vec<u8> {
    sent.iter()
        .filter(|n| n.kind == bl_engine::NoteKind::On)
        .map(|n| n.key)
        .collect()
}

/// The values of the per-note bends among `sent`, with the key each is for.
pub fn bends(sent: &[bl_engine::NoteEvent]) -> Vec<(u8, f32)> {
    sent.iter()
        .filter_map(|n| match n.kind {
            bl_engine::NoteKind::Expression {
                expression: bl_engine::NoteExpression::Bend,
                value,
            } => Some((n.key, value)),
            _ => None,
        })
        .collect()
}
