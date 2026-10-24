//! One loan for every plug-in format (offline-plugin-renders R1, WP-4.21):
//! an offline render of a set that holds CLAP, VST3, Audio Unit and LV2
//! devices lends all four hosts' plug-ins at once.

use bl_engine::Registry;
use bl_model::{DeviceId, PluginFormat};

use crate::au::{self, AuHost};
use crate::clap::{self, ClapHost};
use crate::lv2::{self, Lv2Host};
use crate::vst3::{self, Vst3Host};

/// The plug-in hosts a render can borrow from; a format without a host
/// isn't borrowed, and its devices pass their input through.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hosts<'a> {
    pub clap: Option<&'a ClapHost>,
    pub vst3: Option<&'a Vst3Host>,
    pub au: Option<&'a AuHost>,
    pub lv2: Option<&'a Lv2Host>,
}

impl Hosts<'_> {
    /// Whether a render of a `format` device borrows from the engine's
    /// plug-ins: it has a host, and the platform hosts the format.
    #[must_use]
    pub fn borrows(&self, format: PluginFormat) -> bool {
        match format {
            PluginFormat::Clap => self.clap.is_some(),
            PluginFormat::Vst3 => self.vst3.is_some(),
            PluginFormat::AudioUnit => self.au.is_some() && cfg!(target_os = "macos"),
            PluginFormat::Lv2 => self.lv2.is_some(),
        }
    }

    /// Whether a render of a set that `has` devices of some format borrows
    /// anything.
    #[must_use]
    pub fn borrows_any(&self, has: impl Fn(PluginFormat) -> bool) -> bool {
        [
            PluginFormat::Clap,
            PluginFormat::Vst3,
            PluginFormat::AudioUnit,
            PluginFormat::Lv2,
        ]
        .into_iter()
        .any(|format| self.borrows(format) && has(format))
    }

    /// Lends the plug-ins of each format `has` says the set holds and the
    /// render borrows (R1). A set holding none borrows nothing and never
    /// touches a host.
    ///
    /// # Errors
    ///
    /// Why a host couldn't lend: it is lent to another render, or called
    /// off the main thread. Whatever was lent already goes back.
    pub fn lend(&self, has: impl Fn(PluginFormat) -> bool) -> Result<Loans, String> {
        let wants = |format| self.borrows(format) && has(format);
        let text = |error: &dyn std::error::Error| error.to_string();
        Ok(Loans {
            clap: self
                .clap
                .filter(|_| wants(PluginFormat::Clap))
                .map(ClapHost::lend)
                .transpose()
                .map_err(|e| text(&e))?,
            vst3: self
                .vst3
                .filter(|_| wants(PluginFormat::Vst3))
                .map(Vst3Host::lend)
                .transpose()
                .map_err(|e| text(&e))?,
            au: self
                .au
                .filter(|_| wants(PluginFormat::AudioUnit))
                .map(AuHost::lend)
                .transpose()
                .map_err(|e| text(&e))?,
            lv2: self
                .lv2
                .filter(|_| wants(PluginFormat::Lv2))
                .map(Lv2Host::lend)
                .transpose()
                .map_err(|e| text(&e))?,
        })
    }
}

/// The plug-ins lent to an offline render. Dropping it, on the main thread
/// and after the render's engine, gives every plug-in back to the engine
/// (R4).
#[derive(Debug)]
pub struct Loans {
    clap: Option<clap::Loan>,
    vst3: Option<vst3::Loan>,
    au: Option<au::Loan>,
    lv2: Option<lv2::Loan>,
}

impl Loans {
    /// Adds the loans' factories to the render's registry.
    pub fn register(&self, registry: &mut Registry) {
        if let Some(loan) = &self.clap {
            registry.add(loan.factory());
        }
        if let Some(loan) = &self.vst3 {
            registry.add(loan.factory());
        }
        if let Some(loan) = &self.au {
            registry.add(loan.factory());
        }
        if let Some(loan) = &self.lv2 {
            registry.add(loan.factory());
        }
    }

    /// The devices whose plug-in the render couldn't borrow, and why: the
    /// engine's node didn't hand it back in time (R2).
    #[must_use]
    pub fn problems(&self) -> Vec<(DeviceId, String)> {
        let mut problems = Vec::new();
        if let Some(loan) = &self.clap {
            problems.extend(loan.problems());
        }
        if let Some(loan) = &self.vst3 {
            problems.extend(loan.problems());
        }
        if let Some(loan) = &self.au {
            problems.extend(loan.problems());
        }
        if let Some(loan) = &self.lv2 {
            problems.extend(loan.problems());
        }
        problems
    }
}
