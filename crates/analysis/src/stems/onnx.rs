//! The Demucs network run through ONNX Runtime (IMP-11).
//!
//! ONNX Runtime isn't linked into Beatlayer: the `ort` crate loads its
//! shared library at run time, from the folder the stem separation
//! packages were installed in, so nothing is downloaded when Beatlayer is
//! built and nothing ships that a person hasn't asked for.
//!
//! The model is HTDemucs exported to ONNX in one of two shapes, told apart
//! by their inputs. The whole graph has its spectrogram inside: one stereo
//! window of 343,980 frames at 44.1 kHz in (`mix`, `[1, 2, L]`), four stereo
//! stems out (`stems`, `[1, 4, 2, L]`). The split model leaves the
//! transform and its inverse out, so a GPU can run the rest (R32): it takes
//! the window and its spectrogram (`mix` and `spec`, `[1, 4, F, T]`) and
//! gives the two branches' outputs (`spec_out`, `[1, 4, 4, F, T]`, and
//! `time_out`, `[1, 4, 2, L]`), which [`Transform`] turns back and adds.
//! Only the split model runs on Core ML (R33).

#[cfg(target_os = "macos")]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

use super::Network;
use super::transform::{Settings, Transform};

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

/// The hardware a model runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// The processor, through ONNX Runtime's own kernels.
    Cpu,
    /// The GPU, through ONNX Runtime's Core ML execution provider.
    CoreMl,
}

impl Provider {
    /// What `stems.status` and the progress text call it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::CoreMl => "Core ML",
        }
    }
}

/// How to load the model.
#[derive(Clone, Debug)]
pub struct Options {
    /// Threads for what runs on the processor.
    pub threads: usize,
    /// Whether to try the platform's GPU provider before the CPU (R33).
    pub accelerate: bool,
    /// Where Core ML keeps its compiled model (R34): a folder for each
    /// model and ONNX Runtime build is made in it. `None` compiles on
    /// every load.
    pub cache: Option<PathBuf>,
}

/// What loading the model came to (R33, R34).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The hardware the network runs on.
    pub provider: Provider,
    /// Why it isn't the GPU, when it was asked to be and isn't.
    pub fallback: Option<String>,
    /// How long the model took to compile, when this load compiled it
    /// rather than found it in the cache.
    pub compiled: Option<Duration>,
    /// How long the whole load took.
    pub load: Duration,
}

impl Report {
    /// The provider, and the reason for a fallback, as a sentence.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.fallback {
            Some(why) => format!("{} (not the GPU: {why})", self.provider.name()),
            None => self.provider.name().to_owned(),
        }
    }
}

/// How the model's graph is cut.
#[derive(Clone)]
enum Kind {
    /// The transform is inside the graph.
    Whole,
    /// The transform is Beatlayer's own.
    Split(Transform),
}

/// HTDemucs, loaded into ONNX Runtime.
pub struct Demucs {
    session: Session,
    kind: Kind,
    report: Report,
    /// What a fallback to the CPU loads again.
    model: PathBuf,
    threads: usize,
}

impl Demucs {
    /// Loads ONNX Runtime's shared library from `runtime` (once per
    /// process) and the model from `model`. `stage` hears what a slow step
    /// is about to be.
    ///
    /// # Errors
    ///
    /// A message if the library or the model can't be loaded on the CPU.
    pub fn load(
        runtime: &Path,
        model: &Path,
        options: &Options,
        stage: &mut dyn FnMut(&str),
    ) -> Result<Self, String> {
        let started = Instant::now();
        let environment = environment(runtime)?;
        // A light look at the model, for its kind and its transform's
        // settings; the session that runs it is made after.
        let probe = session(environment, model, 1, GraphOptimizationLevel::Disable, &[])?;
        let kind = kind_of(&probe)?;
        drop(probe);
        let mut fallback = None;
        if options.accelerate && cfg!(target_os = "macos") {
            match &kind {
                Kind::Whole => {
                    fallback = Some("the model has its transform inside the graph".to_owned());
                }
                Kind::Split(_) => {
                    match Self::on_core_ml(environment, model, options, kind.clone(), stage) {
                        Ok(mut demucs) => {
                            demucs.report.load = started.elapsed();
                            return Ok(demucs);
                        }
                        Err(e) => fallback = Some(e),
                    }
                }
            }
        }
        let session = session(
            environment,
            model,
            options.threads.max(1),
            GraphOptimizationLevel::Level3,
            &[],
        )?;
        Ok(Self {
            session,
            kind,
            report: Report {
                provider: Provider::Cpu,
                fallback,
                compiled: None,
                load: started.elapsed(),
            },
            model: model.to_owned(),
            threads: options.threads,
        })
    }

    /// The model on Core ML, proved by a run on silence: Core ML takes a
    /// model's nodes at load but only shows it can run them on a run.
    fn on_core_ml(
        environment: &ort::environment::Environment,
        model: &Path,
        options: &Options,
        kind: Kind,
        stage: &mut dyn FnMut(&str),
    ) -> Result<Self, String> {
        let (session, compiled) = core_ml(environment, model, options, stage)?;
        let mut demucs = Self {
            session,
            kind,
            report: Report {
                provider: Provider::CoreMl,
                fallback: None,
                compiled,
                load: Duration::ZERO,
            },
            model: model.to_owned(),
            threads: options.threads,
        };
        let started = Instant::now();
        demucs.run_session(&vec![0.0; 2 * demucs.window()])?;
        // Whatever the first run cost is compiling too.
        demucs.report.compiled = demucs.report.compiled.map(|c| c + started.elapsed());
        Ok(demucs)
    }

    /// What loading came to.
    #[must_use]
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// Runs one window on the session as it is.
    fn run_session(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        let window = self.window();
        let mix =
            Tensor::from_array(([1usize, 2, window], input.to_vec())).map_err(|e| e.to_string())?;
        let failed = |e: &dyn std::fmt::Display| format!("the model failed: {e}");
        match &self.kind {
            Kind::Whole => {
                let outputs = self
                    .session
                    .run(ort::inputs!["mix" => mix])
                    .map_err(|e| failed(&e))?;
                let (shape, values) = extract(&outputs, "stems")?;
                expect_shape(shape, &[1, 4, 2, window as i64])?;
                Ok(values.to_vec())
            }
            Kind::Split(transform) => {
                let (bins, frames) = (transform.bins(), transform.frames(window));
                let spec = Tensor::from_array((
                    [1usize, 4, bins, frames],
                    transform.spectrogram(input, window),
                ))
                .map_err(|e| e.to_string())?;
                let outputs = self
                    .session
                    .run(ort::inputs!["mix" => mix, "spec" => spec])
                    .map_err(|e| failed(&e))?;
                let (shape, spectra) = extract(&outputs, "spec_out")?;
                expect_shape(shape, &[1, 4, 4, bins as i64, frames as i64])?;
                let (shape, time) = extract(&outputs, "time_out")?;
                expect_shape(shape, &[1, 4, 2, window as i64])?;
                let mut out = transform.waveforms(spectra, window);
                for (x, t) in out.iter_mut().zip(time) {
                    *x += t;
                }
                Ok(out)
            }
        }
    }
}

impl Network for Demucs {
    fn window(&self) -> usize {
        match &self.kind {
            Kind::Whole => WINDOW,
            Kind::Split(transform) => transform.window_frames(),
        }
    }

    /// Runs one window. A GPU run that fails is run again on the CPU, for
    /// this and every later window (R33).
    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        match self.run_session(input) {
            Err(e) if self.report.provider != Provider::Cpu => {
                let environment = environment(Path::new(""))?;
                self.session = session(
                    environment,
                    &self.model,
                    self.threads.max(1),
                    GraphOptimizationLevel::Level3,
                    &[],
                )?;
                self.report.provider = Provider::Cpu;
                self.report.fallback = Some(e);
                self.run_session(input)
            }
            result => result,
        }
    }
}

/// A session on `model`, on the CPU or with `providers` first.
fn session(
    environment: &ort::environment::Environment,
    model: &Path,
    threads: usize,
    level: GraphOptimizationLevel,
    providers: &[ort::ep::ExecutionProviderDispatch],
) -> Result<Session, String> {
    let failed =
        |e: &dyn std::fmt::Display| format!("couldn't load the model {}: {e}", model.display());
    let mut builder = Session::builder(environment).map_err(|e| failed(&e))?;
    builder = builder
        .with_optimization_level(level)
        .map_err(|e| failed(&e))?;
    builder = builder
        .with_intra_threads(threads)
        .map_err(|e| failed(&e))?;
    if !providers.is_empty() {
        builder = builder
            .with_execution_providers(providers)
            .map_err(|e| failed(&e))?;
    }
    builder.commit_from_file(model).map_err(|e| failed(&e))
}

fn extract<'a>(
    outputs: &'a ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<(&'a ort::value::Shape, &'a [f32]), String> {
    outputs
        .get(name)
        .ok_or_else(|| format!("the model has no output named `{name}`"))?
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())
}

fn expect_shape(shape: &ort::value::Shape, expected: &[i64]) -> Result<(), String> {
    if shape[..] == *expected {
        Ok(())
    } else {
        Err(format!(
            "the model gave an output shaped {:?}, not {expected:?}",
            &shape[..]
        ))
    }
}

/// Which shape of model this is, and for the split one, its transform's
/// settings from its metadata.
fn kind_of(session: &Session) -> Result<Kind, String> {
    if !session.inputs().iter().any(|i| i.name() == "spec") {
        return Ok(Kind::Whole);
    }
    let metadata = session.metadata().map_err(|e| e.to_string())?;
    Settings::from_metadata(|key| metadata.custom(key)).map(|s| Kind::Split(Transform::new(s)))
}

/// Core ML's session for the split model, with its compiled model cached
/// by the model's SHA-256 and the ONNX Runtime build (R34), and how long
/// compiling took if this load did it.
#[cfg(target_os = "macos")]
fn core_ml(
    environment: &ort::environment::Environment,
    model: &Path,
    options: &Options,
    stage: &mut dyn FnMut(&str),
) -> Result<(Session, Option<Duration>), String> {
    use ort::ep::coreml::{ComputeUnits, ModelFormat};
    let mut provider = ort::ep::CoreML::default()
        .with_model_format(ModelFormat::MLProgram)
        // The Neural Engine takes few of the nodes and loses its compiled
        // model on every load; the GPU keeps it and is as fast.
        .with_compute_units(ComputeUnits::CPUAndGPU)
        .with_static_input_shapes(true);
    let mut cold = true;
    if let Some(cache) = &options.cache {
        let folder = cache.join(format!("{}-{}", sha256(model)?, runtime_build()));
        cold = std::fs::read_dir(&folder).map_or(true, |mut entries| entries.next().is_none());
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("couldn't make the Core ML cache {}: {e}", folder.display()))?;
        provider = provider.with_model_cache_dir(folder.to_string_lossy());
    }
    if cold {
        stage("Compiling the model for Core ML");
    }
    let started = Instant::now();
    let session = session(
        environment,
        model,
        options.threads.max(1),
        GraphOptimizationLevel::Level3,
        &[provider.build().error_on_failure()],
    )
    .map_err(|e| format!("Core ML didn't take the model: {e}"))?;
    Ok((session, cold.then(|| started.elapsed())))
}

#[cfg(not(target_os = "macos"))]
fn core_ml(
    _: &ort::environment::Environment,
    _: &Path,
    _: &Options,
    _: &mut dyn FnMut(&str),
) -> Result<(Session, Option<Duration>), String> {
    Err("Core ML is for macOS".to_owned())
}

/// The loaded ONNX Runtime's build, by the commit its build information
/// names: a release's version says no more.
#[cfg(target_os = "macos")]
fn runtime_build() -> String {
    let info = ort::info();
    info.split("git-commit-id=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap_or(info)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(40)
        .collect()
}

/// A file's SHA-256, as lower-case hex.
#[cfg(target_os = "macos")]
fn sha256(path: &Path) -> Result<String, String> {
    let failed = |e: std::io::Error| format!("couldn't read {}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(failed)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(failed)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
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
