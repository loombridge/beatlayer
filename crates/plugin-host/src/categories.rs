//! The table from each plug-in format's categories to the browser's
//! Function tags (spec: browser-tags R49 and R50).
//!
//! A CLAP plug-in declares features, a VST3 class subcategories, an Audio
//! Unit a type and a subtype, an LV2 plug-in classes; each is a word the
//! scanner keeps in [`ScannedPlugin::categories`](crate::scan::ScannedPlugin).
//! The table below gives the Function tag each word stands for, in
//! Beatlayer's own vocabulary. A word with no row gives no tag.

use bl_model::PluginFormat;

/// One format's rows: a category, and the Function tag it gives.
type Rows = &'static [(&'static str, &'static str)];

const CLAP: Rows = &[
    ("synthesizer", "Synth"),
    ("sampler", "Sampler"),
    ("drum", "Drums"),
    ("drum-machine", "Drums"),
    ("analyzer", "Analyser"),
    ("filter", "Filter"),
    ("equalizer", "EQ"),
    ("phaser", "Modulation"),
    ("flanger", "Modulation"),
    ("chorus", "Modulation"),
    ("tremolo", "Modulation"),
    ("de-esser", "Dynamics"),
    ("transient-shaper", "Dynamics"),
    ("compressor", "Dynamics"),
    ("expander", "Dynamics"),
    ("gate", "Dynamics"),
    ("limiter", "Dynamics"),
    ("distortion", "Distortion"),
    ("delay", "Delay"),
    ("reverb", "Reverb"),
    ("pitch-shifter", "Pitch"),
    ("pitch-correction", "Pitch"),
    ("frequency-shifter", "Pitch"),
    ("utility", "Utility"),
];

const VST3: Rows = &[
    ("Synth", "Synth"),
    ("Sampler", "Sampler"),
    ("Drum", "Drums"),
    ("Piano", "Piano"),
    ("Analyzer", "Analyser"),
    ("Filter", "Filter"),
    ("EQ", "EQ"),
    ("Modulation", "Modulation"),
    ("Dynamics", "Dynamics"),
    ("Distortion", "Distortion"),
    ("Delay", "Delay"),
    ("Reverb", "Reverb"),
    ("Pitch Shift", "Pitch"),
    ("Spatial", "Spatial"),
    ("Tools", "Utility"),
];

/// An Audio Unit's rows are its subtypes, four characters each.
const AUDIO_UNIT: Rows = &[
    ("dls ", "Synth"),
    ("samp", "Sampler"),
    ("lpas", "Filter"),
    ("hpas", "Filter"),
    ("bpas", "Filter"),
    ("hshf", "EQ"),
    ("lshf", "EQ"),
    ("pmeq", "EQ"),
    ("greq", "EQ"),
    ("nbeq", "EQ"),
    ("ipeq", "EQ"),
    ("dcmp", "Dynamics"),
    ("mcmp", "Dynamics"),
    ("lmtr", "Dynamics"),
    ("dist", "Distortion"),
    ("dely", "Delay"),
    ("sdly", "Delay"),
    ("rvb2", "Reverb"),
    ("mrev", "Reverb"),
    ("nutp", "Pitch"),
    ("tmpt", "Pitch"),
    ("vari", "Pitch"),
];

/// An LV2 plug-in's rows are its classes in the core vocabulary.
const LV2: Rows = &[
    ("OscillatorPlugin", "Synth"),
    ("FilterPlugin", "Filter"),
    ("LowpassPlugin", "Filter"),
    ("HighpassPlugin", "Filter"),
    ("BandpassPlugin", "Filter"),
    ("CombPlugin", "Filter"),
    ("AllpassPlugin", "Filter"),
    ("EQPlugin", "EQ"),
    ("ParaEQPlugin", "EQ"),
    ("MultiEQPlugin", "EQ"),
    ("ModulatorPlugin", "Modulation"),
    ("ChorusPlugin", "Modulation"),
    ("FlangerPlugin", "Modulation"),
    ("PhaserPlugin", "Modulation"),
    ("DynamicsPlugin", "Dynamics"),
    ("CompressorPlugin", "Dynamics"),
    ("ExpanderPlugin", "Dynamics"),
    ("GatePlugin", "Dynamics"),
    ("LimiterPlugin", "Dynamics"),
    ("DistortionPlugin", "Distortion"),
    ("WaveshaperPlugin", "Distortion"),
    ("DelayPlugin", "Delay"),
    ("ReverbPlugin", "Reverb"),
    ("PitchPlugin", "Pitch"),
    ("SpatialPlugin", "Spatial"),
    ("AnalyserPlugin", "Analyser"),
    ("UtilityPlugin", "Utility"),
    ("AmplifierPlugin", "Utility"),
    ("ConverterPlugin", "Utility"),
    ("FunctionPlugin", "Utility"),
    ("MixerPlugin", "Utility"),
];

fn rows(format: PluginFormat) -> Rows {
    match format {
        PluginFormat::Clap => CLAP,
        PluginFormat::Vst3 => VST3,
        PluginFormat::AudioUnit => AUDIO_UNIT,
        PluginFormat::Lv2 => LV2,
    }
}

/// The Function tags a plug-in's categories give, each once, in the order
/// of the categories.
#[must_use]
pub fn function_tags(format: PluginFormat, categories: &[String]) -> Vec<&'static str> {
    let rows = rows(format);
    let mut tags: Vec<&'static str> = Vec::new();
    for category in categories {
        let found = rows
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(category))
            .map(|(_, tag)| *tag);
        if let Some(tag) = found
            && !tags.contains(&tag)
        {
            tags.push(tag);
        }
    }
    tags
}

/// Every Function tag the table can give, for a check that they are all in
/// the browser's vocabulary.
pub fn all_function_tags() -> impl Iterator<Item = &'static str> {
    [CLAP, VST3, AUDIO_UNIT, LV2]
        .into_iter()
        .flatten()
        .map(|(_, tag)| *tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &[&str]) -> Vec<String> {
        text.iter().map(|word| (*word).to_owned()).collect()
    }

    // Spec: browser-tags R50.
    #[test]
    fn each_formats_categories_map_to_function_tags() {
        let clap = words(&["audio-effect", "reverb", "stereo", "delay"]);
        assert_eq!(
            function_tags(PluginFormat::Clap, &clap),
            ["Reverb", "Delay"]
        );
        let vst3 = words(&["Fx", "Dynamics", "Tools"]);
        assert_eq!(
            function_tags(PluginFormat::Vst3, &vst3),
            ["Dynamics", "Utility"]
        );
        assert_eq!(
            function_tags(PluginFormat::AudioUnit, &words(&["aufx", "rvb2"])),
            ["Reverb"]
        );
        assert_eq!(
            function_tags(
                PluginFormat::Lv2,
                &words(&["FilterPlugin", "LowpassPlugin"])
            ),
            ["Filter"]
        );
    }

    // Spec: browser-tags R50.
    #[test]
    fn a_category_with_no_row_gives_no_tag() {
        let clap = words(&["instrument", "glitch", "stereo"]);
        assert!(function_tags(PluginFormat::Clap, &clap).is_empty());
        assert!(function_tags(PluginFormat::Vst3, &words(&["Fx"])).is_empty());
    }
}
