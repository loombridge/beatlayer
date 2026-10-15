//! Metronome commands and serialization tests (WP-2.15).

use bl_commands::Registry;
use bl_core::time::{Ticks, TimeSignature};
use bl_model::{
    Command, CountIn, Document, Editor, MetronomeRhythm, MetronomeSettings, MetronomeSound,
    registry,
};

#[test]
fn metronome_commands_and_undo_redo() {
    // Spec: metronome R8
    let mut editor = Editor::new();
    assert_eq!(
        editor.document().settings().metronome,
        MetronomeSettings::default()
    );

    // 1. Enable metronome
    editor
        .apply(Command::SetMetronomeEnabled { enabled: true })
        .unwrap();
    assert!(editor.document().settings().metronome.enabled);

    // 2. Count-In
    editor
        .apply(Command::SetMetronomeCountIn {
            count_in: CountIn::Bars2,
        })
        .unwrap();
    assert_eq!(
        editor.document().settings().metronome.count_in,
        CountIn::Bars2
    );

    // 3. Sound
    editor
        .apply(Command::SetMetronomeSound {
            sound: MetronomeSound::Wood,
        })
        .unwrap();
    assert_eq!(
        editor.document().settings().metronome.sound,
        MetronomeSound::Wood
    );

    // 4. Rhythm
    editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: MetronomeRhythm::Eighth,
        })
        .unwrap();
    assert_eq!(
        editor.document().settings().metronome.rhythm,
        MetronomeRhythm::Eighth
    );

    // 5. Only while recording
    editor
        .apply(Command::SetMetronomeOnlyWhileRecording {
            only_while_recording: true,
        })
        .unwrap();
    assert!(editor.document().settings().metronome.only_while_recording);

    // Undo step by step
    editor.undo().unwrap();
    assert!(!editor.document().settings().metronome.only_while_recording);

    editor.undo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.rhythm,
        MetronomeRhythm::Auto
    );

    editor.undo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.sound,
        MetronomeSound::Classic
    );

    editor.undo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.count_in,
        CountIn::None
    );

    editor.undo().unwrap();
    assert!(!editor.document().settings().metronome.enabled);

    // Redo step by step
    editor.redo().unwrap();
    assert!(editor.document().settings().metronome.enabled);

    editor.redo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.count_in,
        CountIn::Bars2
    );

    editor.redo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.sound,
        MetronomeSound::Wood
    );

    editor.redo().unwrap();
    assert_eq!(
        editor.document().settings().metronome.rhythm,
        MetronomeRhythm::Eighth
    );

    editor.redo().unwrap();
    assert!(editor.document().settings().metronome.only_while_recording);
}

#[test]
fn metronome_set_roundtrip() {
    // Spec: metronome R8
    let editor_default = Editor::new();
    let doc_default = editor_default.document();
    let json_default = serde_json::to_string_pretty(doc_default).unwrap();
    // Default settings are skipped in serialization
    assert!(!json_default.contains("\"metronome\""));

    let doc_restored: Document = serde_json::from_str(&json_default).unwrap();
    assert_eq!(
        doc_restored.settings().metronome,
        MetronomeSettings::default()
    );

    // Non-default settings
    let mut editor = Editor::new();
    let custom = MetronomeSettings {
        enabled: true,
        count_in: CountIn::Bars4,
        sound: MetronomeSound::Click,
        rhythm: MetronomeRhythm::Sixteenth,
        only_while_recording: true,
    };
    editor
        .apply(Command::SetMetronomeEnabled {
            enabled: custom.enabled,
        })
        .unwrap();
    editor
        .apply(Command::SetMetronomeCountIn {
            count_in: custom.count_in,
        })
        .unwrap();
    editor
        .apply(Command::SetMetronomeSound {
            sound: custom.sound,
        })
        .unwrap();
    editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: custom.rhythm,
        })
        .unwrap();
    editor
        .apply(Command::SetMetronomeOnlyWhileRecording {
            only_while_recording: custom.only_while_recording,
        })
        .unwrap();

    let json_custom = serde_json::to_string_pretty(editor.document()).unwrap();
    assert!(json_custom.contains("\"metronome\""));
    assert!(json_custom.contains("\"4_bars\""));
    assert!(json_custom.contains("\"click\""));
    assert!(json_custom.contains("\"1/16\""));

    let doc_custom_restored: Document = serde_json::from_str(&json_custom).unwrap();
    assert_eq!(doc_custom_restored.settings().metronome, custom);
}

#[test]
fn metronome_registered_commands() {
    // Spec: metronome R8
    let mut reg = Registry::new();
    registry::register(&mut reg).unwrap();

    for id in [
        "transport.set_metronome_enabled",
        "transport.set_metronome_count_in",
        "transport.set_metronome_sound",
        "transport.set_metronome_rhythm",
        "transport.set_metronome_only_while_recording",
    ] {
        let spec = reg.get(id).expect("command must be registered");
        assert!(!spec.description.is_empty());
        assert!(spec.args.is_some());
    }
}

#[test]
fn auto_follows_the_denominator_and_divisions_fit_the_bar() {
    // Spec: metronome R4
    let four_four = TimeSignature::new(4, 4).unwrap();
    // Auto follows the time signature's denominator: a quarter in 4/4.
    assert_eq!(MetronomeRhythm::Auto.interval(four_four), Ticks::QUARTER);
    assert_eq!(
        MetronomeRhythm::Half.interval(four_four),
        Ticks(Ticks::WHOLE.get() / 2)
    );
    for rhythm in [
        MetronomeRhythm::Auto,
        MetronomeRhythm::Bar1,
        MetronomeRhythm::Half,
        MetronomeRhythm::HalfTriplet,
        MetronomeRhythm::Quarter,
        MetronomeRhythm::QuarterTriplet,
        MetronomeRhythm::Eighth,
        MetronomeRhythm::EighthTriplet,
        MetronomeRhythm::Sixteenth,
        MetronomeRhythm::SixteenthTriplet,
    ] {
        assert!(rhythm.fits_in_bar(four_four), "{rhythm:?} fits a 4/4 bar");
    }

    // A half note doesn't fit a 1/4 bar, so its clicks fall back to Auto.
    let one_four = TimeSignature::new(1, 4).unwrap();
    assert!(!MetronomeRhythm::Half.fits_in_bar(one_four));
    assert_eq!(
        MetronomeRhythm::Half.effective_interval(one_four),
        MetronomeRhythm::Auto.interval(one_four)
    );

    // In 6/8, Auto clicks the eighths.
    let six_eight = TimeSignature::new(6, 8).unwrap();
    assert_eq!(
        MetronomeRhythm::Auto.interval(six_eight),
        Ticks(Ticks::WHOLE.get() / 8)
    );
}
