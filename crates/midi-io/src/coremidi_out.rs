//! Core MIDI output that hands the system each message ahead of time with
//! its time stamp, so Core MIDI sends it on time
//! (`docs/specs/external-devices.md`, R5). `midir` always sends at once, so
//! output is opened here with the `coremidi` crate that `midir` is built on.

use std::time::Instant;

use coremidi::{Client, Destination, Destinations, OutputPort, PacketBuffer};

use crate::backend::{MidiError, OutputConnection};

/// The client name Core MIDI shows for Beatlayer's output ports.
const CLIENT: &str = "Beatlayer";

#[repr(C)]
struct TimebaseInfo {
    numer: u32,
    denom: u32,
}

// The Mach clock Core MIDI's host time counts: ticks of a fixed length that
// `mach_timebase_info` gives as a fraction of a nanosecond.
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
}

fn system(error: impl std::fmt::Display) -> MidiError {
    MidiError::System(error.to_string())
}

/// An open Core MIDI output.
pub(crate) struct CoreMidiOutput {
    // Dropped in this order: the port, then the destination it sent to,
    // then the client that made the port.
    port: OutputPort,
    destination: Destination,
    _client: Client,
    /// Nanoseconds per host time tick, as `numer / denom`.
    numer: u32,
    denom: u32,
}

impl CoreMidiOutput {
    /// Opens the destination called `name`: the one `midir` lists under that
    /// name, which is its display name.
    pub(crate) fn open(name: &str) -> Result<Self, MidiError> {
        let destination = Destinations
            .into_iter()
            .find(|d| d.display_name().is_some_and(|n| n == name))
            .ok_or_else(|| MidiError::NoSuchPort(name.to_owned()))?;
        let client = Client::new(CLIENT).map_err(system)?;
        let port = client.output_port(CLIENT).map_err(system)?;
        let mut info = TimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid, writable `mach_timebase_info_data_t`.
        let status = unsafe { mach_timebase_info(&raw mut info) };
        if status != 0 || info.numer == 0 || info.denom == 0 {
            return Err(MidiError::System(
                "the system clock's tick length is unknown".to_owned(),
            ));
        }
        Ok(Self {
            port,
            destination,
            _client: client,
            numer: info.numer,
            denom: info.denom,
        })
    }

    /// Core MIDI's host time for `at`: the clock now plus the time from now
    /// to `at`. Time stamp 0 means now, so a moment already past is 0.
    fn host_time(&self, at: Instant) -> u64 {
        let now = Instant::now();
        // SAFETY: `mach_absolute_time` takes no arguments and has no
        // preconditions.
        let host_now = unsafe { mach_absolute_time() };
        let Some(ahead) = at.checked_duration_since(now) else {
            return 0;
        };
        let ticks = ahead.as_nanos() * u128::from(self.denom) / u128::from(self.numer);
        host_now.saturating_add(u64::try_from(ticks).unwrap_or(u64::MAX))
    }

    fn send_stamped(&self, bytes: &[u8], host_time: u64) -> Result<(), MidiError> {
        let packets = PacketBuffer::new(host_time, bytes);
        self.port
            .send(&self.destination, &packets)
            .map_err(|status| MidiError::System(format!("Core MIDI refused a message: {status}")))
    }
}

impl OutputConnection for CoreMidiOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.send_stamped(bytes, 0)
    }

    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), MidiError> {
        self.send_stamped(bytes, self.host_time(at))
    }
}
