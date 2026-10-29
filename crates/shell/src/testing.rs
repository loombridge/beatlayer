//! Helpers for tests that run the shell on a null device paced by hand
//! (T199): advance the set by milliseconds or blocks, and poll the engine
//! while doing it, so a test never waits on the wall clock for a result.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bl_audio_io::{BackendKind, Cable, NullBackend, NullControl, NullOptions, Pacing};

use crate::Shell;
use crate::audio::{AudioHost, AudioSettings};
use crate::files::ProjectFiles;
use crate::settings::Preferences;

/// How much test time one step of [`advance`] and [`wait_for`] covers.
pub const STEP_MS: u64 = 5;

/// A shell on the null device paced by hand (T199), with the test clock
/// driving its MIDI stamps and its blocks, and the control that runs the
/// blocks. The audio isn't open yet: set the shell up the way the test
/// needs, then call [`Shell::start_audio`].
#[must_use]
pub fn rig() -> (Shell, NullControl) {
    rig_with(AudioSettings::default())
}

/// [`rig`] with the audio settings `settings`, such as a buffer size.
#[must_use]
pub fn rig_with(settings: AudioSettings) -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null device")],
    );
    rig_on(backend, control)
}

/// As [`rig`], on `backend` and the `control` it came with: a test gives the
/// null device more channels or a cable (WP-7.17).
#[must_use]
pub fn rig_on(backend: NullBackend, control: NullControl) -> (Shell, NullControl) {
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        settings,
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    // Headless runs and tests touch no real keychain.
    shell.use_keychain(std::sync::Arc::new(
        bl_assistant::keychain::MemoryKeychain::new(),
    ));
    shell.use_test_clock(control.clock().expect("manual pacing"));
    (shell, control)
}

/// An oscillator patched to the null device's cable (WP-7.17): it follows the
/// voltage output channel 3 played the block before, at 0.97 octaves a volt
/// from 164 Hz at −0.13 V with a slight bow, and sounds a sine on input
/// channel 4. `heard` collects what it sounded while `record` is set, and
/// `silent` takes the sound away. A test gives the device 4 channels.
#[must_use]
pub fn oscillator(
    heard: &Arc<Mutex<Vec<f32>>>,
    record: &Arc<AtomicBool>,
    silent: &Arc<AtomicBool>,
) -> Cable {
    let (heard, record, silent) = (heard.clone(), record.clone(), silent.clone());
    let mut phase = 0.0f64;
    Cable::new(move |played, next| {
        let volts = f64::from(played[2].last().copied().unwrap_or(0.0)) * 10.0;
        let hz = 164.0 * (0.97 * (volts + 0.13) + 0.00005 * (volts * volts - 3.0 * volts)).exp2();
        let silent = silent.load(Ordering::Relaxed);
        for (c, channel) in next.iter_mut().enumerate() {
            for sample in channel.iter_mut() {
                *sample = if c == 3 && !silent {
                    phase = (phase + hz / 48_000.0).fract();
                    (phase * std::f64::consts::TAU).sin() as f32 * 0.5
                } else {
                    0.0
                };
            }
        }
        if record.load(Ordering::Relaxed) {
            heard.lock().unwrap().extend_from_slice(&next[3]);
        }
    })
}

/// Runs `blocks` blocks on the null stream and lets the shell read what the
/// engine published.
pub fn run_blocks(control: &NullControl, shell: &mut Shell, blocks: u64) {
    control.run_blocks(blocks);
    shell.poll_engine();
}

/// Advances the set by `millis` of test time in [`STEP_MS`] steps, polling the
/// engine after each. The null device is paced by hand, so this runs at once
/// and lands on the same frames however busy the machine is.
pub fn advance(control: &NullControl, shell: &mut Shell, millis: u64) {
    let clock = control.clock().expect("a manually paced null stream");
    let rate = f64::from(shell.audio().sample_rate().unwrap_or(48_000));
    let target = clock.elapsed() + Duration::from_millis(millis);
    while clock.elapsed() < target {
        let remaining = target - clock.elapsed();
        let capped = remaining.min(Duration::from_millis(STEP_MS));
        let frames = (capped.as_secs_f64() * rate).ceil() as u64;
        control.run_frames(frames.max(1));
        shell.poll_engine();
    }
}

/// Waits until the engine plays the set as it is now: the change has reached
/// the audio thread and a few blocks have run with it, so a note played next
/// reaches a track just armed or changed.
pub fn settle(control: &NullControl, shell: &mut Shell) {
    assert!(
        shell.audio().flush(Duration::from_secs(10)),
        "the engine never took the set"
    );
    let blocks = |shell: &Shell| shell.audio().stats().map_or(0, |s| s.blocks);
    let start = blocks(shell);
    for _ in 0..2_000 {
        if blocks(shell) >= start + 4 {
            return;
        }
        advance(control, shell, STEP_MS);
    }
    panic!("the stream stopped");
}

/// Polls the engine and advances the set by [`STEP_MS`] until `done` says
/// so, for up to two thousand steps (about ten seconds of test time). Panics
/// with `what` if it never does.
pub fn wait_for(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    mut done: impl FnMut(&mut Shell) -> bool,
) {
    for _ in 0..2_000 {
        shell.poll_engine();
        if done(shell) {
            return;
        }
        advance(control, shell, STEP_MS);
    }
    panic!("waited for {what}");
}

/// Puts the real-time render's session into its waiting period, so a test
/// can show the dialog an export with external hardware opens (the external
/// devices spec, R23).
pub fn realtime_waiting(shell: &Shell, seconds: u64) {
    shell
        .realtime_session()
        .begin_wait(Duration::from_secs(seconds));
}
