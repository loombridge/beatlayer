//! DSP building blocks: filters, oscillators, envelopes, FFT, resampling, and SIMD helpers.
//!
//! So far: parameter smoothing ([`smooth`]) and oversampling
//! ([`oversample`]), which the native device framework (WP-2.5) gives every
//! device, and the effects' envelope follower ([`follower`]) and seeded
//! random source ([`random`]) (WP-5.42). Everything here that runs per
//! sample allocates nothing, locks nothing, and never blocks, so it's safe
//! on an audio thread.

pub mod follower;
pub mod oversample;
pub mod random;
/// Parameter smoothing, from the plug-in kit's `lspk-params`.
pub use lspk_params::smooth;

pub use follower::EnvelopeFollower;
pub use oversample::Oversampler;
pub use random::Random;
pub use smooth::LinearSmoother;
