//! The display's frame: its header, pixel format, filler and XOR
//! (`docs/specs/push-2.md`, R17).

/// The display's width in pixels.
pub const WIDTH: usize = 960;
/// The display's height in lines.
pub const HEIGHT: usize = 160;
/// One line as sent: 960 pixels of 2 bytes and 128 bytes of filler.
pub const LINE: usize = 2048;
/// The bytes of a pixel line.
const PIXELS: usize = WIDTH * 2;
/// The frame header.
pub const HEADER: [u8; 16] = [
    0xFF, 0xCC, 0xAA, 0x88, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// A frame as sent: the header and 160 lines (327,696 bytes).
pub const FRAME_BYTES: usize = HEADER.len() + HEIGHT * LINE;
/// The bytes of an RGB image of the display.
pub const RGB_BYTES: usize = WIDTH * HEIGHT * 3;
/// The pattern every 4 bytes of a line are XORed with.
const XOR: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];

/// Encodes an RGB image (`WIDTH` × `HEIGHT`, 3 bytes a pixel) as the bytes
/// Push 2 takes (R17).
///
/// # Panics
///
/// If `rgb` isn't `RGB_BYTES` long.
#[must_use]
pub fn encode(rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), RGB_BYTES, "an image of the display's size");
    let mut out = Vec::with_capacity(FRAME_BYTES);
    out.extend_from_slice(&HEADER);
    for line in rgb.chunks_exact(WIDTH * 3) {
        let start = out.len();
        for pixel in line.chunks_exact(3) {
            let (r, g, b) = (u16::from(pixel[0]), u16::from(pixel[1]), u16::from(pixel[2]));
            let value = r >> 3 | (g >> 2) << 5 | (b >> 3) << 11;
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.resize(start + LINE, 0);
        for (byte, mask) in out[start..].iter_mut().zip(XOR.iter().cycle()) {
            *byte ^= mask;
        }
    }
    out
}

/// Decodes a frame as Push 2 would show it: the RGB image, or `None` if the
/// header, the length or a line's filler is wrong (R36).
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() != FRAME_BYTES || bytes[..16] != HEADER {
        return None;
    }
    let mut rgb = Vec::with_capacity(RGB_BYTES);
    for line in bytes[16..].chunks_exact(LINE) {
        let line: Vec<u8> = line
            .iter()
            .zip(XOR.iter().cycle())
            .map(|(byte, mask)| byte ^ mask)
            .collect();
        if line[PIXELS..].iter().any(|&b| b != 0) {
            return None;
        }
        for pixel in line[..PIXELS].chunks_exact(2) {
            let value = u16::from_le_bytes([pixel[0], pixel[1]]);
            let (r, g, b) = (value & 31, value >> 5 & 63, value >> 11);
            rgb.extend_from_slice(&[
                (r << 3 | r >> 2) as u8,
                (g << 2 | g >> 4) as u8,
                (b << 3 | b >> 2) as u8,
            ]);
        }
    }
    Some(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2 R17. 327,696 bytes: the header, 160 lines of 2,048 bytes,
    // pixel bits red 0 to 4, green 5 to 10, blue 11 to 15, the XOR pattern
    // over every 4 bytes, and the filler.
    #[test]
    fn a_frame_is_327696_bytes_with_the_pixel_bits_the_filler_and_the_xor() {
        let mut rgb = vec![0u8; RGB_BYTES];
        // The top left pixel pure red, the next pure green, the one after
        // pure blue.
        rgb[..9].copy_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255]);
        let frame = encode(&rgb);
        assert_eq!(frame.len(), 327_696);
        assert_eq!(frame[..16], HEADER);
        let line = &frame[16..16 + LINE];
        let plain: Vec<u8> = line.iter().zip(XOR.iter().cycle()).map(|(b, m)| b ^ m).collect();
        assert_eq!(u16::from_le_bytes([plain[0], plain[1]]), 0b0000_0000_0001_1111);
        assert_eq!(u16::from_le_bytes([plain[2], plain[3]]), 0b0000_0111_1110_0000);
        assert_eq!(u16::from_le_bytes([plain[4], plain[5]]), 0b1111_1000_0000_0000);
        assert!(plain[1920..].iter().all(|&b| b == 0), "the filler is zeros before the XOR");
        // A black pixel is the XOR pattern itself.
        assert_eq!(line[6..10], [XOR[2], XOR[3], XOR[0], XOR[1]]);
    }

    #[test]
    fn decoding_gives_back_the_image_to_the_pixel_format() {
        let mut rgb = vec![0u8; RGB_BYTES];
        for (n, byte) in rgb.iter_mut().enumerate() {
            *byte = (n % 251) as u8;
        }
        let back = decode(&encode(&rgb)).expect("a frame");
        for (a, b) in rgb.iter().zip(&back) {
            assert!(a.abs_diff(*b) < 8, "{a} and {b}");
        }
        assert!(decode(&[0; 100]).is_none());
        let mut bad = encode(&rgb);
        bad[0] = 0;
        assert!(decode(&bad).is_none());
    }
}
