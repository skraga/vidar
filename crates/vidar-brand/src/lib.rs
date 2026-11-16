//! Vidar's icon, drawn in code so it is sharp at every size (tray, window, .ico, endpoints).
//!
//! A neon waveform on a transparent background: one spoken syllable. Faint specks at both edges
//! (the noise floor) swell into a smooth bell of tall, bright bars in the middle (the voice).
//! Colours run cyan -> magenta -> violet -> amber. When off, the same shape in muted grey.

/// Distance from (px, py) to a vertical segment at x from `top` to `bot`.
fn vbar_sd(px: f32, py: f32, x: f32, top: f32, bot: f32) -> f32 {
    let dy = if py < top {
        top - py
    } else if py > bot {
        py - bot
    } else {
        0.0
    };
    ((px - x).powi(2) + dy * dy).sqrt()
}

/// Neon gradient along x (-0.7..0.7): cyan -> magenta -> violet -> amber, as 0..255 RGB.
pub fn neon(x: f32) -> [f32; 3] {
    const STOPS: [(f32, [f32; 3]); 4] = [
        (-0.70, [0.0, 220.0, 255.0]),
        (-0.20, [255.0, 40.0, 190.0]),
        (0.25, [160.0, 80.0, 255.0]),
        (0.70, [255.0, 190.0, 20.0]),
    ];
    let x = x.clamp(STOPS[0].0, STOPS[3].0);
    for w in STOPS.windows(2) {
        let ((x0, c0), (x1, c1)) = (w[0], w[1]);
        if x <= x1 {
            let t = (x - x0) / (x1 - x0);
            return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * t);
        }
    }
    STOPS[3].1
}

/// Half the thickness of a bar, in the same units as [`BARS`].
pub const BAR_HALF_WIDTH: f32 = 0.046;

/// The bars: (x, half-height, brightness), in the -1..1 box. Eleven evenly spaced bars under a
/// Gaussian envelope: half-height 0.035 + 0.58 * e^(-(x / 0.3)^2), dimmer towards the edges.
pub const BARS: [(f32, f32, f32); 11] = [
    (-0.640, 0.041, 0.46),
    (-0.512, 0.067, 0.50),
    (-0.384, 0.148, 0.62),
    (-0.256, 0.315, 0.87),
    (-0.128, 0.518, 1.00),
    (0.000, 0.615, 1.00),
    (0.128, 0.518, 1.00),
    (0.256, 0.315, 0.87),
    (0.384, 0.148, 0.62),
    (0.512, 0.067, 0.50),
    (0.640, 0.041, 0.46),
];

/// RGBA pixels (straight alpha), `size` x `size`, on a transparent background.
/// `active` = noise cancellation on.
pub fn render(size: u32, active: bool) -> Vec<u8> {
    let s = size as f32;
    // Tubes never thinner than ~1.4 px; the glow only where there is room for it.
    let width = BAR_HALF_WIDTH.max(1.4 / s);
    let glow = if size >= 48 { 1.0 } else { 0.0 };
    const N: u32 = 4; // 4x4 supersampling
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // Premultiplied colour and alpha, accumulated over the samples.
            let (mut acc, mut acc_a) = ([0.0f32; 3], 0.0f32);
            for sy in 0..N {
                for sx in 0..N {
                    // Normalised coordinates; the bars span about -0.7..0.7.
                    let px = (((x as f32 + (sx as f32 + 0.5) / N as f32) / s) * 2.0 - 1.0) * 0.86;
                    let py = (((y as f32 + (sy as f32 + 0.5) / N as f32) / s) * 2.0 - 1.0) * 0.86;
                    let (mut c, mut a) = ([0.0f32; 3], 0.0f32); // premultiplied, starts transparent
                    for &(bx, half_h, bright) in &BARS {
                        let d = vbar_sd(px, py, bx, -half_h, half_h) - width;
                        let mut col = neon(bx);
                        let mut br = bright;
                        if !active {
                            let g = 0.3 * col[0] + 0.59 * col[1] + 0.11 * col[2];
                            col = [g * 0.45 + 70.0, g * 0.45 + 74.0, g * 0.45 + 86.0];
                            br *= 0.8;
                        }
                        // Layer = soft glow, or the solid tube; composited "over" what is there.
                        let (layer, la) = if d <= 0.0 {
                            let core = if active {
                                (-d / width).clamp(0.0, 1.0).powi(4) * 0.14
                            } else {
                                0.0
                            };
                            (
                                [0, 1, 2].map(|i| col[i] * (1.0 - core) + 255.0 * core),
                                0.35 + 0.65 * br,
                            )
                        } else {
                            (
                                col,
                                glow * br * (-d / 0.06).exp() * if active { 0.45 } else { 0.12 },
                            )
                        };
                        if la > 0.0 {
                            for i in 0..3 {
                                c[i] = layer[i] * la + c[i] * (1.0 - la);
                            }
                            a = la + a * (1.0 - la);
                        }
                    }
                    for i in 0..3 {
                        acc[i] += c[i];
                    }
                    acc_a += a;
                }
            }
            let n = (N * N) as f32;
            let a = acc_a / n;
            if a > 0.0 {
                let i = ((y * size + x) * 4) as usize;
                for k in 0..3 {
                    out[i + k] = ((acc[k] / n) / a).clamp(0.0, 255.0).round() as u8;
                }
                out[i + 3] = (a * 255.0).round() as u8;
            }
        }
    }
    out
}

/// A Windows .ico with PNG-compressed images at the usual sizes.
pub fn ico(active: bool) -> Vec<u8> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| png(&render(s, active), s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, p) in sizes.iter().zip(&pngs) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += p.len() as u32;
    }
    for p in pngs {
        out.extend_from_slice(&p);
    }
    out
}

/// PNG-encode RGBA pixels.
pub fn png(rgba: &[u8], size: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(rgba).expect("png data");
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_corners_and_coloured_bars() {
        let px = render(64, true);
        assert_eq!(px[3], 0, "corner is transparent");
        let centre_gap = ((32 * 64 + 4) * 4) as usize;
        assert!(
            px[centre_gap + 3] < 40,
            "no opaque background at the left edge, only faint glow"
        );
        // The tallest bar (x = 0) crosses the centre row in vivid colour.
        let (x, y) = (32u32, 32u32);
        let i = ((y * 64 + x) * 4) as usize;
        let (r, g, b) = (px[i] as i32, px[i + 1] as i32, px[i + 2] as i32);
        assert!(
            r.max(g).max(b) - r.min(g).min(b) > 80,
            "saturated: {:?}",
            &px[i..i + 4]
        );
    }

    #[test]
    fn off_state_is_grey() {
        let px = render(64, false);
        for p in px.as_chunks::<4>().0.iter().filter(|p| p[3] > 200) {
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            assert!((r - g).abs() < 30 && (g - b).abs() < 45, "not grey: {p:?}");
        }
    }

    #[test]
    fn ico_header_is_valid() {
        let ico = ico(true);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 9);
    }

    /// Writes preview PNGs when VIDAR_ICON_PREVIEW names a folder.
    #[test]
    fn write_preview() {
        if let Ok(dir) = std::env::var("VIDAR_ICON_PREVIEW") {
            for (name, active) in [("on", true), ("off", false)] {
                for size in [256u32, 32, 16] {
                    std::fs::write(
                        format!("{dir}/vidar_{name}_{size}.png"),
                        png(&render(size, active), size),
                    )
                    .unwrap();
                }
            }
        }
    }
}
