//! Tuning files (WP-6.9; the tunings spec, R3 to R5): the `.scl` reader over
//! the files `tests/fixtures/gen/gen_tunings.py` writes, and over generated
//! text.

use std::fmt::Write as _;
use std::path::PathBuf;

use bl_interop::scala::{self, ScalaError};
use bl_model::Tuning;
use proptest::prelude::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/tunings")
        .join(name)
}

fn read(name: &str) -> Result<Tuning, ScalaError> {
    scala::read(&fixture(name))
}

// Spec: tunings R3, R5
#[test]
fn equal_divisions_read_as_their_steps() {
    for steps in [5usize, 12, 19, 31] {
        let tuning = read(&format!("equal-{steps}.scl")).unwrap();
        assert_eq!(tuning.name, format!("equal-{steps}"));
        assert_eq!(tuning.degrees(), steps);
        for (i, cents) in tuning.cents.iter().enumerate() {
            let want = 1200.0 * (i + 1) as f64 / steps as f64;
            assert!((cents - want).abs() < 1e-5, "{steps}: step {i}");
        }
        assert_eq!(
            tuning.description,
            format!("{steps} equal steps to 1200 cents")
        );
    }
}

// Spec: tunings R3
#[test]
fn ratios_read_as_cents() {
    let tuning = read("just-12.scl").unwrap();
    assert_eq!(tuning.degrees(), 12);
    // 5/4 is the fourth pitch, 386.3137 cents; 2/1 closes the octave.
    assert!((tuning.cents[3] - 1200.0 * 1.25f64.log2()).abs() < 1e-9);
    assert_eq!(tuning.pseudo_octave(), 1200.0);
}

// Spec: tunings R3
#[test]
fn a_non_octave_tuning_keeps_its_pseudo_octave() {
    let tuning = read("bohlen-pierce.scl").unwrap();
    assert_eq!(tuning.degrees(), 13);
    assert!((tuning.pseudo_octave() - 1200.0 * 3f64.log2()).abs() < 1e-9);
}

// Spec: tunings R3
#[test]
fn steps_that_do_not_rise_in_order_are_all_used() {
    let tuning = read("scrambled.scl").unwrap();
    assert_eq!(tuning.cents, vec![300.0, 100.0, 700.0, 1200.0]);
}

// Spec: tunings R3
#[test]
fn comments_blank_lines_and_text_after_a_pitch_are_skipped() {
    let tuning = read("empty-description.scl").unwrap();
    assert_eq!(tuning.description, "");
    assert_eq!(tuning.cents, vec![400.0, 800.0, 1200.0]);
}

// Spec: tunings R5
#[test]
fn the_name_is_the_files_without_its_extension_dash_and_number_kept() {
    let tuning = scala::parse(b"! x\nd\n1\n2/1\n", "Meantone-19.5").unwrap();
    assert_eq!(tuning.name, "Meantone-19.5");
    assert_eq!(read("equal-31.scl").unwrap().name, "equal-31");
    let ascl = std::env::temp_dir().join(format!("bl-scala-{}", std::process::id()));
    std::fs::create_dir_all(&ascl).unwrap();
    let path = ascl.join("Some-tuning-3.ascl");
    std::fs::write(&path, "d\n1\n2/1\n").unwrap();
    assert_eq!(scala::read(&path).unwrap().name, "Some-tuning-3");
    std::fs::remove_file(&path).unwrap();
}

// Spec: tunings R3
#[test]
fn each_refusal_names_its_problem() {
    assert_eq!(read("no-pitches.scl"), Err(ScalaError::NoPitches));
    assert_eq!(read("falling.scl"), Err(ScalaError::PseudoOctaveFalls));
    assert_eq!(
        read("short.scl"),
        Err(ScalaError::CountMismatch { count: 4 })
    );
    assert_eq!(
        read("long.scl"),
        Err(ScalaError::CountMismatch { count: 2 })
    );
    assert_eq!(read("ratio-zero.scl"), Err(ScalaError::OutOfRange(1)));
    assert!(matches!(
        read("missing.scl"),
        Err(ScalaError::Unreadable(_))
    ));
    let refuse = |text: &str| scala::parse(text.as_bytes(), "x").unwrap_err();
    assert_eq!(refuse(""), ScalaError::NoDescription);
    assert_eq!(refuse("! only a comment\n"), ScalaError::NoDescription);
    assert_eq!(refuse("d\nmany\n"), ScalaError::NoCount);
    assert_eq!(refuse("d\n"), ScalaError::NoCount);
    assert_eq!(refuse("d\n1\nfast\n"), ScalaError::BadPitch(1));
    assert_eq!(refuse("d\n1\n3/\n"), ScalaError::BadPitch(1));
    assert_eq!(refuse("d\n1\n-3/2\n"), ScalaError::OutOfRange(1));
    assert_eq!(refuse("d\n1\n3/0\n"), ScalaError::OutOfRange(1));
    assert_eq!(refuse("d\n1\n0.0\n"), ScalaError::PseudoOctaveFalls);
    assert_eq!(refuse("d\n1\n1/1\n"), ScalaError::PseudoOctaveFalls);
    assert_eq!(refuse("d\n1\ninf.0\n"), ScalaError::BadPitch(1));
    assert_eq!(refuse("d\n1\n999999.0\n"), ScalaError::OutOfRange(1));
    assert_eq!(refuse("d\n99999\n"), ScalaError::TooMany(99999));
    assert!(refuse("d\n1\n1\n").to_string().contains("pseudo-octave"));
}

// Spec: tunings R3
#[test]
fn latin_1_text_and_a_byte_order_mark_are_read() {
    let tuning = scala::parse(b"! x\nd\xe9j\xe0\n1\n1200.0\n", "x").unwrap();
    assert_eq!(tuning.description, "déjà");
    let tuning = scala::parse("\u{feff}! x\ndéjà\n1\n1200.0\n".as_bytes(), "x").unwrap();
    assert_eq!(tuning.description, "déjà");
}

// Spec: tunings R3
#[test]
fn a_file_is_read_with_the_default_reference_and_range() {
    let tuning = read("equal-19.scl").unwrap();
    assert_eq!(tuning.reference.octave, 3);
    assert_eq!(tuning.reference.degree, 0);
    assert_eq!(tuning.reference.frequency, 261.626);
    assert_eq!(tuning.key_count(), 128);
    assert!((tuning.key_frequency(60).unwrap() - 261.626).abs() < 1e-9);
}

/// A file's text for `cents`, each with a decimal point and no exponent.
fn written(cents: &[f64]) -> String {
    let mut text = String::from("! generated\nA generated tuning\n");
    writeln!(text, "{}", cents.len()).unwrap();
    for cents in cents {
        match format!("{cents:?}") {
            short if !short.contains('e') => writeln!(text, "{short}").unwrap(),
            _ => writeln!(text, "{cents:.40}").unwrap(),
        }
    }
    text
}

proptest! {
    // Spec: tunings R3. Pitches in any order, negative ones included, are all
    // used as written.
    #[test]
    fn any_pitches_that_end_above_the_unison_read_back_as_written(
        mut cents in prop::collection::vec(-5000.0f64..5000.0, 0..40),
        last in 0.001f64..5000.0,
    ) {
        cents.push(last);
        let tuning = scala::parse(written(&cents).as_bytes(), "t").unwrap();
        prop_assert_eq!(tuning.cents, cents);
    }

    // Spec: tunings R3. A count that is not the number of pitches is refused.
    #[test]
    fn a_wrong_count_is_always_refused(
        cents in prop::collection::vec(1.0f64..5000.0, 1..20),
        extra in 1usize..5,
    ) {
        let text = written(&cents).replacen(
            &format!("\n{}\n", cents.len()),
            &format!("\n{}\n", cents.len() + extra),
            1,
        );
        prop_assert_eq!(
            scala::parse(text.as_bytes(), "t"),
            Err(ScalaError::CountMismatch { count: cents.len() + extra })
        );
    }
}
