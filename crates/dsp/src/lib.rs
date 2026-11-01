//! DSP building blocks: filters, oscillators, envelopes, FFT, resampling, and SIMD helpers.
//!
//! So far: parameter smoothing ([`smooth`]) and oversampling
//! ([`oversample`]), which the native device framework (WP-2.5) gives every
//! device, and the effects' envelope follower ([`follower`]) and seeded
//! random source ([`random`]) (WP-5.42), the physical models
//! ([`physical`]): modal resonators and waveguides (WP-8.5), and spectral
//! processing ([`stft`], WP-8.28) and zero-latency partitioned convolution
//! ([`convolution`], WP-8.29). Everything here that runs per
//! sample allocates nothing, locks nothing, and never blocks, so it's safe
//! on an audio thread.

pub mod convolution;
pub mod follower;
pub mod oversample;
pub mod physical;
pub mod random;
pub mod stft;
/// Parameter smoothing, from the plug-in kit's `lspk-params`.
pub use lspk_params::smooth;

pub use convolution::Convolver;
pub use follower::EnvelopeFollower;
pub use oversample::Oversampler;
pub use random::Random;
pub use smooth::LinearSmoother;
pub use stft::Stft;
