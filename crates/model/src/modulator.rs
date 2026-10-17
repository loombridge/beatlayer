//! Modulators' mappings and the Curve Mod's shape (WP-5.10; the modulation
//! spec, R12 to R19, R23): what a modulator device maps, how a mapping is
//! kept and removed, and the breakpoint shape a Curve Mod plays.
//!
//! A mapping is kept on the modulator, in the document. The modulator's
//! output, its mode, polarity and each slot's amount and range are
//! parameters of the modulator ([`MODE`] and the rest), so they can be
//! automated and modulated like any parameter.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, MixerParam, curve_value};
use crate::device::{Device, DeviceKind};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{DeviceId, ParamId, TrackId};

/// The Wave Mod's device type: Beatlayer's equivalent of an LFO device.
pub const WAVE_MOD: &str = "beatlayer.wave_mod";
/// The Curve Mod's device type: Beatlayer's equivalent of a Shaper device.
pub const CURVE_MOD: &str = "beatlayer.curve_mod";

/// How many targets a modulator maps: the first slot is always shown, and
/// Show Multimap shows the other seven (R17).
pub const SLOTS: u8 = 8;
/// The most breakpoints a Curve Mod's shape holds.
pub const MAX_POINTS: usize = 64;

/// The Mod switch: Modulation (0, the default) or Remote Control (1) for
/// all the modulator's mappings (R13). A parameter of every modulator.
pub const MODE: ParamId = ParamId(64);
/// The Polarity switch: Bipolar (0, the default) or Unipolar (1) (R14).
pub const POLARITY: ParamId = ParamId(65);
/// The Depth, from 0 % (no effect) to 100 % (full), which scales every
/// mapping's effect (R13). A parameter of every modulator.
pub const DEPTH: ParamId = ParamId(66);
/// The Curve Mod's Grid: the display's divisions of the cycle, a choice among
/// [`GRIDS`] (R23).
pub const GRID_DIVISIONS: ParamId = ParamId(13);
/// The Curve Mod's Snap: breakpoints snap to the grid (R23).
pub const SNAP: ParamId = ParamId(14);
/// The Grid's choices: each one's name and its divisions of the cycle.
pub const GRIDS: [(&str, u32); 5] = [("1/2", 2), ("1/4", 4), ("1/8", 8), ("1/16", 16), ("1/32", 32)];
/// The first parameter id a slot owns; each slot has three.
const SLOT_BASE: u32 = 67;

/// A slot's Modulation Amount, from −100 % to 100 % (R14).
#[must_use]
pub const fn amount(slot: u8) -> ParamId {
    ParamId(SLOT_BASE + slot as u32 * 3)
}

/// A slot's Min, a position on its target's travel (R16).
#[must_use]
pub const fn min(slot: u8) -> ParamId {
    ParamId(SLOT_BASE + slot as u32 * 3 + 1)
}

/// A slot's Max, a position on its target's travel (R16).
#[must_use]
pub const fn max(slot: u8) -> ParamId {
    ParamId(SLOT_BASE + slot as u32 * 3 + 2)
}

/// Whether `kind` is one of the modulator devices.
#[must_use]
pub fn is_modulator_kind(kind: &DeviceKind) -> bool {
    matches!(kind, DeviceKind::Native { device_type } if device_type == WAVE_MOD || device_type == CURVE_MOD)
}

/// A parameter a modulator maps (R17): a mixer control or a send of any
/// track, or a parameter of any device. A device is named by its own ID, so
/// the mapping follows it when it moves to another track.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "A parameter a modulator maps: `mixer` with a `track` and a `param`, `send` \
                   with the sending `track` and the return track `to`, or `device` with the \
                   device and its `param`."
)]
pub enum ModTarget {
    /// A control of a track's mixer.
    Mixer {
        /// The track.
        track: TrackId,
        /// The control.
        param: MixerParam,
    },
    /// The level of a track's send to a return track.
    Send {
        /// The sending track.
        track: TrackId,
        /// The return track the send feeds.
        to: TrackId,
    },
    /// A parameter of a device, on any track.
    Device {
        /// The device.
        device: DeviceId,
        /// Its parameter.
        param: ParamId,
    },
}

impl ModTarget {
    /// The same parameter as an automation target of its track `owner`
    /// (the track holding the device for a device target).
    #[must_use]
    pub fn automation(self) -> AutomationTarget {
        match self {
            Self::Mixer { param, .. } => AutomationTarget::Mixer { param },
            Self::Send { to, .. } => AutomationTarget::Send { to },
            Self::Device { device, param } => AutomationTarget::Device { device, param },
        }
    }

    /// The target of `target` on `track`: for a device, `track` isn't used.
    #[must_use]
    pub fn of(track: TrackId, target: AutomationTarget) -> Self {
        match target {
            AutomationTarget::Mixer { param } => Self::Mixer { track, param },
            AutomationTarget::Send { to } => Self::Send { track, to },
            AutomationTarget::Device { device, param } => Self::Device { device, param },
        }
    }
}

/// One slot's mapping: the parameter it modulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Mapping {
    /// The slot, from 0 to 7.
    pub slot: u8,
    /// The parameter it maps.
    pub target: ModTarget,
}

/// One breakpoint of a Curve Mod's shape (R23): a place in the cycle, the
/// output there, and the bend of the segment starting at it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShapePoint {
    /// Where in the cycle, from 0 (its start) to 1 (its end).
    pub x: f64,
    /// The output there, from 0 to 1.
    pub y: f64,
    /// The bend of the segment starting here, from −1 to 1, by the
    /// automation spec's curve law; 0 is straight.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend: f64,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// How close two breakpoints may sit along the cycle.
pub const MIN_GAP: f64 = 1.0 / 4096.0;

/// The output of `points` at `x` in the cycle: the segment's curve between
/// the two points around it, the nearest point's value before the first
/// and after the last, and 0 for no points at all. Allocates nothing.
#[must_use]
pub fn shape_value(points: &[ShapePoint], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if x <= first.x {
        return first.y;
    }
    if x >= last.x {
        return last.y;
    }
    let after = points.partition_point(|p| p.x <= x);
    let (a, b) = (points[after - 1], points[after]);
    curve_value(a.y, b.y, (x - a.x) / (b.x - a.x), a.bend)
}

/// The six preset shapes (R23), Beatlayer's own, by number from 1 to 6: a
/// rise, a fall, a triangle, a pulse, a staircase of four and a decay.
/// `None` for another number.
#[must_use]
pub fn shape_preset(number: u8) -> Option<Vec<ShapePoint>> {
    let p = |x, y, bend| ShapePoint { x, y, bend };
    Some(match number {
        1 => vec![p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0)],
        2 => vec![p(0.0, 1.0, 0.0), p(1.0, 0.0, 0.0)],
        3 => vec![p(0.0, 0.0, 0.0), p(0.5, 1.0, 0.0), p(1.0, 0.0, 0.0)],
        4 => vec![
            p(0.0, 1.0, 0.0),
            p(0.5 - MIN_GAP, 1.0, 0.0),
            p(0.5, 0.0, 0.0),
            p(1.0, 0.0, 0.0),
        ],
        5 => {
            let mut steps = Vec::with_capacity(8);
            for i in 0..4u32 {
                let (x, y) = (f64::from(i) / 4.0, f64::from(i) / 3.0);
                steps.push(p(x + if i == 0 { 0.0 } else { MIN_GAP }, y, 0.0));
                steps.push(p(f64::from(i + 1) / 4.0 - if i == 3 { 0.0 } else { MIN_GAP }, y, 0.0));
            }
            steps
        }
        6 => vec![p(0.0, 1.0, 0.6), p(1.0, 0.0, 0.0)],
        _ => return None,
    })
}

/// `points` with a breakpoint added at (`x`, `y`), kept in order (R23); the
/// same list if another breakpoint is within a gap of `x` or the list is
/// full.
#[must_use]
pub fn with_point_added(points: &[ShapePoint], x: f64, y: f64) -> Vec<ShapePoint> {
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    if points.len() >= MAX_POINTS || points.iter().any(|p| (p.x - x).abs() < MIN_GAP) {
        return points.to_vec();
    }
    let at = points.partition_point(|p| p.x < x);
    let mut out = points.to_vec();
    out.insert(at, ShapePoint { x, y, bend: 0.0 });
    out
}

/// `points` with breakpoint `index` moved to (`x`, `y`): it keeps its bend
/// and its place in the order, a gap from its neighbours (R23).
#[must_use]
pub fn with_point_moved(points: &[ShapePoint], index: usize, x: f64, y: f64) -> Vec<ShapePoint> {
    let mut out = points.to_vec();
    let Some(point) = out.get_mut(index) else {
        return out;
    };
    let low = if index == 0 { 0.0 } else { points[index - 1].x + MIN_GAP };
    let high = points.get(index + 1).map_or(1.0, |next| next.x - MIN_GAP);
    point.x = x.clamp(low, high.max(low));
    point.y = y.clamp(0.0, 1.0);
    out
}

/// `points` with the bend of the segment starting at breakpoint `index` set
/// (R23), from −1 to 1.
#[must_use]
pub fn with_bend(points: &[ShapePoint], index: usize, bend: f64) -> Vec<ShapePoint> {
    let mut out = points.to_vec();
    if let Some(point) = out.get_mut(index) {
        point.bend = bend.clamp(-1.0, 1.0);
    }
    out
}

/// `points` without breakpoint `index` (R23).
#[must_use]
pub fn without_point(points: &[ShapePoint], index: usize) -> Vec<ShapePoint> {
    let mut out = points.to_vec();
    if index < out.len() {
        out.remove(index);
    }
    out
}

/// The shape a new Curve Mod plays: the triangle.
#[must_use]
pub fn default_shape() -> Vec<ShapePoint> {
    shape_preset(3).unwrap_or_default()
}

/// Why `points` isn't a shape: too many, one out of the cycle or the range,
/// or two that aren't in order with a gap between them.
pub(crate) fn check_shape(points: &[ShapePoint]) -> Result<(), ModelError> {
    if points.len() > MAX_POINTS {
        return Err(ModelError::InvalidInput("a shape holds at most 64 breakpoints"));
    }
    for (index, point) in points.iter().enumerate() {
        let ok = [point.x, point.y].iter().all(|v| (0.0..=1.0).contains(v))
            && (-1.0..=1.0).contains(&point.bend);
        if !ok {
            return Err(ModelError::InvalidInput(
                "a breakpoint's place and output are from 0 to 1 and its bend from -1 to 1",
            ));
        }
        if index > 0 && point.x - points[index - 1].x < MIN_GAP * 0.5 {
            return Err(ModelError::InvalidInput(
                "breakpoints are in order along the cycle, with a gap between them",
            ));
        }
    }
    Ok(())
}

/// Why a device's mappings aren't valid: a modulator-less device with any, a
/// slot twice or out of range, a parameter mapped twice.
pub(crate) fn check_mappings(device: &Device) -> Result<(), ModelError> {
    if device.mappings.is_empty() {
        return Ok(());
    }
    if !is_modulator_kind(&device.kind) {
        return Err(ModelError::InvalidInput("only a modulator has mappings"));
    }
    for (i, mapping) in device.mappings.iter().enumerate() {
        let own = matches!(mapping.target, ModTarget::Device { device: d, .. } if d == device.id);
        let repeated = device.mappings[..i]
            .iter()
            .any(|m| m.slot == mapping.slot || m.target == mapping.target);
        if mapping.slot >= SLOTS || own || repeated {
            return Err(ModelError::InvalidInput(
                "a mapping has a slot from 0 to 7 used once, a parameter mapped once, and not \
                 the modulator's own",
            ));
        }
    }
    Ok(())
}

impl Document {
    /// Whether the modulator `device` is in Remote Control: it sets what it
    /// maps rather than modulating it (R16).
    #[must_use]
    pub fn in_remote_control(&self, device: &Device) -> bool {
        device.params.get(&MODE).is_some_and(|mode| *mode >= 0.5)
    }

    /// The modulator, other than `except`, that holds `target` in Remote
    /// Control (R16), if one does.
    #[must_use]
    pub fn held_by(&self, target: ModTarget, except: DeviceId) -> Option<DeviceId> {
        self.tracks().flat_map(|t| t.devices()).find_map(|d| {
            (d.id != except
                && self.in_remote_control(d)
                && d.mappings.iter().any(|m| m.target == target))
            .then_some(d.id)
        })
    }

    /// Refuses a hand move of `target` while a modulator in Remote Control
    /// holds it (R16).
    pub(crate) fn check_not_held(&self, target: ModTarget) -> Result<(), ModelError> {
        let held = self.tracks().flat_map(|t| t.devices()).any(|d| {
            self.in_remote_control(d) && d.mappings.iter().any(|m| m.target == target)
        });
        if held {
            Err(ModelError::InvalidInput(
                "a modulator in Remote Control holds that parameter",
            ))
        } else {
            Ok(())
        }
    }

    /// Whether `target` names a parameter this document has: the track or
    /// device exists, and a send's return track.
    #[must_use]
    pub fn has_target(&self, target: ModTarget) -> bool {
        match target {
            ModTarget::Mixer { track, .. } => self.track(track).is_some(),
            ModTarget::Send { track, to } => {
                self.track(track).is_some_and(|t| t.kind().has_sends()) && self.returns().any(|r| r == to)
            }
            ModTarget::Device { device, .. } => self.device(device).is_some(),
        }
    }

    /// Maps `slot` of the modulator `device` to `target` (R15, R17), in place
    /// of the slot's mapping.
    pub(crate) fn map_modulator(
        &mut self,
        device: DeviceId,
        slot: u8,
        target: ModTarget,
    ) -> Result<(), ModelError> {
        let current = self.device_ref(device)?;
        if !is_modulator_kind(&current.kind) {
            return Err(ModelError::InvalidInput("only a modulator maps parameters"));
        }
        if slot >= SLOTS {
            return Err(ModelError::InvalidInput("a modulator's slots are 0 to 7"));
        }
        if !self.has_target(target) {
            return Err(ModelError::InvalidInput("the parameter isn't in the set"));
        }
        if matches!(target, ModTarget::Device { device: d, .. } if d == device) {
            return Err(ModelError::InvalidInput("a modulator doesn't map its own parameters"));
        }
        if current.mappings.iter().any(|m| m.target == target && m.slot != slot) {
            return Err(ModelError::InvalidInput(
                "a modulator doesn't map one parameter twice",
            ));
        }
        if self.in_remote_control(current) && self.held_by(target, device).is_some() {
            return Err(ModelError::InvalidInput(
                "another modulator in Remote Control already holds that parameter",
            ));
        }
        let mut mappings = current.mappings.clone();
        mappings.retain(|m| m.slot != slot);
        mappings.push(Mapping { slot, target });
        mappings.sort_by_key(|m| m.slot);
        if mappings != current.mappings {
            self.device_mut(device)?.mappings = mappings;
        }
        Ok(())
    }

    /// Removes the mapping of `slot` of the modulator `device`, if it has one.
    pub(crate) fn unmap_modulator(&mut self, device: DeviceId, slot: u8) -> Result<(), ModelError> {
        let current = self.device_ref(device)?;
        if current.mappings.iter().any(|m| m.slot == slot) {
            self.device_mut(device)?.mappings.retain(|m| m.slot != slot);
        }
        Ok(())
    }

    /// Replaces the shape of the Curve Mod `device` (R23).
    pub(crate) fn set_shape(
        &mut self,
        device: DeviceId,
        points: &[ShapePoint],
    ) -> Result<(), ModelError> {
        let current = self.device_ref(device)?;
        let is_curve = matches!(&current.kind, DeviceKind::Native { device_type } if device_type == CURVE_MOD);
        if !is_curve {
            return Err(ModelError::InvalidInput("only a Curve Mod has a shape"));
        }
        check_shape(points)?;
        if current.shape.as_deref() != Some(points) {
            self.device_mut(device)?.shape = Some(points.to_vec());
        }
        Ok(())
    }

    /// Takes every mapping whose target the document no longer has out of
    /// the modulators that held it (R19): the target's device or track was
    /// deleted. It runs in the same undo step as the deletion.
    pub(crate) fn prune_mappings(&mut self) {
        let stale: Vec<DeviceId> = self
            .tracks()
            .flat_map(|t| t.devices())
            .filter(|d| d.mappings.iter().any(|m| !self.has_target(m.target)))
            .map(|d| d.id)
            .collect();
        for id in stale {
            let kept: Vec<Mapping> = self
                .device(id)
                .map(|d| d.mappings.iter().copied().filter(|m| self.has_target(m.target)).collect())
                .unwrap_or_default();
            if let Ok(device) = self.device_mut(id) {
                device.mappings = kept;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_valid_shapes_in_order() {
        for number in 1..=6 {
            let points = shape_preset(number).unwrap();
            check_shape(&points).unwrap_or_else(|e| panic!("preset {number}: {e}"));
        }
        assert!(shape_preset(0).is_none() && shape_preset(7).is_none());
    }

    #[test]
    fn a_shape_follows_its_points_and_curves() {
        let points = shape_preset(1).unwrap();
        assert_eq!(shape_value(&points, 0.25), 0.25);
        assert_eq!(shape_value(&points, 2.0), 1.0, "past the end it holds");
        assert_eq!(shape_value(&[], 0.5), 0.0, "no points, no output");
        let curved = [
            ShapePoint { x: 0.0, y: 0.0, bend: 1.0 },
            ShapePoint { x: 1.0, y: 1.0, bend: 0.0 },
        ];
        assert!(shape_value(&curved, 0.5) < 0.5, "a positive bend bows below the line");
    }

    #[test]
    fn edits_keep_a_shape_valid() {
        let mut points = shape_preset(1).unwrap();
        points = with_point_added(&points, 0.5, 0.9);
        assert_eq!(points.len(), 3);
        assert_eq!(points, with_point_added(&points, 0.5 + MIN_GAP / 2.0, 0.1), "a gap is kept");
        points = with_point_moved(&points, 1, 2.0, -3.0);
        assert!(check_shape(&points).is_ok(), "{points:?}");
        assert!(points[1].x < points[2].x && points[1].y == 0.0);
        points = with_bend(&points, 0, 5.0);
        assert_eq!(points[0].bend, 1.0);
        points = without_point(&points, 1);
        assert_eq!(points.len(), 2);
        assert!(check_shape(&without_point(&points, 0)).is_ok());
    }

    #[test]
    fn a_shape_out_of_order_or_range_is_refused() {
        let p = |x, y| ShapePoint { x, y, bend: 0.0 };
        assert!(check_shape(&[p(0.5, 0.0), p(0.4, 1.0)]).is_err());
        assert!(check_shape(&[p(0.0, 1.5)]).is_err());
        assert!(check_shape(&[p(0.0, 0.0), p(1.0, 1.0)]).is_ok());
    }
}

#[cfg(test)]
mod document_tests {
    use super::*;
    use crate::command::Command;
    use crate::editor::Editor;
    use crate::track::TrackKind;

    struct Set {
        editor: Editor,
        audio: TrackId,
        other: TrackId,
        ret: TrackId,
        wave: DeviceId,
        target: DeviceId,
    }

    fn device(editor: &mut Editor, track: TrackId, device_type: &str) -> DeviceId {
        let id = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::new(
                    id,
                    device_type,
                    DeviceKind::Native {
                        device_type: device_type.to_owned(),
                    },
                ),
            })
            .unwrap();
        id
    }

    fn set() -> Set {
        let mut editor = Editor::new();
        let mut track = |kind| {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: String::new(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            id
        };
        let audio = track(TrackKind::Audio);
        let other = track(TrackKind::Audio);
        let ret = track(TrackKind::Return);
        let wave = device(&mut editor, audio, WAVE_MOD);
        let target = device(&mut editor, other, "beatlayer.eq");
        Set {
            editor,
            audio,
            other,
            ret,
            wave,
            target,
        }
    }

    fn map(editor: &mut Editor, device: DeviceId, slot: u8, target: ModTarget) -> Result<(), String> {
        editor
            .apply(Command::MapModulator {
                device,
                slot,
                target,
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    // Spec: modulation R17
    #[test]
    fn a_modulator_maps_any_track_but_not_itself() {
        let mut set = set();
        let (wave, target) = (set.wave, set.target);
        let on = |param| ModTarget::Device { device: target, param: ParamId(param) };
        map(&mut set.editor, wave, 0, on(1)).unwrap();
        map(&mut set.editor,
            wave,
            1,
            ModTarget::Mixer { track: set.other, param: MixerParam::Pan },
        )
        .unwrap();
        map(&mut set.editor, wave, 2, ModTarget::Send { track: set.other, to: set.ret }).unwrap();
        assert!(
            map(&mut set.editor, wave, 3, ModTarget::Device { device: wave, param: ParamId(1) }).is_err(),
            "not its own parameters"
        );
        assert!(map(&mut set.editor, wave, 3, on(1)).is_err(), "not one parameter twice");
        // A second modulator may modulate the same parameter.
        let second = device(&mut set.editor, set.audio, WAVE_MOD);
        map(&mut set.editor, second, 0, on(1)).unwrap();
        // Only a modulator maps.
        assert!(map(&mut set.editor, target, 0, on(2)).is_err());
        assert!(map(&mut set.editor, wave, 8, on(2)).is_err(), "eight slots");
    }

    // Spec: modulation R17
    #[test]
    fn eight_slots_and_one_per_expression_tab() {
        let mut set = set();
        let wave = set.wave;
        for slot in 0..SLOTS {
            map(&mut set.editor,
                wave,
                slot,
                ModTarget::Device { device: set.target, param: ParamId(u32::from(slot)) },
            )
            .unwrap();
        }
        assert_eq!(set.editor.document().device(wave).unwrap().mappings.len(), 8);
        // A slot takes a new target in place of its old one.
        map(&mut set.editor, wave, 0, ModTarget::Mixer { track: set.audio, param: MixerParam::Volume })
            .unwrap();
        assert_eq!(set.editor.document().device(wave).unwrap().mappings.len(), 8);
    }

    // Spec: modulation R19
    #[test]
    fn a_deleted_target_takes_its_mapping() {
        let mut set = set();
        let (wave, target) = (set.wave, set.target);
        map(&mut set.editor, wave, 0, ModTarget::Device { device: target, param: ParamId(1) }).unwrap();
        map(&mut set.editor, wave, 1, ModTarget::Mixer { track: set.other, param: MixerParam::Volume })
            .unwrap();
        map(&mut set.editor, wave, 2, ModTarget::Mixer { track: set.audio, param: MixerParam::Volume })
            .unwrap();
        // A target moved to another track keeps its mapping.
        set.editor
            .apply(Command::MoveDevice { device: target, track: set.audio, index: 0 })
            .unwrap();
        assert_eq!(set.editor.document().device(wave).unwrap().mappings.len(), 3);
        // Deleting the device and the track take theirs in the same undo step.
        set.editor.apply(Command::RemoveDevice { device: target }).unwrap();
        assert_eq!(set.editor.document().device(wave).unwrap().mappings.len(), 2);
        set.editor.apply(Command::DeleteTrack { track: set.other }).unwrap();
        let slots: Vec<u8> =
            set.editor.document().device(wave).unwrap().mappings.iter().map(|m| m.slot).collect();
        assert_eq!(slots, [2]);
        set.editor.undo().unwrap();
        assert_eq!(set.editor.document().device(wave).unwrap().mappings.len(), 2, "one step");
    }

    // Spec: modulation R16
    #[test]
    fn a_held_parameter_refuses_a_second_holder() {
        let mut set = set();
        let (wave, target) = (set.wave, set.target);
        let second = device(&mut set.editor, set.audio, WAVE_MOD);
        let on = ModTarget::Device { device: target, param: ParamId(1) };
        for who in [wave, second] {
            map(&mut set.editor, who, 0, on).unwrap();
        }
        let hold = |editor: &mut Editor, device| {
            editor.apply(Command::SetDeviceParam { device, param: MODE, value: 1.0 })
        };
        hold(&mut set.editor, wave).unwrap();
        assert!(hold(&mut set.editor, second).is_err(), "the second can't hold it too");
        // Mapping while in Remote Control is refused too.
        let other = ModTarget::Device { device: target, param: ParamId(2) };
        map(&mut set.editor, wave, 1, other).unwrap();
        set.editor.apply(Command::UnmapModulator { device: wave, slot: 0 }).unwrap();
        hold(&mut set.editor, second).unwrap();
        assert!(map(&mut set.editor, wave, 0, on).is_err(), "held by another in Remote Control");
        let third = ModTarget::Device { device: target, param: ParamId(3) };
        assert!(map(&mut set.editor, wave, 2, third).is_ok(), "an unheld parameter maps");
    }
}
