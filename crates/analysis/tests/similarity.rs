//! The sound descriptor on generated families (spec: sound-similarity R9).

use std::path::PathBuf;

use bl_analysis::similarity::{Descriptor, Normaliser, distance};

fn families() -> Vec<(String, Descriptor)> {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/families");
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let family = name.split('-').next().unwrap().to_owned();
            let data = bl_media::decode_file(&path).unwrap();
            (family, Descriptor::of_sample(&data).unwrap())
        })
        .collect()
}

// Spec: sound-similarity R9.
#[test]
fn the_five_nearest_neighbours_are_of_a_sounds_own_family() {
    let sounds = families();
    assert_eq!(sounds.len(), 40);
    let norm = Normaliser::fit(sounds.iter().map(|(_, d)| d));
    let scaled: Vec<Vec<f32>> = sounds.iter().map(|(_, d)| norm.apply(d)).collect();
    let (mut right, mut total) = (0, 0);
    for (i, (family, _)) in sounds.iter().enumerate() {
        let mut others: Vec<(f32, &str)> = (0..sounds.len())
            .filter(|&j| j != i)
            .map(|j| (distance(&scaled[i], &scaled[j]), sounds[j].0.as_str()))
            .collect();
        others.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, other) in others.iter().take(5) {
            total += 1;
            right += usize::from(other == family);
        }
    }
    let share = right as f64 / total as f64;
    assert!(share >= 0.8, "{right} of {total} neighbours: {share}");
}
