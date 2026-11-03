//! Pixel conversion shared by the decoders that hand back YUV: NV12 to RGBA.

/// An NV12 picture as RGBA: `w` by `h` pixels from a buffer whose luma rows
/// are `stride` bytes apart, `aligned_h` rows tall, with the interleaved
/// chroma plane after it. Video range (16 to 235); `bt709` picks the matrix,
/// else BT.601. Alpha is 255. `None` if the buffer is too short.
#[cfg(any(windows, test))]
pub(crate) fn nv12_to_rgba(
    data: &[u8],
    stride: usize,
    aligned_h: usize,
    w: usize,
    h: usize,
    bt709: bool,
) -> Option<Vec<u8>> {
    let chroma = stride.checked_mul(aligned_h)?;
    let needed = chroma.checked_add(stride.checked_mul(h.div_ceil(2))?)?;
    if stride < w || aligned_h < h || data.len() < needed {
        return None;
    }
    // Fixed point, eight fractional bits: (red, green by blue, green by red,
    // blue) factors on the chroma.
    let (rv, gu, gv, bu) = if bt709 {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let luma = &data[y * stride..y * stride + w];
        let chroma_row = &data[chroma + (y / 2) * stride..];
        for (x, &yy) in luma.iter().enumerate() {
            let c = 298 * (i32::from(yy) - 16) + 128;
            let u = i32::from(chroma_row[x / 2 * 2]) - 128;
            let v = i32::from(chroma_row[x / 2 * 2 + 1]) - 128;
            rgba.extend_from_slice(&[
                clamp((c + rv * v) >> 8),
                clamp((c - gu * u - gv * v) >> 8),
                clamp((c + bu * u) >> 8),
                255,
            ]);
        }
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_range_black_and_white_and_a_primary() {
        // A 2 by 2 picture, stride 4, two rows of padding: luma, then chroma.
        let mut data = vec![0u8; 4 * 4 + 4];
        data[0..2].copy_from_slice(&[16, 235]);
        data[4..6].copy_from_slice(&[235, 16]);
        data[16..18].copy_from_slice(&[128, 128]);
        let rgba = nv12_to_rgba(&data, 4, 4, 2, 2, true).expect("converted");
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
        assert_eq!(&rgba[8..12], &[255, 255, 255, 255]);
        assert_eq!(&rgba[12..16], &[0, 0, 0, 255]);
        // BT.709 red: Y 63, Cb 102, Cr 240.
        let red = nv12_to_rgba(&[63, 63, 63, 63, 102, 240], 2, 2, 2, 2, true).expect("converted");
        let [r, g, b, _] = [red[0], red[1], red[2], red[3]];
        assert!(r > 245 && g < 12 && b < 12, "{r} {g} {b}");
    }

    #[test]
    fn a_short_buffer_gives_nothing() {
        assert!(nv12_to_rgba(&[0; 5], 2, 2, 2, 2, false).is_none());
    }
}
