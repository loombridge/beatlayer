//! SFZ import (WP-5.14; the multisampler spec, R47 to R53): reading an SFZ
//! file, the open text format for multisample instruments, into the regions
//! a multisampler's layers are made of, the device-wide settings most of the
//! regions agree on, and a report of what was left unread or out.
//!
//! The reader makes no claim about the audio: a region names its sample's
//! path and the settings its opcodes give, and whoever imports it opens the
//! file, takes what the file itself holds (its root key and loop where an
//! opcode says to) and turns the region into a layer. Nothing here panics on
//! a damaged or foreign file: it reads what it can.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bl_model::{ReleaseMode, SustainMode, Zone, ZoneKind};

/// The most regions one file imports; the rest are left out.
pub const MAX_REGIONS: usize = 16_384;

/// How deep `#include` goes, and how much text a file and what it includes
/// may expand to, so a damaged file can't grow without end.
const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_TEXT: usize = 32 << 20;
const MAX_DEFINES: usize = 4096;
const MAX_DEFINE_VALUE: usize = 4096;

/// Where a region's Root Key comes from (R49).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    /// A key the file names.
    Key(u8),
    /// The sample file's own, note 0 where it has none.
    File,
}

/// A region's `loop_mode` (R50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    NoLoop,
    OneShot,
    Continuous,
    Sustain,
}

/// What a region's loop mode makes of its layer (R50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playback {
    pub sustain: SustainMode,
    pub release: ReleaseMode,
    /// The layer's voices play on to Sample End whatever the note does.
    pub one_shot: bool,
}

/// One `<region>`, with the opcodes of the headers above it, as the layer it
/// makes (R48, R49, R50).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// Its place among the file's regions, from 1.
    pub number: usize,
    /// The sample as the file names it.
    pub sample_text: String,
    /// The sample's path: `default_path` and the file's folder in front of
    /// it, slashes forward.
    pub sample: PathBuf,
    pub key: Zone,
    pub velocity: Zone,
    pub root: Root,
    /// Semitones the Root Key moves by, from `tune` past ±50 cents and from
    /// `transpose`.
    pub root_shift: i32,
    /// The Detune in cents, −50 to 50.
    pub detune: f64,
    /// The Volume as a gain.
    pub volume: f64,
    /// The Pan, −50 to 50.
    pub pan: f64,
    pub offset: u64,
    pub end: Option<u64>,
    pub loop_mode: Option<LoopMode>,
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
    /// The loop's crossfade in seconds.
    pub crossfade: Option<f64>,
}

impl Region {
    /// The layer's loops, from its `loop_mode` or, with none, from whether
    /// the sample file has a loop of its own (R50).
    #[must_use]
    pub fn playback(&self, file_has_loop: bool) -> Playback {
        let mode = self.loop_mode.unwrap_or(if file_has_loop {
            LoopMode::Continuous
        } else {
            LoopMode::NoLoop
        });
        let (sustain, release) = match mode {
            LoopMode::NoLoop | LoopMode::OneShot => (SustainMode::Off, ReleaseMode::Off),
            LoopMode::Continuous => (SustainMode::Loop, ReleaseMode::Off),
            LoopMode::Sustain => (SustainMode::Loop, ReleaseMode::Release),
        };
        Playback {
            sustain,
            release,
            one_shot: mode == LoopMode::OneShot,
        }
    }
}

/// How the layers rotate, if the file asks for it (R52).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundRobin {
    Forward,
    Random,
}

/// The filter families (R51).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
}

/// The device's filter, from `fil_type`, `cutoff` and `resonance` (R51).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    pub kind: FilterKind,
    /// 24 dB per octave rather than 12.
    pub steep: bool,
    pub cutoff: f64,
    /// A share of the range, 0 to 100.
    pub resonance: f64,
}

/// An envelope's settings, where the regions give them (R51): times in
/// milliseconds, the sustain in per cent, the depth in semitones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Envelope {
    pub attack: Option<f64>,
    pub decay: Option<f64>,
    pub sustain: Option<f64>,
    pub release: Option<f64>,
    pub depth: Option<f64>,
}

/// The device-wide settings most regions agree on (R51).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// Vel > Vol, per cent.
    pub vel_vol: Option<f64>,
    pub volume_env: Envelope,
    pub filter: Option<Filter>,
    pub filter_env: Envelope,
    pub pitch_env: Envelope,
    /// Pitch Bend Range in semitones.
    pub bend_range: Option<f64>,
    pub reverse: bool,
    /// Fade Mode: constant-power if true, linear if false.
    pub fade_power: Option<bool>,
}

/// A region the import leaves out, and why (R53).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOut {
    /// Its place among the file's regions, from 1.
    pub region: usize,
    /// Its sample, as the file names it.
    pub sample: String,
    pub reason: String,
}

/// What the import left unread or out (R53).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Opcodes and headers left unread, with how many regions use each.
    pub unread: Vec<(String, usize)>,
    /// Opcodes settled for the whole device by the majority of regions, with
    /// how many regions use each.
    pub majority: Vec<(String, usize)>,
    pub left_out: Vec<LeftOut>,
}

/// What an SFZ file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sfz {
    /// The regions that import, in the order their layers take.
    pub regions: Vec<Region>,
    pub device: Device,
    pub round_robin: Option<RoundRobin>,
    pub report: Report,
}

/// The opcodes a region is made from, and the device-wide ones, that are
/// read (everything else is reported).
const REGION_OPCODES: &[&str] = &[
    "sample",
    "lokey",
    "hikey",
    "key",
    "lovel",
    "hivel",
    "pitch_keycenter",
    "xfin_lokey",
    "xfin_hikey",
    "xfout_lokey",
    "xfout_hikey",
    "xfin_lovel",
    "xfin_hivel",
    "xfout_lovel",
    "xfout_hivel",
    "tune",
    "transpose",
    "volume",
    "pan",
    "offset",
    "end",
    "loop_mode",
    "loop_start",
    "loop_end",
    "loop_crossfade",
    "seq_length",
    "seq_position",
    "lorand",
    "hirand",
    "trigger",
];
const DEVICE_OPCODES: &[&str] = &[
    "amp_veltrack",
    "ampeg_attack",
    "ampeg_decay",
    "ampeg_sustain",
    "ampeg_release",
    "fil_type",
    "cutoff",
    "resonance",
    "fileg_attack",
    "fileg_decay",
    "fileg_sustain",
    "fileg_release",
    "fileg_depth",
    "pitcheg_attack",
    "pitcheg_decay",
    "pitcheg_sustain",
    "pitcheg_release",
    "pitcheg_depth",
    "bend_up",
    "bend_down",
    "direction",
    "xf_keycurve",
    "xf_velcurve",
];
const CONTROL_OPCODES: &[&str] = &["default_path", "note_offset", "octave_offset"];

/// The name an opcode is read under: the older spellings the format also
/// has.
fn canonical(name: &str) -> &str {
    match name {
        "loopmode" => "loop_mode",
        "loopstart" => "loop_start",
        "loopend" => "loop_end",
        other => other,
    }
}

/// Reads the SFZ file at `path`, its `#include`s read from disk.
///
/// # Errors
///
/// If the file can't be read.
pub fn read(path: &Path) -> std::io::Result<Sfz> {
    let bytes = std::fs::read(path)?;
    let dir = path.parent().unwrap_or(Path::new(""));
    Ok(parse(&bytes, dir, &|file| std::fs::read(file).ok()))
}

/// Reads SFZ text from `bytes`, the file of the folder `dir`; `include`
/// gives the bytes of an `#include`d file, or `None` where there is none.
#[must_use]
pub fn parse(bytes: &[u8], dir: &Path, include: &dyn Fn(&Path) -> Option<Vec<u8>>) -> Sfz {
    let mut defines = Vec::new();
    let mut budget = MAX_TEXT;
    let text = expand(bytes, dir, include, &mut defines, 0, &mut budget);
    read_text(&text, dir)
}

/// UTF-8 text, or Latin-1 where the bytes aren't valid UTF-8 (R48).
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// Takes the comments out, runs the `#define`s and `#include`s, and gives
/// the text that is left (R48).
fn expand(
    bytes: &[u8],
    dir: &Path,
    include: &dyn Fn(&Path) -> Option<Vec<u8>>,
    defines: &mut Vec<(String, String)>,
    depth: usize,
    budget: &mut usize,
) -> String {
    let text = strip_comments(&decode(bytes));
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#define") {
            let rest = rest.trim();
            let (name, value) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(n, v)| (n, v.trim()));
            if name.starts_with('$') && name.len() > 1 && defines.len() < MAX_DEFINES {
                // A later definition of a name replaces the earlier one. A
                // value that grows past a line's worth is dropped, so
                // defines built on defines can't double without end.
                let value = substitute(value, defines);
                defines.retain(|(n, _)| n != name);
                if value.len() <= MAX_DEFINE_VALUE {
                    defines.push((name.to_owned(), value));
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("#include") {
            let name = rest.trim().trim_matches('"');
            // A path relative to the including file, never an absolute one.
            if depth < MAX_INCLUDE_DEPTH && !name.is_empty() && Path::new(name).is_relative() {
                let path = dir.join(name.replace('\\', "/"));
                if let Some(bytes) = include(&path) {
                    let inner = path.parent().unwrap_or(dir);
                    out.push_str(&expand(&bytes, inner, include, defines, depth + 1, budget));
                }
            }
        } else {
            let line = substitute(line, defines);
            if line.len() >= *budget {
                break;
            }
            *budget -= line.len() + 1;
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// `//` comments to the end of the line and `/* … */` comments between
/// their marks, replaced by nothing (the newlines stay).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('/') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        if after.starts_with("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else if let Some(inside) = after.strip_prefix("/*") {
            match inside.find("*/") {
                Some(end) => {
                    // Keep the lines the comment spanned.
                    out.extend(after[..end + 4].chars().filter(|c| *c == '\n'));
                    rest = &after[end + 4..];
                }
                None => {
                    out.extend(after.chars().filter(|c| *c == '\n'));
                    rest = "";
                }
            }
        } else {
            out.push('/');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `line` with each defined `$NAME` replaced, the longest name first so
/// `$KEY2` isn't taken for `$KEY` and a 2.
fn substitute(line: &str, defines: &[(String, String)]) -> String {
    if defines.is_empty() || !line.contains('$') {
        return line.to_owned();
    }
    let mut order: Vec<&(String, String)> = defines.iter().collect();
    order.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut out = line.to_owned();
    for (name, value) in order {
        if out.contains(name.as_str()) {
            out = out.replace(name.as_str(), value);
        }
    }
    out
}

/// One thing in the text: a header, or an opcode with its value.
enum Item {
    Header(String),
    Opcode(String, String),
}

/// Whether `text` starts an opcode: a name and an equals sign.
fn starts_opcode(text: &str) -> bool {
    let name = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    name > 0 && text[name..].starts_with('=')
}

/// The text as headers and opcodes. A value runs to the line's end or to
/// the next opcode or header, so a `sample` path may hold spaces (R48).
fn items(text: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let Some(first) = rest.chars().next() else {
            break;
        };
        if first.is_whitespace() {
            at += first.len_utf8();
        } else if first == '<' {
            let end = rest.find(['>', '\n']).unwrap_or(rest.len());
            items.push(Item::Header(rest[1..end].trim().to_ascii_lowercase()));
            at += (end + 1).min(rest.len());
        } else if starts_opcode(rest) {
            let eq = rest.find('=').unwrap_or(0);
            let name = rest[..eq].to_ascii_lowercase();
            let value = &rest[eq + 1..];
            let mut end = value.len();
            for (i, c) in value.char_indices() {
                if c == '\n' || c == '<' {
                    end = i;
                    break;
                }
                if c.is_whitespace() {
                    let next = value[i..].trim_start_matches([' ', '\t']);
                    if next.starts_with('<') || starts_opcode(next) {
                        end = i;
                        break;
                    }
                }
            }
            items.push(Item::Opcode(name, value[..end].trim().to_owned()));
            at += eq + 1 + end;
        } else {
            // Something that is neither: skip to the next space.
            at += rest.find(char::is_whitespace).unwrap_or(rest.len()).max(1);
        }
    }
    items
}

type Opcodes = HashMap<String, String>;

/// A note value: a number, or a name such as `c#4` or `eb3`, with `c4`
/// MIDI note 60 (R48).
fn note(value: &str) -> Option<i32> {
    let value = value.trim();
    if let Ok(number) = value.parse::<f64>() {
        return number.is_finite().then(|| number.round() as i32);
    }
    let mut chars = value.chars();
    let letter = chars.next()?.to_ascii_lowercase();
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, octave) = match rest.chars().next()? {
        '#' => (1, &rest[1..]),
        'b' if rest.len() > 1 => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let octave: i32 = octave.parse().ok()?;
    (octave.checked_add(1)?)
        .checked_mul(12)?
        .checked_add(base + shift)
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A frame count from an opcode: a whole number, not below 0.
fn frames(value: &str) -> Option<u64> {
    number(value).filter(|n| *n >= 0.0).map(|n| n as u64)
}

/// A zone over `low` to `high` widened to hold its crossfades (R49), the
/// scale's limits applied; `None` where the range is empty.
fn zone(
    kind: ZoneKind,
    (low, high): (i32, i32),
    fade_in: Option<(i32, i32)>,
    fade_out: Option<(i32, i32)>,
) -> Option<Zone> {
    let (min, max) = kind.range();
    let (mut lo, mut hi) = (low, high);
    let (mut fade_low, mut fade_high) = (0, 0);
    if let Some((from, to)) = fade_in {
        lo = lo.min(from);
        hi = hi.max(to);
        fade_low = (to - from).max(0);
    }
    if let Some((from, to)) = fade_out {
        lo = lo.min(from);
        hi = hi.max(to);
        fade_high = (to - from).max(0);
    }
    // A range wholly off the scale is empty; one partly off is cut to it.
    if hi < i32::from(min) || lo > i32::from(max) || lo > hi {
        return None;
    }
    let (lo, hi) = (
        lo.clamp(i32::from(min), i32::from(max)),
        hi.clamp(i32::from(min), i32::from(max)),
    );
    let span = hi - lo;
    let fade_low = fade_low.min(span);
    let fade_high = fade_high.min(span - fade_low);
    Some(Zone {
        low: lo as u8,
        high: hi as u8,
        fade_low: fade_low as u8,
        fade_high: fade_high as u8,
    })
}

/// The control section's settings.
#[derive(Default)]
struct Control {
    default_path: String,
    /// `note_offset` and `octave_offset` together, in semitones.
    offset: i32,
}

/// The headers' opcodes a region inherits, outermost first.
#[derive(Default)]
struct Scopes {
    global: Opcodes,
    master: Opcodes,
    group: Opcodes,
}

fn read_text(text: &str, dir: &Path) -> Sfz {
    let mut control = Control::default();
    let mut scopes = Scopes::default();
    // Each region as the headers above it and its own opcodes make it, with
    // the control section as it stood.
    let mut merged: Vec<(Opcodes, i32, String)> = Vec::new();
    let mut current: Option<Opcodes> = None;
    let mut unread_headers: BTreeMap<String, usize> = BTreeMap::new();
    let mut section = String::new();
    let mut control_unread: BTreeMap<String, usize> = BTreeMap::new();
    let mut finish = |current: &mut Option<Opcodes>, scopes: &Scopes, control: &Control| {
        if let Some(own) = current.take() {
            let mut all = scopes.global.clone();
            all.extend(scopes.master.clone());
            all.extend(scopes.group.clone());
            all.extend(own);
            merged.push((all, control.offset, control.default_path.clone()));
        }
    };
    for item in items(text) {
        match item {
            Item::Header(name) => {
                finish(&mut current, &scopes, &control);
                match name.as_str() {
                    "control" => {
                        control = Control::default();
                        scopes = Scopes::default();
                    }
                    "global" => scopes = Scopes::default(),
                    "master" => {
                        scopes.master.clear();
                        scopes.group.clear();
                    }
                    "group" => scopes.group.clear(),
                    "region" => current = Some(Opcodes::new()),
                    other => *unread_headers.entry(format!("<{other}>")).or_default() += 1,
                }
                section = name;
            }
            Item::Opcode(name, value) => {
                let name = canonical(&name).to_owned();
                match section.as_str() {
                    "control" => match name.as_str() {
                        "default_path" => control.default_path = value,
                        "note_offset" => {
                            control.offset += number(&value).map_or(0, |n| n as i32);
                        }
                        "octave_offset" => {
                            control.offset +=
                                number(&value).map_or(0, |n| (n as i32).saturating_mul(12));
                        }
                        _ => *control_unread.entry(name).or_default() += 1,
                    },
                    "global" => {
                        scopes.global.insert(name, value);
                    }
                    "master" => {
                        scopes.master.insert(name, value);
                    }
                    "group" => {
                        scopes.group.insert(name, value);
                    }
                    "region" => {
                        if let Some(own) = &mut current {
                            own.insert(name, value);
                        }
                    }
                    // Opcodes of the other headers (curves, effects) and any
                    // before the first header are not read.
                    other => {
                        let key = if other.is_empty() {
                            name
                        } else {
                            format!("<{other}> {name}")
                        };
                        *control_unread.entry(key).or_default() += 1;
                    }
                }
            }
        }
    }
    finish(&mut current, &scopes, &control);
    build(merged, dir, unread_headers, control_unread)
}

/// Turns the merged regions into the import.
fn build(
    merged: Vec<(Opcodes, i32, String)>,
    dir: &Path,
    unread_headers: BTreeMap<String, usize>,
    control_unread: BTreeMap<String, usize>,
) -> Sfz {
    let mut unread: BTreeMap<String, usize> = unread_headers;
    for (name, count) in control_unread {
        *unread.entry(name).or_default() += count;
    }
    let mut regions = Vec::new();
    let mut kept: Vec<&Opcodes> = Vec::new();
    let mut left_out = Vec::new();
    let mut sequences = false;
    let mut windows = false;
    let mut positions = Vec::new();
    for (index, (ops, offset, default_path)) in merged.iter().enumerate() {
        let number_in_file = index + 1;
        for name in ops.keys() {
            let known = REGION_OPCODES.contains(&name.as_str())
                || DEVICE_OPCODES.contains(&name.as_str())
                || CONTROL_OPCODES.contains(&name.as_str());
            if !known {
                *unread.entry(name.clone()).or_default() += 1;
            }
        }
        // A crossfade needs both of its ends to be read.
        for (a, b) in [
            ("xfin_lokey", "xfin_hikey"),
            ("xfout_lokey", "xfout_hikey"),
            ("xfin_lovel", "xfin_hivel"),
            ("xfout_lovel", "xfout_hivel"),
        ] {
            match (ops.contains_key(a), ops.contains_key(b)) {
                (true, false) => *unread.entry(a.to_owned()).or_default() += 1,
                (false, true) => *unread.entry(b.to_owned()).or_default() += 1,
                _ => {}
            }
        }
        let sample_name = ops.get("sample").cloned().unwrap_or_default();
        let mut out = |reason: String| {
            left_out.push(LeftOut {
                region: number_in_file,
                sample: sample_name.clone(),
                reason,
            });
        };
        if regions.len() >= MAX_REGIONS {
            out(format!("past the {MAX_REGIONS} regions one file imports"));
            continue;
        }
        if sample_name.is_empty() {
            out("it has no sample".to_owned());
            continue;
        }
        if let Some(trigger) = ops.get("trigger")
            && !trigger.trim().eq_ignore_ascii_case("attack")
        {
            out(format!("trigger={trigger} isn't played, only attack is"));
            continue;
        }
        match build_region(ops, *offset, default_path, dir, number_in_file) {
            Ok(region) => {
                let sequence = ops.get("seq_length").and_then(|v| frames(v)).is_some()
                    && ops.get("seq_position").and_then(|v| frames(v)).is_some();
                sequences |= sequence;
                windows |= !sequence && (ops.contains_key("lorand") || ops.contains_key("hirand"));
                positions.push(if sequence {
                    ops.get("seq_position").and_then(|v| frames(v)).unwrap_or(0)
                } else {
                    0
                });
                regions.push(region);
                kept.push(ops);
            }
            Err(reason) => out(reason),
        }
    }
    // R52: a sequence turns Round Robin on, in order of position within
    // each key zone.
    let round_robin = if sequences {
        order_by_position(&mut regions, &mut kept, &positions);
        Some(RoundRobin::Forward)
    } else if windows {
        Some(RoundRobin::Random)
    } else {
        None
    };
    let (device, majority) = device_settings(&kept);
    Sfz {
        regions,
        device,
        round_robin,
        report: Report {
            unread: unread.into_iter().collect(),
            majority,
            left_out,
        },
    }
}

/// Puts the regions that share a key zone together, in order of
/// `seq_position`, each zone where its first region was.
fn order_by_position(regions: &mut Vec<Region>, kept: &mut Vec<&Opcodes>, positions: &[u64]) {
    let mut zones: Vec<(u8, u8)> = Vec::new();
    for region in regions.iter() {
        let zone = (region.key.low, region.key.high);
        if !zones.contains(&zone) {
            zones.push(zone);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for zone in zones {
        let mut members: Vec<usize> = (0..regions.len())
            .filter(|&i| (regions[i].key.low, regions[i].key.high) == zone)
            .collect();
        members.sort_by_key(|&i| positions[i]);
        order.extend(members);
    }
    *regions = order.iter().map(|&i| regions[i].clone()).collect();
    *kept = order.iter().map(|&i| kept[i]).collect();
}

/// One region's layer settings from its opcodes (R48 to R50), or why it
/// can't be one.
fn build_region(
    ops: &Opcodes,
    offset: i32,
    default_path: &str,
    dir: &Path,
    number_in_file: usize,
) -> Result<Region, String> {
    let get = |name: &str| ops.get(name).map(String::as_str);
    let key = |name: &str| get(name).and_then(note);
    // `key` sets the zone and the root together; the opcodes after it win.
    let shorthand = key("key");
    let low = key("lokey").or(shorthand).unwrap_or(0);
    let high = key("hikey").or(shorthand).unwrap_or(127);
    if low > high {
        return Err("its lokey is above its hikey".to_owned());
    }
    // The control section's offsets move the keys down (R48).
    let moved = |value: i32| value.saturating_sub(offset);
    let both = |a: &str, b: &str| {
        key(a)
            .zip(key(b))
            .map(|(from, to)| (moved(from), moved(to)))
    };
    let key_zone = zone(
        ZoneKind::Key,
        (moved(low), moved(high)),
        both("xfin_lokey", "xfin_hikey"),
        both("xfout_lokey", "xfout_hikey"),
    )
    .ok_or_else(|| "its keys fall outside the keyboard after note_offset".to_owned())?;
    let velocity = |name: &str| get(name).and_then(number).map(|n| n.round() as i32);
    let vel_pair = |a: &str, b: &str| velocity(a).zip(velocity(b));
    let velocity_zone = zone(
        ZoneKind::Velocity,
        (
            velocity("lovel").unwrap_or(0).max(1),
            velocity("hivel").unwrap_or(127),
        ),
        vel_pair("xfin_lovel", "xfin_hivel"),
        vel_pair("xfout_lovel", "xfout_hivel"),
    )
    .ok_or_else(|| "its velocities are empty".to_owned())?;
    let root = match get("pitch_keycenter") {
        Some(value) if value.trim().eq_ignore_ascii_case("sample") => Root::File,
        Some(value) => {
            note(value).map_or(Root::Key(60), |n| Root::Key(moved(n).clamp(0, 127) as u8))
        }
        None => shorthand.map_or(Root::Key(60), |n| Root::Key(moved(n).clamp(0, 127) as u8)),
    };
    // Detune is within ±50 cents: a tune past it moves the Root Key a
    // semitone the other way and keeps the rest (R49).
    let tune = get("tune")
        .and_then(number)
        .unwrap_or(0.0)
        .clamp(-9600.0, 9600.0);
    let whole = (tune / 100.0).round();
    let transpose = get("transpose")
        .and_then(number)
        .unwrap_or(0.0)
        .clamp(-96.0, 96.0)
        .round();
    let root_shift = -(whole + transpose) as i32;
    let volume_db = get("volume")
        .and_then(number)
        .unwrap_or(0.0)
        .clamp(-144.0, 24.0);
    let mut path = String::new();
    if !default_path.is_empty() {
        path.push_str(&default_path.replace('\\', "/"));
        if !path.ends_with('/') {
            path.push('/');
        }
    }
    path.push_str(&get("sample").unwrap_or_default().replace('\\', "/"));
    let end = match get("end").and_then(number) {
        Some(n) if n < 0.0 => return Err("end is below 0, which plays nothing".to_owned()),
        Some(n) => Some(n as u64),
        None => None,
    };
    Ok(Region {
        number: number_in_file,
        sample_text: get("sample").unwrap_or_default().to_owned(),
        sample: dir.join(path),
        key: key_zone,
        velocity: velocity_zone,
        root,
        root_shift,
        detune: tune - whole * 100.0,
        volume: 10f64.powf(volume_db / 20.0),
        pan: (get("pan").and_then(number).unwrap_or(0.0) / 2.0).clamp(-50.0, 50.0),
        offset: get("offset").and_then(frames).unwrap_or(0),
        end,
        loop_mode: get("loop_mode").and_then(|mode| {
            match mode.trim().to_ascii_lowercase().as_str() {
                "no_loop" => Some(LoopMode::NoLoop),
                "one_shot" => Some(LoopMode::OneShot),
                "loop_continuous" => Some(LoopMode::Continuous),
                "loop_sustain" => Some(LoopMode::Sustain),
                _ => None,
            }
        }),
        loop_start: get("loop_start").and_then(frames),
        loop_end: get("loop_end").and_then(frames),
        crossfade: get("loop_crossfade").and_then(number).filter(|n| *n >= 0.0),
    })
}

/// The value most regions give an opcode, the first region's on a tie, or
/// `None` where most give none; and how many regions give it.
fn majority(regions: &[&Opcodes], name: &str) -> (Option<String>, usize) {
    // (value, count, the first region to give it)
    let mut tally: Vec<(Option<String>, usize, usize)> = Vec::new();
    for (i, ops) in regions.iter().enumerate() {
        let value = ops.get(name).map(|v| v.trim().to_ascii_lowercase());
        match tally.iter_mut().find(|(v, _, _)| *v == value) {
            Some((_, count, _)) => *count += 1,
            None => tally.push((value, 1, i)),
        }
    }
    let given = tally
        .iter()
        .filter(|(v, _, _)| v.is_some())
        .map(|(_, n, _)| n)
        .sum();
    let winner = tally
        .into_iter()
        .max_by_key(|(_, count, first)| (*count, std::cmp::Reverse(*first)))
        .and_then(|(value, _, _)| value);
    (winner, given)
}

/// The regions' opcodes, and which of them settled a device-wide setting.
struct Settle<'a> {
    regions: &'a [&'a Opcodes],
    used: BTreeMap<String, usize>,
}

impl Settle<'_> {
    /// The value most regions give `name`, if most give one.
    fn text(&mut self, name: &str) -> Option<String> {
        let (winner, count) = majority(self.regions, name);
        if count > 0 {
            self.used.insert(name.to_owned(), count);
        }
        winner
    }

    fn value(&mut self, name: &str) -> Option<f64> {
        self.text(name).and_then(|v| number(&v))
    }

    /// An envelope's settings under `prefix`; times from seconds to
    /// milliseconds.
    fn envelope(&mut self, prefix: &str, depth: bool) -> Envelope {
        let time = |this: &mut Self, part: &str| {
            this.value(&format!("{prefix}_{part}")).map(|s| s * 1000.0)
        };
        Envelope {
            attack: time(self, "attack"),
            decay: time(self, "decay"),
            sustain: self.value(&format!("{prefix}_sustain")),
            release: time(self, "release"),
            depth: if depth {
                self.value(&format!("{prefix}_depth"))
                    .map(|cents| cents / 100.0)
            } else {
                None
            },
        }
    }
}

/// The device-wide settings of R51, and which opcodes settled them.
fn device_settings(regions: &[&Opcodes]) -> (Device, Vec<(String, usize)>) {
    let mut settle = Settle {
        regions,
        used: BTreeMap::new(),
    };
    let volume_env = settle.envelope("ampeg", false);
    let filter_env = settle.envelope("fileg", true);
    let pitch_env = settle.envelope("pitcheg", true);
    let vel_vol = settle.value("amp_veltrack");
    let cutoff = settle.value("cutoff");
    let resonance = settle.value("resonance");
    let fil_type = settle.text("fil_type");
    let bend_up = settle.value("bend_up");
    let bend_down = settle.value("bend_down");
    let reverse = settle.text("direction").as_deref() == Some("reverse");
    let curve = settle
        .text("xf_keycurve")
        .or_else(|| settle.text("xf_velcurve"));
    let filter = cutoff.map(|cutoff| {
        let (kind, steep) = match fil_type.as_deref().unwrap_or("lpf_2p") {
            "lpf_4p" | "lpf_6p" => (FilterKind::LowPass, true),
            "hpf_1p" | "hpf_2p" => (FilterKind::HighPass, false),
            "hpf_4p" | "hpf_6p" => (FilterKind::HighPass, true),
            "bpf_1p" | "bpf_2p" => (FilterKind::BandPass, false),
            "brf_1p" | "brf_2p" => (FilterKind::Notch, false),
            _ => (FilterKind::LowPass, false),
        };
        Filter {
            kind,
            steep,
            cutoff,
            resonance: (resonance.unwrap_or(0.0) / 40.0 * 100.0).clamp(0.0, 100.0),
        }
    });
    let device = Device {
        vel_vol,
        volume_env,
        filter,
        filter_env,
        pitch_env,
        bend_range: bend_up
            .or(bend_down)
            .map(|cents| (cents.abs() / 100.0).min(24.0)),
        reverse,
        fade_power: curve.map(|c| c == "power"),
    };
    (device, settle.used.into_iter().collect())
}
