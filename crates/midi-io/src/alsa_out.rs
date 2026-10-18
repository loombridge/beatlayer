//! ALSA sequencer output that puts each message on a queue of the
//! sequencer's with its time, so the system sends it on time
//! (`docs/specs/external-devices.md`, R5). `midir` always sends at once, so
//! output is opened here with the `alsa` crate that `midir` is built on.

use std::ffi::CString;
use std::time::{Duration, Instant};

use alsa::Direction;
use alsa::seq::{Addr, EventType, MidiEvent, PortCap, PortSubscribe, PortType, Seq};
use midir::MidiOutput;

use crate::backend::{MidiError, OutputConnection};

/// The name the sequencer shows for Beatlayer's client, port, and queue.
const CLIENT: &str = "Beatlayer";

fn system(error: impl std::fmt::Display) -> MidiError {
    MidiError::System(error.to_string())
}

/// The sequencer address of the port `midir` lists as `name`: its id is
/// `client:port`.
fn destination(name: &str) -> Result<Addr, MidiError> {
    let listing = MidiOutput::new(CLIENT).map_err(system)?;
    let port = listing
        .ports()
        .into_iter()
        .find(|port| listing.port_name(port).is_ok_and(|n| n == name))
        .ok_or_else(|| MidiError::NoSuchPort(name.to_owned()))?;
    port.id().parse().map_err(|_| {
        MidiError::System(format!(
            "the sequencer gave port \"{name}\" an address that isn't client:port"
        ))
    })
}

/// An open ALSA sequencer output: a client with one port subscribed to the
/// destination, and a queue running from the moment it opened.
pub(crate) struct AlsaOutput {
    seq: Seq,
    port: i32,
    queue: i32,
    destination: Addr,
}

impl AlsaOutput {
    /// Opens the destination `midir` lists as `name`.
    pub(crate) fn open(name: &str) -> Result<Self, MidiError> {
        let destination = destination(name)?;
        let name_c = CString::new(CLIENT).map_err(system)?;
        // Not blocking, as `midir` opens it: a full output buffer is an
        // error to report, not a wait the output thread sits in.
        let seq = Seq::open(None, Some(Direction::Playback), true).map_err(system)?;
        seq.set_client_name(&name_c).map_err(system)?;
        let port = seq
            .create_simple_port(
                &name_c,
                PortCap::READ | PortCap::SUBS_READ,
                PortType::MIDI_GENERIC | PortType::APPLICATION,
            )
            .map_err(system)?;
        // From here the port and the queue are freed by `Drop` if a step
        // fails, so the output exists before they are subscribed and started.
        let mut output = Self {
            seq,
            port,
            queue: -1,
            destination,
        };
        let subscription = PortSubscribe::empty().map_err(system)?;
        subscription.set_sender(Addr {
            client: output.seq.client_id().map_err(system)?,
            port,
        });
        subscription.set_dest(destination);
        output.seq.subscribe_port(&subscription).map_err(system)?;
        output.queue = output.seq.alloc_named_queue(&name_c).map_err(system)?;
        output
            .seq
            .control_queue(output.queue, EventType::Start, 0, None)
            .map_err(system)?;
        output.seq.drain_output().map_err(system)?;
        Ok(output)
    }

    /// Outputs `bytes` scheduled at `when` on the queue's clock.
    fn output(&self, bytes: &[u8], when: Duration) -> Result<(), MidiError> {
        // The encoder's buffer takes a whole message, system exclusive
        // included, and a new one is cheap beside the sequencer's own work:
        // this runs on the MIDI output thread, not the audio thread.
        let mut encoder =
            MidiEvent::new(u32::try_from(bytes.len().max(3)).map_err(system)?).map_err(system)?;
        let mut event = match encoder.encode(bytes) {
            Ok((_, Some(event))) => event,
            _ => {
                return Err(MidiError::System(
                    "the sequencer couldn't encode a message".to_owned(),
                ));
            }
        };
        event.set_source(self.port);
        event.set_subs();
        event.schedule_real(self.queue, false, when);
        self.seq.event_output(&mut event).map_err(system)?;
        // A buffer the sequencer can't take yet drains on the next call.
        let _ = self.seq.drain_output();
        Ok(())
    }
}

impl OutputConnection for AlsaOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.send_at(bytes, Instant::now())
    }

    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), MidiError> {
        // A moment already past goes on the queue at its time now, so it
        // leaves at once but after what the queue already holds.
        let ahead = at.saturating_duration_since(Instant::now());
        // The queue's clock has its own zero: its reading now, plus the time
        // from now to `at`, is the queue time of `at`.
        let queue_now = self
            .seq
            .get_queue_status(self.queue)
            .map_err(system)?
            .get_real_time();
        self.output(bytes, queue_now + ahead)
    }
}

impl Drop for AlsaOutput {
    fn drop(&mut self) {
        // What the sequencer still holds for the queue is dropped with it;
        // the client's close frees whatever these leave.
        if let Ok(client) = self.seq.client_id() {
            let _ = self.seq.unsubscribe_port(
                Addr {
                    client,
                    port: self.port,
                },
                self.destination,
            );
        }
        let _ = self.seq.delete_port(self.port);
        if self.queue >= 0 {
            let _ = self.seq.free_queue(self.queue);
        }
    }
}
