//! Modulation as `modulation.state` reports it (WP-5.9; the modulation
//! spec, R11): the parameters a track's clips modulate, what each played in
//! the last block, and the envelopes modulating it.

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_engine::clip_envelope_value_at;
use bl_model::modulation::Travel;
use bl_model::modulator::ModTarget;
use bl_model::{AutomationTarget, Document, EnvelopeKind, MixerParam, TrackId};
use bl_shell_spec::modulation::{
    ModulatedParameter, ModulationSource, ModulationState, ModulatorSource,
};

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
        // A MIDI controller sends messages; it modulates no parameter.
        AutomationTarget::MidiCc { .. }
        | AutomationTarget::MidiPitchBend
        | AutomationTarget::MidiChannelPressure => return None,
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
        AutomationTarget::Chain { chain, param } => {
            let mixer = &track.find_chain(chain)?.mixer;
            match param {
                MixerParam::Volume => (Travel::Fader, mixer.volume),
                MixerParam::Pan => (Travel::Pan, mixer.pan),
                MixerParam::Mute => (Travel::Switch, f64::from(u8::from(!mixer.enabled))),
            }
        }
        AutomationTarget::Device { device, param } => {
            let device = track.find_device(device)?;
            let description = devices.description(&device.kind)?.param(param)?;
            let value = device
                .params
                .get(&param)
                .copied()
                .unwrap_or(description.default);
            (Travel::Param(description), value)
        }
    })
}

/// The value of the control `target` names on `track`, in its own units: the
/// mixer's, the send's or the device's, or a default parameter's default.
pub(crate) fn control_value(
    document: &Document,
    devices: &DeviceView,
    track: TrackId,
    target: &AutomationTarget,
) -> Option<f64> {
    travel_of(document, devices, track, target).map(|(_, control)| control)
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
    for owner in document
        .tracks()
        .filter(|t| track.is_none_or(|id| t.id() == id))
    {
        let mut targets: BTreeMap<AutomationTarget, (Vec<ModulationSource>, Vec<ModulatorSource>)> =
            BTreeMap::new();
        for clip in owner.clips() {
            // Only the Modulation envelopes modulate (WP-3.12).
            for envelope in clip
                .envelopes()
                .filter(|envelope| envelope.kind == EnvelopeKind::Modulation)
            {
                let Some((travel, control)) =
                    travel_of(document, devices, owner.id(), &envelope.target)
                else {
                    continue;
                };
                let (base, _) = played(owner.id(), &envelope.target).unwrap_or((control, control));
                let share = clip_envelope_value_at(clip, envelope, at);
                targets
                    .entry(envelope.target)
                    .or_default()
                    .0
                    .push(ModulationSource {
                        clip: clip.id,
                        share,
                        contribution: share
                            .map_or(0.0, |share| travel.clip_contribution(base, share)),
                    });
            }
        }
        // Spec: modulation R11, R16. The modulator slots that map this
        // track's parameters, from any track.
        for modulator in document.tracks().flat_map(|t| t.devices_deep()) {
            for mapping in &modulator.mappings {
                let (track, target) = match mapping.target {
                    ModTarget::Mixer { track, param } => {
                        (Some(track), AutomationTarget::Mixer { param })
                    }
                    ModTarget::Send { track, to } => (Some(track), AutomationTarget::Send { to }),
                    ModTarget::Device { device, param } => (
                        document.device_track(device),
                        AutomationTarget::Device { device, param },
                    ),
                };
                if track == Some(owner.id()) {
                    targets.entry(target).or_default().1.push(ModulatorSource {
                        device: modulator.id,
                        slot: mapping.slot,
                        remote_control: document.in_remote_control(modulator),
                        contribution: None,
                    });
                }
            }
        }
        for (target, (sources, mut modulators)) in targets {
            // A mapping may name a parameter its device doesn't describe.
            let Some((travel, control)) = travel_of(document, devices, owner.id(), &target) else {
                continue;
            };
            let (base, played) = played(owner.id(), &target).unwrap_or((control, control));
            let held_by = modulators
                .iter()
                .find(|m| m.remote_control)
                .map(|m| m.device);
            if let ([only], true) = (modulators.as_mut_slice(), sources.is_empty())
                && !only.remote_control
            {
                only.contribution = Some(travel.position(played) - travel.position(base));
            }
            parameters.push(ModulatedParameter {
                track: owner.id(),
                target,
                base,
                played,
                sources,
                modulators,
                held_by,
            });
        }
    }
    ModulationState { parameters }
}
