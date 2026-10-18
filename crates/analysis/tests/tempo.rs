//! Tempo and beats on generated loops (the warping spec, R30).

use bl_analysis::cache::Analysis;
use bl_analysis::signals::click_track;
use bl_media::SampleData;

const RATE: u32 = 48_000;

fn analyse(bpm: f64, offset: f64, seconds: f64) -> (Analysis, Vec<f64>) {
    let (samples, starts) = click_track(bpm, offset, seconds, RATE);
    let data = SampleData::new(RATE, vec![samples]).unwrap();
    (Analysis::of(&data).unwrap(), starts)
}

// Spec: warping R30
#[test]
fn generated_loops_read_their_tempo_within_half_a_bpm() {
    for bpm in [74.0, 90.0, 97.0, 100.0, 120.0, 128.0, 140.0] {
        let (analysis, _) = analyse(bpm, 0.1, 16.0);
        let tempo = analysis.tempo.unwrap_or_else(|| panic!("{bpm}: no tempo"));
        assert!((tempo.bpm - bpm).abs() < 0.5, "{bpm}: {tempo:?}");
    }
}

// Spec: warping R30, R35
#[test]
fn the_beats_fall_on_the_generated_tracks_clicks() {
    let (analysis, starts) = analyse(128.0, 0.37, 30.0);
    assert_eq!(analysis.beats.len(), starts.len());
    for (beat, click) in analysis.beats.iter().zip(&starts) {
        assert!((beat - click).abs() < 0.001, "{beat} vs {click}");
    }
}

// Spec: warping R30
#[test]
fn a_silent_file_has_no_tempo_and_no_beats() {
    let data = SampleData::new(RATE, vec![vec![0.0; RATE as usize * 8]]).unwrap();
    let analysis = Analysis::of(&data).unwrap();
    assert!(analysis.tempo.is_none() && analysis.beats.is_empty());
}
