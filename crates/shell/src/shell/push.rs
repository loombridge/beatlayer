//! Push 2 for agents in the shell (`docs/specs/push-2.md`, R34): reads each
//! running Push 2's surface through its handle and sets its modes and its
//! Setup values.

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_commands::NoArgs;
use bl_control::RunState;
use bl_push::content::{DisplayMode, MixPage, PadMode, StripUse};
use bl_push::curve::{Setup, Workflow};
use bl_push::leds::Led;
use bl_push::sysex::MidiMode;
use bl_push::{Command, Handle, Snapshot};
use bl_shell_spec::push as ids;
use serde_json::Value;

use super::{Shell, ShellError, to_value};

fn setup_info(setup: Setup) -> ids::PushSetup {
    ids::PushSetup {
        pad_sensitivity: setup.sensitivity,
        pad_gain: setup.gain,
        pad_dynamics: setup.dynamics,
        display_light: setup.display_light,
        led_brightness: setup.led_brightness,
        workflow: match setup.workflow {
            Workflow::Scene => ids::PushWorkflow::Scene,
            Workflow::Clip => ids::PushWorkflow::Clip,
        },
    }
}

fn fixed_info(fixed: bl_push::recording::FixedLength) -> ids::PushFixedLength {
    ids::PushFixedLength {
        on: fixed.on,
        length: ids::PushFixedLengthValue::ALL
            [usize::from(fixed.length).min(ids::PushFixedLengthValue::ALL.len() - 1)],
        phrase_sync: fixed.phrase_sync,
    }
}

fn led_info(led: Led) -> ids::PushLed {
    ids::PushLed {
        colour: led.colour,
        animation: led
            .animation
            .map(|(colour, channel)| ids::PushLedAnimation {
                colour,
                channel,
                kind: match channel {
                    1..=5 => "one_shot",
                    6..=10 => "pulsing",
                    _ => "blinking",
                }
                .to_owned(),
            }),
    }
}

/// Every LED of a snapshot by control name: the 64 pads and the buttons.
fn leds(snapshot: &Snapshot) -> BTreeMap<String, ids::PushLed> {
    let mut leds = BTreeMap::new();
    for button in bl_push::controls::BUTTONS {
        if let Some(led) = snapshot.led(button.name) {
            leds.insert(button.name.to_owned(), led_info(led));
        }
    }
    for row in 0..8 {
        for column in 0..8 {
            let name = format!("pad_{row}_{column}");
            if let Some(led) = snapshot.led(&name) {
                leds.insert(name, led_info(led));
            }
        }
    }
    leds
}

/// What Mix Mode shows (push-2-tracks R42).
fn mix_info(mix: &bl_push::mix::State) -> ids::PushMix {
    use bl_push::mix::{Global, View};
    ids::PushMix {
        view: match mix.view {
            View::Mix => "mix",
            View::InOut => "in_out",
        }
        .to_owned(),
        global: match mix.global {
            Global::Volumes => "volumes".to_owned(),
            Global::Pans => "pans".to_owned(),
            Global::Send(n) => format!("send_{}", bl_model::return_letter(n).to_lowercase()),
        },
        sends_shift: mix.shift,
        places: mix
            .places
            .iter()
            .map(|place| ids::PushMixPlace {
                name: place.name.clone(),
                track: place.track,
                chain: place.chain,
                selected: place.selected,
            })
            .collect(),
        encoders: mix
            .encoders
            .iter()
            .map(|control| {
                control.as_ref().map(|control| ids::PushMixControl {
                    name: control.name.clone(),
                    value: control.value.clone(),
                    position: control.position,
                    inert: control.inert,
                })
            })
            .collect(),
        unfolded_groups: mix.unfolded_groups.clone(),
        unfolded_racks: mix.unfolded_racks.clone(),
    }
}

fn info(row: usize, input: String, output: String, snapshot: Snapshot) -> ids::PushInfo {
    let mix = (snapshot.display_mode == DisplayMode::Mix).then_some(match snapshot.mix_page {
        MixPage::Track => ids::PushMixPage::Track,
        MixPage::Global => ids::PushMixPage::Global,
    });
    ids::PushInfo {
        row: u8::try_from(row + 1).unwrap_or(u8::MAX),
        input,
        output,
        firmware: snapshot.firmware.clone(),
        external_power: snapshot.external_power,
        error: snapshot.error.clone(),
        midi_mode: match snapshot.midi_mode {
            MidiMode::Live => ids::PushMidiMode::Live,
            MidiMode::User => ids::PushMidiMode::User,
        },
        pad_mode: match snapshot.pad_mode {
            PadMode::Note => ids::PushPadMode::Note,
            PadMode::Session => ids::PushPadMode::Session,
        },
        display_mode: match snapshot.display_mode {
            DisplayMode::Device => ids::PushDisplayMode::Device,
            DisplayMode::Browse => ids::PushDisplayMode::Browse,
            DisplayMode::Mix => ids::PushDisplayMode::Mix,
            DisplayMode::Clip => ids::PushDisplayMode::Clip,
        },
        mix,
        menu: snapshot.menu.map(|menu| match menu {
            bl_push::state::Menu::Setup => ids::PushMenu::Setup,
            bl_push::state::Menu::Scale => ids::PushMenu::Scale,
            bl_push::state::Menu::Quantize => ids::PushMenu::Quantize,
            bl_push::state::Menu::FixedLength => ids::PushMenu::FixedLength,
            bl_push::state::Menu::Convert => ids::PushMenu::Convert,
        }),
        recording: ids::PushRecording {
            record: snapshot.record.to_owned(),
            fixed_length: fixed_info(snapshot.fixed_length),
            swing: (snapshot.swing * 100.0).round() as u8,
            automation_arm: snapshot.automation_arm,
            quantize_menu: snapshot
                .quantize_menu
                .clone()
                .map(|menu| ids::PushQuantizeMenu {
                    swing: menu.swing,
                    quantize_to: menu.quantize_to,
                    amount: menu.amount,
                    record_quantize: menu.record_quantize,
                    stays: menu.stays,
                }),
        },
        held: snapshot.held.iter().map(|b| (*b).to_owned()).collect(),
        locked: snapshot.locked.iter().map(|b| (*b).to_owned()).collect(),
        colour_picker: snapshot.picker,
        leds: leds(&snapshot),
        touch_strip: ids::PushTouchStrip {
            configuration: match snapshot.strip {
                StripUse::PitchBend => "pitch_bend",
                StripUse::Modulation => "modulation",
                StripUse::Range => "range",
            }
            .to_owned(),
            flags: snapshot.strip.flags(),
            leds: snapshot.strip_leds.to_vec(),
        },
        setup: setup_info(snapshot.setup),
        browse: snapshot.browse.as_ref().map(|browse| ids::PushBrowse {
            load: match browse.load {
                bl_push::browse::LoadKind::Add => "add",
                bl_push::browse::LoadKind::Replace(_) => "replace",
                bl_push::browse::LoadKind::AddTrack => "add_track",
            }
            .to_owned(),
            replaces: match browse.load {
                bl_push::browse::LoadKind::Replace(device) => Some(device),
                _ => None,
            },
            columns: browse
                .columns
                .iter()
                .map(|column| ids::PushBrowseColumn {
                    entries: column.entries.clone(),
                    selected: column.selected,
                })
                .collect(),
            in_use: browse.in_use,
        }),
        device: snapshot.device.as_ref().map(|device| ids::PushDeviceMode {
            devices: device
                .devices
                .iter()
                .map(|d| ids::PushDevice {
                    button: d.button,
                    device: d.device,
                    name: d.name.clone(),
                    depth: d.depth,
                    kind: d.kind.to_owned(),
                    enabled: d.enabled,
                    selected: d.selected,
                    unfolded: d.unfolded,
                })
                .collect(),
            left_arrow: device.arrows.0,
            right_arrow: device.arrows.1,
            empty: device.empty,
            edit: device.edit,
            bank: device.bank,
            banks: device.banks.clone(),
            bank_page: device.page,
            bank_pages: device.pages,
            params: device
                .params
                .iter()
                .map(|p| ids::PushParam {
                    name: p.name.clone(),
                    value: p.value.clone(),
                    held: p.held,
                })
                .collect(),
            chains: device
                .chains
                .iter()
                .map(|c| ids::PushChain {
                    chain: c.chain,
                    name: c.name.clone(),
                    selected: c.selected,
                })
                .collect(),
            mute_locked: snapshot.locked.contains(&"mute"),
            simpler: device.simpler.as_ref().map(|simpler| ids::PushSimpler {
                empty: simpler.empty,
                mode: simpler.mode.to_owned(),
                main_bank: simpler.main,
                zoom: format!("×{:.1}", simpler.zoom),
                zoom_around: simpler.around.to_owned(),
                region: simpler.region,
                slices: simpler.slices,
                selected_slice: simpler.selected,
                pad_slicing: simpler.pad_slicing,
                edit_buttons: simpler
                    .buttons
                    .iter()
                    .map(|button| ids::PushSimplerButton {
                        button: button.button,
                        name: button.name.to_owned(),
                        text: button.text.clone(),
                        on: button.on,
                    })
                    .collect(),
            }),
        }),
        session: snapshot.session.as_ref().map(|session| {
            let look = |look: &bl_push::session::Look| ids::PushSessionLook {
                shows: look.what.to_owned(),
                clip: look.clip,
                colour: look.colour,
            };
            ids::PushSession {
                first_track: session.first_track,
                first_scene: session.first_scene,
                tracks: session.tracks,
                scenes: session.scenes,
                overview: session.overview,
                overview_locked: session.locked,
                overview_part: session.part,
                stop_clip_locked: snapshot.locked.contains(&"stop_clip"),
                pads: session.pads.iter().map(look).collect(),
                scene_buttons: session.scene_buttons.iter().map(look).collect(),
            }
        }),
        mix_mode: snapshot.mix.as_ref().map(mix_info),
        display: ids::PushDisplay {
            text: snapshot.screen.text(),
            claimed: snapshot.display.claimed,
            error: snapshot.display.error.clone(),
            frames: snapshot.display.frames,
        },
        note: snapshot.note.map(|note| ids::PushNote {
            track: note.track,
            layout: note.layout,
            overlay: note.overlay,
            overlay_locked: note.overlay_locked,
            bank: note.bank,
            selected_pad: note.selected_pad,
            step_size: note.step_size,
            edited_clip: note.edited_clip,
            edited_scene: note.edited_scene,
            page: note.page,
            following: note.following,
            velocity: note.velocity,
            accent: note.accent,
            repeat: note.repeat,
            held_steps: note
                .held_steps
                .into_iter()
                .map(|step| ids::PushHeldStep {
                    clip: step.clip,
                    pad: step.key,
                    start: step.start,
                    length: step.length,
                })
                .collect(),
            melody: note.melody.map(|melody| ids::PushMelody {
                in_key: melody.in_key,
                fixed: melody.fixed,
                layout: melody.spacing,
                direction: melody.direction,
                octave: melody.octave,
                low: melody.low,
                high: melody.high,
                range: melody.range,
                selected_notes: melody.selected_notes,
                strip_modulation: melody.modulation,
            }),
        }),
    }
}

impl Shell {
    /// The Push 2 surfaces that run: their rows, from 0, and their handles.
    pub(super) fn pushes(&self) -> Vec<(usize, Arc<Handle>)> {
        let surfaces = self.midi.surfaces();
        let report = self.midi.surface_rows();
        (0..bl_control::ROWS)
            .filter(|&row| {
                report[row].run == RunState::Running
                    && report[row].config.surface.as_deref() == Some(bl_push::SURFACE_ID)
            })
            .filter_map(|row| {
                let handle = surfaces.probe(row)?.downcast::<Handle>().ok()?;
                Some((row, handle))
            })
            .collect()
    }

    /// Runs a Push 2 command (R34).
    pub(super) fn run_push(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let row_of = |row: u8| -> Result<usize, ShellError> {
            (1..=bl_control::ROWS)
                .contains(&usize::from(row))
                .then(|| usize::from(row) - 1)
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!(
                        "the row is {row}; the table has rows 1 to {}",
                        bl_control::ROWS
                    ),
                })
        };
        match id {
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                self.refresh_midi();
                let rows = self.midi.surfaces().rows().clone();
                // The Setup values are the preferences', not the surface's
                // last tick's.
                let setup = setup_info(self.midi.push().setup());
                let pushes = self
                    .pushes()
                    .into_iter()
                    .map(|(row, handle)| {
                        let config = &rows[row];
                        let mut push = info(
                            row,
                            config.input.clone().unwrap_or_default(),
                            config.output.clone().unwrap_or_default(),
                            handle.snapshot(),
                        );
                        push.setup = setup;
                        push
                    })
                    .collect();
                Ok(to_value(&ids::PushState { pushes }))
            }
            ids::SET_MODE => {
                let args = Self::args::<ids::PushSetModeArgs>(id, args)?;
                let only = args.row.map(row_of).transpose()?;
                let pad = args.pad.map(|mode| match mode {
                    ids::PushPadMode::Note => PadMode::Note,
                    ids::PushPadMode::Session => PadMode::Session,
                });
                let display = args.display.map(|mode| {
                    let mode = match mode {
                        ids::PushDisplayMode::Device => DisplayMode::Device,
                        ids::PushDisplayMode::Browse => DisplayMode::Browse,
                        ids::PushDisplayMode::Mix => DisplayMode::Mix,
                        ids::PushDisplayMode::Clip => DisplayMode::Clip,
                    };
                    let page = match args.mix {
                        Some(ids::PushMixPage::Global) => MixPage::Global,
                        _ => MixPage::Track,
                    };
                    (mode, page)
                });
                let pushes: Vec<_> = self
                    .pushes()
                    .into_iter()
                    .filter(|(row, _)| only.is_none_or(|only| only == *row))
                    .collect();
                if pushes.is_empty() {
                    return Err(failed("no Push 2 runs".to_owned()));
                }
                for (_, handle) in &pushes {
                    handle.send(Command::SetMode { pad, display });
                }
                Ok(to_value(&ids::PushSetModeResult {
                    rows: pushes
                        .iter()
                        .map(|(row, _)| u8::try_from(row + 1).unwrap_or(u8::MAX))
                        .collect(),
                }))
            }
            ids::SET_SETUP => {
                let args = Self::args::<ids::PushSetSetupArgs>(id, args)?;
                let mut setup = self.midi.push().setup();
                setup.sensitivity = args.pad_sensitivity.unwrap_or(setup.sensitivity);
                setup.gain = args.pad_gain.unwrap_or(setup.gain);
                setup.dynamics = args.pad_dynamics.unwrap_or(setup.dynamics);
                setup.display_light = args.display_light.unwrap_or(setup.display_light);
                setup.led_brightness = args.led_brightness.unwrap_or(setup.led_brightness);
                if let Some(workflow) = args.workflow {
                    setup.workflow = match workflow {
                        ids::PushWorkflow::Scene => Workflow::Scene,
                        ids::PushWorkflow::Clip => Workflow::Clip,
                    };
                }
                self.midi.push().set_setup(setup);
                // A preference: kept for the next start, no part of the set.
                self.surface_push_saved();
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                Ok(to_value(&setup_info(self.midi.push().setup())))
            }
            ids::SET_FIXED_LENGTH => {
                let args = Self::args::<ids::PushSetFixedLengthArgs>(id, args)?;
                let mut fixed = self.midi.push().fixed_length();
                let was = fixed.on;
                fixed.on = args.enabled.unwrap_or(fixed.on);
                fixed.phrase_sync = args.phrase_sync.unwrap_or(fixed.phrase_sync);
                if let Some(length) = args.length {
                    fixed.length = ids::PushFixedLengthValue::ALL
                        .iter()
                        .position(|&l| l == length)
                        .map_or(fixed.length, |at| at as u8);
                }
                self.midi.push().set_fixed_length(fixed);
                // A preference: kept for the next start, no part of the set.
                self.surface_push_saved();
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                if fixed.on && !was {
                    let signatures = &self.editor.document().settings().time_signatures;
                    let at = self.arrangement.transport().position;
                    let length = fixed.value().ticks(signatures.signature_at(at));
                    self.loop_last_stretch(length);
                }
                Ok(to_value(&fixed_info(fixed)))
            }
            ids::SET_SWING => {
                let args = Self::args::<ids::PushSetSwingArgs>(id, args)?;
                self.midi.push().set_swing(f64::from(args.amount) / 100.0);
                self.surface_push_saved();
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                Ok(to_value(&args.amount))
            }
            ids::NEW => {
                let args = Self::args::<ids::PushSessionArgs>(id, args)?;
                self.push_new(id, args)
            }
            ids::DUPLICATE => {
                let args = Self::args::<ids::PushSessionArgs>(id, args)?;
                self.push_duplicate(id, args)
            }
            ids::CONVERT => {
                let args = Self::args::<ids::PushConvertArgs>(id, args)?;
                self.push_convert(id, args.to)
            }
            ids::DISPLAY_FRAME => {
                let args = Self::args::<ids::PushDisplayFrameArgs>(id, args)?;
                let only = args.row.map(row_of).transpose()?;
                let (row, handle) = self
                    .pushes()
                    .into_iter()
                    .find(|(row, _)| only.is_none_or(|only| only == *row))
                    .ok_or_else(|| failed("no Push 2 runs".to_owned()))?;
                let rgb = bl_push::paint::render(&handle.snapshot().screen);
                let png = bl_push::paint::png(&rgb).map_err(failed)?;
                Ok(to_value(&ids::PushDisplayFrame {
                    row: u8::try_from(row + 1).unwrap_or(u8::MAX),
                    width: bl_push::frame::WIDTH as u32,
                    height: bl_push::frame::HEIGHT as u32,
                    png: bl_model::base64::encode(&png),
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
