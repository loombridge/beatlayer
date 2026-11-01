//! Automatic tags for samples, from their sound (browser-tags R49, R50, R53).
//!
//! A sample of 60 seconds or less has a descriptor ([`similarity`]). From it
//! this module says what the sample is:
//!
//! - **Loop** when the sound repeats: at least four onsets in at least a
//!   second and a half of sound. Otherwise **One-Shot** when it lasts no more
//!   than four seconds.
//! - A class, for a sample that isn't a loop: a softmax regression over the
//!   descriptor, scaled by its mean and spread in the training sounds. The
//!   class is given only at a confidence of 0.6 or more.
//!
//! The regression's weights are `autotag_weights.txt`, written by
//! `tests/fixtures/gen/fit_autotags.py` from generated, labelled sounds. The
//! file is the script's output; [`VERSION`] goes up when it is fitted again.

use std::sync::OnceLock;

use crate::similarity::{Descriptor, LEN};

/// The classifier's version, kept with each descriptor's tags: a sound
/// tagged by another version is described and tagged again.
pub const VERSION: u32 = 1;

/// The confidence a class needs to become a tag (R53).
pub const THRESHOLD: f32 = 0.6;

/// A loop has at least this many onsets.
const LOOP_ONSETS: f32 = 4.0;

/// A loop lasts at least this long, in seconds.
const LOOP_SECONDS: f32 = 1.5;

/// A one-shot lasts no longer than this, in seconds.
const ONE_SHOT_SECONDS: f32 = 4.0;

/// The weights as the fitting script wrote them.
const WEIGHTS: &str = include_str!("autotag_weights.txt");

/// The regression: the classes it knows, how it scales a descriptor, and each
/// class's weights and bias.
struct Model {
    classes: Vec<&'static str>,
    mean: Vec<f32>,
    scale: Vec<f32>,
    bias: Vec<f32>,
    weights: Vec<Vec<f32>>,
}

fn numbers<'a>(words: impl Iterator<Item = &'a str>) -> Vec<f32> {
    words
        .map(|w| w.parse().expect("a number in the weights"))
        .collect()
}

fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        let mut model = Model {
            classes: Vec::new(),
            mean: Vec::new(),
            scale: Vec::new(),
            bias: Vec::new(),
            weights: Vec::new(),
        };
        for line in WEIGHTS.lines().filter(|l| !l.starts_with('#')) {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("classes") => model.classes = words.collect(),
                Some("mean") => model.mean = numbers(words),
                Some("scale") => model.scale = numbers(words),
                Some("bias") => model.bias = numbers(words),
                // The class's name is the line's second word; the classes
                // line gives the order.
                Some("weights") => {
                    words.next();
                    model.weights.push(numbers(words));
                }
                _ => {}
            }
        }
        assert!(
            model.mean.len() == LEN
                && model.scale.len() == LEN
                && model.bias.len() == model.classes.len()
                && model.weights.len() == model.classes.len()
                && model.weights.iter().all(|w| w.len() == LEN),
            "the weights fit the descriptor"
        );
        model
    })
}

/// What the classifier knows each class by.
#[must_use]
pub fn classes() -> &'static [&'static str] {
    &model().classes
}

/// Each class's confidence for `descriptor`: they add up to one.
#[must_use]
pub fn confidences(descriptor: &Descriptor) -> Vec<(&'static str, f32)> {
    let model = model();
    let x: Vec<f32> = descriptor
        .values()
        .iter()
        .zip(&model.mean)
        .zip(&model.scale)
        .map(|((v, m), s)| (v - m) * s)
        .collect();
    let scores: Vec<f32> = model
        .weights
        .iter()
        .zip(&model.bias)
        .map(|(w, b)| w.iter().zip(&x).map(|(w, x)| w * x).sum::<f32>() + b)
        .collect();
    let top = scores.iter().copied().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = scores.iter().map(|s| (s - top).exp()).collect();
    let total: f32 = exps.iter().sum();
    model
        .classes
        .iter()
        .zip(exps)
        .map(|(class, e)| (*class, e / total))
        .collect()
}

/// Whether the sound repeats (R53's loop detector).
#[must_use]
pub fn is_loop(descriptor: &Descriptor) -> bool {
    let seconds = descriptor.seconds();
    seconds >= LOOP_SECONDS && descriptor.onsets_per_second() * seconds >= LOOP_ONSETS
}

/// The tags a sample's sound earns, by their names in the Function group
/// (R49, R50, R53).
#[must_use]
pub fn sound_tags(descriptor: &Descriptor) -> Vec<&'static str> {
    if is_loop(descriptor) {
        return vec!["Loop"];
    }
    let mut tags = Vec::new();
    if descriptor.seconds() <= ONE_SHOT_SECONDS {
        tags.push("One-Shot");
    }
    let best = confidences(descriptor)
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((class, confidence)) = best
        && confidence >= THRESHOLD
    {
        tags.push(class);
    }
    tags
}
