//! A drum rack in the Device View (WP-5.6; the drum racks spec): its pad
//! view, the pads' switches, dropping on pads, swapping pads, the default
//! pad, hot-swapping a pad, Group to Drum Rack, and extracting chains.
//!
//! The view's own state (the selected pad, the pad hot-swap targets, the
//! held Preview buttons, the default pad) is never part of the set; the
//! edits go through the document's commands, each gesture one undo step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bl_model::{
    Chain, ChainId, Clip, ClipData, Command, ContentHash, Device, DeviceId, DeviceKind, DeviceRole,
    Document, Editor, MidiData, RackKind, Receive, TrackId, TrackKind,
};
use bl_project::preset::Preset;
use bl_shell_spec::devices::{ChainRow, Grouped, NewDevice};
use bl_shell_spec::pads::{
    DrumRow, ExtractChainArgs, Extracted, GroupToDrumRackArgs, MovePadsArgs, PADS_PER_ROW,
    PADS_SHOWN, PadArgs, PadDropArgs, PadDropped, PadFrame, PadHotSwapArgs, PadKind, PadRenameArgs,
    PadState, PadSwapArgs, PadSwitch, PadSwitchArgs, ReturnChoice, ReturnRow,
};

use super::{DeviceError, DeviceView, Outcome, new_device, preset_path};

/// The values and name a preset gives a device.
type PresetState = (
    bl_model::OrdMap<bl_model::ParamId, f64>,
    String,
    Option<Vec<bl_model::modulator::ShapePoint>>,
);

/// The notes the pad view can start at: the last group of sixteen.
const LAST_FIRST_PAD: u8 = 112;

/// An audio file a drop brings, imported into the set's media.
#[derive(Clone, Debug)]
pub struct PadSample {
    /// Its content hash.
    pub media: ContentHash,
    /// The file's name without its extension: what a new chain is named.
    pub name: String,
}

/// The file the default pad is kept in, in the User Library (drum racks
/// spec, R17).
fn default_pad_file(library: &Path) -> PathBuf {
    library
        .join("Defaults")
        .join("On Drum Rack")
        .join("Default Pad.json")
}

/// The name a General MIDI sound or a note gives a pad's status line.
fn hover_text(note: u8) -> String {
    let name = bl_model::note_name(note);
    match bl_model::general_midi_drum(note) {
        Some(sound) => format!("{name} ({note}): {sound}"),
        None => format!("{name} ({note})"),
    }
}

/// The chains of `rack` that receive `note`: the pad's chains.
fn pad_chains(rack: &Device, note: u8) -> Vec<&Chain> {
    rack.chains
        .iter()
        .filter(|c| !c.is_return && c.receive == Receive::Note(note))
        .collect()
}

/// Whether `chain` plays a sample: a device in it that plays samples holds
/// one.
fn plays_a_sample(chain: &Chain) -> bool {
    chain.devices.iter().any(|d| {
        matches!(
            &d.kind,
            DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
        ) && d.media.iter().any(Option::is_some)
    })
}

fn switch_of(states: impl Iterator<Item = bool>) -> PadSwitch {
    let (mut on, mut off) = (false, false);
    for state in states {
        if state {
            on = true;
        } else {
            off = true;
        }
    }
    match (on, off) {
        (true, true) => PadSwitch::Mixed,
        (true, false) => PadSwitch::On,
        _ => PadSwitch::Off,
    }
}

impl DeviceView {
    /// The drum rack a command acts on: `rack`, or the rack in focus.
    fn drum_rack<'a>(
        &self,
        doc: &'a Document,
        rack: Option<DeviceId>,
    ) -> Result<&'a Device, DeviceError> {
        let found = match rack {
            Some(id) => doc
                .device(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?,
            None => self
                .focus_rack(doc)
                .ok_or_else(|| DeviceError::Unavailable("select a drum rack first".to_owned()))?,
        };
        if found.rack_kind() == Some(RackKind::Drum) {
            Ok(found)
        } else {
            Err(DeviceError::Unavailable(format!(
                "{} isn't a drum rack",
                found.id
            )))
        }
    }

    /// The pad view of `rack` (drum racks spec, R11 to R13).
    pub(super) fn pad_frame(&self, rack: &Device) -> PadFrame {
        let first =
            self.views_of(rack.id).first_pad.min(LAST_FIRST_PAD) / PADS_PER_ROW * PADS_PER_ROW;
        let selected = self.pads_selected.get(&rack.id).copied();
        let hot_swap = self
            .hot_swap_pad
            .filter(|(id, _)| *id == rack.id)
            .map(|(_, note)| note);
        let pads = (first..first + PADS_SHOWN)
            .map(|note| {
                let chains = pad_chains(rack, note);
                let kind = match chains.len() {
                    0 => PadKind::Empty,
                    1 => PadKind::Single,
                    _ => PadKind::Multi,
                };
                PadState {
                    note,
                    name: bl_model::note_name(note),
                    hover: hover_text(note),
                    kind,
                    label: match kind {
                        PadKind::Empty => bl_model::note_name(note),
                        PadKind::Single => chains[0].name.clone(),
                        PadKind::Multi => "Multi".to_owned(),
                    },
                    mute: switch_of(chains.iter().map(|c| !c.mixer.enabled)),
                    solo: switch_of(chains.iter().map(|c| c.mixer.solo)),
                    previewing: chains.iter().any(|c| self.previewing.contains(&c.id)),
                    has_sample: kind == PadKind::Single && plays_a_sample(chains[0]),
                    selected: selected == Some(note),
                    chains: chains.iter().map(|c| c.id).collect(),
                }
            })
            .collect();
        let occupied = (0..128)
            .filter(|&note| !pad_chains(rack, note).is_empty())
            .collect();
        PadFrame {
            first,
            pads,
            occupied,
            selected,
            hot_swap,
        }
    }

    /// A drum chain's row (drum racks spec, R21).
    pub(super) fn drum_row(&self, rack: &Device, chain: &Chain) -> DrumRow {
        DrumRow {
            receive: chain.receive,
            receive_label: bl_model::receive_label(chain.receive),
            play: chain.play,
            play_label: bl_model::receive_label(Receive::Note(chain.play)),
            choke: chain.choke,
            sends: rack
                .chains
                .iter()
                .filter(|c| c.is_return)
                .map(|ret| chain.send(ret.id))
                .collect(),
            previewing: self.previewing.contains(&chain.id),
            all_notes: chain.receive == Receive::AllNotes,
        }
    }

    /// A return chain's output (drum racks spec, R8).
    pub(super) fn return_row(doc: &Document, chain: &Chain) -> ReturnRow {
        ReturnRow {
            audio_to: chain.audio_to,
            audio_to_label: chain.audio_to.map_or_else(
                || "Rack Output".to_owned(),
                |track| crate::routing::track_label(doc, track),
            ),
        }
    }

    /// The return tracks a return chain can send to, in order.
    pub(super) fn return_tracks(doc: &Document) -> Vec<ReturnChoice> {
        doc.returns()
            .map(|track| ReturnChoice {
                track,
                label: crate::routing::track_label(doc, track),
            })
            .collect()
    }

    /// Fills `row`'s drum fields.
    pub(super) fn fill_drum_row(
        &self,
        doc: &Document,
        rack: &Device,
        chain: &Chain,
        row: &mut ChainRow,
    ) {
        if rack.rack_kind() != Some(RackKind::Drum) {
            return;
        }
        if chain.is_return {
            row.output = Some(Self::return_row(doc, chain));
        } else {
            row.drum = Some(self.drum_row(rack, chain));
        }
    }

    /// `rack.move_pads`: moves the notes the pad view shows.
    pub(super) fn move_pads(
        &mut self,
        doc: &Document,
        args: MovePadsArgs,
    ) -> Result<(), DeviceError> {
        let rack = self.drum_rack(doc, args.rack)?.id;
        let views = self.views_of(rack);
        let moved = match args.first {
            Some(first) => i32::from(first),
            None => {
                i32::from(views.first_pad)
                    + args.groups.unwrap_or(0) * i32::from(PADS_SHOWN)
                    + args.rows.unwrap_or(0) * i32::from(PADS_PER_ROW)
            }
        };
        let first = moved.clamp(0, i32::from(LAST_FIRST_PAD)) as u8 / PADS_PER_ROW * PADS_PER_ROW;
        self.rack_views.insert(rack, views_with_first(views, first));
        Ok(())
    }

    /// `pad.select`: selects a pad and its chains.
    pub(super) fn select_pad(&mut self, doc: &Document, args: PadArgs) -> Result<(), DeviceError> {
        let rack = self.drum_rack(doc, args.rack)?;
        let chains: Vec<ChainId> = pad_chains(rack, args.note).iter().map(|c| c.id).collect();
        let siblings: Vec<ChainId> = rack.chains.iter().map(|c| c.id).collect();
        let id = rack.id;
        self.pads_selected.insert(id, args.note);
        self.selected = Some(id);
        if !chains.is_empty() {
            self.chains_selected.retain(|c| !siblings.contains(c));
            self.chain_anchor = chains.last().copied();
            self.chains_selected.extend(chains);
        }
        self.chain_list_focus = false;
        Ok(())
    }

    /// Makes the pad of a chain the selected pad when the chain is selected
    /// alone (drum racks spec, R13).
    pub(super) fn follow_chain_selection(&mut self, doc: &Document, chain: ChainId) {
        if let (Some(rack), Some(c)) = (doc.chain_rack(chain), doc.chain(chain))
            && rack.rack_kind() == Some(RackKind::Drum)
            && let Receive::Note(note) = c.receive
            && !c.is_return
        {
            self.pads_selected.insert(rack.id, note);
        }
    }

    /// `pad.set_mute` and `pad.set_solo`: a pad's chains' activators or
    /// Solo, as one undo step.
    pub(super) fn set_pad_switch(
        &mut self,
        editor: &mut Editor,
        args: PadSwitchArgs,
        solo: bool,
    ) -> Result<(), DeviceError> {
        let PadSwitchArgs { rack, note, on } = args;
        let doc = editor.document();
        let rack = self.drum_rack(doc, rack)?;
        let chains = pad_chains(rack, note);
        if chains.is_empty() {
            return Err(DeviceError::Unavailable(format!(
                "the pad on {} holds no chain",
                bl_model::note_name(note)
            )));
        }
        let state = switch_of(
            chains
                .iter()
                .map(|c| if solo { c.mixer.solo } else { !c.mixer.enabled }),
        );
        let on = on.unwrap_or(state != PadSwitch::On);
        let ids: Vec<ChainId> = chains.iter().map(|c| c.id).collect();
        let label = if solo {
            "Solo chain"
        } else {
            "Change \"Chain On\""
        };
        editor
            .transact(label, |editor| {
                ids.iter().try_for_each(|&chain| {
                    let command = if solo {
                        Command::SetChainSolo { chain, solo: on }
                    } else {
                        Command::SetChainEnabled {
                            chain,
                            enabled: !on,
                        }
                    };
                    editor.apply(command).map(|_| ())
                })
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// `pad.preview` and `chain.preview`: holds or lets go of Preview
    /// buttons. The shell tells the engine what `outcome.previews` lists.
    pub(super) fn preview(
        &mut self,
        doc: &Document,
        chains: &[ChainId],
        on: bool,
        outcome: &mut Outcome,
    ) -> Result<(), DeviceError> {
        for &chain in chains {
            let found = doc
                .chain(chain)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
            let drum = doc
                .chain_rack(chain)
                .is_some_and(|r| r.rack_kind() == Some(RackKind::Drum));
            if found.is_return || !drum {
                return Err(DeviceError::Unavailable(format!(
                    "{chain} isn't a drum chain"
                )));
            }
            let changed = if on {
                self.previewing.insert(chain)
            } else {
                self.previewing.remove(&chain)
            };
            if changed {
                outcome.previews.push((chain, on));
            }
        }
        Ok(())
    }

    /// The chains of the pad at `note` in `rack`.
    pub(super) fn pad_chain_ids(
        &self,
        doc: &Document,
        rack: Option<DeviceId>,
        note: u8,
    ) -> Result<Vec<ChainId>, DeviceError> {
        let rack = self.drum_rack(doc, rack)?;
        Ok(pad_chains(rack, note).iter().map(|c| c.id).collect())
    }

    /// `pad.rename`.
    pub(super) fn rename_pad(
        &mut self,
        editor: &mut Editor,
        args: PadRenameArgs,
    ) -> Result<(), DeviceError> {
        let ids = self.pad_chain_ids(editor.document(), args.rack, args.note)?;
        let [chain] = ids[..] else {
            return Err(DeviceError::Unavailable(
                "only a pad of one chain can be renamed".to_owned(),
            ));
        };
        editor
            .apply(Command::RenameChain {
                chain,
                name: args.name,
            })
            .map(|_| ())
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// `pad.save_default`: keeps the pad's chain as the default pad, in
    /// memory and in the User Library (drum racks spec, R17).
    pub(super) fn save_default_pad(
        &mut self,
        doc: &Document,
        args: PadArgs,
        user_library: Option<&Path>,
    ) -> Result<(), DeviceError> {
        let rack = self.drum_rack(doc, args.rack)?;
        let chains = pad_chains(rack, args.note);
        let [chain] = chains[..] else {
            return Err(DeviceError::Unavailable(
                "only a pad of one chain can be the default".to_owned(),
            ));
        };
        let chain = chain.clone();
        if let Some(library) = user_library {
            let path = default_pad_file(library);
            if let Some(folder) = path.parent() {
                let _ = std::fs::create_dir_all(folder);
            }
            if let Ok(json) = serde_json::to_vec_pretty(&chain) {
                let _ = std::fs::write(path, json);
            }
        }
        self.default_pad = Some(chain);
        Ok(())
    }

    /// The default pad: the one saved last, from memory or the User
    /// Library.
    fn default_pad(&self, user_library: Option<&Path>) -> Option<Chain> {
        self.default_pad.clone().or_else(|| {
            let bytes = std::fs::read(default_pad_file(user_library?)).ok()?;
            serde_json::from_slice(&bytes).ok()
        })
    }

    /// A new sampler holding `sample`.
    fn sampler(&self, editor: &mut Editor, sample: &PadSample) -> Device {
        let kind = DeviceKind::Native {
            device_type: bl_devices::sampler::SAMPLER.to_owned(),
        };
        let name = self
            .types
            .iter()
            .find(|t| t.kind == kind)
            .map_or("Snapshot", |t| t.name.as_str())
            .to_owned();
        let mut device = new_device(
            editor.new_id(),
            &bl_shell_spec::devices::AddDeviceArgs {
                device: NewDevice {
                    kind,
                    name,
                    role: DeviceRole::Instrument,
                    sidechain: false,
                },
                track: None,
                chain: None,
                index: None,
            },
        );
        device.media = vec![Some(sample.media)];
        device
    }

    /// Puts `sample` in `chain`, a chain not yet in the set: its first
    /// sample-playing instrument takes it, else a new sampler replaces the
    /// instrument.
    fn put_sample_in(&self, editor: &mut Editor, chain: &mut Chain, sample: &PadSample) {
        let playing = chain.devices.iter_mut().find(|d| {
            matches!(
                &d.kind,
                DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
            )
        });
        if let Some(device) = playing {
            if device.media.is_empty() {
                device.media.push(None);
            }
            device.media[0] = Some(sample.media);
            return;
        }
        let sampler = self.sampler(editor, sample);
        let at = chain
            .devices
            .iter()
            .position(|d| d.role == DeviceRole::Instrument);
        match at {
            Some(at) => chain.devices[at] = sampler,
            None => {
                let index = chain
                    .devices
                    .iter()
                    .take_while(|d| d.role == DeviceRole::MidiEffect)
                    .count();
                chain.devices.insert(index, sampler);
            }
        }
    }

    /// A chain for a sample dropped on an empty pad: the default pad's with
    /// the sample in its first sampler, or a new sampler's.
    fn new_pad_chain(
        &self,
        editor: &mut Editor,
        note: u8,
        sample: &PadSample,
        default: Option<&Chain>,
    ) -> Chain {
        let id: ChainId = editor.new_id();
        let mut chain = match default {
            Some(default) => {
                let mut chain = default.clone();
                chain.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
                chain.id = id;
                chain.receive = Receive::Note(note);
                chain.is_return = false;
                chain.sends.clear();
                chain.audio_to = None;
                chain.devices.iter_mut().for_each(Device::clear_mappings);
                chain
            }
            None => Chain::for_pad(id, "", note),
        };
        chain.name.clone_from(&sample.name);
        self.put_sample_in(editor, &mut chain, sample);
        chain
    }

    /// Where a chain of a drum rack goes in the rack's list: after its last
    /// drum chain, before the return chains.
    fn drum_chain_end(rack: &Device) -> usize {
        rack.chains.iter().take_while(|c| !c.is_return).count()
    }

    /// `pad.drop` (drum racks spec, R14 to R16): drops samples, a device or
    /// a preset on a pad. `samples` are the files the shell imported.
    #[expect(clippy::too_many_lines, reason = "one drop per kind of item")]
    pub(crate) fn pad_drop(
        &mut self,
        editor: &mut Editor,
        user_library: Option<&Path>,
        args: PadDropArgs,
        samples: &[PadSample],
    ) -> Result<PadDropped, DeviceError> {
        let PadDropArgs {
            rack,
            note,
            layer,
            device,
            preset,
            move_device,
            ..
        } = args;
        if note > 127 {
            return Err(DeviceError::InvalidArgs(format!(
                "{note} isn't a MIDI note"
            )));
        }
        let given = [
            !samples.is_empty(),
            device.is_some(),
            preset.is_some(),
            move_device.is_some(),
        ];
        if given.iter().filter(|g| **g).count() != 1 {
            return Err(DeviceError::InvalidArgs(
                "give one of `samples`, `device`, `preset` and `move_device`".to_owned(),
            ));
        }
        let doc = editor.document();
        let rack = self.drum_rack(doc, rack)?.clone();
        let default = self.default_pad(user_library);
        let mut chains = Vec::new();
        let mut notes = Vec::new();
        let result: Result<(), DeviceError> = if samples.is_empty() {
            // One device on one pad.
            let held = match (device, preset) {
                (Some(device), _) => {
                    let id: DeviceId = editor.new_id();
                    let new = new_device(
                        id,
                        &bl_shell_spec::devices::AddDeviceArgs {
                            device,
                            track: None,
                            chain: None,
                            index: None,
                        },
                    );
                    Some((new, None))
                }
                (None, Some(path)) => {
                    let path = preset_path(&path)?;
                    let loaded =
                        Preset::load(&path).map_err(|e| DeviceError::Failed(e.to_string()))?;
                    let id: DeviceId = editor.new_id();
                    let new = Device::new(id, loaded.device_name.clone(), loaded.kind.clone())
                        .with_role(loaded.role);
                    let params = self.sanitize(&loaded.kind, loaded.params);
                    Some((
                        new,
                        Some((params, bl_project::preset::preset_name(&path), loaded.shape)),
                    ))
                }
                (None, None) => None,
            };
            self.drop_device(editor, &rack, note, held, move_device, &mut chains)?;
            notes.push(note);
            Ok(())
        } else if layer && samples.len() > 1 {
            self.drop_layered(editor, &rack, note, samples, default.as_ref(), &mut chains)?;
            notes.push(note);
            Ok(())
        } else {
            let plan: Vec<(u8, &PadSample)> = samples
                .iter()
                .enumerate()
                .map(|(n, s)| (u32::from(note) + n as u32, s))
                .take_while(|(at, _)| *at <= 127)
                .map(|(at, s)| (at as u8, s))
                .collect();
            editor.transact("Insert Chain", |editor| {
                for &(at, sample) in &plan {
                    self.drop_sample(editor, &rack, at, sample, default.as_ref(), &mut chains)?;
                    notes.push(at);
                }
                Ok::<_, DeviceError>(())
            })
        };
        result?;
        let first = chains.first().copied();
        if let Some(first) = first {
            self.pads_selected
                .insert(rack.id, notes.first().copied().unwrap_or(note));
            self.select_new_chains(&[first]);
            self.chain_list_focus = false;
        }
        Ok(PadDropped { chains, notes })
    }

    /// One sample on one pad (R14), inside a transaction.
    fn drop_sample(
        &self,
        editor: &mut Editor,
        rack: &Device,
        note: u8,
        sample: &PadSample,
        default: Option<&Chain>,
        made: &mut Vec<ChainId>,
    ) -> Result<(), DeviceError> {
        let existing = pad_chains(rack, note);
        match existing[..] {
            [] => {
                let chain = self.new_pad_chain(editor, note, sample, default);
                made.push(chain.id);
                let index = Self::drum_chain_end(editor.document().device(rack.id).unwrap_or(rack));
                editor
                    .apply(Command::AddChain {
                        rack: rack.id,
                        index,
                        chain,
                    })
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
            }
            [chain] => {
                made.push(chain.id);
                let mut copy = chain.clone();
                self.put_sample_in(editor, &mut copy, sample);
                // Only the instrument and its sample change: the chain's
                // effects stay where they are.
                let before = chain
                    .devices
                    .iter()
                    .position(|d| d.role == DeviceRole::Instrument);
                match (before, &copy.devices) {
                    (Some(at), devices)
                        if devices
                            .get(at)
                            .is_some_and(|d| d.id == chain.devices[at].id) =>
                    {
                        // The same sampler takes the sample.
                        let slot = self.slot_of(&devices[at]) as u8;
                        editor
                            .apply(Command::SetDeviceSample {
                                device: devices[at].id,
                                media: Some(sample.media),
                                slot,
                            })
                            .map_err(|e| DeviceError::Failed(e.to_string()))?;
                    }
                    _ => {
                        // A new sampler replaces the instrument.
                        let sampler = copy
                            .devices
                            .iter()
                            .find(|d| !chain.devices.iter().any(|o| o.id == d.id))
                            .cloned()
                            .ok_or_else(|| DeviceError::Failed("no sampler".to_owned()))?;
                        let index = match before {
                            Some(at) => {
                                editor
                                    .apply(Command::RemoveDevice {
                                        device: chain.devices[at].id,
                                    })
                                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                                at
                            }
                            None => chain
                                .devices
                                .iter()
                                .take_while(|d| d.role == DeviceRole::MidiEffect)
                                .count(),
                        };
                        editor
                            .apply(Command::InsertChainDevice {
                                chain: chain.id,
                                index,
                                device: sampler,
                            })
                            .map_err(|e| DeviceError::Failed(e.to_string()))?;
                    }
                }
            }
            _ => {
                return Err(DeviceError::Unavailable(
                    "a pad of several chains takes no drop".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Several samples layered on one pad (R15): one chain holding an
    /// instrument rack with a chain per sample.
    fn drop_layered(
        &self,
        editor: &mut Editor,
        rack: &Device,
        note: u8,
        samples: &[PadSample],
        default: Option<&Chain>,
        made: &mut Vec<ChainId>,
    ) -> Result<(), DeviceError> {
        let inner_id: DeviceId = editor.new_id();
        let mut inner = Device::rack(inner_id, RackKind::Instrument);
        for sample in samples {
            let id: ChainId = editor.new_id();
            let mut chain = Chain::new(id, sample.name.clone());
            let sampler = self.sampler(editor, sample);
            chain.devices.push(sampler);
            inner.chains.push(chain);
        }
        let existing = pad_chains(rack, note);
        match existing[..] {
            [] => {
                let id: ChainId = editor.new_id();
                let mut chain = match default {
                    Some(default) => {
                        let mut chain = default.clone();
                        chain.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
                        chain.id = id;
                        chain.receive = Receive::Note(note);
                        chain.is_return = false;
                        chain.sends.clear();
                        chain.audio_to = None;
                        chain.devices.retain(|d| d.role != DeviceRole::Instrument);
                        chain
                    }
                    None => Chain::for_pad(id, "", note),
                };
                chain.name = "Layers".to_owned();
                let index = chain
                    .devices
                    .iter()
                    .take_while(|d| d.role == DeviceRole::MidiEffect)
                    .count();
                chain.devices.insert(index, inner);
                made.push(chain.id);
                let at = Self::drum_chain_end(rack);
                editor
                    .transact("Insert Chain", |editor| {
                        editor
                            .apply(Command::AddChain {
                                rack: rack.id,
                                index: at,
                                chain,
                            })
                            .map(|_| ())
                    })
                    .map_err(|e| DeviceError::Failed(e.to_string()))
            }
            [chain] => {
                made.push(chain.id);
                let before = chain
                    .devices
                    .iter()
                    .position(|d| d.role == DeviceRole::Instrument);
                editor
                    .transact("Insert Device", |editor| {
                        let index = match before {
                            Some(at) => {
                                editor.apply(Command::RemoveDevice {
                                    device: chain.devices[at].id,
                                })?;
                                at
                            }
                            None => chain
                                .devices
                                .iter()
                                .take_while(|d| d.role == DeviceRole::MidiEffect)
                                .count(),
                        };
                        editor
                            .apply(Command::InsertChainDevice {
                                chain: chain.id,
                                index,
                                device: inner,
                            })
                            .map(|_| ())
                    })
                    .map_err(|e| DeviceError::Failed(e.to_string()))
            }
            _ => Err(DeviceError::Unavailable(
                "a pad of several chains takes no drop".to_owned(),
            )),
        }
    }

    /// A device, a preset or a device moved from elsewhere on one pad (R14).
    fn drop_device(
        &mut self,
        editor: &mut Editor,
        rack: &Device,
        note: u8,
        held: Option<(Device, Option<PresetState>)>,
        moved: Option<DeviceId>,
        made: &mut Vec<ChainId>,
    ) -> Result<(), DeviceError> {
        let doc = editor.document();
        let (role, title, id) = match (&held, moved) {
            (Some((device, _)), _) => (device.role, device.title().to_owned(), device.id),
            (None, Some(moved)) => {
                let device = doc
                    .device(moved)
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("no {moved}")))?;
                (device.role, device.title().to_owned(), device.id)
            }
            (None, None) => {
                return Err(DeviceError::InvalidArgs("nothing to drop".to_owned()));
            }
        };
        let existing = pad_chains(rack, note);
        let target: Option<Chain> = match existing[..] {
            [] => None,
            [chain] => Some(chain.clone()),
            _ => {
                return Err(DeviceError::Unavailable(
                    "a pad of several chains takes no drop".to_owned(),
                ));
            }
        };
        let label = if target.is_some() {
            "Insert Device"
        } else {
            "Insert Chain"
        };
        let chain_id: ChainId = target.as_ref().map_or_else(|| editor.new_id(), |c| c.id);
        let state = held.as_ref().and_then(|(_, state)| state.clone());
        let device = held.map(|(device, _)| device);
        editor
            .transact(label, |editor| {
                let mut index = 0;
                if let Some(chain) = &target {
                    // An instrument takes the instrument's place; a MIDI
                    // effect goes after the MIDI effects, an audio effect
                    // at the end.
                    let midi_effects = chain
                        .devices
                        .iter()
                        .take_while(|d| d.role == DeviceRole::MidiEffect)
                        .count();
                    index = match role {
                        DeviceRole::AudioEffect => chain.devices.len(),
                        DeviceRole::MidiEffect => midi_effects,
                        DeviceRole::Instrument => {
                            match chain
                                .devices
                                .iter()
                                .position(|d| d.role == DeviceRole::Instrument)
                            {
                                Some(at) => {
                                    editor.apply(Command::RemoveDevice {
                                        device: chain.devices[at].id,
                                    })?;
                                    at
                                }
                                None => midi_effects,
                            }
                        }
                    };
                } else {
                    editor.apply(Command::AddChain {
                        rack: rack.id,
                        index: Self::drum_chain_end(rack),
                        chain: Chain::for_pad(chain_id, title.clone(), note),
                    })?;
                }
                match device {
                    Some(device) => {
                        editor.apply(Command::InsertChainDevice {
                            chain: chain_id,
                            index,
                            device,
                        })?;
                        if let Some((params, name, shape)) = state {
                            editor.apply(Command::SetDeviceState {
                                device: id,
                                params,
                                name,
                                shape,
                            })?;
                        }
                    }
                    None => {
                        editor.apply(Command::MoveDeviceToChain {
                            device: id,
                            chain: chain_id,
                            index,
                        })?;
                    }
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        made.push(chain_id);
        Ok(())
    }

    /// `pad.swap` (drum racks spec, R16).
    pub(super) fn swap_pads(
        &mut self,
        editor: &mut Editor,
        args: PadSwapArgs,
    ) -> Result<(), DeviceError> {
        let PadSwapArgs {
            rack,
            from,
            to,
            layer,
        } = args;
        if from > 127 || to > 127 {
            return Err(DeviceError::InvalidArgs("a pad is a MIDI note".to_owned()));
        }
        let doc = editor.document();
        let rack = self.drum_rack(doc, rack)?.clone();
        let a: Vec<ChainId> = pad_chains(&rack, from).iter().map(|c| c.id).collect();
        let b: Vec<ChainId> = pad_chains(&rack, to).iter().map(|c| c.id).collect();
        if a.is_empty() {
            return Err(DeviceError::Unavailable(format!(
                "the pad on {} holds no chain",
                bl_model::note_name(from)
            )));
        }
        if from == to {
            return Ok(());
        }
        if layer && !b.is_empty() {
            let inner: DeviceId = editor.new_id();
            let outer: ChainId = editor.new_id();
            let name = pad_chains(&rack, to)[0].name.clone();
            let index = Self::drum_chain_end(&rack);
            let moving: Vec<ChainId> = b.iter().chain(&a).copied().collect();
            editor
                .transact("Move Chain", |editor| {
                    editor.apply(Command::AddChain {
                        rack: rack.id,
                        index,
                        chain: Chain::for_pad(outer, name, to)
                            .with_devices(vec![Device::rack(inner, RackKind::Instrument)]),
                    })?;
                    for chain in moving {
                        let end = editor
                            .document()
                            .device(inner)
                            .map_or(0, |d| d.chains.len());
                        editor.apply(Command::MoveChain {
                            chain,
                            rack: inner,
                            index: end,
                        })?;
                    }
                    Ok::<_, bl_model::EditError>(())
                })
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
            self.pads_selected.insert(rack.id, to);
            return Ok(());
        }
        editor
            .transact("Move Chain", |editor| {
                for chain in &a {
                    editor.apply(Command::SetChainReceive {
                        chain: *chain,
                        receive: Receive::Note(to),
                    })?;
                }
                for chain in &b {
                    editor.apply(Command::SetChainReceive {
                        chain: *chain,
                        receive: Receive::Note(from),
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        if self.pads_selected.get(&rack.id) == Some(&from) {
            self.pads_selected.insert(rack.id, to);
        }
        Ok(())
    }

    /// `pad.hot_swap`: makes a pad the hot-swap target, or ends it.
    pub(super) fn hot_swap_pad(
        &mut self,
        doc: &Document,
        args: PadHotSwapArgs,
    ) -> Result<(), DeviceError> {
        let rack = self.drum_rack(doc, args.rack)?;
        if pad_chains(rack, args.note).len() > 1 {
            return Err(DeviceError::Unavailable(
                "a pad of several chains can't be hot-swapped".to_owned(),
            ));
        }
        let id = rack.id;
        let target = Some((id, args.note));
        if args.on.unwrap_or(self.hot_swap_pad != target) {
            self.hot_swap_pad = target;
            self.hot_swap = Some(id);
            self.hot_swap_chain = None;
            self.pads_selected.insert(id, args.note);
        } else if self.hot_swap_pad == target {
            self.hot_swap_pad = None;
            self.hot_swap = None;
        }
        Ok(())
    }

    /// `rack.switch_hot_swap_target` (S-VIEW-007): D.
    pub(super) fn switch_hot_swap_target(&mut self, doc: &Document) -> Result<(), DeviceError> {
        if let Some((rack, _)) = self.hot_swap_pad {
            self.hot_swap_pad = None;
            self.hot_swap = Some(rack);
            return Ok(());
        }
        let rack = self
            .hot_swap
            .and_then(|d| doc.device(d))
            .filter(|d| d.rack_kind() == Some(RackKind::Drum))
            .ok_or_else(|| {
                DeviceError::Unavailable("no drum rack is being hot-swapped".to_owned())
            })?
            .id;
        let note =
            self.pads_selected.get(&rack).copied().ok_or_else(|| {
                DeviceError::Unavailable("select a pad of the rack first".to_owned())
            })?;
        self.hot_swap_pad = Some((rack, note));
        Ok(())
    }

    /// The pad hot-swap targets: its drum rack and note.
    #[must_use]
    pub fn hot_swap_pad_target(&self) -> Option<(DeviceId, u8)> {
        self.hot_swap_pad
    }

    /// The sample the pad at `note` of `rack` (the rack in focus if left
    /// out) plays, if its one chain plays one: the media of its first
    /// sample-playing device.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] if there is no drum rack.
    pub fn pad_sample_of(
        &self,
        doc: &Document,
        rack: Option<DeviceId>,
        note: u8,
    ) -> Result<Option<ContentHash>, DeviceError> {
        let rack = self.drum_rack(doc, rack)?;
        let chains = pad_chains(rack, note);
        let [chain] = chains[..] else {
            return Ok(None);
        };
        Ok(chain
            .devices
            .iter()
            .filter(|d| {
                matches!(
                    &d.kind,
                    DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
                )
            })
            .find_map(|d| d.media.iter().flatten().next().copied()))
    }

    /// Which chains of drum racks the engine says notes reached since the
    /// last call, with Auto Select on in their rack: each rack selects the
    /// last of them (drum racks spec, R20). Returns whether anything
    /// changed.
    pub fn auto_select(&mut self, doc: &Document, counts: &BTreeMap<ChainId, u64>) -> bool {
        let mut changed = false;
        let seen = std::mem::take(&mut self.hit_counts);
        for track in doc.tracks() {
            for rack in track
                .devices_deep()
                .filter(|d| d.rack_kind() == Some(RackKind::Drum))
            {
                if !self.views_of(rack.id).auto_select {
                    continue;
                }
                let hit = rack
                    .chains
                    .iter()
                    .filter(|c| !c.is_return)
                    .filter(|c| {
                        let now = counts.get(&c.id).copied().unwrap_or(0);
                        seen.get(&c.id).is_some_and(|before| now > *before)
                    })
                    .map(|c| c.id)
                    .next_back();
                if let Some(chain) = hit {
                    let siblings: Vec<ChainId> = rack.chains.iter().map(|c| c.id).collect();
                    self.chains_selected.retain(|c| !siblings.contains(c));
                    self.chains_selected.insert(chain);
                    self.follow_chain_selection(doc, chain);
                    changed = true;
                }
            }
        }
        self.hit_counts.clone_from(counts);
        changed
    }

    /// `device.group_to_drum_rack` (drum racks spec, R25).
    pub(super) fn group_to_drum_rack(
        &mut self,
        editor: &mut Editor,
        args: GroupToDrumRackArgs,
    ) -> Result<Grouped, DeviceError> {
        let doc = editor.document();
        let devices = match args.devices {
            Some(devices) if !devices.is_empty() => devices,
            _ => vec![
                self.selected
                    .ok_or_else(|| DeviceError::Unavailable("select a device first".to_owned()))?,
            ],
        };
        let mut placed = Vec::with_capacity(devices.len());
        for &id in &devices {
            let device = doc
                .device(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            let place = doc
                .device_place(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            placed.push((place.index, device.title().to_owned()));
        }
        placed.sort();
        let rack: DeviceId = editor.new_id();
        let chain: ChainId = editor.new_id();
        let first = placed[0].1.clone();
        editor
            .apply(Command::GroupDevices {
                devices,
                rack: Device::rack(rack, RackKind::Drum).with_chain(Chain::for_pad(
                    chain,
                    first,
                    bl_model::FIRST_PAD,
                )),
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(rack);
        Ok(Grouped { rack, chain })
    }

    /// `device.group` with several chains of a drum rack selected (drum
    /// racks spec, R26): one drum chain holding an instrument rack of them,
    /// on the first one's Receive, Play and Choke.
    pub(super) fn group_drum_chains(
        &mut self,
        editor: &mut Editor,
        rack: &Device,
        chains: &[ChainId],
    ) -> Result<Grouped, DeviceError> {
        let mut picked: Vec<&Chain> = rack
            .chains
            .iter()
            .filter(|c| chains.contains(&c.id))
            .collect();
        if picked.iter().any(|c| c.is_return) {
            return Err(DeviceError::Unavailable(
                "return chains can't be grouped".to_owned(),
            ));
        }
        picked.dedup_by_key(|c| c.id);
        let first = picked
            .first()
            .ok_or_else(|| DeviceError::InvalidArgs("there are no chains to group".to_owned()))?;
        let (inner, outer): (DeviceId, ChainId) = (editor.new_id(), editor.new_id());
        let mut new = Chain::new(outer, "Chain")
            .with_devices(vec![Device::rack(inner, RackKind::Instrument)]);
        new.receive = first.receive;
        new.play = first.play;
        new.choke = first.choke;
        let index = rack
            .chains
            .iter()
            .position(|c| c.id == first.id)
            .unwrap_or(0);
        let moving: Vec<ChainId> = picked.iter().map(|c| c.id).collect();
        editor
            .transact("Group", |editor| {
                editor.apply(Command::AddChain {
                    rack: rack.id,
                    index,
                    chain: new,
                })?;
                for chain in moving {
                    let end = editor
                        .document()
                        .device(inner)
                        .map_or(0, |d| d.chains.len());
                    editor.apply(Command::MoveChain {
                        chain,
                        rack: inner,
                        index: end,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(inner);
        Ok(Grouped {
            rack: inner,
            chain: outer,
        })
    }

    /// `chain.extract` (drum racks spec, R23): a drum chain leaves its rack
    /// for a new MIDI track, with its notes.
    pub(super) fn extract_chain(
        &mut self,
        editor: &mut Editor,
        args: ExtractChainArgs,
    ) -> Result<Extracted, DeviceError> {
        let doc = editor.document();
        let chain = doc
            .chain(args.chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {}", args.chain)))?
            .clone();
        let rack = doc
            .chain_rack(args.chain)
            .filter(|r| r.rack_kind() == Some(RackKind::Drum))
            .ok_or_else(|| DeviceError::Unavailable("not a drum rack's chain".to_owned()))?;
        if chain.is_return {
            return Err(DeviceError::Unavailable(
                "use chain.extract_return for a return chain".to_owned(),
            ));
        }
        let rack_id = rack.id;
        let source = doc
            .device_track(rack_id)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no track holds {rack_id}")))?;
        let track = doc
            .track(source)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {source}")))?;
        // The notes the chain received, clip by clip: Arrangement clips and
        // Session slots alike.
        let answers = |key: u8| chain.receive.answers(key);
        let mut notes_moved: Vec<(Option<bl_model::SceneId>, Clip, Vec<bl_model::Note>)> =
            Vec::new();
        let lanes = track
            .clips()
            .map(|clip| (None, clip))
            .chain(track.slots().map(|(scene, clip)| (Some(scene), clip)));
        for (scene, clip) in lanes {
            let ClipData::Midi(midi) = &clip.data else {
                continue;
            };
            let taken: Vec<bl_model::Note> = midi
                .notes
                .iter()
                .filter(|n| answers(n.key))
                .cloned()
                .collect();
            if !taken.is_empty() {
                notes_moved.push((scene, clip.clone(), taken));
            }
        }
        let top = doc.top_level().len();
        let new_track: TrackId = editor.new_id();
        let devices: Vec<DeviceId> = chain.devices.iter().map(|d| d.id).collect();
        let play = chain.play;
        let receive_all = chain.receive == Receive::AllNotes;
        let clip_ids: Vec<bl_model::ClipId> = notes_moved.iter().map(|_| editor.new_id()).collect();
        let mut clips = 0;
        editor
            .transact("Move Chain", |editor| {
                editor.apply(Command::CreateTrack {
                    id: new_track,
                    kind: TrackKind::Midi,
                    name: format!("{} {}", top + 1, chain.name),
                    parent: None,
                    index: top,
                })?;
                for (n, device) in devices.iter().enumerate() {
                    editor.apply(Command::MoveDevice {
                        device: *device,
                        track: new_track,
                        index: n,
                    })?;
                }
                for ((scene, clip, taken), id) in notes_moved.iter().zip(&clip_ids) {
                    // The notes leave the source clip...
                    let ClipData::Midi(midi) = &clip.data else {
                        continue;
                    };
                    let mut left = midi.clone();
                    left.notes = midi
                        .notes
                        .iter()
                        .filter(|n| !answers(n.key))
                        .cloned()
                        .collect();
                    editor.apply(Command::SetClipData {
                        clip: clip.id,
                        data: ClipData::Midi(left),
                    })?;
                    // ...for a clip of the same place on the new track,
                    // at the pitch the chain's devices heard in the rack.
                    let mut copy = clip.clone();
                    copy.id = *id;
                    copy.envelopes.clear();
                    let mut data: MidiData = midi.clone();
                    data.notes = taken
                        .iter()
                        .cloned()
                        .map(|mut n| {
                            if !receive_all {
                                n.key = play;
                            }
                            n
                        })
                        .collect();
                    copy.data = ClipData::Midi(data);
                    match scene {
                        None => editor.apply(Command::CreateClip {
                            track: new_track,
                            clip: copy,
                        })?,
                        Some(scene) => editor.apply(Command::CreateSlotClip {
                            track: new_track,
                            scene: *scene,
                            clip: copy,
                        })?,
                    };
                    clips += 1;
                }
                editor.apply(Command::RemoveChain { chain: chain.id })?;
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(Extracted {
            track: new_track,
            devices,
            clips,
        })
    }

    /// `chain.extract_return` (drum racks spec, R24): a return chain leaves
    /// its rack for a new return track after the last one.
    pub(super) fn extract_return(
        &mut self,
        editor: &mut Editor,
        args: ExtractChainArgs,
    ) -> Result<Extracted, DeviceError> {
        let doc = editor.document();
        let chain = doc
            .chain(args.chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {}", args.chain)))?
            .clone();
        if !chain.is_return
            || doc
                .chain_rack(args.chain)
                .is_none_or(|r| r.rack_kind() != Some(RackKind::Drum))
        {
            return Err(DeviceError::Unavailable(
                "not a drum rack's return chain".to_owned(),
            ));
        }
        let count = doc.returns().count();
        let new_track: TrackId = editor.new_id();
        let letter = crate::routing::return_letter(count);
        let devices: Vec<DeviceId> = chain.devices.iter().map(|d| d.id).collect();
        editor
            .transact("Move Chain", |editor| {
                editor.apply(Command::CreateTrack {
                    id: new_track,
                    kind: TrackKind::Return,
                    name: format!("{letter} {}", chain.name),
                    parent: None,
                    index: count,
                })?;
                for (n, device) in devices.iter().enumerate() {
                    editor.apply(Command::MoveDevice {
                        device: *device,
                        track: new_track,
                        index: n,
                    })?;
                }
                editor.apply(Command::RemoveChain { chain: chain.id })?;
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(Extracted {
            track: new_track,
            devices,
            clips: 0,
        })
    }
}

/// `views` showing the pad view from `first`.
fn views_with_first(mut views: bl_model::RackViews, first: u8) -> bl_model::RackViews {
    views.first_pad = first;
    views
}
