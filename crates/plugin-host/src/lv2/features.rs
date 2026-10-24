//! What the host gives a plug-in when it instantiates it, beyond the URID
//! map (the LV2 hosting spec, R9): the options with the block-size bounds
//! and the rate, a worker that runs the plug-in's non-real-time jobs on a
//! thread of its own, and logging (whose messages are dropped, as a CLAP
//! plug-in's are: a plug-in may log from its audio thread).

use std::cell::UnsafeCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use super::{Urids, sys};

/// The most bytes a worker job or its response carries.
pub(super) const WORK_BYTES: usize = 4096;
/// How many jobs, and how many responses, wait at a time.
const WORK_SLOTS: usize = 16;
/// How long the worker thread sleeps when no job waits. The audio thread
/// never signals it: a wake-up call is a system call.
const WORK_POLL: Duration = Duration::from_millis(1);

const OPTIONS: &str = "http://lv2plug.in/ns/ext/options#";
const BUFSZ: &str = "http://lv2plug.in/ns/ext/buf-size#";
const ATOM: &str = "http://lv2plug.in/ns/ext/atom#";
const PARAMETERS: &str = "http://lv2plug.in/ns/ext/parameters#";

/// A worker job or response: a length and its bytes, copied.
#[derive(Clone, Copy)]
pub(super) struct Msg {
    len: u32,
    data: [u8; WORK_BYTES],
}

impl Msg {
    fn new(size: u32, data: *const c_void) -> Option<Self> {
        if size as usize > WORK_BYTES {
            return None;
        }
        let mut msg = Self {
            len: size,
            data: [0; WORK_BYTES],
        };
        if !data.is_null() {
            // SAFETY: the caller's `data` holds `size` bytes.
            msg.data[..size as usize]
                .copy_from_slice(unsafe { std::slice::from_raw_parts(data.cast(), size as usize) });
        }
        Some(msg)
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// The audio side of the job queue: what `schedule_work` pushes to.
struct Schedule(UnsafeCell<Producer<Msg>>);

// SAFETY: one thread schedules work at a time (the one running the
// plug-in), as LV2 has it.
unsafe impl Send for Schedule {}
// SAFETY: as above.
unsafe impl Sync for Schedule {}

unsafe extern "C" fn schedule_work(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    // SAFETY: `handle` is the `Schedule` the feature was made with.
    let schedule = unsafe { &*handle.cast::<Schedule>() };
    let Some(msg) = Msg::new(size, data) else {
        return 2;
    };
    // SAFETY: only the thread running the plug-in schedules work.
    match unsafe { (*schedule.0.get()).push(msg) } {
        Ok(()) => 0,
        Err(_) => 2,
    }
}

/// The worker thread's end of the response queue.
struct Respond(Producer<Msg>);

unsafe extern "C" fn respond(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    // SAFETY: `handle` is the `Respond` the worker thread passed to `work`.
    let respond = unsafe { &mut *handle.cast::<Respond>() };
    match Msg::new(size, data) {
        Some(msg) if respond.0.push(msg).is_ok() => 0,
        _ => 2,
    }
}

unsafe extern "C" fn log_printf(_: *mut c_void, _: u32, _: *const c_char) -> i32 {
    0
}

unsafe extern "C" fn log_vprintf(_: *mut c_void, _: u32, _: *const c_char, _: *mut c_void) -> i32 {
    0
}

/// The values the options point at.
struct OptionValues {
    min_block: i32,
    max_block: i32,
    nominal_block: i32,
    sequence_size: i32,
    rate: f32,
}

/// The features of one instantiation, which must outlive the plug-in: the
/// list lilv takes, and what its pointers reach.
pub(super) struct Features {
    pub(super) list: Vec<*const sys::Feature>,
    _uris: Vec<CString>,
    _features: Vec<sys::Feature>,
    _options: Box<[sys::OptionsOption]>,
    _values: Box<OptionValues>,
    _schedule: Box<Schedule>,
    _worker: Box<sys::WorkerSchedule>,
    _log: Box<sys::LogLog>,
    /// The jobs the worker thread takes, and the responses it sends back
    /// and the audio thread takes, until the plug-in turns out to have a
    /// worker.
    pub(super) jobs: Option<Consumer<Msg>>,
    pub(super) responses_in: Option<Producer<Msg>>,
    pub(super) responses_out: Option<Consumer<Msg>>,
}

// SAFETY: the raw pointers reach the boxes the value owns, and the map and
// unmap structs of the world that outlives it.
unsafe impl Send for Features {}
// SAFETY: as above.
unsafe impl Sync for Features {}

impl Features {
    pub(super) fn new(
        urids: &Urids,
        map: *mut sys::UridMap,
        unmap: *mut sys::UridUnmap,
        rate: f64,
        max_frames: usize,
        sequence_bytes: usize,
    ) -> Self {
        let (jobs_in, jobs) = RingBuffer::new(WORK_SLOTS);
        let (responses_in, responses_out) = RingBuffer::new(WORK_SLOTS);
        let urid = |prefix: &str, name: &str| {
            urids.map(&CString::new(format!("{prefix}{name}")).expect("a URI without NUL"))
        };
        let int = urid(ATOM, "Int");
        let float = urid(ATOM, "Float");
        let max_block = i32::try_from(max_frames).unwrap_or(i32::MAX);
        let mut values = Box::new(OptionValues {
            min_block: 1,
            max_block,
            nominal_block: max_block,
            sequence_size: i32::try_from(sequence_bytes).unwrap_or(i32::MAX),
            rate: rate as f32,
        });
        let option = |key: u32, type_: u32, value: *const c_void, size: u32| sys::OptionsOption {
            context: 0,
            subject: 0,
            key,
            size,
            type_,
            value,
        };
        let v = ptr::from_mut(&mut *values);
        // SAFETY: `v` points at the box just made.
        let options: Box<[sys::OptionsOption]> = unsafe {
            vec![
                option(
                    urid(BUFSZ, "minBlockLength"),
                    int,
                    (&raw const (*v).min_block).cast(),
                    4,
                ),
                option(
                    urid(BUFSZ, "maxBlockLength"),
                    int,
                    (&raw const (*v).max_block).cast(),
                    4,
                ),
                option(
                    urid(BUFSZ, "nominalBlockLength"),
                    int,
                    (&raw const (*v).nominal_block).cast(),
                    4,
                ),
                option(
                    urid(BUFSZ, "sequenceSize"),
                    int,
                    (&raw const (*v).sequence_size).cast(),
                    4,
                ),
                option(
                    urid(PARAMETERS, "sampleRate"),
                    float,
                    (&raw const (*v).rate).cast(),
                    4,
                ),
                option(0, 0, ptr::null(), 0),
            ]
        }
        .into_boxed_slice();
        let mut schedule = Box::new(Schedule(UnsafeCell::new(jobs_in)));
        let mut worker = Box::new(sys::WorkerSchedule {
            handle: ptr::from_mut(&mut *schedule).cast(),
            schedule_work,
        });
        let mut log = Box::new(sys::LogLog {
            handle: ptr::null_mut(),
            printf: log_printf,
            vprintf: log_vprintf,
        });
        let uris: Vec<CString> = [
            "http://lv2plug.in/ns/ext/urid#map".to_owned(),
            "http://lv2plug.in/ns/ext/urid#unmap".to_owned(),
            format!("{OPTIONS}options"),
            format!("{BUFSZ}boundedBlockLength"),
            "http://lv2plug.in/ns/ext/worker#schedule".to_owned(),
            "http://lv2plug.in/ns/ext/log#log".to_owned(),
        ]
        .into_iter()
        .map(|uri| CString::new(uri).expect("a URI without NUL"))
        .collect();
        let data: [*mut c_void; 6] = [
            map.cast(),
            unmap.cast(),
            options.as_ptr().cast_mut().cast(),
            ptr::null_mut(),
            ptr::from_mut(&mut *worker).cast(),
            ptr::from_mut(&mut *log).cast(),
        ];
        let features: Vec<sys::Feature> = uris
            .iter()
            .zip(data)
            .map(|(uri, data)| sys::Feature {
                uri: uri.as_ptr(),
                data,
            })
            .collect();
        let mut list: Vec<*const sys::Feature> = features.iter().map(ptr::from_ref).collect();
        list.push(ptr::null());
        Self {
            list,
            _uris: uris,
            _features: features,
            _options: options,
            _values: values,
            _schedule: schedule,
            _worker: worker,
            _log: log,
            jobs: Some(jobs),
            responses_in: Some(responses_in),
            responses_out: Some(responses_out),
        }
    }
}

/// The thread that runs a plug-in's worker jobs, stopped and joined before
/// the plug-in is freed.
pub(super) struct WorkerThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Handles {
    interface: *const sys::WorkerInterface,
    instance: *mut c_void,
}

// SAFETY: the thread is joined before the plug-in goes, and LV2 lets any
// non-real-time thread call `work`.
unsafe impl Send for Handles {}

impl WorkerThread {
    pub(super) fn start(
        interface: *const sys::WorkerInterface,
        instance: *mut c_void,
        mut jobs: Consumer<Msg>,
        responses: Producer<Msg>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handles = Handles {
            interface,
            instance,
        };
        let thread = std::thread::Builder::new()
            .name("lv2-worker".into())
            .spawn(move || {
                let handles = handles;
                let mut respond_to = Respond(responses);
                loop {
                    match jobs.pop() {
                        // SAFETY: the plug-in offered the interface, and is
                        // alive until this thread is joined.
                        Ok(job) => unsafe {
                            ((*handles.interface).work)(
                                handles.instance,
                                respond,
                                ptr::from_mut(&mut respond_to).cast(),
                                job.len,
                                job.data.as_ptr().cast(),
                            );
                        },
                        Err(_) if flag.load(Relaxed) => break,
                        Err(_) => std::thread::sleep(WORK_POLL),
                    }
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for WorkerThread {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The worker interface's URI, which a plug-in's `extension_data` answers.
pub(super) const WORKER_INTERFACE: &CStr = c"http://lv2plug.in/ns/ext/worker#interface";
