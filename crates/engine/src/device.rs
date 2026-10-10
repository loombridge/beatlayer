//! The engine on an audio device, kept going through device changes
//! (WP-1.13).
//!
//! [`AudioEngine`] runs an [`Engine`] on a stream that a `bl-audio-io`
//! [`Supervisor`] keeps open. When the device goes away, its sample rate
//! changes, its stream stalls, or the machine sleeps, the supervisor closes
//! the stream. The engine then stops cleanly: the processor comes back from
//! the closed stream, publishes a stopped transport, and hands on the song
//! position and the scheduled commands that hadn't fired; the compiler
//! stops compiling for a queue nobody reads. The device's state says what
//! happened ([`AudioEngine::device`], [`AudioEngine::message`], and the
//! [`DeviceEvent`]s), for the app to show. When the stream opens again, on
//! the same device or another, the engine restarts with a processor made
//! for the rate and block size the stream runs at, the newest document
//! compiled anew for it, and the song where it was, stopped. The rules are
//! in `docs/specs/device-changes.md`.

use std::time::Duration;

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, BackendFactory, DeviceEvent, DeviceStatus, Error,
    ErrorKind, PowerMonitor, ProcessContext, StreamClient, StreamInfo, StreamRequest, StreamStats,
    Supervisor, SupervisorOptions,
};
use bl_core::time::SampleRate;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::{Engine, Restarter};
use crate::processor::{Carry, Processor};
use crate::registry::Registry;
use crate::state::EngineState;
use crate::{EngineConfig, ParamEvent};

/// How long a restart waits for the compiler thread to make the new
/// processor before the supervisor tries again.
const RESTART_TIMEOUT: Duration = Duration::from_secs(10);

/// The engine running on an audio device, through unplugging, sample-rate
/// changes, stalls, and sleep (WP-1.13; the rules are in
/// `docs/specs/device-changes.md`).
///
/// A `bl-audio-io` [`Supervisor`] keeps the stream open. When it closes the
/// stream, the engine stops cleanly: the processor comes back from the
/// closed stream, publishes a stopped transport, and hands on the song
/// position and the scheduled commands that hadn't fired, and the compiler
/// stops compiling for a queue nobody reads. [`device`](Self::device),
/// [`message`](Self::message), and [`events`](Self::events) say what
/// happened, for the app to show. When the stream opens again, on the same
/// device or another, the engine restarts with a processor made for the
/// stream's rate and block size, the newest document compiled anew for it,
/// and the song where it was, stopped.
///
/// Dropping it closes the stream, then stops the engine's threads.
pub struct AudioEngine {
    // Dropped first: closing the stream hands the processor back while the
    // engine's threads still run.
    supervisor: Supervisor,
    engine: Engine,
}

impl AudioEngine {
    /// Starts the engine on `request`'s device, with the backend
    /// `backend` makes, noticing sleep from the clocks. `config` gives the
    /// engine's channels and capacities; each stream sets its sample rate
    /// and block size. It returns once the first attempt to open the device
    /// has finished: if that failed, [`device`](Self::device) says why, and
    /// the device opens as soon as it can.
    ///
    /// # Errors
    ///
    /// If the backend can't be made or a thread can't start.
    pub fn start(
        config: EngineConfig,
        registry: Registry,
        backend: BackendFactory,
        request: StreamRequest,
        options: SupervisorOptions,
    ) -> Result<Self, Error> {
        Self::start_with_power(
            config,
            registry,
            backend,
            request,
            options,
            Box::new(bl_audio_io::ClockJump::default()),
        )
    }

    /// [`start`](Self::start), learning of sleep and wake from `power`.
    ///
    /// # Errors
    ///
    /// If the backend can't be made or a thread can't start.
    pub fn start_with_power(
        config: EngineConfig,
        registry: Registry,
        backend: BackendFactory,
        request: StreamRequest,
        options: SupervisorOptions,
        power: Box<dyn PowerMonitor>,
    ) -> Result<Self, Error> {
        let engine = Engine::start_detached(config, registry).map_err(|e| {
            Error::new(
                ErrorKind::Backend,
                format!("couldn't start the engine's threads: {e}"),
            )
        })?;
        let client = EngineClient {
            restarter: engine.restarter(),
            base: config,
            returning: None,
            carry: Carry::default(),
        };
        let supervisor =
            Supervisor::start_with_power(backend, request, Box::new(client), options, power)?;
        Ok(Self { supervisor, engine })
    }

    /// The engine: sync documents to it, play, stop, schedule.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The engine, for what needs it mutably.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// The engine's latest state: playhead, meters, load, and the sample
    /// rate it runs at. While the device is interrupted, the transport shows
    /// stopped where it was.
    pub fn state(&mut self) -> &EngineState {
        self.engine.state()
    }

    /// Where the device stands: running, interrupted and why, on the
    /// fallback device, or stopped.
    #[must_use]
    pub fn device(&self) -> DeviceStatus {
        self.supervisor.status()
    }

    /// What to tell a person about the device, or `None` while all is well.
    /// The app shows it in the Status Bar's message area (L-STAT-002).
    #[must_use]
    pub fn message(&self) -> Option<String> {
        self.supervisor.status().message()
    }

    /// What happened to the device since the last call, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<DeviceEvent> {
        self.supervisor.events()
    }

    /// The running stream's counters, as of the supervisor's last check.
    #[must_use]
    pub fn stats(&self) -> Option<StreamStats> {
        self.supervisor.stats()
    }

    /// The device settings in force (see [`Supervisor::request`]).
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        self.supervisor.request()
    }

    /// Moves the engine to another device, rate, or buffer size. If that
    /// can't open, the previous settings open again and the error is
    /// returned.
    ///
    /// # Errors
    ///
    /// If `request` can't open.
    pub fn reconfigure(&self, request: StreamRequest) -> Result<StreamInfo, Error> {
        self.supervisor.reconfigure(request)
    }

    /// Tries to open an interrupted device now, rather than at the next
    /// retry.
    pub fn retry(&self) {
        self.supervisor.retry();
    }

    /// Sends a parameter change (see [`Engine::send_param`]).
    pub fn send_param(&self, event: ParamEvent) {
        self.engine.send_param(event);
    }
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("device", &self.device())
            .field("engine", &self.engine)
            .finish()
    }
}

/// The engine's side of a supervised stream, on the supervisor's thread.
struct EngineClient {
    restarter: Restarter,
    /// The settings each processor keeps; a stream sets the rate and block
    /// size.
    base: EngineConfig,
    /// Where the running stream's callback hands its processor back.
    returning: Option<Consumer<Processor>>,
    /// What the last processor handed on, for the next.
    carry: Carry,
}

impl StreamClient for EngineClient {
    fn open(&mut self, info: &StreamInfo) -> Result<Box<dyn AudioCallback>, Error> {
        let sample_rate = SampleRate::new(info.sample_rate)
            .ok_or_else(|| Error::new(ErrorKind::UnsupportedConfig, "the device runs at 0 Hz"))?;
        let config = EngineConfig {
            sample_rate,
            max_frames: (info.max_block_frames as usize).max(1),
            ..self.base
        };
        // Taken, so a processor that never comes back can't leave a stale
        // position for the restart after.
        let carry = std::mem::take(&mut self.carry);
        let Some(processor) = self
            .restarter
            .restart(config, carry.clone(), RESTART_TIMEOUT)
        else {
            self.carry = carry;
            return Err(Error::new(
                ErrorKind::Backend,
                "the engine's compiler didn't make a processor in time",
            ));
        };
        let (returns, returning) = RingBuffer::new(1);
        self.returning = Some(returning);
        Ok(Box::new(EngineCallback {
            processor: Some(processor),
            returns,
        }))
    }

    fn closed(&mut self) {
        // The stream has been dropped, and its callback with it, which handed
        // the processor back.
        if let Some(mut returning) = self.returning.take()
            && let Ok(mut processor) = returning.pop()
        {
            processor.park();
            self.carry = processor.carry();
        }
        self.restarter.detach();
    }
}

/// A stream's callback: runs the processor block by block into the device's
/// output. Device channels past the engine's stay silent, and a mono device
/// gets the engine's first channel. When the stream drops it, it hands the
/// processor back through a lock-free queue made for it, which moves one
/// pointer.
struct EngineCallback {
    processor: Option<Processor>,
    returns: Producer<Processor>,
}

impl AudioCallback for EngineCallback {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let Some(processor) = self.processor.as_mut() else {
            return;
        };
        let mut done = 0;
        while done < ctx.frames {
            let frames = (ctx.frames - done).min(processor.config().max_frames);
            let block = processor.process(frames);
            let channels = output.channels().min(block.channels());
            for c in 0..channels {
                output.channel_mut(c)[done..done + frames].copy_from_slice(block.channel(c));
            }
            done += frames;
        }
    }
}

impl Drop for EngineCallback {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            // The queue has room for the one processor. Dropping it here
            // instead would only lose the song position.
            let _ = self.returns.push(processor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use bl_core::time::{Frames, Ticks};
    use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

    use crate::message::{Action, Report, ScheduledCommand};
    use crate::nodes::{TEST_TONE, test_tone};
    use crate::testing::{self, Probe};

    /// A song with one test tone at 1 kHz, −60 dBFS.
    fn song() -> Editor {
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: "Tone".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let mut device = Device::new(
            editor.new_id(),
            "",
            DeviceKind::Native {
                device_type: TEST_TONE.to_owned(),
            },
        );
        device.params.insert(test_tone::FREQUENCY, 1_000.0);
        device.params.insert(test_tone::LEVEL, -60.0);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        editor
    }

    fn config(rate: SampleRate) -> EngineConfig {
        EngineConfig::new(rate, 256)
    }

    /// A detached engine with the song synced.
    fn detached() -> (Engine, Editor) {
        let probe = std::sync::Arc::new(Probe::default());
        let engine =
            Engine::start_detached(config(SampleRate::HZ_48000), testing::registry(&probe))
                .unwrap();
        let editor = song();
        engine.sync(editor.document());
        assert!(engine.flush(Duration::from_secs(5)));
        (engine, editor)
    }

    fn restart(engine: &Engine, rate: SampleRate, carry: Carry) -> Processor {
        engine
            .restarter()
            .restart(config(rate), carry, Duration::from_secs(5))
            .expect("a processor")
    }

    /// The first channel of `blocks` blocks of 256 frames.
    fn render(processor: &mut Processor, blocks: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..blocks {
            out.extend_from_slice(processor.process(256).channel(0));
        }
        out
    }

    // Spec: device-changes R3
    #[test]
    fn a_restart_at_a_new_rate_renders_like_a_fresh_engine() {
        let (engine, editor) = detached();
        let mut first = restart(&engine, SampleRate::HZ_48000, Carry::default());
        render(&mut first, 4);
        let carry = first.carry();
        drop(first);
        engine.restarter().detach();
        let mut restarted = restart(&engine, SampleRate::HZ_44100, carry);

        let probe = std::sync::Arc::new(Probe::default());
        let mut fresh =
            crate::Offline::new(config(SampleRate::HZ_44100), testing::registry(&probe));
        fresh.sync(editor.document());
        let mut expected = Vec::new();
        for _ in 0..8 {
            expected.extend_from_slice(fresh.process(256).channel(0));
        }
        let actual = render(&mut restarted, 8);
        assert!(actual.iter().any(|&s| s != 0.0));
        assert_eq!(
            actual, expected,
            "the restarted engine plays the song afresh at 44.1 kHz"
        );
        assert_eq!(
            restarted.program().unwrap().sample_rate,
            SampleRate::HZ_44100
        );
    }

    // Spec: device-changes R7
    #[test]
    fn a_restart_keeps_the_beat_and_stops_the_transport() {
        let (mut engine, _editor) = detached();
        let mut first = restart(&engine, SampleRate::HZ_48000, Carry::default());
        engine.play();
        assert!(engine.flush(Duration::from_secs(5)));
        // Two beats at 120 bpm: 48,000 frames.
        for _ in 0..(48_000 / 256 + 1) {
            first.process(256);
        }
        let played = first.position();
        assert!(first.playing());
        first.park();
        assert!(
            !engine.state().playing,
            "parking publishes a stopped transport"
        );
        let carry = first.carry();
        drop(first);
        engine.restarter().detach();

        let restarted = restart(&engine, SampleRate::HZ_44100, carry.clone());
        assert!(!restarted.playing());
        // The same beat, to the frame: a frame at 44.1 kHz and 120 bpm is 32
        // ticks.
        let tempo = &restarted.program().unwrap().tempo;
        let at = tempo.frames_to_ticks(restarted.position(), SampleRate::HZ_44100);
        assert!((at.get() - carry.position.get()).abs() <= 32, "{at:?}");
        let expected = Frames((played.get() as f64 * 44_100.0 / 48_000.0).round() as i64);
        assert!((restarted.position().get() - expected.get()).abs() <= 1);
        // The engine reads the new processor's state, ready before a block.
        let state = engine.state();
        assert_eq!(state.sample_rate, SampleRate::HZ_44100);
        assert_eq!(state.program, Some(1));
        assert_eq!(state.position, restarted.position());
        assert_eq!(engine.counts().restarts, 2);
    }

    // Spec: device-changes R7
    #[test]
    fn scheduled_commands_survive_a_restart() {
        let (mut engine, _editor) = detached();
        let mut first = restart(&engine, SampleRate::HZ_48000, Carry::default());

        for (id, at) in [
            (1, Ticks::QUARTER),
            (2, Ticks::from_quarters(3)),
            (3, Ticks::from_quarters(4)),
        ] {
            engine.schedule(ScheduledCommand {
                id,
                at,
                action: Action::Notify,
            });
        }
        engine.play();
        assert!(engine.flush(Duration::from_secs(5)));
        // Past the first beat (24,000 frames), short of the third.
        for _ in 0..120 {
            first.process(256);
        }
        engine.cancel(3);
        assert!(engine.flush(Duration::from_secs(5)));
        first.process(256);
        let carry = first.carry();
        assert_eq!(
            carry.scheduled.iter().map(|c| c.id).collect::<Vec<_>>(),
            [2]
        );
        drop(first);
        engine.restarter().detach();

        let mut restarted = restart(&engine, SampleRate::HZ_44100, carry);
        // Play again through the new processor's queue.
        engine.play();
        assert!(engine.flush(Duration::from_secs(5)));
        for _ in 0..400 {
            restarted.process(256);
        }
        let reports = engine.reports();
        let fired: Vec<(u64, Frames)> = reports
            .iter()
            .filter_map(|report| match *report {
                Report::Fired { id, position, .. } => Some((id, position)),
                _ => None,
            })
            .collect();
        // Beat 3 lands at 66,150 frames at 44.1 kHz.
        assert_eq!(fired, [(1, Frames(24_000)), (2, Frames(66_150))]);
    }

    // Spec: device-changes R7
    #[test]
    fn while_detached_play_is_ignored_and_a_locate_is_kept() {
        let (mut engine, mut editor) = detached();
        let first = restart(&engine, SampleRate::HZ_48000, Carry::default());
        drop(first);
        engine.restarter().detach();
        let programs = engine.counts().programs;
        let deferred = engine.counts().deferred;
        engine.play();
        engine.locate(Ticks::from_quarters(8));
        // Edits while detached are kept, not compiled.
        let track = editor.document().top_level().next().unwrap();
        editor
            .apply(Command::SetVolume { track, volume: 0.5 })
            .unwrap();
        engine.sync(editor.document());
        let began = Instant::now();
        assert!(engine.flush(Duration::from_secs(5)));
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "a flush returns at once while detached"
        );
        assert_eq!(engine.counts().programs, programs);
        assert_eq!(engine.counts().deferred, deferred + 1);

        let restarted = restart(&engine, SampleRate::HZ_44100, Carry::default());
        assert!(!restarted.playing(), "play while detached is dropped");
        assert_eq!(restarted.position(), Frames(4 * 44_100));
        assert_eq!(engine.counts().programs, programs + 1);
        let state = engine.state();
        assert_eq!(state.position_ticks, Ticks::from_quarters(8));
    }

    /// Runs the engine's callback for blocks of a device buffer larger than
    /// the engine's, with every block a real-time section under the
    /// allocation trap.
    // Spec: device-changes R10
    #[test]
    fn the_engine_callback_stays_off_the_heap() {
        let (engine, _editor) = detached();
        let processor = engine
            .restarter()
            .restart(
                EngineConfig::new(SampleRate::HZ_48000, 128),
                Carry::default(),
                Duration::from_secs(5),
            )
            .unwrap();
        let (returns, mut returning) = RingBuffer::new(1);
        let mut callback = EngineCallback {
            processor: Some(processor),
            returns,
        };
        let mut data = vec![0.0f32; 3 * 512];
        let ctx = ProcessContext {
            sample_rate: 48_000,
            frames: 512,
            position: 0,
            timestamp: None,
        };
        for _ in 0..4 {
            let mut output = AudioBufferMut::new(&mut data, 3, 512, 512);
            bl_rt::rt_test!({ callback.process(&ctx, &AudioBuffer::empty(512), &mut output) });
        }
        assert!(data[..512].iter().any(|&s| s != 0.0), "the tone plays");
        assert_eq!(&data[..512], &data[512..1024], "both channels");
        assert!(
            data[1024..].iter().all(|&s| s == 0.0),
            "a third channel stays silent"
        );
        drop(callback);
        let processor = returning.pop().expect("the processor comes back");
        assert_eq!(processor.clock(), 4 * 512);
    }
}
