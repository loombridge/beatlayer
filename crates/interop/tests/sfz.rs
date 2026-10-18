//! SFZ import (WP-5.14; the multisampler spec, R48 to R53): the reader over
//! written fixtures, one test for each rule's opcodes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bl_interop::sfz::{self, FilterKind, LoopMode, Root, RoundRobin, Sfz};
use bl_model::{ReleaseMode, SustainMode};

/// Reads `text` as a file in `/lib`, whose `#include`s are `files`.
fn read_with(text: &[u8], files: &[(&str, &str)]) -> Sfz {
    let files: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(name, text)| (PathBuf::from(name), text.as_bytes().to_vec()))
        .collect();
    sfz::parse(text, Path::new("/lib"), &|path| files.get(path).cloned())
}

fn read(text: &str) -> Sfz {
    read_with(text.as_bytes(), &[])
}

fn gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

// Spec: multisampler R48
#[test]
fn headers_inherit_comments_defines_includes_and_note_names() {
    let sfz = read_with(
        br#"
// a comment to the line's end
<control>
default_path=Samples\
#define $LOW c3 // the lowest
/* a comment
   over two lines <region> sample=never.wav */
#include "inc\more.sfz"
<global> volume=-6
<master> pan=20
<group> lovel=1 hivel=63
<region> sample=Kick Drum 1.wav key=$LOW
<region> sample=low/snare.wav lokey=$LOW hikey=$HIGH volume=-12 pitch_keycenter=eb3
<group> lovel=64 hivel=127
<region> sample=hard.wav lokey=c#4 hikey=d4
<master> pan=-20
<region> sample=other.wav lokey=a-1 hikey=10
"#,
        &[(
            "/lib/inc/more.sfz",
            "#define $HIGH e3\n<region> sample=inc.wav key=g3 // included\n",
        )],
    );
    let [included, kick, snare, hard, other] = &sfz.regions[..] else {
        panic!("{:?}", sfz.regions);
    };
    // default_path in front of each sample, both slashes, spaces kept.
    assert_eq!(included.sample, PathBuf::from("/lib/Samples/inc.wav"));
    assert_eq!(kick.sample, PathBuf::from("/lib/Samples/Kick Drum 1.wav"));
    assert_eq!(snare.sample, PathBuf::from("/lib/Samples/low/snare.wav"));
    // A note value is a number or a name, with c4 as 60.
    assert_eq!((included.key.low, included.key.high), (55, 55));
    assert_eq!((kick.key.low, kick.key.high), (48, 48));
    assert_eq!((snare.key.low, snare.key.high), (48, 52));
    assert_eq!(snare.root, Root::Key(51), "eb3");
    assert_eq!((hard.key.low, hard.key.high), (61, 62), "c#4 to d4");
    assert_eq!((other.key.low, other.key.high), (9, 10), "a-1 is 9");
    // Each header lasts until one of its own kind or a higher one.
    assert_eq!((kick.velocity.low, kick.velocity.high), (1, 63));
    assert_eq!((hard.velocity.low, hard.velocity.high), (64, 127));
    assert_eq!(
        (other.velocity.low, other.velocity.high),
        (1, 127),
        "a new master ends the group"
    );
    assert!((kick.pan - 10.0).abs() < 1e-9 && (hard.pan - 10.0).abs() < 1e-9);
    assert!((other.pan + 10.0).abs() < 1e-9);
    assert!(
        (included.volume - 1.0).abs() < 1e-9,
        "the global comes after the include"
    );
    assert!((kick.volume - gain(-6.0)).abs() < 1e-9 && (other.volume - gain(-6.0)).abs() < 1e-9);
    // A region's own opcodes win over the headers'.
    assert!((snare.volume - gain(-12.0)).abs() < 1e-9);
    assert!(sfz.report.left_out.is_empty());
}

// Spec: multisampler R48
#[test]
fn note_offsets_and_latin_1_files_and_stray_text() {
    // note_offset and octave_offset move the keys down; the file here is
    // Latin-1 (the é) and holds text that is neither header nor opcode.
    let mut text = b"<control> note_offset=2 octave_offset=1\nignored-word\n<region> sample=caf\xe9.wav key=72 lokey=70 hikey=80\n".to_vec();
    text.extend_from_slice(b"<region> sample=low.wav hikey=5\n");
    let sfz = read_with(&text, &[]);
    assert_eq!(sfz.regions.len(), 1, "{:?}", sfz.report);
    let region = &sfz.regions[0];
    assert_eq!(region.sample, PathBuf::from("/lib/caf\u{e9}.wav"));
    assert_eq!(
        (region.key.low, region.key.high),
        (56, 66),
        "14 semitones down"
    );
    assert_eq!(region.root, Root::Key(58));
    assert_eq!(
        sfz.report.left_out.len(),
        1,
        "keys 0 to 5 fall below the keyboard"
    );
}

// Spec: multisampler R49
#[test]
fn a_regions_opcodes_set_its_layer() {
    let sfz = read(
        "<region> sample=a.wav key=60 lovel=10 hivel=100 pitch_keycenter=64
           xfin_lokey=50 xfin_hikey=55 xfout_lokey=65 xfout_hikey=70
           xfin_lovel=10 xfin_hivel=20 xfout_lovel=90 xfout_hivel=100
           tune=120 transpose=2 volume=-3 pan=-50 offset=100 end=2000
         <region> sample=b.wav tune=-70 pitch_keycenter=sample
         <region> sample=c.wav key=c4
         <region> sample=d.wav lokey=40 hikey=50 pitch_keycenter=45 xfin_hikey=55",
    );
    let [a, b, c, d] = &sfz.regions[..] else {
        panic!("{:?}", sfz.regions);
    };
    // The key zone widens to hold its crossfades, silent at the first key of
    // a fade in and full at the second.
    assert_eq!(
        (a.key.low, a.key.high, a.key.fade_low, a.key.fade_high),
        (50, 70, 5, 5)
    );
    assert_eq!(
        (
            a.velocity.low,
            a.velocity.high,
            a.velocity.fade_low,
            a.velocity.fade_high
        ),
        (10, 100, 10, 10)
    );
    assert_eq!(a.root, Root::Key(64));
    // A tune past 50 cents moves the Root Key a semitone and keeps the rest;
    // transpose moves the Root Key the other way.
    assert_eq!(a.root_shift, -3);
    assert!((a.detune - 20.0).abs() < 1e-9);
    assert_eq!((b.root_shift, b.root), (1, Root::File));
    assert!((b.detune - 30.0).abs() < 1e-9);
    assert!((a.volume - gain(-3.0)).abs() < 1e-9);
    assert!(
        (a.pan + 25.0).abs() < 1e-9,
        "-100 to 100 is scaled to -50 to 50"
    );
    assert_eq!((a.offset, a.end), (100, Some(2000)));
    // The shorthand key sets the zone and the root together.
    assert_eq!((c.key.low, c.key.high, c.root), (60, 60, Root::Key(60)));
    // Defaults.
    assert_eq!((c.velocity.low, c.velocity.high), (1, 127));
    assert_eq!((c.offset, c.end, c.loop_mode), (0, None, None));
    assert!((c.volume - 1.0).abs() < 1e-9 && c.pan == 0.0 && c.detune == 0.0);
    assert_eq!(c.root_shift, 0);
    // A crossfade with only one of its keys isn't read, and is reported.
    assert_eq!((d.key.low, d.key.high, d.key.fade_low), (40, 50, 0));
    assert!(
        sfz.report
            .unread
            .iter()
            .any(|(name, _)| name == "xfin_hikey")
    );
}

// Spec: multisampler R50
#[test]
fn each_loop_mode_and_the_files_own_loop() {
    let sfz = read(
        "<region> sample=a.wav loop_mode=no_loop
         <region> sample=b.wav loop_mode=loop_continuous loop_start=100 loop_end=900 loop_crossfade=0.01
         <region> sample=c.wav loop_mode=loop_sustain loopstart=5 loopend=50
         <region> sample=d.wav loop_mode=one_shot
         <region> sample=e.wav",
    );
    let [a, b, c, d, e] = &sfz.regions[..] else {
        panic!("{:?}", sfz.regions);
    };
    let play = |r: &sfz::Region, file_loops| {
        let p = r.playback(file_loops);
        (p.sustain, p.release, p.one_shot)
    };
    for file_loops in [false, true] {
        assert_eq!(
            play(a, file_loops),
            (SustainMode::Off, ReleaseMode::Off, false)
        );
        assert_eq!(
            play(b, file_loops),
            (SustainMode::Loop, ReleaseMode::Off, false)
        );
        assert_eq!(
            play(c, file_loops),
            (SustainMode::Loop, ReleaseMode::Release, false)
        );
        assert_eq!(
            play(d, file_loops),
            (SustainMode::Off, ReleaseMode::Off, true)
        );
    }
    // With no loop_mode a sample with a loop of its own loops continuously,
    // and one without doesn't.
    assert_eq!(play(e, true), (SustainMode::Loop, ReleaseMode::Off, false));
    assert_eq!(play(e, false), (SustainMode::Off, ReleaseMode::Off, false));
    assert_eq!(b.loop_mode, Some(LoopMode::Continuous));
    assert_eq!(
        (b.loop_start, b.loop_end, b.crossfade),
        (Some(100), Some(900), Some(0.01))
    );
    assert_eq!(
        (c.loop_start, c.loop_end),
        (Some(5), Some(50)),
        "the older spellings"
    );
    assert_eq!((e.loop_start, e.loop_end, e.crossfade), (None, None, None));
}

// Spec: multisampler R51
#[test]
fn device_wide_opcodes_take_the_majority() {
    let sfz = read(
        "<region> sample=a.wav amp_veltrack=50 ampeg_attack=0.1 ampeg_release=0.5 cutoff=2000 fil_type=lpf_4p resonance=20
            fileg_depth=1200 fileg_attack=0.2 pitcheg_decay=1 bend_up=200 direction=reverse xf_keycurve=gain
         <region> sample=b.wav amp_veltrack=50 ampeg_attack=0.2 cutoff=2000 fil_type=lpf_4p resonance=20
            fileg_depth=1200 bend_up=200 direction=reverse
         <region> sample=c.wav amp_veltrack=100 ampeg_decay=0.3 cutoff=2000 fil_type=lpf_4p resonance=20
            fileg_depth=600 bend_up=400 xf_keycurve=gain",
    );
    let d = &sfz.device;
    assert_eq!(d.vel_vol, Some(50.0), "two regions against one");
    // Three regions, three values: the first region's wins the tie.
    assert_eq!(d.volume_env.attack, Some(100.0), "seconds to milliseconds");
    assert_eq!(d.volume_env.release, None, "most regions give none");
    let filter = d.filter.expect("a cutoff switches the filter on");
    assert_eq!(
        (filter.kind, filter.steep, filter.cutoff),
        (FilterKind::LowPass, true, 2000.0)
    );
    assert!((filter.resonance - 50.0).abs() < 1e-9, "20 dB of 40");
    assert_eq!(d.filter_env.depth, Some(12.0), "cents read as semitones");
    assert_eq!(d.bend_range, Some(2.0));
    assert!(d.reverse);
    assert_eq!(d.fade_power, Some(false), "gain is Lin");
    let used: HashMap<_, _> = sfz.report.majority.iter().cloned().collect();
    assert_eq!(used["amp_veltrack"], 3);
    assert_eq!(used["ampeg_release"], 1);
    assert_eq!(used["direction"], 2);
    // Each filter family.
    for (name, kind, steep) in [
        ("lpf_1p", FilterKind::LowPass, false),
        ("hpf_2p", FilterKind::HighPass, false),
        ("hpf_4p", FilterKind::HighPass, true),
        ("bpf_2p", FilterKind::BandPass, false),
        ("brf_2p", FilterKind::Notch, false),
    ] {
        let sfz = read(&format!("<region> sample=a.wav cutoff=500 fil_type={name}"));
        let filter = sfz.device.filter.unwrap();
        assert_eq!((filter.kind, filter.steep), (kind, steep), "{name}");
    }
    let plain = read("<region> sample=a.wav cutoff=500");
    assert_eq!(
        plain.device.filter.unwrap().kind,
        FilterKind::LowPass,
        "the default type"
    );
    assert!(read("<region> sample=a.wav").device.filter.is_none());
}

// Spec: multisampler R52
#[test]
fn sequences_and_random_windows_turn_round_robin_on() {
    let sequence = read(
        "<group> lokey=40 hikey=50 seq_length=3
         <region> sample=third.wav seq_position=3
         <region> sample=first.wav seq_position=1
         <region> sample=second.wav seq_position=2
         <group> lokey=60 hikey=70 seq_length=2
         <region> sample=y2.wav seq_position=2
         <region> sample=y1.wav seq_position=1",
    );
    assert_eq!(sequence.round_robin, Some(RoundRobin::Forward));
    let order: Vec<_> = sequence
        .regions
        .iter()
        .map(|r| r.sample.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        order,
        ["first.wav", "second.wav", "third.wav", "y1.wav", "y2.wav"],
        "in order of position within each key zone"
    );
    let windows = read(
        "<group> lokey=40 hikey=50
         <region> sample=a.wav lorand=0 hirand=0.5
         <region> sample=b.wav lorand=0.5 hirand=1",
    );
    assert_eq!(windows.round_robin, Some(RoundRobin::Random));
    assert_eq!(read("<region> sample=a.wav").round_robin, None);
    // A sequence wins over windows.
    let both = read("<region> sample=a.wav seq_length=2 seq_position=1 lorand=0 hirand=1");
    assert_eq!(both.round_robin, Some(RoundRobin::Forward));
}

// Spec: multisampler R53
#[test]
fn unread_opcodes_and_skipped_regions_are_reported() {
    let sfz = read(
        "<control> label_cc1=Volume
         <curve> curve_index=1 v000=0 v127=1
         <effect> bus=main
         <region> sample=a.wav group=1 off_by=2 locc64=10 sw_last=36 ampeg_delay=0.1
         <region> sample=b.wav group=1 lochan=2 trigger=attack
         <region> sample=c.wav trigger=release
         <region> sample=d.wav trigger=first
         <region> lokey=1
         <region> sample=e.wav lokey=9 hikey=3
         <region> sample=f.wav end=-1",
    );
    let names: Vec<_> = sfz.regions.iter().map(|r| r.number).collect();
    assert_eq!(names, [1, 2], "only the first two regions are read");
    let unread: HashMap<_, _> = sfz.report.unread.iter().cloned().collect();
    assert_eq!(unread["group"], 2);
    assert_eq!(unread["off_by"], 1);
    assert_eq!(unread["locc64"], 1);
    assert_eq!(unread["sw_last"], 1);
    assert_eq!(unread["lochan"], 1);
    assert_eq!(unread["ampeg_delay"], 1);
    assert_eq!(unread["label_cc1"], 1);
    assert_eq!(unread["<curve>"], 1);
    assert_eq!(unread["<effect>"], 1);
    assert!(!unread.contains_key("trigger") && !unread.contains_key("sample"));
    let out = &sfz.report.left_out;
    assert_eq!(out.len(), 5);
    assert_eq!((out[0].region, out[0].sample.as_str()), (3, "c.wav"));
    assert!(out[0].reason.contains("release"), "{}", out[0].reason);
    assert!(out[1].reason.contains("first"));
    assert!(out[2].reason.contains("no sample"));
    assert!(out[3].reason.contains("lokey"));
    assert!(out[4].reason.contains("end"));
}

// Spec: multisampler R53
#[test]
fn a_damaged_file_never_panics() {
    // The ends of a file in every state: cut off in the middle of a
    // header, an opcode, a comment, an include and a define.
    let whole = "<control> default_path=a/ // c\n#define $K 60\n#include \"x.sfz\"\n/* c */\n\
                 <global> volume=-3\n<group> lokey=$K\n<region> sample=s.wav hikey=c4 tune=1e400 pan=nan\n";
    for end in 0..whole.len() {
        if whole.is_char_boundary(end) {
            let _ = read(&whole[..end]);
        }
    }
    // A file that includes itself, a define that names itself, huge and odd
    // numbers, empty names, and bytes that are no text at all.
    let files = [(
        "/lib/loop.sfz",
        "#include \"loop.sfz\"\n<region> sample=r.wav\n",
    )];
    let looped = read_with(b"#include \"loop.sfz\"\n", &files);
    assert!(looped.regions.len() <= 9, "the include depth is capped");
    for text in [
        "#define $A $A$A$A\n<region> sample=$A key=$A\n",
        "<region> sample=a.wav lokey=99999999999999999999 hikey=-5 tune=-1e308 end=1e30 offset=-4\n",
        "<region> sample= key= =key = <> < <region",
        "<region> sample=a.wav pitch_keycenter=c#\n<region> sample=b.wav key=#4\n",
        "<region> sample=a.wav lovel=300 hivel=-3 xfin_lovel=1 xfin_hivel=900\n",
        "<region> sample=ünï.wav key=é\n",
    ] {
        let _ = read(text);
    }
    // Defines built on defines would double with every line: dropped past a
    // line's worth, so this stays small and quick.
    let mut bomb = String::from("#define $A0 xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    for n in 1..64 {
        bomb.push_str(&format!("#define $A{n} $A{}$A{}\n", n - 1, n - 1));
    }
    bomb.push_str("<region> sample=$A63.wav\n");
    let _ = read(&bomb);
    // An include is relative to the including file, never an absolute path.
    let files = [("/etc/hosts", "<region> sample=leak.wav\n")];
    assert!(
        read_with(b"#include \"/etc/hosts\"\n", &files)
            .regions
            .is_empty()
    );
    let mut state = 0x1234_5678u32;
    for length in [1usize, 7, 64, 500, 4000] {
        for _ in 0..40 {
            let bytes: Vec<u8> = (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    // Mostly the characters an SFZ file is made of.
                    b"<>=/*# \n_$abcdeyz0123456789.-\"\\"[(state >> 8) as usize % 30]
                        | if state & 0x10000 == 0 { 0 } else { 0x80 }
                })
                .collect();
            let _ = read_with(&bytes, &[]);
        }
    }
    let read_from_disk = sfz::read(Path::new("/definitely/not/there.sfz"));
    assert!(
        read_from_disk.is_err(),
        "a missing file is an error, not a panic"
    );
}
