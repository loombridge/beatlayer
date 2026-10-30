//! Delay compensation by null test: the same signal on two tracks, one of
//! them polarity-inverted with Utility, and a device that reports latency on
//! one path. With compensation the two paths reach Main aligned, so they
//! cancel: what is left must be at least 80 dB below the signal, live on the
//! hand-paced null device and in an offline export alike. A case that
//! doesn't null is either listed in `known-failures.toml` under suite
//! `live_offline`, key the case's name, property "paths stay aligned", or
//! fails.
//!
//! The devices are the native ones that report a latency and can be set to
//! pass the signal unchanged: Brickwall (the limiter) with its look-ahead
//! and the signal well under its ceiling, Squeeze (the compressor) with a
//! look-ahead and a ratio of 1, and Contour (the EQ) with Oversample on and
//! its bands flat. Align Delay reports none by design (alignment-delay R6),
//! so it has no case here. Each runs on the direct path, through a send
//! into a return, and inside a group; then a look-ahead changes while the
//! song plays, and compensation must follow it.

use bl_devices::compressor::{
    COMPRESSOR, LOOKAHEAD as COMPRESSOR_LOOKAHEAD, RATIO as COMPRESSOR_RATIO,
};
use bl_devices::eq::{EQ, OVERSAMPLE};
use bl_devices::limiter::{LIMITER, LOOKAHEAD as LIMITER_LOOKAHEAD};
use bl_devices::utility::{PHASE_LEFT, PHASE_RIGHT, UTILITY};
use bl_e2e::audio::Audio;
use bl_e2e::{BAR, Driver, ExportSpec, Host, Known, Ratchet};
use serde_json::json;

use crate::live_offline::{SUITE, clear, fixture_file, insert, partials, place, set_param, track};

/// The property every case checks.
const ALIGNED: &str = "paths stay aligned";

/// The most a null may leave, relative to the signal, in dB.
const NULL_DB: f64 = -80.0;

/// The test signal's length, in seconds: one bar at 120 BPM.
const SIGNAL_SECONDS: f64 = 2.0;

/// A latency-bearing device and the settings that make it pass the signal
/// unchanged, apart from its delay.
#[derive(Clone, Copy, Debug)]
struct Latent {
    /// The case's name for it.
    name: &'static str,
    /// Its native type ID.
    device_type: &'static str,
    /// Parameter settings: (param, value).
    settings: &'static [(u32, f64)],
}

/// The devices under test.
const LATENT: [Latent; 3] = [
    // Look-ahead 6 ms; the signal peaks 12 dB under the ceiling.
    Latent {
        name: "limiter look-ahead",
        device_type: LIMITER,
        settings: &[(LIMITER_LOOKAHEAD.0, 2.0)],
    },
    // Look-ahead 10 ms, ratio 1: no gain change at any level.
    Latent {
        name: "compressor look-ahead",
        device_type: COMPRESSOR,
        settings: &[(COMPRESSOR_LOOKAHEAD.0, 2.0), (COMPRESSOR_RATIO.0, 1.0)],
    },
    Latent {
        name: "EQ oversampled",
        device_type: EQ,
        settings: &[(OVERSAMPLE.0, 1.0)],
    },
];

/// Where the latency sits.
#[derive(Clone, Copy, Debug)]
enum Path {
    /// On the first track, both tracks straight to Main.
    Direct,
    /// On a return the first track reaches only through a send at 0 dB.
    Send,
    /// On the first track, both tracks inside a group.
    Group,
}

impl Path {
    fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Send => "through a send and a return",
            Self::Group => "inside a group",
        }
    }
}

/// The signal both tracks play, as the file holds it.
fn signal() -> Audio {
    partials(SIGNAL_SECONDS, 0.05)
}

/// Inserts `latent` at `index` on `track` and gives it its settings.
fn latent_device(host: &mut Host, track: u64, index: usize, latent: Latent) -> u64 {
    let device = insert(host, track, index, latent.device_type, "audio_effect");
    for &(param, value) in latent.settings {
        set_param(host, device, param, value);
    }
    device
}

/// Builds the two tracks of a null test with `latent` on `path`, and gives
/// the latency device's ID.
fn build(host: &mut Host, latent: Latent, path: Path) -> u64 {
    clear(host);
    let file = fixture_file(host, "partials.wav", &signal());
    let group = matches!(path, Path::Group).then(|| track(host, "Group", "group", 0, None));
    let plain = track(host, "Plain", "audio", 0, group);
    let inverted = track(host, "Inverted", "audio", 1, group);
    place(host, plain, &file, 0);
    place(host, inverted, &file, 0);
    let utility = insert(host, inverted, 0, UTILITY, "audio_effect");
    set_param(host, utility, PHASE_LEFT.0, 1.0);
    set_param(host, utility, PHASE_RIGHT.0, 1.0);
    let device = match path {
        Path::Direct | Path::Group => latent_device(host, plain, 0, latent),
        Path::Send => {
            let ret = track(host, "Return", "return", 0, None);
            host.run_ok(
                "routing.set_output",
                json!({ "track": plain, "output": { "type": "sends_only" } }),
            );
            host.run_ok(
                "mixer.set_send",
                json!({ "track": plain, "to": ret, "level": 1.0 }),
            );
            latent_device(host, ret, 0, latent)
        }
    };
    host.check();
    host.settle();
    device
}

/// What is left of the null in `audio` between `from` and `to` seconds,
/// relative to the signal, in dB.
fn residual_db(audio: &Audio, from: f64, to: f64) -> f64 {
    let at = |seconds: f64| ((seconds * f64::from(audio.rate)) as usize).min(audio.frames());
    audio.slice(at(from), at(to)).rms_dbfs() - signal().rms_dbfs()
}

/// `Ok` when `residual` is a null.
fn nulls(what: &str, residual: f64) -> Result<(), String> {
    if residual <= NULL_DB {
        Ok(())
    } else {
        Err(format!(
            "{what} leaves {residual:.1} dB of the signal (at most {NULL_DB} dB)"
        ))
    }
}

/// Plays and exports the null test, and checks both null.
fn null_case(ratchet: &mut Ratchet, latent: Latent, path: Path) {
    let name = format!("{}/{}", latent.name, path.name());
    let mut host = Host::new();
    build(&mut host, latent, path);
    let live = host.play_from(0, 2_000);
    let offline = host.export(0, BAR, &ExportSpec::default());
    // The signal starts and stops at full level, so an oversampler's
    // filters ring at the edges; the middle of the bar is the null.
    let live_db = residual_db(&live, 0.1, 1.8);
    let offline_db = residual_db(&offline, 0.1, 1.8);
    eprintln!("{name}: live {live_db:.1} dB, offline {offline_db:.1} dB");
    let outcome = nulls("live", live_db).and(nulls("the export", offline_db));
    ratchet.check(&name, ALIGNED, outcome);
}

// Spec: delay-compensation R2. Every path into a summing point is held back
// to the longest, so a signal and its inverse cancel at Main whatever latency
// one path carries: on the track, through a send into a return, and inside a
// group.
#[test]
fn paths_through_latency_null_live_and_offline() {
    let mut ratchet = Known::load().ratchet(SUITE);
    for latent in LATENT {
        for path in [Path::Direct, Path::Send, Path::Group] {
            null_case(&mut ratchet, latent, path);
        }
    }
    ratchet.finish();
}

// The null holds only because compensation is on: with it off, the limiter's
// look-ahead puts the paths out of step and the sum no longer cancels, so
// the null tests above measure compensation and not a device that adds no
// delay.
#[test]
fn without_compensation_the_paths_part() {
    let mut host = Host::new();
    build(&mut host, LATENT[0], Path::Direct);
    host.run_ok("mixer.set_delay_compensation", json!({ "enabled": false }));
    host.settle();
    let offline = host.export(0, BAR, &ExportSpec::default());
    let residual = residual_db(&offline, 0.1, 1.8);
    assert!(
        residual > -20.0,
        "without compensation the paths still null: {residual:.1} dB"
    );
}

// Spec: delay-compensation R6. Compensation follows a native device whose
// latency follows a parameter: the limiter's look-ahead, changed from 1.5 ms
// to 6 ms while the song plays, still nulls once the change has landed, and
// an export made afterwards nulls too.
#[test]
fn a_look_ahead_changed_during_playback_stays_aligned() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let mut host = Host::new();
    let limiter = build(&mut host, LATENT[0], Path::Direct);
    set_param(&mut host, limiter, LIMITER_LOOKAHEAD.0, 0.0);
    host.locate(0);
    host.settle();
    host.play();
    host.advance_ms(300);
    let before = host.capture_for(300);
    set_param(&mut host, limiter, LIMITER_LOOKAHEAD.0, 2.0);
    // The new latency reaches the shell, which recompiles with it.
    host.advance_ms(200);
    let after = host.capture_for(800);
    host.stop();
    let before_db = residual_db(&before, 0.0, 0.3);
    let after_db = residual_db(&after, 0.0, 0.8);
    let offline = host.export(0, BAR, &ExportSpec::default());
    let offline_db = residual_db(&offline, 0.1, 1.8);
    eprintln!(
        "look-ahead change: before {before_db:.1} dB, after {after_db:.1} dB, \
         offline {offline_db:.1} dB"
    );
    ratchet.check(
        "limiter look-ahead changed while playing/before the change",
        ALIGNED,
        nulls("before the change", before_db),
    );
    ratchet.check(
        "limiter look-ahead changed while playing/after the change",
        ALIGNED,
        nulls("after the change", after_db).and(nulls("the export", offline_db)),
    );
    ratchet.finish();
}
