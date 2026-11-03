//! A sample's automatic tags from its sound, on generated sounds (spec:
//! browser-tags R49, R50 and R53).

use std::path::PathBuf;

use bl_analysis::autotag::{self, THRESHOLD, sound_tags};
use bl_analysis::similarity::{Descriptor, RATE};
use bl_media::SampleData;

fn fixtures(folder: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(folder)
}

fn load(path: &std::path::Path) -> SampleData {
    bl_media::decode_file(path).unwrap()
}

/// The family fixtures' sounds as `(family, sound)`: the generated sounds the
/// classifier wasn't fitted on.
fn families() -> Vec<(String, SampleData)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(fixtures("families"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name.split('-').next().unwrap().to_owned(), load(&path))
        })
        .collect()
}

// Spec: browser-tags R53. Each class of generated sound gets its tag at least
// 85 % of the time, and the classifier gives a tag only at 0.6 or more.
#[test]
fn generated_kicks_snares_hats_basses_and_pads_get_their_tags() {
    let tag = |family: &str| match family {
        "kick" => "Kick",
        "snare" => "Snare",
        "hat" => "Hi-Hat",
        "bass" => "Bass",
        _ => "Pad",
    };
    let sounds = families();
    for family in ["kick", "snare", "hat", "bass", "pad"] {
        let of_family: Vec<&SampleData> = sounds
            .iter()
            .filter(|(name, _)| name == family)
            .map(|(_, data)| data)
            .collect();
        let right = of_family
            .iter()
            .filter(|data| {
                let descriptor = Descriptor::of_sample(data).unwrap();
                sound_tags(&descriptor).contains(&tag(family))
            })
            .count();
        let share = right as f64 / of_family.len() as f64;
        assert!(share >= 0.85, "{family}: {right} of {}", of_family.len());
    }
    // A confidence under the threshold gives no class: a tone is nothing
    // the classifier was fitted on, and the tags never name a class below it.
    for (_, data) in &sounds {
        let descriptor = Descriptor::of_sample(data).unwrap();
        let confident = autotag::confidences(&descriptor)
            .into_iter()
            .filter(|(_, confidence)| *confidence >= THRESHOLD)
            .count();
        let classes = sound_tags(&descriptor)
            .iter()
            .filter(|tag| autotag::classes().contains(tag))
            .count();
        assert_eq!(classes, confident);
    }
}

/// A loop of two seconds made of the family fixtures' hits, `shift` choosing
/// which hits and `step` the time between them in seconds.
fn drum_loop(sounds: &[(String, SampleData)], shift: usize, step: f64) -> SampleData {
    let hit = |family: &str, index: usize| -> &SampleData {
        let of: Vec<&SampleData> = sounds
            .iter()
            .filter(|(name, _)| name == family)
            .map(|(_, data)| data)
            .collect();
        of[index % of.len()]
    };
    let mut mix = vec![0.0f32; 2 * RATE as usize];
    for beat in 0..8 {
        let family = ["kick", "hat", "snare", "hat"][beat % 4];
        let at = (beat as f64 * step * f64::from(RATE)) as usize;
        let data = hit(family, shift + beat);
        for (i, sample) in data.channels().next().unwrap().iter().enumerate() {
            if let Some(slot) = mix.get_mut(at + i) {
                *slot += sample;
            }
        }
    }
    SampleData::new(RATE, vec![mix]).unwrap()
}

// Spec: browser-tags R53. Loop or One-Shot comes from whether the sound
// repeats: generated loops are loops, and the single sounds are not.
#[test]
fn generated_loops_are_loops_and_single_sounds_are_one_shots() {
    let sounds = families();
    let loops: Vec<SampleData> = (0..8)
        .map(|i| drum_loop(&sounds, i, 0.22 + 0.01 * i as f64))
        .collect();
    let right = loops
        .iter()
        .filter(|data| sound_tags(&Descriptor::of_sample(data).unwrap()) == ["Loop"])
        .count();
    assert!(
        right as f64 / loops.len() as f64 >= 0.85,
        "{right} of 8 loops"
    );
    // The drum loop of the warping tests is one too.
    let fixture = load(&fixtures("drums").join("drum-loop.wav"));
    assert_eq!(
        sound_tags(&Descriptor::of_sample(&fixture).unwrap()),
        ["Loop"]
    );
    for (family, data) in &sounds {
        let tags = sound_tags(&Descriptor::of_sample(data).unwrap());
        assert!(tags.contains(&"One-Shot"), "{family}: {tags:?}");
        assert!(!tags.contains(&"Loop"), "{family}: {tags:?}");
    }
}

// Spec: browser-tags R53. The classes are the Function group's tags.
#[test]
fn the_classes_are_the_classifiers_known_tags_and_their_confidences_add_up_to_one() {
    assert_eq!(
        autotag::classes(),
        ["Kick", "Snare", "Hi-Hat", "Bass", "Pad"]
    );
    let descriptor = Descriptor::of_sample(&families()[0].1).unwrap();
    let total: f32 = autotag::confidences(&descriptor)
        .iter()
        .map(|(_, c)| c)
        .sum();
    assert!((total - 1.0).abs() < 1e-4, "{total}");
}
