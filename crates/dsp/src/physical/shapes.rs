//! The partials of each modal shape: ratios to the fundamental and the mode
//! shapes the excitation and listening positions read (physical models R1
//! to R7). Worked out in `f64` and handed over as a [`ModalTable`], which
//! allocates: build it off the audio thread.

use std::f64::consts::PI;

/// A modal resonator's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A free bar.
    Beam,
    /// A bar tuned like a marimba's.
    Marimba,
    /// A bar clamped at one end, for a tine.
    Cantilever,
    /// A rectangular membrane.
    Membrane,
    /// A circular membrane.
    CircularMembrane,
    /// A simply supported rectangular plate.
    Plate,
}

/// How many partials a modal resonator has (R3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 8 partials.
    Eco,
    /// 16 partials.
    Low,
    /// 32 partials, also Meld's resonators' count.
    Medium,
    /// 64 partials.
    High,
}

impl Quality {
    /// How many partials it gives.
    #[must_use]
    pub const fn partials(self) -> usize {
        match self {
            Self::Eco => 8,
            Self::Low => 16,
            Self::Medium => 32,
            Self::High => MAX_PARTIALS,
        }
    }
}

/// The most partials a resonator has.
pub const MAX_PARTIALS: usize = 64;

/// Points along a mode shape, from the centre (0) to the edge (1), between
/// which a position is interpolated.
const POINTS: usize = 257;

/// The roots of cos β cosh β = 1, the free bar's: the first five exactly,
/// the rest by (2n + 1) π / 2, which is exact to 10⁻⁶ from the third.
const FREE_BETA: [f64; 5] = [
    4.730_040_745,
    7.853_204_624,
    10.995_607_84,
    14.137_165_49,
    17.278_759_6,
];

/// The roots of cos β cosh β = −1, the clamped bar's.
const CLAMPED_BETA: [f64; 5] = [
    1.875_104_069,
    4.694_091_133,
    7.854_757_438,
    10.995_540_73,
    14.137_168_39,
];

/// A modal shape's partials: their ratios to the fundamental, and each mode
/// shape sampled along the way from the centre to the edge.
#[derive(Clone, Debug)]
pub struct ModalTable {
    pub(super) ratio: Vec<f64>,
    pub(super) ln_ratio: Vec<f64>,
    /// `partials × POINTS` values, each partial's shape scaled to peak at 1.
    pub(super) profile: Vec<f32>,
    /// What `profile` was made for, so a change of inharmonicity alone
    /// leaves it be: the shape, the partial count, and the aspect (of a
    /// membrane or a plate; 0 for the rest).
    built: Option<(Shape, usize, u64)>,
    /// Room the in-place rebuild works in.
    modes: Vec<Mode>,
    order: Vec<(f64, u32, u32)>,
}

impl ModalTable {
    /// The table of `shape` at `quality`. `aspect` is Ratio, a / b of a
    /// membrane or plate, from 0.25 to 4 (R2); `inharmonicity` is I, from
    /// −1 to 1, moving partial n's ratio r to r^(1 + I / 2) (R4). Values
    /// outside those ranges are clamped; NaN takes the default.
    #[must_use]
    pub fn new(shape: Shape, quality: Quality, aspect: f32, inharmonicity: f32) -> Self {
        let mut table = Self {
            ratio: Vec::with_capacity(MAX_PARTIALS),
            ln_ratio: Vec::with_capacity(MAX_PARTIALS),
            profile: Vec::with_capacity(MAX_PARTIALS * POINTS),
            built: None,
            modes: Vec::with_capacity(MAX_PARTIALS),
            order: Vec::with_capacity(RECTANGLE_MODES),
        };
        table.rebuild(shape, quality, aspect, inharmonicity);
        table
    }

    /// Makes the table `new` would, in place: a table that holds a bank
    /// already makes its next one without allocating, so a control that
    /// changes the shape, the quality, Ratio or inharmonicity can be
    /// followed on the audio thread. The partials' mode shapes are worked
    /// out again only when the shape, the quality or a membrane's or
    /// plate's Ratio changes; a change of inharmonicity alone costs one
    /// power per partial. The one shape that allocates is
    /// [`Shape::CircularMembrane`], whose Bessel zeros are searched for:
    /// make its table with [`new`](Self::new), off the audio thread.
    pub fn rebuild(&mut self, shape: Shape, quality: Quality, aspect: f32, inharmonicity: f32) {
        let aspect = super::sane(aspect, 0.25, 4.0, 1.0);
        let inharmonicity = super::sane(inharmonicity, -1.0, 1.0, 0.0);
        let count = quality.partials();
        self.modes.clear();
        match shape {
            Shape::Beam => free_bar(&mut self.modes, count, false),
            Shape::Marimba => free_bar(&mut self.modes, count, true),
            Shape::Cantilever => cantilever(&mut self.modes, count),
            Shape::Membrane => rectangle(&mut self.modes, &mut self.order, count, aspect, 1.0),
            Shape::Plate => rectangle(&mut self.modes, &mut self.order, count, aspect, 2.0),
            Shape::CircularMembrane => circular(&mut self.modes, count),
        }
        self.ratio.clear();
        self.ln_ratio.clear();
        for mode in &self.modes {
            let ratio = mode.ratio.powf(1.0 + inharmonicity / 2.0);
            self.ratio.push(ratio);
            self.ln_ratio.push(ratio.ln());
        }
        let key = (
            shape,
            count,
            if matches!(shape, Shape::Membrane | Shape::Plate) {
                aspect.to_bits()
            } else {
                0
            },
        );
        if self.built == Some(key) {
            return;
        }
        self.built = Some(key);
        self.profile.clear();
        let mut row = [0.0_f64; POINTS];
        for mode in &self.modes {
            for (i, value) in row.iter_mut().enumerate() {
                *value = mode.form.at(i as f64 / (POINTS - 1) as f64);
            }
            let peak = row.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            self.profile
                .extend(row.iter().map(|&v| (v / peak.max(1e-12)) as f32));
        }
    }

    /// How many partials it has.
    #[must_use]
    pub fn partials(&self) -> usize {
        self.ratio.len()
    }

    /// Partial `n`'s ratio to the fundamental, inharmonicity included.
    #[must_use]
    pub fn ratio(&self, n: usize) -> f64 {
        self.ratio[n]
    }

    pub(super) fn ln_ratio(&self, n: usize) -> f64 {
        self.ln_ratio[n]
    }

    /// How many partials sit below `ratio` times the fundamental.
    pub(super) fn below(&self, ratio: f64) -> usize {
        self.ratio.partition_point(|&r| r < ratio)
    }

    /// Partial `n`'s mode shape at position `p`, from 0 (the centre) to 1
    /// (the edge), between −1 and 1.
    pub(super) fn shape(&self, n: usize, p: f64) -> f64 {
        let at = p.clamp(0.0, 1.0) * (POINTS - 1) as f64;
        let i = (at as usize).min(POINTS - 2);
        let row = &self.profile[n * POINTS..(n + 1) * POINTS];
        let f = at - i as f64;
        f64::from(row[i]) * (1.0 - f) + f64::from(row[i + 1]) * f
    }
}

/// How a partial's mode shape runs from the centre to the edge.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// A free bar's, for a root.
    Free(f64),
    /// A clamped bar's, for a root.
    Clamped(f64),
    /// A rectangle's mode (m, n): sin(mπx) sin(nπy) along the diagonal.
    Rectangle(u32, u32),
    /// A circular membrane's: J_m of the zero along a radius.
    Bessel(u32, f64),
}

impl Form {
    fn at(self, p: f64) -> f64 {
        match self {
            Self::Free(beta) => free_bar_shape(beta, along(p)),
            Self::Clamped(beta) => clamped_bar_shape(beta, along(p)),
            Self::Rectangle(m, n) => {
                let x = along(p);
                (f64::from(m) * PI * x).sin() * (f64::from(n) * PI * x).sin()
            }
            Self::Bessel(m, zero) => bessel(m, zero * p),
        }
    }
}

/// One partial before its table is made: its ratio and its shape along the
/// centre-to-edge position.
#[derive(Clone, Copy, Debug)]
struct Mode {
    ratio: f64,
    form: Form,
}

/// The modes (m, n) of a rectangle that are ordered by frequency: 1 to 16
/// each.
const RECTANGLE_MODES: usize = 256;

/// A free bar's mode shape at `xi` along its length, for root `beta`,
/// written in terms of decaying exponentials so it stays exact at the high
/// roots.
fn free_bar_shape(beta: f64, xi: f64) -> f64 {
    let e = (-beta).exp();
    let (a, b) = ((-beta * xi).exp(), (-beta * (1.0 - xi)).exp());
    let c = (-beta * (1.0 + xi)).exp();
    let (s, co) = (beta.sin(), beta.cos());
    let (sx, cx) = ((beta * xi).sin(), (beta * xi).cos());
    let num = (a - (-beta * (2.0 - xi)).exp()) - s * (b + c)
        + co * (b - c)
        + ((1.0 - e * e) - 2.0 * e * s) * cx
        - ((1.0 + e * e) - 2.0 * e * co) * sx;
    num / ((1.0 - e * e) - 2.0 * e * s)
}

/// A clamped bar's mode shape, written as the free bar's is.
fn clamped_bar_shape(beta: f64, xi: f64) -> f64 {
    let e = (-beta).exp();
    let (a, b) = ((-beta * xi).exp(), (-beta * (1.0 - xi)).exp());
    let c = (-beta * (1.0 + xi)).exp();
    let (s, co) = (beta.sin(), beta.cos());
    let (sx, cx) = ((beta * xi).sin(), (beta * xi).cos());
    let num = (a - (-beta * (2.0 - xi)).exp()) + s * (b + c)
        - co * (b - c)
        - ((1.0 - e * e) + 2.0 * e * s) * cx
        + ((1.0 + e * e) + 2.0 * e * co) * sx;
    num / ((1.0 - e * e) + 2.0 * e * s)
}

/// The nth (from 0) free-bar root.
fn free_beta(n: usize) -> f64 {
    FREE_BETA
        .get(n)
        .copied()
        .unwrap_or((2 * n + 3) as f64 * PI / 2.0)
}

/// The nth (from 0) clamped-bar root.
fn clamped_beta(n: usize) -> f64 {
    CLAMPED_BETA
        .get(n)
        .copied()
        .unwrap_or((2 * n + 1) as f64 * PI / 2.0)
}

/// A bar's position, 0 at its middle and 1 at its end, as a length fraction.
fn along(p: f64) -> f64 {
    (1.0 + p) / 2.0
}

fn free_bar(out: &mut Vec<Mode>, count: usize, marimba: bool) {
    let free = |n: usize| (free_beta(n) / free_beta(0)).powi(2);
    out.extend((0..count).map(|n| {
        let ratio = match (marimba, n) {
            (false, _) => free(n),
            (true, 0) => 1.0,
            (true, 1) => 4.0,
            (true, 2) => 10.0,
            // The free bar's own ratios, scaled to follow the third.
            (true, _) => free(n) * 10.0 / free(2),
        };
        Mode {
            ratio,
            form: Form::Free(free_beta(n)),
        }
    }));
}

fn cantilever(out: &mut Vec<Mode>, count: usize) {
    out.extend((0..count).map(|n| {
        let beta = clamped_beta(n);
        Mode {
            ratio: (beta / clamped_beta(0)).powi(2),
            form: Form::Clamped(beta),
        }
    }));
}

/// A rectangle's modes (m, n), 1 to 16 each, in order of frequency: the
/// frequency goes as the square root of `(m/a)² + (n/b)²` for a membrane
/// (`power` 1) and as `(m/a)² + (n/b)²` for a plate (`power` 2). Each shape
/// is sin(mπx) sin(nπy) along the diagonal from the centre to a corner.
/// `order` is the room for the sort.
fn rectangle(
    out: &mut Vec<Mode>,
    order: &mut Vec<(f64, u32, u32)>,
    count: usize,
    aspect: f64,
    power: f64,
) {
    // Only the ratio of the sides matters: a = aspect, b = 1.
    let freq =
        |m: u32, n: u32| ((f64::from(m) / aspect).powi(2) + f64::from(n * n)).powf(power / 2.0);
    let first = freq(1, 1);
    order.clear();
    order.extend((1..=16).flat_map(|m| (1..=16).map(move |n| (freq(m, n) / first, m, n))));
    // In place, and with ties in the order a stable sort would keep.
    order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    out.extend(order.iter().take(count).map(|&(ratio, m, n)| Mode {
        ratio,
        form: Form::Rectangle(m, n),
    }));
}

/// A circular membrane's modes: each zero of a Bessel function, in order,
/// over the first zero; the shape is J_m(j ρ) along a radius.
fn circular(out: &mut Vec<Mode>, count: usize) {
    let mut zeros: Vec<(f64, u32)> = Vec::new();
    for m in 0..=22 {
        let mut x = f64::from(m).max(0.1);
        let mut last = bessel(m, x);
        while x < 24.0 {
            let next = x + 0.1;
            let value = bessel(m, next);
            if (last < 0.0) != (value < 0.0) {
                let (mut lo, mut hi) = (x, next);
                for _ in 0..50 {
                    let mid = (lo + hi) / 2.0;
                    if (bessel(m, mid) < 0.0) == (last < 0.0) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                zeros.push(((lo + hi) / 2.0, m));
            }
            (x, last) = (next, value);
        }
    }
    zeros.sort_by(|a, b| a.0.total_cmp(&b.0));
    let first = zeros[0].0;
    out.extend(zeros.into_iter().take(count).map(|(zero, m)| Mode {
        ratio: zero / first,
        form: Form::Bessel(m, zero),
    }));
}

/// Bessel's function of the first kind, order `m`, from its integral
/// J_m(x) = 1/π ∫ cos(mτ − x sin τ) dτ over 0 to π, by the trapezoid rule,
/// which converges exponentially on a periodic integrand.
fn bessel(m: u32, x: f64) -> f64 {
    let n = (2.0 * (x.abs() + f64::from(m))) as usize + 64;
    let h = PI / n as f64;
    let mut sum = 0.5 * (1.0 + if m.is_multiple_of(2) { 1.0 } else { -1.0 });
    for i in 1..n {
        let tau = i as f64 * h;
        sum += (f64::from(m) * tau - x * tau.sin()).cos();
    }
    sum / n as f64
}
