//! Modulation as `modulation.state` reports it (WP-5.9; the modulation
//! spec, R11): the parameters a track's clips modulate, what each played in
//! the last block, and the envelopes modulating it.

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_engine::clip_envelope_value_at;
use bl_model::modulation::Travel;
use bl_model::{AutomationTarget, Document, MixerParam, TrackId};
use bl_shell_spec::modulation::{ModulatedParameter, ModulationSource, ModulationState};

use crate::devices::DeviceView;

/// The travel of `target` on `track`, with its control's value: `None` for a
/// parameter the track doesn't have or whose device isn't described.
fn travel_of<'a>(
    document: &Document,
    devices: &'a DeviceView,
    track: TrackId,
    target: &AutomationTarget,
) -> Option<(Travel<'a>, f64)> {
    let track = document.track(track)?;
    Some(match *target {
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        } => (Travel::Fader, track.mixer().volume),
        AutomationTarget::Mixer {
            param: MixerParam::Pan,
        } => (Travel::Pan, track.mixer().pan),
        AutomationTarget::Mixer {
            param: MixerParam::Mute,
        } => (Travel::Switch, f64::from(u8::from(track.mixer().mute))),
        AutomationTarget::Send { to } => (Travel::Send, track.send(to)),
        AutomationTarget::Device { device, param } => {
            let device = track.devices().find(|d| d.id == device)?;
            let description = devices.description(&device.kind)?.param(param)?;
            let value = device.params.get(&param).copied().unwrap_or(description.default);
            (Travel::Param(description), value)
        }
    })
}

/// What `modulation.state` reports for `track`, or every track when `None`,
/// at play position `at`, with `played` giving what a parameter played in
/// the last block: its base and played value, in its units.
pub(crate) fn state(
    document: &Document,
    devices: &DeviceView,
    track: Option<TrackId>,
    at: Ticks,
    played: impl Fn(TrackId, &AutomationTarget) -> Option<(f64, f64)>,
) -> ModulationState {
    let mut parameters = Vec::new();
    for owner in document.tracks().filter(|t| track.is_none_or(|id| t.id() == id)) {
        let mut targets: BTreeMap<AutomationTarget, Vec<ModulationSource>> = BTreeMap::new();
        for clip in owner.clips() {
            for envelope in clip.envelopes() {
                let Some((travel, control)) = travel_of(document, devices, owner.id(), &envelope.target)
                else {
                    continue;
                };
                let (base, _) = played(owner.id(), &envelope.target).unwrap_or((control, control));
                let share = clip_envelope_value_at(clip, envelope, at);
                targets.entry(envelope.target).or_default().push(ModulationSource {
                    clip: clip.id,
                    share,
                    contribution: share.map_or(0.0, |share| travel.clip_contribution(base, share)),
                });
            }
        }
        for (target, sources) in targets {
            let (_, control) = travel_of(document, devices, owner.id(), &target)
                .expect("a target that has sources was found");
            let (base, played) = played(owner.id(), &target).unwrap_or((control, control));
            parameters.push(ModulatedParameter {
                track: owner.id(),
                target,
                base,
                played,
                sources,
            });
        }
    }
    ModulationState { parameters }
}
