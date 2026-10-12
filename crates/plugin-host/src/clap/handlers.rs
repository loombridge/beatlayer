//! What a CLAP plug-in can ask of Beatlayer: the host's callbacks.
//!
//! Every request a plug-in makes from another thread only sets a flag,
//! which [`ClapHost::idle`](super::ClapHost::idle) acts on the next time
//! the main thread runs it: nothing here blocks, allocates, or locks, so a
//! plug-in may call it from its audio thread.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl};
use clack_extensions::latency::{HostLatency, HostLatencyImpl};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
    PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_host::prelude::*;

/// Beatlayer as a CLAP host.
pub(crate) struct Host;

impl HostHandlers for Host {
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread;
    type AudioProcessor<'a> = ProcessorData;

    fn declare_extensions(builder: &mut HostExtensions<'_, Self>, _shared: &Shared) {
        builder
            .register::<HostLog>()
            .register::<HostLatency>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostGui>()
            .register::<HostAudioPorts>();
    }
}

/// The flags a plug-in sets from any thread.
#[derive(Default)]
pub(crate) struct Shared {
    /// It asked to be deactivated and activated again.
    pub(crate) restart: AtomicBool,
    /// It asked the main thread to call it back.
    pub(crate) callback: AtomicBool,
    /// It asked for a parameter flush while not processing.
    pub(crate) flush: AtomicBool,
    /// Its editor's window closed itself.
    pub(crate) editor_closed: AtomicBool,
}

impl SharedHandler<'_> for Shared {
    fn request_restart(&self) {
        self.restart.store(true, Relaxed);
    }

    fn request_process(&self) {
        // Beatlayer calls every active plug-in every block.
    }

    fn request_callback(&self) {
        self.callback.store(true, Relaxed);
    }
}

impl HostLogImpl for Shared {
    fn log(&self, _severity: LogSeverity, _message: &str) {
        // A plug-in may log from its audio thread, where nothing may be
        // printed or allocated (real-time rule 3); its messages are
        // dropped until the log window exists.
    }
}

impl HostParamsImplShared for Shared {
    fn request_flush(&self) {
        self.flush.store(true, Relaxed);
    }
}

impl HostGuiImpl for Shared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, _new_size: GuiSize) -> Result<(), HostError> {
        // A floating window resizes itself; embedded windows are WP-2.10's.
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Err(HostError::Message("the editor opens from the device"))
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Err(HostError::Message("the editor closes from the device"))
    }

    fn closed(&self, _was_destroyed: bool) {
        self.editor_closed.store(true, Relaxed);
    }
}

/// What the main thread keeps for one plug-in instance.
#[derive(Default)]
pub(crate) struct MainThread {
    /// It said its latency changed.
    pub(crate) latency_changed: Cell<bool>,
    /// It asked the host to read its parameters again.
    pub(crate) rescan: Cell<bool>,
}

impl MainThreadHandler<'_> for MainThread {}

impl HostLatencyImpl for MainThread {
    fn changed(&self) {
        self.latency_changed.set(true);
    }
}

impl HostParamsImplMainThread for MainThread {
    fn rescan(&self, _flags: ParamRescanFlags) {
        self.rescan.set(true);
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostStateImpl for MainThread {
    fn mark_dirty(&self) {}
}

impl HostAudioPortsImpl for MainThread {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }

    fn rescan(&self, _flags: AudioPortRescanFlags) {}
}

/// What goes with an active plug-in to the audio thread, made on the main
/// thread when it's activated and dropped there when it's deactivated, so
/// the node never frees it.
pub(crate) struct ProcessorData {
    /// Channels in the main input and output ports, at most
    /// [`MAX_CHANNELS`](super::node::MAX_CHANNELS) each.
    pub(crate) inputs: usize,
    pub(crate) outputs: usize,
    /// Each parameter's default, by ID in order, for a change back to the
    /// default.
    pub(crate) defaults: Vec<(u32, f64)>,
    /// The plug-in's parameters extension, for flushing while switched off.
    pub(crate) params: Option<PluginParams>,
}

impl ProcessorData {
    /// The default of parameter `id`.
    pub(crate) fn default_of(&self, id: u32) -> Option<f64> {
        self.defaults
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|i| self.defaults[i].1)
    }
}

impl AudioProcessorHandler<'_> for ProcessorData {}
