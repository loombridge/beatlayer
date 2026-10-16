//! The null backend: a MIDI system with no ports, for headless runs and CI.

use crate::backend::{
    Direction, InputConnection, InputHandler, MidiBackend, MidiError, OutputConnection,
};

/// A MIDI system with no ports. Opening anything fails with
/// [`MidiError::NoSuchPort`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NullBackend;

impl MidiBackend for NullBackend {
    fn name(&self) -> &'static str {
        "None"
    }

    fn ports(&mut self, _direction: Direction) -> Result<Vec<String>, MidiError> {
        Ok(Vec::new())
    }

    fn open_input(
        &mut self,
        name: &str,
        _handler: InputHandler,
    ) -> Result<Box<dyn InputConnection>, MidiError> {
        Err(MidiError::NoSuchPort(name.to_owned()))
    }

    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputConnection>, MidiError> {
        Err(MidiError::NoSuchPort(name.to_owned()))
    }
}
