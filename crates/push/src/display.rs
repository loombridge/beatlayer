//! The display's thread, its pacing and its USB sink
//! (`docs/specs/push-2.md`, R17).
//!
//! The surface hands the thread a [`Screen`]; the thread draws it and sends
//! a frame when what it shows has changed, at most 60 times a second and at
//! least once a second while nothing has, since Push 2 blanks its display 2 s
//! after the last frame. A display that can't be claimed, or a transfer that
//! fails, leaves everything else running and is tried again every 2 s.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::frame;
use crate::paint;
use crate::screen::Screen;

/// The shortest time between two frames: 60 a second.
pub const FASTEST: Duration = Duration::from_micros(16_667);
/// The longest time between two frames: Push 2 blanks after 2 s, so one a
/// second keeps it lit.
pub const SLOWEST: Duration = Duration::from_secs(1);
/// How long to wait before claiming the display again after a failure.
pub const RETRY: Duration = Duration::from_secs(2);

/// Where frames go: the USB display, or a test's sink.
pub trait Sink: Send {
    /// Sends one frame, whole.
    ///
    /// # Errors
    ///
    /// A message if the transfer failed; the display is claimed again.
    fn send(&mut self, frame: &[u8]) -> Result<(), String>;
}

/// Claims a sink: the USB display in the app, a test's sink in a test.
pub type Claim = Arc<dyn Fn() -> Result<Box<dyn Sink>, String> + Send + Sync>;

/// Claims Push 2's display interface through `nusb` (R17).
#[must_use]
pub fn usb() -> Claim {
    Arc::new(|| Usb::claim().map(|usb| Box::new(usb) as Box<dyn Sink>))
}

/// Push 2's display: vendor 0x2982, product 0x1967, interface 0, bulk
/// endpoint 0x01 (R17).
struct Usb {
    endpoint: nusb::Endpoint<nusb::transfer::Bulk, nusb::transfer::Out>,
}

impl Usb {
    fn claim() -> Result<Self, String> {
        use nusb::MaybeFuture;
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| e.to_string())?
            .find(|d| d.vendor_id() == 0x2982 && d.product_id() == 0x1967)
            .ok_or("no Push 2 on USB")?;
        let device = info.open().wait().map_err(|e| e.to_string())?;
        let interface = device
            .claim_interface(0)
            .wait()
            .map_err(|e| e.to_string())?;
        let endpoint = interface
            .endpoint(0x01)
            .map_err(|e| format!("Push 2's display endpoint: {e}"))?;
        Ok(Self { endpoint })
    }

    fn transfer(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.endpoint.submit(bytes.to_vec().into());
        self.endpoint
            .wait_next_complete(Duration::from_millis(1000))
            .ok_or("a transfer to Push 2's display timed out")?
            .status
            .map_err(|e| e.to_string())
    }
}

impl Sink for Usb {
    fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        let (header, pixels) = frame.split_at(frame::HEADER.len());
        self.transfer(header)?;
        // 16 KB at a time, as the interface manual suggests.
        for chunk in pixels.chunks(16 * 1024) {
            self.transfer(chunk)?;
        }
        Ok(())
    }
}

/// The pacing and the claiming, with no thread: the thread calls
/// [`Renderer::step`] with the time, and a test calls it with the test
/// clock's.
pub struct Renderer {
    claim: Claim,
    sink: Option<Box<dyn Sink>>,
    drawn: Option<Screen>,
    rgb: Option<Vec<u8>>,
    last_sent: Option<Instant>,
    last_claim: Option<Instant>,
    /// Frames sent so far.
    pub frames: u64,
    /// Why the display isn't claimed.
    pub error: Option<String>,
}

impl Renderer {
    /// A renderer that claims its sink with `claim`.
    #[must_use]
    pub fn new(claim: Claim) -> Self {
        Self {
            claim,
            sink: None,
            drawn: None,
            rgb: None,
            last_sent: None,
            last_claim: None,
            frames: 0,
            error: None,
        }
    }

    /// Whether the display is claimed.
    #[must_use]
    pub fn claimed(&self) -> bool {
        self.sink.is_some()
    }

    /// The latest image drawn.
    #[must_use]
    pub fn image(&self) -> Option<&[u8]> {
        self.rgb.as_deref()
    }

    /// One turn of the loop at `now` with `screen` to show: claims the sink
    /// if it hasn't one (every 2 s at most), and sends a frame if the screen
    /// changed (at most 60 a second) or a second has passed since the last
    /// (R17). Returns whether a frame went.
    pub fn step(&mut self, now: Instant, screen: &Screen) -> bool {
        if self.sink.is_none() {
            if self
                .last_claim
                .is_some_and(|at| now.duration_since(at) < RETRY)
            {
                return false;
            }
            self.last_claim = Some(now);
            match (self.claim)() {
                Ok(sink) => {
                    self.sink = Some(sink);
                    self.error = None;
                    // A new sink has seen nothing yet.
                    self.last_sent = None;
                }
                Err(e) => {
                    self.error = Some(e);
                    return false;
                }
            }
        }
        let since = self.last_sent.map(|at| now.duration_since(at));
        let changed = self.drawn.as_ref() != Some(screen);
        let due = match since {
            None => true,
            Some(since) => (changed && since >= FASTEST) || since >= SLOWEST,
        };
        if !due {
            return false;
        }
        if changed || self.rgb.is_none() {
            self.rgb = Some(paint::render(screen));
            self.drawn = Some(screen.clone());
        }
        let Some(rgb) = &self.rgb else {
            return false;
        };
        let Some(sink) = self.sink.as_mut() else {
            return false;
        };
        match sink.send(&frame::encode(rgb)) {
            Ok(()) => {
                self.last_sent = Some(now);
                self.frames += 1;
                true
            }
            Err(e) => {
                self.sink = None;
                self.error = Some(e);
                self.last_claim = Some(now);
                false
            }
        }
    }
}

/// What the thread and the surface share.
#[derive(Default)]
struct Shared {
    screen: Mutex<Screen>,
    stop: AtomicBool,
    status: Mutex<Status>,
}

/// What the display is doing, for `push.state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Whether the display interface is claimed.
    pub claimed: bool,
    /// Why it isn't.
    pub error: Option<String>,
    /// Frames sent.
    pub frames: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The display's thread (R17): never the audio thread, never the host's.
pub struct Display {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Display {
    /// Starts the thread, which claims its sink with `claim`.
    #[must_use]
    pub fn start(claim: Claim) -> Self {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("bl-push-display".to_owned())
                .spawn(move || {
                    let mut renderer = Renderer::new(claim);
                    while !shared.stop.load(Ordering::Acquire) {
                        let screen = lock(&shared.screen).clone();
                        renderer.step(Instant::now(), &screen);
                        *lock(&shared.status) = Status {
                            claimed: renderer.claimed(),
                            error: renderer.error.clone(),
                            frames: renderer.frames,
                        };
                        thread::sleep(Duration::from_millis(4));
                    }
                })
                .ok()
        };
        Self { shared, thread }
    }

    /// Gives the thread what to show next.
    pub fn show(&self, screen: &Screen) {
        let mut shown = lock(&self.shared.screen);
        if *shown != *screen {
            shown.clone_from(screen);
        }
    }

    /// What the display is doing.
    #[must_use]
    pub fn status(&self) -> Status {
        lock(&self.shared.status).clone()
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // The thread ends on its own after the transfer it may be in; the
        // host's call to stop the surface doesn't wait for it.
        self.shared.stop.store(true, Ordering::Release);
        drop(self.thread.take());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use bl_core::clock::TestClock;

    use super::*;
    use crate::screen::Label;

    struct Counting(Arc<Mutex<Vec<usize>>>);

    impl Sink for Counting {
        fn send(&mut self, frame: &[u8]) -> Result<(), String> {
            lock(&self.0).push(frame.len());
            Ok(())
        }
    }

    fn counting() -> (Claim, Arc<Mutex<Vec<usize>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&sent);
        (
            Arc::new(move || Ok(Box::new(Counting(Arc::clone(&seen))) as Box<dyn Sink>)),
            sent,
        )
    }

    fn screen(text: &str) -> Screen {
        let mut screen = Screen::default();
        screen.upper[0] = Label::new(text, [255, 0, 0], false);
        screen
    }

    // Spec: push-2 R17. At most 60 frames a second when the screen keeps
    // changing, and one a second when it doesn't, on the test clock.
    #[test]
    fn frames_go_at_most_sixty_a_second_and_at_least_one_a_second() {
        let (claim, sent) = counting();
        let mut renderer = Renderer::new(claim);
        let clock = TestClock::new();
        // A second of changes every millisecond.
        for n in 0..1000 {
            renderer.step(clock.now(), &screen(&n.to_string()));
            clock.advance(Duration::from_millis(1));
        }
        let busy = lock(&sent).len();
        assert!(
            (55..=60).contains(&busy),
            "{busy} frames in a second of changes"
        );
        assert!(lock(&sent).iter().all(|&len| len == frame::FRAME_BYTES));

        // Three seconds with nothing new: one frame each second.
        let still = screen("still");
        renderer.step(clock.now(), &still);
        clock.advance(Duration::from_millis(20));
        renderer.step(clock.now(), &still);
        let before = lock(&sent).len();
        for _ in 0..3001 {
            renderer.step(clock.now(), &still);
            clock.advance(Duration::from_millis(1));
        }
        assert_eq!(lock(&sent).len() - before, 3);
    }

    // Spec: push-2 R17. A display that can't be claimed is tried again every
    // 2 s, and the first frame goes once it is.
    #[test]
    fn a_failed_claim_is_retried_every_two_seconds() {
        let tries = Arc::new(AtomicU32::new(0));
        let (inner, sent) = counting();
        let claim: Claim = {
            let tries = Arc::clone(&tries);
            Arc::new(move || {
                if tries.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err("claimed by another program".to_owned())
                } else {
                    inner()
                }
            })
        };
        let mut renderer = Renderer::new(claim);
        let clock = TestClock::new();
        let screen = screen("x");
        for _ in 0..5000 {
            renderer.step(clock.now(), &screen);
            clock.advance(Duration::from_millis(1));
            if renderer.claimed() {
                break;
            }
        }
        assert_eq!(tries.load(Ordering::SeqCst), 3, "a try at 0 s, 2 s and 4 s");
        assert!(clock.elapsed() >= Duration::from_secs(4));
        assert_eq!(lock(&sent).len(), 1, "the frame went with the claim");
        assert_eq!(renderer.error, None);
    }

    // Spec: push-2 R17. A failed transfer drops the sink and claims it again.
    #[test]
    fn a_failed_transfer_claims_the_display_again() {
        struct Failing;
        impl Sink for Failing {
            fn send(&mut self, _: &[u8]) -> Result<(), String> {
                Err("unplugged".to_owned())
            }
        }
        let claim: Claim = Arc::new(|| Ok(Box::new(Failing) as Box<dyn Sink>));
        let mut renderer = Renderer::new(claim);
        let clock = TestClock::new();
        assert!(!renderer.step(clock.now(), &screen("a")));
        assert!(!renderer.claimed());
        assert_eq!(renderer.error.as_deref(), Some("unplugged"));
    }
}
