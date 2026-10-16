//! A backend whose ports a test makes and feeds, for CI and for tests of the
//! code above the device layer.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::backend::{
    Direction, InputConnection, InputHandler, MidiBackend, MidiError, OutputConnection,
};

/// A MIDI system in the test's own process. Clones share it, so a test keeps
/// one to plug ports in and send messages while the code under test owns
/// another.
///
/// [`TestBackend::send`] calls the open input's handler on the caller's
/// thread, as a MIDI system calls it on its own thread, with the time the
/// test gives.
///
/// ```
/// use std::time::Instant;
/// use bl_midi_io::{Direction, MidiBackend, TestBackend};
///
/// let test = TestBackend::new();
/// test.add_port(Direction::Input, "Keys");
/// let mut backend = test.clone();
/// let (tx, rx) = std::sync::mpsc::channel();
/// let _port = backend
///     .open_input("Keys", Box::new(move |_, bytes| tx.send(bytes.to_vec()).unwrap()))
///     .unwrap();
/// assert!(test.send("Keys", Instant::now(), &[0x90, 60, 100]));
/// assert_eq!(rx.recv().unwrap(), vec![0x90, 60, 100]);
/// ```
#[derive(Clone, Default)]
pub struct TestBackend {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    ports: Vec<Port>,
    next_connection: u64,
}

struct Port {
    name: String,
    direction: Direction,
    /// The connection that has it open, and an input's handler.
    open: Option<(u64, Option<InputHandler>)>,
    /// What was sent to an output.
    sent: Vec<Vec<u8>>,
    /// Refuse to open, with this message.
    refuse: Option<String>,
}

impl TestBackend {
    /// A system with no ports yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Plugs in a port.
    pub fn add_port(&self, direction: Direction, name: &str) {
        self.lock().ports.push(Port {
            name: name.to_owned(),
            direction,
            open: None,
            sent: Vec::new(),
            refuse: None,
        });
    }

    /// Unplugs a port, closing it if it's open.
    pub fn remove_port(&self, direction: Direction, name: &str) {
        self.lock()
            .ports
            .retain(|p| !(p.direction == direction && p.name == name));
    }

    /// Makes a port refuse to open, as a port another program holds
    /// exclusively does.
    pub fn refuse(&self, direction: Direction, name: &str, message: &str) {
        if let Some(port) = self
            .lock()
            .ports
            .iter_mut()
            .find(|p| p.direction == direction && p.name == name)
        {
            port.refuse = Some(message.to_owned());
        }
    }

    /// Whether a port is open.
    #[must_use]
    pub fn is_open(&self, direction: Direction, name: &str) -> bool {
        self.lock()
            .ports
            .iter()
            .any(|p| p.direction == direction && p.name == name && p.open.is_some())
    }

    /// Sends a message into an input, as if it arrived at `time`: calls the
    /// handler of the connection that has it open, on this thread. Returns
    /// whether the input was open.
    pub fn send(&self, name: &str, time: Instant, bytes: &[u8]) -> bool {
        let mut shared = self.lock();
        let handler = shared
            .ports
            .iter_mut()
            .find(|p| p.direction == Direction::Input && p.name == name)
            .and_then(|p| p.open.as_mut())
            .and_then(|(_, handler)| handler.as_mut());
        match handler {
            Some(handler) => {
                handler(time, bytes);
                true
            }
            None => false,
        }
    }

    /// What was sent to an output.
    #[must_use]
    pub fn sent(&self, name: &str) -> Vec<Vec<u8>> {
        self.lock()
            .ports
            .iter()
            .find(|p| p.direction == Direction::Output && p.name == name)
            .map(|p| p.sent.clone())
            .unwrap_or_default()
    }

    fn open(
        &self,
        direction: Direction,
        name: &str,
        handler: Option<InputHandler>,
    ) -> Result<u64, MidiError> {
        let mut shared = self.lock();
        shared.next_connection += 1;
        let id = shared.next_connection;
        let port = shared
            .ports
            .iter_mut()
            .find(|p| p.direction == direction && p.name == name)
            .ok_or_else(|| MidiError::NoSuchPort(name.to_owned()))?;
        if let Some(message) = &port.refuse {
            return Err(MidiError::System(message.clone()));
        }
        port.open = Some((id, handler));
        Ok(id)
    }

    fn close(&self, direction: Direction, name: &str, id: u64) {
        if let Some(port) = self
            .lock()
            .ports
            .iter_mut()
            .find(|p| p.direction == direction && p.name == name)
            && port.open.as_ref().is_some_and(|(open, _)| *open == id)
        {
            port.open = None;
        }
    }
}

impl std::fmt::Debug for TestBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shared = self.lock();
        f.debug_list()
            .entries(
                shared
                    .ports
                    .iter()
                    .map(|p| (p.direction, &p.name, p.open.is_some())),
            )
            .finish()
    }
}

impl MidiBackend for TestBackend {
    fn name(&self) -> &'static str {
        "Test"
    }

    fn ports(&mut self, direction: Direction) -> Result<Vec<String>, MidiError> {
        Ok(self
            .lock()
            .ports
            .iter()
            .filter(|p| p.direction == direction)
            .map(|p| p.name.clone())
            .collect())
    }

    fn open_input(
        &mut self,
        name: &str,
        handler: InputHandler,
    ) -> Result<Box<dyn InputConnection>, MidiError> {
        let id = self.open(Direction::Input, name, Some(handler))?;
        Ok(Box::new(Connection {
            backend: self.clone(),
            direction: Direction::Input,
            name: name.to_owned(),
            id,
        }))
    }

    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputConnection>, MidiError> {
        let id = self.open(Direction::Output, name, None)?;
        Ok(Box::new(Connection {
            backend: self.clone(),
            direction: Direction::Output,
            name: name.to_owned(),
            id,
        }))
    }
}

/// An open test port; dropping it closes the port.
struct Connection {
    backend: TestBackend,
    direction: Direction,
    name: String,
    id: u64,
}

impl InputConnection for Connection {}

impl OutputConnection for Connection {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        let mut shared = self.backend.lock();
        let port = shared
            .ports
            .iter_mut()
            .find(|p| p.direction == Direction::Output && p.name == self.name)
            .ok_or_else(|| MidiError::NoSuchPort(self.name.clone()))?;
        port.sent.push(bytes.to_vec());
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.backend.close(self.direction, &self.name, self.id);
    }
}
