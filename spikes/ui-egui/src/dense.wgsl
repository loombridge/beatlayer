// Dense views for the egui spike: instanced rectangles, and waveforms drawn
// straight from min/max peak data in a storage buffer.

struct Globals {
    screen_px: vec2<f32>,
    pixels_per_point: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
// Interleaved i8 [min, max] pairs, four bytes per word: two peaks per u32.
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
    return in.color;
}

struct WaveOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) full_px: vec4<f32>,
    @location(1) @interpolate(flat) run: vec2<u32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn wave_vs(
    @builtin(vertex_index) vi: u32,
    @location(0) draw: vec4<f32>,
    @location(1) full: vec4<f32>,
    @location(2) run: vec2<u32>,
    @location(3) color: vec4<f32>,
) -> WaveOut {
    let p = mix(draw.xy, draw.zw, corner(vi)) * g.pixels_per_point;
    var out: WaveOut;
    out.pos = to_clip(p);
    out.full_px = full * g.pixels_per_point;
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
    let count = in.run.y;
    let width = max(in.full_px.z - in.full_px.x, 0.001);
    let r = f32(count) / width;
    // The same column-to-peaks mapping as `dense::column_peaks`.
    let col = floor(in.pos.x) - in.full_px.x;
    let p0 = min(u32(max(floor(col * r), 0.0)), count - 1u);
    let p1 = min(max(u32(clamp(ceil((col + 1.0) * r), 0.0, f32(count))), p0 + 1u), p0 + 8u);
    var lo = 1.0;
    var hi = -1.0;
    for (var i = p0; i < p1; i = i + 1u) {
        let v = peak(in.run.x + i);
        lo = min(lo, v.x);
        hi = max(hi, v.y);
    }
    let mid = 0.5 * (in.full_px.y + in.full_px.w);
    let half = 0.5 * (in.full_px.w - in.full_px.y) * 0.9;
    let top = mid - hi * half;
    let bottom = mid - lo * half;
    let y = in.pos.y;
    if (y < top - 0.5 || y > bottom + 0.5) {
        discard;
    }
    return in.color;
}
