//! The Demucs network run through ONNX Runtime (IMP-11).
//!
//! ONNX Runtime isn't linked into Beatlayer: the `ort` crate loads its
//! shared library at run time, from the folder the stem separation
//! packages were installed in, so nothing is downloaded when Beatlayer is
//! built and nothing ships that a person hasn't asked for. The model is
//! HTDemucs exported to ONNX with its spectrogram inside the graph: one
//! stereo window of 343,980 frames at 44.1 kHz in (`mix`, `[1, 2, L]`),
//! four stereo stems out (`stems`, `[1, 4, 2, L]`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use super::Network;

/// HTDemucs's window, in frames: 7.8 seconds at 44.1 kHz.
pub const WINDOW: usize = 343_980;

/// The process's one ONNX Runtime environment, made by the first feature
/// that needs it; the library it loaded stays loaded.
static ENVIRONMENT: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

/// The library the environment loaded.
static LOADED: OnceLock<PathBuf> = OnceLock::new();

/// How many times a library has been loaded: at most once.
static LOADS: AtomicUsize = AtomicUsize::new(0);

/// How many times this process has loaded ONNX Runtime's library.
#[must_use]
pub fn runtime_loads() -> usize {
    LOADS.load(Ordering::Relaxed)
}

/// The library this process loaded, if one is.
#[must_use]
pub fn loaded_runtime() -> Option<&'static Path> {
    LOADED.get().map(PathBuf::as_path)
}

/// The environment every on-device feature runs its models in. The first
/// call loads `runtime`'s library; later calls, from any feature, use the
/// loaded one whatever path they name (the Packages manager's R19).
///
/// # Errors
///
/// A message if the library can't be loaded.
pub fn environment(runtime: &Path) -> Result<&'static ort::environment::Environment, String> {
    ENVIRONMENT
        .get_or_init(|| {
            LOADS.fetch_add(1, Ordering::Relaxed);
            let environment = ort::init_from(runtime)
                .map_err(|e| format!("couldn't load ONNX Runtime from {}: {e}", runtime.display()))?
                .with_name("beatlayer")
                .with_telemetry(false)
                .build()
                .map_err(|e| format!("couldn't start ONNX Runtime: {e}"))?;
            let _ = LOADED.set(runtime.to_owned());
            Ok(environment)
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// HTDemucs, loaded into ONNX Runtime.
pub struct Demucs {
    session: Session,
}

impl Demucs {
    /// Loads ONNX Runtime's shared library from `runtime` (once per
    /// process) and the model from `model`, running on `threads` threads.
    ///
    /// # Errors
    ///
    /// A message if the library or the model can't be loaded.
    pub fn load(runtime: &Path, model: &Path, threads: usize) -> Result<Self, String> {
        let environment = environment(runtime)?;
        let failed =
            |e: &dyn std::fmt::Display| format!("couldn't load the model {}: {e}", model.display());
        let mut builder = Session::builder(environment)
            .map_err(|e| failed(&e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| failed(&e))?
            .with_intra_threads(threads.max(1))
            .map_err(|e| failed(&e))?;
        let session = builder.commit_from_file(model).map_err(|e| failed(&e))?;
        Ok(Self { session })
    }
}

impl Network for Demucs {
    fn window(&self) -> usize {
        WINDOW
    }

    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        let tensor =
            Tensor::from_array(([1usize, 2, WINDOW], input.to_vec())).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["mix" => tensor])
            .map_err(|e| format!("the model failed: {e}"))?;
        let stems = outputs
            .get("stems")
            .ok_or_else(|| "the model has no output named `stems`".to_owned())?;
        let (shape, values) = stems
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let expected = [1, 4, 2, WINDOW as i64];
        if shape[..] != expected[..] {
            return Err(format!(
                "the model gave stems shaped {:?}, not {expected:?}",
                &shape[..]
            ));
        }
        Ok(values.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: stem-separation R19. Two features asking for the library load
    // it once: the second finds the first's environment, here its refusal,
    // whatever path it names.
    #[test]
    fn the_library_loads_once_for_every_feature() {
        let first = environment(Path::new("no-such-folder/first.lib"));
        let second = environment(Path::new("no-such-folder/second.lib"));
        assert!(first.is_err());
        assert_eq!(first.err(), second.err());
        assert_eq!(runtime_loads(), 1);
        assert_eq!(loaded_runtime(), None);
    }
}
