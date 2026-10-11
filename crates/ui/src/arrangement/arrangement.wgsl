// The Arrangement View's dense drawing (WP-1.8a; ADR-001): instanced
// rectangles for lanes, grid lines, and clips, and waveforms drawn straight
// from peak data (docs/specs/waveform-peaks.md, "The peak data").

struct Globals {
    // The target's size in pixels.
    screen_px: vec2<f32>,
    pixels_per_point: f32,
    // 1 when the target stores linear colour (an sRGB format), so colours,
    // which arrive in gamma space as egui's do, are converted.
    linear_target: u32,
};

@group(0) @binding(0) var<uniform> g: Globals;
// Every uploaded file's peak data, one after another: [min, max] i8 pairs,
// two peaks to a little-endian word, as bl-media lays them out.
@group(0) @binding(1) var<storage, read> peaks: array<u32>;

fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return c[i];
}

fn to_clip(p_px: vec2<f32>) -> vec4<f32> {
    let n = p_px / g.screen_px * 2.0 - 1.0;
    return vec4<f32>(n.x, -n.y, 0.0, 1.0);
}

fn linear_from_gamma(c: vec3<f32>) -> vec3<f32> {
    let cutoff = c < vec3<f32>(0.04045);
    let lower = c / vec3<f32>(12.92);
    let higher = pow((c + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

// Colours arrive premultiplied, as egui's do.
fn output(color: vec4<f32>) -> vec4<f32> {
    if (g.linear_target == 1u && color.a > 0.0) {
        let straight = color.rgb / color.a;
        return vec4<f32>(linear_from_gamma(straight) * color.a, color.a);
    }
    return color;
}

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn rect_vs(
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
) -> RectOut {
    let p = mix(rect.xy, rect.zw, corner(vi)) * g.pixels_per_point;
    var out: RectOut;
    out.pos = to_clip(p);
    out.color = color;
    return out;
}

@fragment
fn rect_fs(in: RectOut) -> @location(0) vec4<f32> {
    return output(in.color);
}

struct WaveOut {
    @builtin(position) pos: vec4<f32>,
    // The draw rectangle's left edge in pixels.
    @location(0) @interpolate(flat) left_px: f32,
    // Band top and bottom in pixels, peaks per pixel, and the fraction of a
    // peak at the left edge.
    @location(1) @interpolate(flat) band: vec4<f32>,
    // The run's first peak in the buffer, its length, and the whole peak at
    // the left edge.
    @location(2) @interpolate(flat) run: vec4<u32>,
    @location(3) color: vec4<f32>,
};

@vertex
fn wave_vs(
    @builtin(vertex_index) vi: u32,
    @location(0) draw: vec4<f32>,
    @location(1) band: vec4<f32>,
    @location(2) run: vec4<u32>,
    @location(3) color: vec4<f32>,
) -> WaveOut {
    let p = mix(draw.xy, draw.zw, corner(vi)) * g.pixels_per_point;
    var out: WaveOut;
    out.pos = to_clip(p);
    out.left_px = draw.x * g.pixels_per_point;
    out.band = vec4<f32>(band.x * g.pixels_per_point, band.y * g.pixels_per_point, band.z, band.w);
    out.run = run;
    out.color = color;
    return out;
}

fn peak(i: u32) -> vec2<f32> {
    let w = bitcast<i32>(peaks[i >> 1u]);
    let shift = (i & 1u) * 16u;
    let lo = extractBits(w, shift, 8u);
    let hi = extractBits(w, shift + 8u, 8u);
    return vec2<f32>(f32(lo), f32(hi)) / 127.0;
}

@fragment
fn wave_fs(in: WaveOut) -> @location(0) vec4<f32> {
    let len = in.run.y;
    let per_px = in.band.z;
    // The peaks under this pixel column, counted from the left edge's peak.
    let col = floor(in.pos.x) - in.left_px;
    let a = in.band.w + col * per_px;
    let b = a + per_px;
    let first = f32(in.run.z) + max(floor(a), 0.0);
    if (first >= f32(len)) {
        discard;
    }
    let p0 = u32(first);
    let p1 = min(min(max(u32(f32(in.run.z) + ceil(b)), p0 + 1u), p0 + 8u), len);
    var lo = 1.0;
    var hi = -1.0;
    for (var i = p0; i < p1; i = i + 1u) {
        let v = peak(in.run.x + i);
        lo = min(lo, v.x);
        hi = max(hi, v.y);
    }
    let mid = 0.5 * (in.band.x + in.band.y);
    let half = 0.5 * (in.band.y - in.band.x) * 0.92;
    let top = mid - hi * half;
    let bottom = mid - lo * half;
    let y = in.pos.y;
    // At least one pixel tall, so digital silence draws as a line.
    if (y < min(top, mid - 0.5) - 0.5 || y > max(bottom, mid + 0.5) + 0.5) {
        discard;
    }
    return output(in.color);
}
