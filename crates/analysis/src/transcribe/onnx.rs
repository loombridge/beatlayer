//! basic-pitch's network run through ONNX Runtime (IMP-06).
//!
//! The model is Spotify's basic-pitch, Apache-2.0, as its project ships it
//! in ONNX: one window of 43,844 samples at 22,050 Hz in (`[1, 43844, 1]`),
//! three posteriorgrams out, in the order onset and note (`[1, 172, 88]`
//! each) and contour (`[1, 172, 264]`, which the note step doesn't use). The spectrogram
//! is inside the graph.

use std::path::Path;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use super::{FRAMES, Frames, KEYS, Network, WINDOW};
use crate::stems::onnx::environment;

/// Threads the network runs on. Fixed, so the same audio gives the same
/// posteriorgrams on every computer (R17); the network is small enough that
/// one thread keeps up.
const THREADS: usize = 1;

/// basic-pitch, loaded into ONNX Runtime.
pub struct BasicPitch {
    session: Session,
}

impl BasicPitch {
    /// Loads ONNX Runtime's shared library from `runtime` (once per process,
    /// shared with every feature that uses it) and the model from `model`.
    ///
    /// # Errors
    ///
    /// A message if the library or the model can't be loaded, or the model
    /// isn't basic-pitch's.
    pub fn load(runtime: &Path, model: &Path) -> Result<Self, String> {
        let environment = environment(runtime)?;
        let failed =
            |e: &dyn std::fmt::Display| format!("couldn't load the model {}: {e}", model.display());
        let session = Session::builder(environment)
            .map_err(|e| failed(&e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| failed(&e))?
            .with_intra_threads(THREADS)
            .map_err(|e| failed(&e))?
            .with_inter_threads(THREADS)
            .map_err(|e| failed(&e))?
            .with_parallel_execution(false)
            .map_err(|e| failed(&e))?
            .commit_from_file(model)
            .map_err(|e| failed(&e))?;
        if session.inputs().len() != 1 || session.outputs().len() != 3 {
            return Err(format!(
                "{} isn't basic-pitch's model: it has {} inputs and {} outputs",
                model.display(),
                session.inputs().len(),
                session.outputs().len()
            ));
        }
        Ok(Self { session })
    }
}

impl Network for BasicPitch {
    fn run(&mut self, window: &[f32]) -> Result<Frames, String> {
        let tensor = Tensor::from_array(([1usize, WINDOW, 1], window.to_vec()))
            .map_err(|e| e.to_string())?;
        let input = self.session.inputs()[0].name().to_owned();
        let outputs = self
            .session
            .run(ort::inputs![input.as_str() => tensor])
            .map_err(|e| format!("the model failed: {e}"))?;
        let posteriorgram = |index: usize| -> Result<Vec<f32>, String> {
            let (shape, values) = outputs[index]
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if shape[..] != [1, FRAMES as i64, KEYS as i64] {
                return Err(format!(
                    "the model gave a posteriorgram shaped {:?}, not [1, {FRAMES}, {KEYS}]",
                    &shape[..]
                ));
            }
            Ok(values.to_vec())
        };
        Ok(Frames {
            onset: posteriorgram(0)?,
            note: posteriorgram(1)?,
        })
    }
}
