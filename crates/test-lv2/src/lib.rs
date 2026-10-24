//! An LV2 plug-in built from source for the plug-in host's tests (WP-5.39).
//! It's never shipped; the tests write its bundle with [`bundle`] and load
//! it from there. The bundle's metadata is generated, so no binary or
//! Turtle file is committed.
//!
//! "Test Gain Echo" is an audio effect that also takes notes, so one
//! plug-in exercises what the host does:
//!
//! - **Audio.** Two channels in and out: each sample is its input times
//!   [`GAIN`]'s value.
//! - **Control.** [`GAIN`] is a control input, 0 to 2, 1 by default.
//! - **Latency.** [`LATENCY_PORT`] is the latency port; the plug-in reports
//!   [`LATENCY`] frames in it (it doesn't delay anything).
//! - **Notes in and out.** Every three-byte MIDI message on the Atom
//!   sequence in goes back out on the same frame, a note an octave up
//!   ([`ECHO_TRANSPOSE`]).
//! - **State.** The last note-on's key is the plug-in's own state, saved and
//!   restored through the state interface; [`LAST_NOTE_PORT`], a control
//!   output, shows it.
//! - **Parameters.** [`TOGGLE`], [`MODE`] (an enumeration), [`CUTOFF`] (a
//!   logarithmic frequency) and [`STEPS`] (an integer) are control inputs of
//!   each kind a plug-in's parameters come in. They change nothing audible.
//! - **Worker.** Each note-on schedules a job with its key; the worker
//!   answers with the key plus [`WORKER_ADDS`], and [`WORKED_PORT`] shows
//!   the answer once the audio thread has it.
//! - **Transport.** The Atom port in also takes `time:Position`;
//!   [`BAR_BEAT_PORT`] and [`SPEED_PORT`] show the last position it got, and
//!   while the song plays it sends a note on at the block's first frame
//!   whose key is [`BEAT_NOTE`] plus the whole beat.
//! - **Scan modes.** `BL_TEST_LV2_SCAN` set to `crash` or `hang` makes
//!   instantiating the plug-in crash or hang, so the scanner's child process
//!   is tested against a plug-in that misbehaves.

use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

/// The plug-in's URI.
pub const PLUGIN_URI: &str = "urn:beatlayer:test:gain-echo";
/// The plug-in's name.
pub const PLUGIN_NAME: &str = "Test Gain Echo";
/// The ports, by index.
pub const IN_L: u32 = 0;
pub const IN_R: u32 = 1;
pub const OUT_L: u32 = 2;
pub const OUT_R: u32 = 3;
/// The gain control input: a linear factor from 0 to [`GAIN_MAX`], 1 by default.
pub const GAIN: u32 = 4;
pub const GAIN_MAX: f32 = 2.0;
/// The latency port, a control output.
pub const LATENCY_PORT: u32 = 5;
/// The last note-on's key, a control output.
pub const LAST_NOTE_PORT: u32 = 6;
pub const MIDI_IN: u32 = 7;
pub const MIDI_OUT: u32 = 8;
/// A toggled control input, off by default.
pub const TOGGLE: u32 = 9;
/// An enumeration of three values, 0 to 2, 1 ("Warm") by default.
pub const MODE: u32 = 10;
/// A logarithmic control input in hertz, 20 to 20,000, 1,000 by default.
pub const CUTOFF: u32 = 11;
/// An integer control input, 1 to 8, 4 by default.
pub const STEPS: u32 = 12;
/// A control output: the worker's last answer.
pub const WORKED_PORT: u32 = 13;
/// Control outputs: the bar beat and speed of the last `time:Position`.
pub const BAR_BEAT_PORT: u32 = 14;
pub const SPEED_PORT: u32 = 15;
/// What the worker adds to a note's key in its answer.
pub const WORKER_ADDS: u32 = 100;
/// The key of the note sent for beat 0 of a bar while the song plays.
pub const BEAT_NOTE: u8 = 32;
/// The frames of latency the plug-in reports.
pub const LATENCY: u32 = 32;
/// How far the echoed notes are transposed, in semitones.
pub const ECHO_TRANSPOSE: u8 = 12;

const PORTS: usize = 16;
const MIDI_EVENT: &CStr = c"http://lv2plug.in/ns/ext/midi#MidiEvent";
const SEQUENCE: &CStr = c"http://lv2plug.in/ns/ext/atom#Sequence";
const INT: &CStr = c"http://lv2plug.in/ns/ext/atom#Int";
const LAST_KEY: &CStr = c"urn:beatlayer:test:last-key";
const MAP: &CStr = c"http://lv2plug.in/ns/ext/urid#map";
const STATE_INTERFACE: &CStr = c"http://lv2plug.in/ns/ext/state#interface";
const WORKER_INTERFACE: &CStr = c"http://lv2plug.in/ns/ext/worker#interface";
const SCHEDULE: &CStr = c"http://lv2plug.in/ns/ext/worker#schedule";
const OBJECT: &CStr = c"http://lv2plug.in/ns/ext/atom#Object";
const POSITION: &CStr = c"http://lv2plug.in/ns/ext/time#Position";
const BAR_BEAT: &CStr = c"http://lv2plug.in/ns/ext/time#barBeat";
const SPEED: &CStr = c"http://lv2plug.in/ns/ext/time#speed";

#[repr(C)]
struct Feature {
    uri: *const c_char,
    data: *mut c_void,
}

#[repr(C)]
struct UridMap {
    handle: *mut c_void,
    map: unsafe extern "C" fn(*mut c_void, *const c_char) -> u32,
}

type Store = unsafe extern "C" fn(*mut c_void, u32, *const c_void, usize, u32, u32) -> i32;
type Retrieve =
    unsafe extern "C" fn(*mut c_void, u32, *mut usize, *mut u32, *mut u32) -> *const c_void;

#[repr(C)]
struct WorkerSchedule {
    handle: *mut c_void,
    schedule_work: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32,
}

type Respond = unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32;

#[repr(C)]
struct WorkerInterface {
    work: unsafe extern "C" fn(*mut c_void, Respond, *mut c_void, u32, *const c_void) -> i32,
    work_response: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> i32,
    end_run: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

#[repr(C)]
struct StateInterface {
    save: unsafe extern "C" fn(*mut c_void, Store, *mut c_void, u32, *const *const Feature) -> i32,
    restore:
        unsafe extern "C" fn(*mut c_void, Retrieve, *mut c_void, u32, *const *const Feature) -> i32,
}

#[repr(C)]
struct Descriptor {
    uri: *const c_char,
    instantiate: unsafe extern "C" fn(
        *const Descriptor,
        f64,
        *const c_char,
        *const *const Feature,
    ) -> *mut c_void,
    connect_port: unsafe extern "C" fn(*mut c_void, u32, *mut c_void),
    activate: Option<unsafe extern "C" fn(*mut c_void)>,
    run: unsafe extern "C" fn(*mut c_void, u32),
    deactivate: Option<unsafe extern "C" fn(*mut c_void)>,
    cleanup: unsafe extern "C" fn(*mut c_void),
    extension_data: unsafe extern "C" fn(*const c_char) -> *const c_void,
}

// SAFETY: the statics hold function pointers and a C string literal.
unsafe impl Sync for Descriptor {}
// SAFETY: as above.
unsafe impl Sync for StateInterface {}
// SAFETY: as above.
unsafe impl Sync for WorkerInterface {}

static DESCRIPTOR: Descriptor = Descriptor {
    uri: c"urn:beatlayer:test:gain-echo".as_ptr(),
    instantiate,
    connect_port,
    activate: None,
    run,
    deactivate: None,
    cleanup,
    extension_data,
};

static STATE: StateInterface = StateInterface { save, restore };

static WORKER: WorkerInterface = WorkerInterface {
    work,
    work_response,
    end_run: None,
};

struct Plugin {
    ports: [*mut c_void; PORTS],
    midi_event: u32,
    sequence: u32,
    int: u32,
    last_key_urid: u32,
    last_key: u32,
    object: u32,
    position: u32,
    bar_beat_urid: u32,
    speed_urid: u32,
    schedule: *const WorkerSchedule,
    worked: u32,
    bar_beat: f32,
    speed: f32,
}

/// # Safety
///
/// Only the host's call to find the plug-ins.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lv2_descriptor(index: u32) -> *const c_void {
    if index == 0 {
        (&raw const DESCRIPTOR).cast()
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn instantiate(
    _: *const Descriptor,
    _rate: f64,
    _bundle: *const c_char,
    features: *const *const Feature,
) -> *mut c_void {
    // The scanner's tests ask for a plug-in that crashes or hangs when it
    // is made.
    match std::env::var("BL_TEST_LV2_SCAN").as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::park();
        },
        _ => {}
    }
    // SAFETY: `features` is a null-terminated list of features.
    unsafe {
        let (mut map, mut schedule) = (None, ptr::null());
        let mut at = features;
        while !at.is_null() && !(*at).is_null() {
            let uri = CStr::from_ptr((**at).uri);
            if uri == MAP {
                map = Some(&*(**at).data.cast::<UridMap>());
            } else if uri == SCHEDULE {
                schedule = (**at).data.cast::<WorkerSchedule>().cast_const();
            }
            at = at.add(1);
        }
        let Some(map) = map else {
            return ptr::null_mut();
        };
        let urid = |uri: &CStr| (map.map)(map.handle, uri.as_ptr());
        Box::into_raw(Box::new(Plugin {
            ports: [ptr::null_mut(); PORTS],
            midi_event: urid(MIDI_EVENT),
            sequence: urid(SEQUENCE),
            int: urid(INT),
            last_key_urid: urid(LAST_KEY),
            last_key: 0,
            object: urid(OBJECT),
            position: urid(POSITION),
            bar_beat_urid: urid(BAR_BEAT),
            speed_urid: urid(SPEED),
            schedule,
            worked: 0,
            bar_beat: 0.0,
            speed: 0.0,
        }))
        .cast()
    }
}

unsafe extern "C" fn connect_port(plugin: *mut c_void, port: u32, data: *mut c_void) {
    // SAFETY: `plugin` is the `Plugin` `instantiate` made.
    unsafe {
        if let Some(slot) = (*plugin.cast::<Plugin>()).ports.get_mut(port as usize) {
            *slot = data;
        }
    }
}

unsafe extern "C" fn cleanup(plugin: *mut c_void) {
    // SAFETY: as `connect_port`; it's the last call.
    drop(unsafe { Box::from_raw(plugin.cast::<Plugin>()) });
}

unsafe extern "C" fn extension_data(uri: *const c_char) -> *const c_void {
    // SAFETY: `uri` is a C string.
    let uri = unsafe { CStr::from_ptr(uri) };
    if uri == STATE_INTERFACE {
        ptr::from_ref(&STATE).cast()
    } else if uri == WORKER_INTERFACE {
        ptr::from_ref(&WORKER).cast()
    } else {
        ptr::null()
    }
}

/// The worker's job: answer a note's key with the key plus [`WORKER_ADDS`].
unsafe extern "C" fn work(
    _plugin: *mut c_void,
    respond: Respond,
    handle: *mut c_void,
    size: u32,
    data: *const c_void,
) -> i32 {
    if size != 4 {
        return 1;
    }
    // SAFETY: the host gives `size` bytes, and its own respond function.
    unsafe {
        let key = data.cast::<u32>().read_unaligned();
        let answer = key + WORKER_ADDS;
        respond(handle, 4, ptr::from_ref(&answer).cast())
    }
}

/// The worker's answer, back on the audio thread.
unsafe extern "C" fn work_response(plugin: *mut c_void, size: u32, body: *const c_void) -> i32 {
    if size == 4 {
        // SAFETY: as `work`; `plugin` is the `Plugin` `instantiate` made.
        unsafe { (*plugin.cast::<Plugin>()).worked = body.cast::<u32>().read_unaligned() };
    }
    0
}

unsafe extern "C" fn save(
    plugin: *mut c_void,
    store: Store,
    handle: *mut c_void,
    _flags: u32,
    _features: *const *const Feature,
) -> i32 {
    // SAFETY: as `connect_port`; `store` is the host's.
    unsafe {
        let p = &*plugin.cast::<Plugin>();
        // POD and portable.
        store(
            handle,
            p.last_key_urid,
            ptr::from_ref(&p.last_key).cast(),
            4,
            p.int,
            3,
        )
    }
}

unsafe extern "C" fn restore(
    plugin: *mut c_void,
    retrieve: Retrieve,
    handle: *mut c_void,
    _flags: u32,
    _features: *const *const Feature,
) -> i32 {
    // SAFETY: as `save`.
    unsafe {
        let p = &mut *plugin.cast::<Plugin>();
        let (mut size, mut type_, mut flags) = (0, 0, 0);
        let value = retrieve(
            handle,
            p.last_key_urid,
            &raw mut size,
            &raw mut type_,
            &raw mut flags,
        );
        if !value.is_null() && size == 4 && type_ == p.int {
            p.last_key = value.cast::<u32>().read_unaligned();
        }
    }
    0
}

/// A `u32` at byte `at` of an Atom buffer.
unsafe fn word(base: *const u8, at: usize) -> u32 {
    // SAFETY: the caller keeps `at` inside the buffer.
    unsafe { base.add(at).cast::<u32>().read_unaligned() }
}

unsafe extern "C" fn run(plugin: *mut c_void, frames: u32) {
    // SAFETY: `plugin` is the `Plugin` `instantiate` made, its ports are
    // connected to buffers of `frames` samples, and the Atom buffers are the
    // host's sequences.
    unsafe {
        let p = &mut *plugin.cast::<Plugin>();
        let gain = p.ports[GAIN as usize].cast::<f32>().read();
        for channel in 0..2 {
            let (input, output) = (
                p.ports[channel].cast::<f32>(),
                p.ports[2 + channel].cast::<f32>(),
            );
            for frame in 0..frames as usize {
                output.add(frame).write(input.add(frame).read() * gain);
            }
        }
        let input = p.ports[MIDI_IN as usize].cast::<u8>().cast_const();
        let output = p.ports[MIDI_OUT as usize].cast::<u8>();
        // The host gives an output sequence its room in the atom's size.
        let room = 8 + word(output, 0) as usize;
        let mut out = 16;
        let end = 8 + word(input, 0) as usize;
        let mut at = 16;
        while at + 16 <= end {
            let size = word(input, at + 8) as usize;
            if word(input, at + 12) == p.object && word(input, at + 20) == p.position {
                // A `time:Position`: its properties follow the object's
                // body, each a key, a context, and an atom.
                let mut property = at + 24;
                while property + 16 <= at + 16 + size {
                    let (key, value_size) =
                        (word(input, property), word(input, property + 8) as usize);
                    if key == p.bar_beat_urid {
                        p.bar_beat = f32::from_bits(word(input, property + 16));
                    } else if key == p.speed_urid {
                        p.speed = f32::from_bits(word(input, property + 16));
                    }
                    property += (16 + value_size).next_multiple_of(8);
                }
                // While the song plays, the beat is heard as a note.
                if p.speed > 0.0 && out + 24 <= room {
                    output.add(out).cast::<i64>().write_unaligned(0);
                    output.add(out + 8).cast::<u32>().write_unaligned(3);
                    output
                        .add(out + 12)
                        .cast::<u32>()
                        .write_unaligned(p.midi_event);
                    output.add(out + 16).write(0x90);
                    output
                        .add(out + 17)
                        .write(BEAT_NOTE.saturating_add(p.bar_beat as u8).min(127));
                    output.add(out + 18).write(100);
                    output.add(out + 19).write_bytes(0, 5);
                    out += 24;
                }
            }
            if word(input, at + 12) == p.midi_event && size == 3 {
                let message = [
                    input.add(at + 16).read(),
                    input.add(at + 17).read(),
                    input.add(at + 18).read(),
                ];
                if message[0] & 0xF0 == 0x90 && message[2] > 0 {
                    p.last_key = u32::from(message[1]);
                    if let Some(schedule) = p.schedule.as_ref() {
                        let key = u32::from(message[1]);
                        (schedule.schedule_work)(schedule.handle, 4, ptr::from_ref(&key).cast());
                    }
                }
                if out + 24 <= room {
                    output
                        .add(out)
                        .cast::<i64>()
                        .write_unaligned(input.add(at).cast::<i64>().read_unaligned());
                    output.add(out + 8).cast::<u32>().write_unaligned(3);
                    output
                        .add(out + 12)
                        .cast::<u32>()
                        .write_unaligned(p.midi_event);
                    output.add(out + 16).write(message[0]);
                    output
                        .add(out + 17)
                        .write((message[1] + ECHO_TRANSPOSE).min(127));
                    output.add(out + 18).write(message[2]);
                    output.add(out + 19).write_bytes(0, 5);
                    out += 24;
                }
            }
            at += (16 + size).next_multiple_of(8);
        }
        output.cast::<u32>().write_unaligned((out - 8) as u32);
        output.add(4).cast::<u32>().write_unaligned(p.sequence);
        output.add(8).cast::<u64>().write_unaligned(0);
        p.ports[LATENCY_PORT as usize]
            .cast::<f32>()
            .write(LATENCY as f32);
        p.ports[LAST_NOTE_PORT as usize]
            .cast::<f32>()
            .write(p.last_key as f32);
        p.ports[WORKED_PORT as usize]
            .cast::<f32>()
            .write(p.worked as f32);
        p.ports[BAR_BEAT_PORT as usize].cast::<f32>().write(p.bar_beat);
        p.ports[SPEED_PORT as usize].cast::<f32>().write(p.speed);
    }
}

/// The test plug-in as an LV2 bundle folder, `test-gain-echo.lv2`, in a
/// temporary folder of this process: its library copied in, and its
/// manifest and description written.
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let library = library();
        let file = library
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a library name");
        let dir = std::env::temp_dir().join(format!("beatlayer-test-lv2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bundle = dir.join("test-gain-echo.lv2");
        std::fs::create_dir_all(&bundle).expect("a temporary folder");
        std::fs::copy(&library, bundle.join(file)).expect("the library");
        std::fs::write(
            bundle.join("manifest.ttl"),
            format!(
                "@prefix lv2: <http://lv2plug.in/ns/lv2core#> .\n\
                 @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                 <{PLUGIN_URI}> a lv2:Plugin ; lv2:binary <{file}> ; rdfs:seeAlso <plugin.ttl> .\n"
            ),
        )
        .expect("the manifest");
        std::fs::write(bundle.join("plugin.ttl"), description()).expect("the description");
        bundle
    })
}

/// The plug-in's description: its features and its ports.
fn description() -> String {
    let audio = |index: u32, symbol: &str, direction: &str| {
        format!(
            "[ a lv2:AudioPort, lv2:{direction}Port ; lv2:index {index} ; lv2:symbol \"{symbol}\" ; lv2:name \"{symbol}\" ]"
        )
    };
    let atom = |index: u32, symbol: &str, direction: &str| {
        format!(
            "[ a atom:AtomPort, lv2:{direction}Port ; lv2:index {index} ; lv2:symbol \"{symbol}\" ; \
             lv2:name \"{symbol}\" ; atom:bufferType atom:Sequence ; atom:supports midi:MidiEvent, time:Position ]"
        )
    };
    format!(
        "@prefix lv2: <http://lv2plug.in/ns/lv2core#> .\n\
         @prefix doap: <http://usefulinc.com/ns/doap#> .\n\
         @prefix atom: <http://lv2plug.in/ns/ext/atom#> .\n\
         @prefix midi: <http://lv2plug.in/ns/ext/midi#> .\n\
         @prefix urid: <http://lv2plug.in/ns/ext/urid#> .\n\
         @prefix state: <http://lv2plug.in/ns/ext/state#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         @prefix pprops: <http://lv2plug.in/ns/ext/port-props#> .\n\
         @prefix units: <http://lv2plug.in/ns/extensions/units#> .\n\
         @prefix time: <http://lv2plug.in/ns/ext/time#> .\n\
         @prefix worker: <http://lv2plug.in/ns/ext/worker#> .\n\
         <{PLUGIN_URI}> a lv2:Plugin ;\n\
           doap:name \"{PLUGIN_NAME}\" ;\n\
           lv2:requiredFeature urid:map ;\n\
           lv2:optionalFeature worker:schedule ;\n\
           lv2:extensionData state:interface, worker:interface ;\n\
           lv2:port\n\
             {in_l} ,\n {in_r} ,\n {out_l} ,\n {out_r} ,\n\
             [ a lv2:ControlPort, lv2:InputPort ; lv2:index {GAIN} ; lv2:symbol \"gain\" ; lv2:name \"Gain\" ;\n\
               lv2:default 1.0 ; lv2:minimum 0.0 ; lv2:maximum {GAIN_MAX:?} ] ,\n\
             [ a lv2:ControlPort, lv2:OutputPort ; lv2:index {LATENCY_PORT} ; lv2:symbol \"latency\" ; lv2:name \"Latency\" ;\n\
               lv2:designation lv2:latency ; lv2:portProperty lv2:reportsLatency, lv2:integer ;\n\
               lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 4096 ] ,\n\
             [ a lv2:ControlPort, lv2:OutputPort ; lv2:index {LAST_NOTE_PORT} ; lv2:symbol \"last_note\" ; lv2:name \"Last note\" ;\n\
               lv2:portProperty lv2:integer ; lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 127 ] ,\n\
             {midi_in} ,\n {midi_out} ,\n\
             [ a lv2:ControlPort, lv2:InputPort ; lv2:index {TOGGLE} ; lv2:symbol \"toggle\" ; lv2:name \"Toggle\" ;\n\
               lv2:portProperty lv2:toggled ; lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 1 ] ,\n\
             [ a lv2:ControlPort, lv2:InputPort ; lv2:index {MODE} ; lv2:symbol \"mode\" ; lv2:name \"Mode\" ;\n\
               lv2:portProperty lv2:integer, lv2:enumeration ; lv2:default 1 ; lv2:minimum 0 ; lv2:maximum 2 ;\n\
               lv2:scalePoint [ rdfs:label \"Clean\" ; rdf:value 0 ] , [ rdfs:label \"Warm\" ; rdf:value 1 ] ,\n\
                 [ rdfs:label \"Hot\" ; rdf:value 2 ] ] ,\n\
             [ a lv2:ControlPort, lv2:InputPort ; lv2:index {CUTOFF} ; lv2:symbol \"cutoff\" ; lv2:name \"Cutoff\" ;\n\
               lv2:portProperty pprops:logarithmic ; units:unit units:hz ;\n\
               lv2:default 1000.0 ; lv2:minimum 20.0 ; lv2:maximum 20000.0 ] ,\n\
             [ a lv2:ControlPort, lv2:InputPort ; lv2:index {STEPS} ; lv2:symbol \"steps\" ; lv2:name \"Steps\" ;\n\
               lv2:portProperty lv2:integer ; lv2:default 4 ; lv2:minimum 1 ; lv2:maximum 8 ] ,\n\
             [ a lv2:ControlPort, lv2:OutputPort ; lv2:index {WORKED_PORT} ; lv2:symbol \"worked\" ; lv2:name \"Worked\" ;\n\
               lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 1000 ] ,\n\
             [ a lv2:ControlPort, lv2:OutputPort ; lv2:index {BAR_BEAT_PORT} ; lv2:symbol \"bar_beat\" ; lv2:name \"Bar beat\" ;\n\
               lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 64 ] ,\n\
             [ a lv2:ControlPort, lv2:OutputPort ; lv2:index {SPEED_PORT} ; lv2:symbol \"speed\" ; lv2:name \"Speed\" ;\n\
               lv2:default 0 ; lv2:minimum 0 ; lv2:maximum 1 ] .\n",
        in_l = audio(IN_L, "in_l", "Input"),
        in_r = audio(IN_R, "in_r", "Input"),
        out_l = audio(OUT_L, "out_l", "Output"),
        out_r = audio(OUT_R, "out_r", "Output"),
        midi_in = atom(MIDI_IN, "midi_in", "Input"),
        midi_out = atom(MIDI_OUT, "midi_out", "Output"),
    )
}

/// The newest test plug-in library Cargo built beside the running test
/// binary.
fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let deps = exe.parent().expect("the test binary's folder");
    let prefix = format!("{}bl_test_lv2", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = [deps, deps.parent().unwrap_or(deps)]
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    found.sort();
    found
        .pop()
        .map(|(_, path)| path)
        .expect("Cargo builds the test plug-in's cdylib beside the tests")
}
