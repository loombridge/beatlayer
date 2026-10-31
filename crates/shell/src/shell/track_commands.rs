//! The track commands: selecting, copying, pasting and renaming tracks.

use super::{KEY_TRACK_NUMBER, Shell, ShellError, to_value};
use crate::arrangement::editing::Clipboard;
use crate::mixer_controls;
use crate::mixer_controls::Switch;
use bl_commands::Context;
use bl_model::TrackId;
use bl_model::TrackKind;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::comping as comping_ids;
use bl_shell_spec::tracks as track_ids;
use serde_json::Value;
use serde_json::json;

impl Shell {
    /// Selects tracks, as the Arrangement's selection. The title bars stay
    /// selected when they were, so the keys that act on a title bar keep
    /// working (the track context menu spec, R12, R14).
    pub(super) fn select_tracks(&mut self, tracks: &[TrackId]) {
        let title_bars = self.arrangement.title_bars_selected();
        self.arrangement
            .run(
                view_ids::SELECT_TRACKS,
                json!({ "tracks": tracks, "title_bars": title_bars }),
                self.editor.document(),
            )
            .ok();
    }

    /// Copies the tracks to the clipboard, with everything they hold (the
    /// track context menu spec, R13, R14), and returns their IDs. The
    /// clipboard holds whichever was copied last, these tracks or the
    /// Arrangement's material.
    pub(super) fn copy_tracks(
        &mut self,
        id: &str,
        tracks: &[TrackId],
    ) -> Result<Vec<TrackId>, ShellError> {
        let originals = crate::tracks::duplicated(self.editor.document(), tracks);
        if originals.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "there's no track to copy".to_owned(),
            });
        }
        let doc = self.editor.document();
        let tracks: Vec<crate::tracks::TrackData> = originals
            .iter()
            .filter_map(|&track| crate::tracks::snapshot(doc, track))
            .collect();
        // The files the tracks play go with them (R15; the set exchange
        // spec, R7), for a paste into a set that doesn't list them.
        let used = crate::tracks::media_hashes(&tracks);
        let media = self
            .store
            .media_files()
            .into_iter()
            .filter(|(hash, ..)| used.contains(hash))
            .collect();
        self.clipboard = Some(Clipboard::Tracks(crate::tracks::CopiedTracks {
            tracks,
            returns: doc.returns().collect(),
            set: self.editor_generation,
            media,
        }));
        Ok(originals)
    }

    /// The last selected track, in display order: where a paste goes.
    pub(super) fn last_selected_track(&self) -> Option<TrackId> {
        let order = self.editor.document().track_order();
        self.arrangement
            .selected_tracks()
            .iter()
            .copied()
            .filter(|track| order.contains(track))
            .max_by_key(|track| order.iter().position(|t| t == track).unwrap_or(0))
    }

    /// Where tracks added after `after` go: straight after it, at its level
    /// and inside its group when it's in one, or after the last track when
    /// there is none (the track context menu spec, R14).
    pub(crate) fn place_after(&self, after: Option<TrackId>) -> crate::tracks::Place {
        let doc = self.editor.document();
        let returns: Vec<TrackId> = doc.returns().collect();
        match after {
            Some(track) => {
                let parent = doc.parent(track);
                if doc
                    .track(track)
                    .is_some_and(|track| track.kind() == bl_model::TrackKind::Return)
                {
                    // A return track goes among the returns.
                    let at = returns
                        .iter()
                        .position(|&r| r == track)
                        .map_or(returns.len(), |i| i + 1);
                    crate::tracks::Place {
                        parent: None,
                        index: doc.top_level().len(),
                        returns: at,
                    }
                } else {
                    let siblings: Vec<TrackId> = match parent {
                        Some(group) => doc
                            .track(group)
                            .map(|g| g.children().collect())
                            .unwrap_or_default(),
                        None => doc.top_level().collect(),
                    };
                    let at = siblings
                        .iter()
                        .position(|&t| t == track)
                        .map_or(siblings.len(), |i| i + 1);
                    crate::tracks::Place {
                        parent,
                        index: at,
                        returns: returns.len(),
                    }
                }
            }
            None => crate::tracks::Place {
                parent: None,
                index: doc.top_level().len(),
                returns: returns.len(),
            },
        }
    }

    /// Pastes the tracks on the clipboard, straight after the track to
    /// paste after, at its level and inside its group when it's in one, or
    /// after the last track when none is selected (R14).
    fn paste_tracks(
        &mut self,
        id: &str,
        copied: &crate::tracks::CopiedTracks,
        after: Option<TrackId>,
    ) -> Result<Vec<TrackId>, ShellError> {
        let place = self.place_after(after);
        // Tracks from another set: their files join this set's media table
        // (R15), and the IDs they name mean nothing here (R15; the set
        // exchange spec, R6).
        let source_returns = (copied.set != self.editor_generation).then(|| copied.returns.clone());
        if source_returns.is_some() {
            for (hash, path, size) in &copied.media {
                self.store.add_media(*hash, path, *size);
                let hash = bl_media::ContentHash::from_bytes(hash.0);
                if self.media.entry(hash).is_none() {
                    let _ = self.media.import(path);
                }
            }
        }
        crate::tracks::copy_tracks(
            &mut self.editor,
            &copied.tracks,
            place,
            source_returns,
            track_ids::PASTE_STEP,
        )
        .map_err(|reason| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        })
    }

    /// Runs a track command that needs the shell: Duplicate Track Without
    /// Clips (WP-2.19b, IMP-12), Group Tracks (T24), Ungroup Tracks (T24),
    /// and the whole-track copies of WP-6.52.
    pub(super) fn run_tracks(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if matches!(
            id,
            track_ids::DUPLICATE
                | track_ids::COPY
                | track_ids::CUT
                | track_ids::DUPLICATE_WITHOUT_CLIPS
        ) {
            self.capture_plugin_states();
        }
        match id {
            track_ids::DUPLICATE => {
                let track_ids::DuplicateTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let copies = crate::tracks::duplicate_tracks(
                    &mut self.editor,
                    &tracks,
                    track_ids::DUPLICATE_STEP,
                )
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
                self.select_tracks(&copies);
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksDuplicated { tracks: copies }))
            }
            track_ids::COPY => {
                let track_ids::DuplicateTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let copied = self.copy_tracks(id, &tracks)?;
                self.touch();
                Ok(to_value(&track_ids::TracksCopied { tracks: copied }))
            }
            track_ids::CUT => {
                let track_ids::DuplicateTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let copied = self.copy_tracks(id, &tracks)?;
                // The tracks go as Delete does (R4): every selected track
                // but Main, and a track inside a deleted group with it.
                let doomed = crate::tracks::duplicated(self.editor.document(), &tracks);
                if !doomed.is_empty() {
                    self.editor
                        .transact(track_ids::CUT_STEP, |editor| {
                            for track in &doomed {
                                editor.apply(bl_model::Command::DeleteTrack { track: *track })?;
                            }
                            Ok::<_, bl_model::EditError>(())
                        })
                        .map_err(|error| ShellError::Failed {
                            command: id.to_owned(),
                            message: error.to_string(),
                        })?;
                }
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksCopied { tracks: copied }))
            }
            track_ids::PASTE => {
                let track_ids::PasteTracksArgs { track } = Self::args(id, args)?;
                // The clipboard holds whichever was copied last: the tracks,
                // or the Arrangement's material (R14).
                let Some(clipboard) = self.clipboard.clone() else {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "there's nothing on the clipboard to paste".to_owned(),
                    });
                };
                let Clipboard::Tracks(copied) = clipboard else {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the clipboard holds Arrangement material; paste it with \
                                 arrangement.paste"
                            .to_owned(),
                    });
                };
                let after = track.or_else(|| self.last_selected_track());
                let copies = self.paste_tracks(id, &copied, after)?;
                self.select_tracks(&copies);
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksCopied { tracks: copies }))
            }
            track_ids::DUPLICATE_WITHOUT_CLIPS => {
                let track_ids::DuplicateTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let copies = crate::tracks::duplicate_without_clips(
                    &mut self.editor,
                    &tracks,
                    track_ids::DUPLICATE_WITHOUT_CLIPS_STEP,
                )
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": copies }),
                        self.editor.document(),
                    )
                    .ok();
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksDuplicated { tracks: copies }))
            }
            track_ids::GROUP => {
                let track_ids::GroupTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let look = &self.prefs.look;
                let group_id = self
                    .editor
                    .transact(track_ids::GROUP_STEP, |editor| {
                        let group =
                            crate::tracks::group_tracks(editor, &tracks, track_ids::GROUP_STEP)?;
                        crate::colours::colour_new(editor, look, &[group], &[])
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(group)
                    })
                    .map_err(|reason| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason,
                    })?;
                self.arrangement.unfold(group_id);
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": [group_id] }),
                        self.editor.document(),
                    )
                    .ok();
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksGrouped { track: group_id }))
            }
            track_ids::UNGROUP => {
                let track_ids::UngroupTracksArgs { track } = Self::args(id, args)?;
                let target = track
                    .or_else(|| self.arrangement.selected_tracks().first().copied())
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "no track is selected".to_owned(),
                    })?;
                let children = crate::tracks::ungroup_tracks(
                    &mut self.editor,
                    target,
                    track_ids::UNGROUP_STEP,
                )
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
                if let Some(&first) = children.first() {
                    self.arrangement
                        .run(
                            view_ids::SELECT_TRACKS,
                            json!({ "tracks": [first] }),
                            self.editor.document(),
                        )
                        .ok();
                }
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksUngrouped { tracks: children }))
            }
            track_ids::LINK | track_ids::UNLINK => {
                // Spec: linked-track editing R2 to R5.
                let (named, into) = if id == track_ids::LINK {
                    let track_ids::LinkTracksArgs { tracks, into } = Self::args(id, args)?;
                    (tracks, into)
                } else {
                    let track_ids::UnlinkTracksArgs { tracks } = Self::args(id, args)?;
                    (tracks, None)
                };
                let selected = named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let doc = self.editor.document();
                let mut tracks = crate::tracks::linkable(doc, &selected);
                let unavailable = |reason: &str| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: reason.to_owned(),
                };
                let group = if id == track_ids::LINK {
                    let group = match into {
                        Some(into) => {
                            let group = doc.track(into).and_then(bl_model::Track::link);
                            Some(group.ok_or_else(|| unavailable("`into` isn't a linked track"))?)
                        }
                        None => None,
                    };
                    let enough = match group {
                        Some(_) => tracks
                            .iter()
                            .any(|t| doc.track(*t).and_then(bl_model::Track::link) != group),
                        None => tracks.len() >= 2,
                    };
                    if !enough {
                        return Err(unavailable("linking needs two audio or MIDI tracks"));
                    }
                    Some(group.unwrap_or_else(|| self.editor.new_id()))
                } else {
                    tracks.retain(|t| doc.track(*t).is_some_and(|t| t.link().is_some()));
                    if tracks.is_empty() {
                        return Err(unavailable("no selected track is linked"));
                    }
                    None
                };
                let step = if group.is_some() {
                    track_ids::LINK_STEP
                } else {
                    track_ids::UNLINK_STEP
                };
                self.editor
                    .transact(step, |editor| {
                        editor.apply(bl_model::Command::SetTrackLinks {
                            tracks: tracks.clone(),
                            group,
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksLinked { tracks }))
            }
            track_ids::SET_DEVICES_ENABLED => {
                // Spec: device-chains R39. Deactivate All Devices switches
                // off every top-level device on the selected tracks, and
                // switches them back on when they are all off already; it
                // reaches no rack's contents. One undo step either way.
                let track_ids::SetDevicesEnabledArgs { tracks, enabled } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let on = enabled.unwrap_or_else(|| self.all_devices_off(&tracks));
                let step = if on {
                    track_ids::ACTIVATE_ALL_DEVICES_STEP
                } else {
                    track_ids::DEACTIVATE_ALL_DEVICES_STEP
                };
                let opened = !self.gesture;
                if opened {
                    self.begin_gesture(step);
                }
                let mut result = Ok(());
                for &track in &tracks {
                    let devices: Vec<bl_model::DeviceId> = self
                        .editor
                        .document()
                        .track(track)
                        .into_iter()
                        .flat_map(|track| track.devices().map(|device| device.id))
                        .collect();
                    for device in devices {
                        result = self
                            .run(
                                "device.set_enabled",
                                json!({ "device": device, "enabled": on }),
                            )
                            .map(|_| ());
                        if result.is_err() {
                            break;
                        }
                    }
                    if result.is_err() {
                        break;
                    }
                }
                if opened {
                    self.end_gesture();
                }
                result?;
                Ok(to_value(&track_ids::DevicesEnabled {
                    tracks,
                    enabled: on,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The contexts a track's title-bar menu works in (WP-4.22): the active
    /// ones, with `track-headers` first, because the menu acts on the
    /// selected title bars whichever view it opened in.
    #[must_use]
    pub fn track_menu_contexts(&self) -> Vec<Context> {
        let mut contexts = self.active_contexts();
        contexts.retain(|c| *c != Context::TrackHeaders);
        contexts.insert(0, Context::TrackHeaders);
        contexts
    }

    /// Whether a comping command can run with `args` now: the menus' enabled
    /// state (the comping spec, R29). Its arguments must read, name things the
    /// set has, and, for Insert Take Lane, there must be a track to add to.
    pub(super) fn can_comp(&self, id: &str, args: &Value) -> bool {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        let doc = self.editor.document();
        let lane = |lane: &bl_model::TakeLaneId| doc.take_lane_track(*lane).is_some();
        let track = |track: &TrackId| {
            doc.track(*track)
                .is_some_and(|t| matches!(t.kind(), TrackKind::Audio | TrackKind::Midi))
        };
        match id {
            comping_ids::INSERT => serde_json::from_value::<comping_ids::InsertArgs>(args)
                .ok()
                .is_some_and(|args| {
                    let tracks = args
                        .tracks
                        .unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    !tracks.is_empty() && tracks.iter().all(track)
                }),
            comping_ids::DUPLICATE => serde_json::from_value::<comping_ids::DuplicateArgs>(args)
                .ok()
                .is_some_and(|args| match args.lanes {
                    Some(lanes) => !lanes.is_empty() && lanes.iter().all(lane),
                    None => !self.arrangement.selected_take_lanes().is_empty(),
                }),
            comping_ids::COPY_TO_MAIN_LANE => {
                // Spec: comping R17, R29. Named lanes and a range, or the
                // selection's: a take lane and the time on it.
                serde_json::from_value::<comping_ids::CopyToMainLaneArgs>(args)
                    .ok()
                    .is_some_and(|args| {
                        let lanes_ok = match &args.lanes {
                            Some(lanes) => !lanes.is_empty() && lanes.iter().all(lane),
                            None => self.arrangement.take_lane_selected(),
                        };
                        let range_ok = (args.start.is_some() && args.end.is_some())
                            || self.arrangement.time_selection().is_some();
                        lanes_ok && range_ok
                    })
            }
            comping_ids::REPLACE_WITH_TAKE => {
                // Spec: comping R19. A named track, or the selection's audio
                // and MIDI tracks and its time.
                serde_json::from_value::<comping_ids::ReplaceWithTakeArgs>(args)
                    .ok()
                    .is_some_and(|args| {
                        let tracks_ok = match args.track {
                            Some(named) => track(&named),
                            None => self.arrangement.selected_tracks().iter().any(track),
                        };
                        let range_ok = (args.start.is_some() && args.end.is_some())
                            || self.arrangement.time_selection().is_some();
                        tracks_ok && range_ok
                    })
            }
            comping_ids::AUDITION => serde_json::from_value::<comping_ids::AuditionArgs>(args)
                .ok()
                .is_some_and(|args| match args.lanes {
                    Some(lanes) => !lanes.is_empty() && lanes.iter().all(lane),
                    None => self.arrangement.take_lane_selected(),
                }),
            comping_ids::MOVE_COMP_EDGE => {
                serde_json::from_value::<comping_ids::MoveCompEdgeArgs>(args)
                    .ok()
                    .is_some_and(|args| track(&args.track))
            }
            comping_ids::COMP_SOURCES => {
                serde_json::from_value::<comping_ids::CompSourcesArgs>(args)
                    .ok()
                    .is_some_and(|args| track(&args.track))
            }
            _ => false,
        }
    }

    /// The frozen track that refuses `id`, if any (the freeze and bounce
    /// spec, R8).
    pub(super) fn frozen_blocks(&self, id: &str, args: &Value) -> Option<TrackId> {
        // Spec: freeze-and-bounce R30. A frozen track's take lanes play no part
        // in the freeze, but a lane can't be auditioned on it and nothing is
        // copied from one to the main lane.
        let doc = self.editor.document();
        let lane_track = |lane: &Value| {
            let lane = serde_json::from_value::<bl_model::TakeLaneId>(lane.clone()).ok()?;
            doc.take_lane_track(lane)
        };
        let frozen = |track: Option<TrackId>| {
            track.filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()))
        };
        match id {
            "take_lane.set_audition" if args.get("on").and_then(Value::as_bool) == Some(true) => {
                return frozen(args.get("lane").and_then(lane_track));
            }
            // It copies from one lane on each track it names.
            comping_ids::COPY_TO_MAIN_LANE => {
                return args
                    .get("lanes")
                    .and_then(Value::as_array)
                    .and_then(|lanes| lanes.iter().find_map(|lane| frozen(lane_track(lane))));
            }
            comping_ids::REPLACE_WITH_TAKE => {
                return frozen(
                    args.get("track")
                        .and_then(|track| serde_json::from_value::<TrackId>(track.clone()).ok()),
                );
            }
            _ => {}
        }
        // R7 keeps a frozen track's mixer live: volume, pan, sends, the
        // activator, solo and the crossfader all work; only its devices, its
        // clips' own settings, arming and new clips are locked (R8).
        if !(id.starts_with("device.")
            || id.starts_with("clip.")
            || matches!(
                id,
                "mixer.set_arm"
                    | "mixer.arm_exclusive"
                    | "browser.add_to_arrangement"
                    | "browser.insert"
                    | "track.create"
            ))
        {
            return None;
        }
        // R7: moving, copying, deleting, trimming and launching a frozen
        // clip still work.
        if matches!(
            id,
            "clip.move"
                | "clip.move_by"
                | "clip.move_to_slot"
                | "clip.delete"
                | "clip.set_length"
                | "clip.trim_start"
                | "clip.set_launch_mode"
                | "clip.set_launch_quantization"
                | "clip.set_legato"
                | "clip.set_follow_actions"
        ) {
            return None;
        }
        let doc = self.editor.document();
        if let Some(clip) = args
            .get("clip")
            .and_then(|value| serde_json::from_value::<bl_model::ClipId>(value.clone()).ok())
        {
            // A take-lane clip plays no part in the freeze (R30).
            return doc
                .clip_track(clip)
                .filter(|_| doc.clip_lane(clip).is_none())
                .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()));
        }
        if let Some(device) = args
            .get("device")
            .and_then(|value| serde_json::from_value::<bl_model::DeviceId>(value.clone()).ok())
        {
            return doc
                .device_track(device)
                .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()));
        }
        args.get("track")
            .and_then(|value| serde_json::from_value::<TrackId>(value.clone()).ok())
            .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()))
    }

    /// Whether a track can be copied: one is selected, and it isn't Main
    /// (the track context menu spec, R3).
    pub(super) fn can_copy_tracks(&self) -> bool {
        self.arrangement.selected_tracks().iter().any(|track| {
            self.editor
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() != bl_model::TrackKind::Main)
        })
    }

    /// The tracks a toggling mixer key acts on: the track numbered by
    /// `track_number` (F1 to F8, counting the mixer's columns from 1), which
    /// it takes out of `fields`, or the selected tracks.
    pub(super) fn key_tracks(&self, fields: &mut serde_json::Map<String, Value>) -> Vec<TrackId> {
        match fields.remove(KEY_TRACK_NUMBER).and_then(|n| n.as_u64()) {
            Some(number) => self
                .mixer_columns()
                .get((number as usize).wrapping_sub(1))
                .copied()
                .into_iter()
                .collect(),
            None => self.arrangement.selected_tracks().to_vec(),
        }
    }

    /// Runs a toggling mixer key (S, 0, C, F1 to F8) on every track it acts
    /// on, as one undo step (the mixer view spec, R9).
    pub(super) fn toggle_tracks(
        &mut self,
        id: &str,
        switch: Switch,
        args: Value,
    ) -> Result<Value, ShellError> {
        let Value::Object(mut fields) = args else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the arguments must be a JSON object".to_owned(),
            });
        };
        let tracks = self.key_tracks(&mut fields);
        let invocations = mixer_controls::toggled(self.editor.document(), switch, &tracks);
        if invocations.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no selected track has this switch".to_owned(),
            });
        }
        let opened = !self.gesture;
        if opened {
            self.begin_gesture(switch.gesture());
        }
        let mut result = Ok(Value::Null);
        for invocation in invocations {
            result = self.run(&invocation.id, invocation.args);
            if result.is_err() {
                break;
            }
        }
        if opened {
            self.end_gesture();
        }
        result
    }

    /// The tracks Delete reaches: the selected ones, less Main, which can't
    /// be deleted.
    pub(super) fn deletable_tracks(&self) -> Vec<TrackId> {
        let doc = self.editor.document();
        self.arrangement
            .selected_tracks()
            .iter()
            .copied()
            .filter(|&t| doc.track(t).is_some_and(|t| t.kind() != TrackKind::Main))
            .collect()
    }

    /// The track Rename reaches: the one `args` names, else the first
    /// selected, unless it's Main, whose name is fixed.
    pub(super) fn renamable_track(&self, args: &Value) -> Option<TrackId> {
        let named = args
            .get("track")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        named
            .or_else(|| self.arrangement.selected_tracks().first().copied())
            .filter(|&t| {
                self.editor
                    .document()
                    .track(t)
                    .is_some_and(|t| t.kind() != TrackKind::Main)
            })
    }

    /// The locator Rename reaches: the one `args` names, else the first
    /// selected.
    pub(super) fn renamable_locator(&self, args: &Value) -> Option<bl_model::LocatorId> {
        let named: Option<Vec<bl_model::LocatorId>> = args
            .get("locators")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        named
            .and_then(|mut locators| {
                if locators.is_empty() {
                    None
                } else {
                    Some(locators.remove(0))
                }
            })
            .or_else(|| self.arrangement.selected_locators().first().copied())
    }
}
