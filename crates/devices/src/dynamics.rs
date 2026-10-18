//! What the dynamics devices share: the time-constant step, the level in
//! dB, the static curve with its knee, look-ahead, and the sidechain's
//! filter. Squeeze's gain computer and SC Filter, used by Shutter and Bond
//! too.

use bl_model::param::db_to_gain;

/// The look-ahead times the chooser offers, in seconds (the manual: zero,
/// one and ten ms).
pub(crate) const LOOKAHEAD_SECONDS: [f64; 3] = [0.0, 0.001, 0.010];

/// The SC Filter's Type choices.
pub(crate) const SC_TYPES: [&str; 3] = ["Low Pass", "Band Pass", "High Pass"];

/// The lowest level the static curve reads, in dB: silence is the floor,
/// not minus infinity.
pub(crate) const FLOOR_DB: f64 = -120.0;

/// The deepest reduction the curve asks for, and the highest expansion, in
/// dB. The expansion's cap is Beatlayer's own; Live's limit is to observe.
pub(crate) const MIN_GAIN_DB: f64 = -120.0;
pub(crate) const MAX_EXPAND_DB: f64 = 24.0;

/// A second-order filter (an RBJ biquad), for the sidechain's SC Filter.
/// Coefficients are worked out off the audio thread's hot path, once per
/// stretch, from the parameters' values.
pub(crate) struct ScFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl ScFilter {
    pub(crate) fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Sets its coefficients for `kind` at `hz` with resonance `q`, at
    /// `rate`. The cutoff is held just below Nyquist, so any value is safe.
    pub(crate) fn set(&mut self, kind: usize, hz: f64, q: f64, rate: f64) {
        let w = core::f64::consts::TAU * hz.min(rate * 0.49) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q.max(0.05));
        let (b0, b1, b2, a0, a1, a2) = match kind {
            // A band pass with a 0 dB peak at the center.
            1 => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            // A high pass.
            2 => {
                let k = 0.5 * (1.0 + cos);
                (k, -2.0 * k, k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            // A low pass.
            _ => {
                let k = 0.5 * (1.0 - cos);
                (k, 1.0 - cos, k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
        };
        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    /// One sample through it.
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The look-ahead in samples for the Look chooser's `choice`.
pub(crate) fn lookahead_frames(choice: f64, rate: f64, max: usize) -> usize {
    let choice = (choice.max(0.0) as usize).min(LOOKAHEAD_SECONDS.len() - 1);
    ((LOOKAHEAD_SECONDS[choice] * rate).ceil() as usize).min(max)
}

/// The per-sample step of a one-pole whose time constant is `ms`
/// milliseconds: after `ms` it has closed the gap by 1 - 1/e.
pub(crate) fn step_of(ms: f64, rate: f64) -> f32 {
    let samples = (ms * 0.001 * rate).max(1.0);
    (1.0 - (-1.0 / samples).exp()) as f32
}

/// The detector's level in dB for `level`, floored at [`FLOOR_DB`].
pub(crate) fn level_db(level: f64) -> f64 {
    20.0 * level.max(db_to_gain(FLOOR_DB)).log10()
}

/// The static curve's gain in dB for a level `db`: compression turns the
/// gain down above the threshold over the knee, expansion turns it up.
/// `slope` is the dB the curve moves per dB over the threshold.
pub(crate) fn curve_db(db: f64, threshold: f64, slope: f64, knee: f64, expand: bool) -> f64 {
    let over = db - threshold;
    let half = 0.5 * knee;
    let shape = if knee > 0.0 && over < half && over > -half {
        (over + half) * (over + half) / (2.0 * knee)
    } else {
        over.max(0.0)
    };
    if expand {
        (slope * shape).min(MAX_EXPAND_DB)
    } else {
        (-slope * shape).max(MIN_GAIN_DB)
    }
}
