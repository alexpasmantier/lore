/// Tray icon generator.
///
/// Produces an RGBA image of a glowing red eye — a radial gradient
/// lens inside a dark metallic ring. Default size is 64×64 (see [`ICON_SIZE`]).
pub const ICON_SIZE: u32 = 64;

#[derive(Debug, Clone, Copy)]
pub enum IconColor {
    Red,
    Orange,
}

/// Generate a `size × size` RGBA pixel buffer for the eye icon.
///
/// * `size` – width and height of the square icon in pixels.
/// * `brightness` – overall glow intensity, 0.0 (off) to 1.0 (full).
/// * `color` – lens hue (red for normal/ingesting, orange for consolidating).
pub fn generate(size: u32, brightness: f32, color: IconColor) -> Vec<u8> {
    let s = size as usize;
    let center = s as f64 / 2.0;
    let outer_r = center - 2.0; // 2 px margin for anti-aliasing
    let housing_width = 2.5;
    let lens_r = outer_r - housing_width;

    let (base_r, base_g, base_b) = match color {
        IconColor::Red => (1.0_f64, 0.06, 0.02),
        IconColor::Orange => (1.0, 0.50, 0.06),
    };

    let b = brightness as f64;
    let mut rgba = vec![0u8; s * s * 4];

    for y in 0..s {
        for x in 0..s {
            let dx = x as f64 - center + 0.5;
            let dy = y as f64 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = (y * s + x) * 4;

            if dist > outer_r + 0.5 {
                // Outside the icon – fully transparent.
                continue;
            } else if dist > lens_r + 0.5 {
                // Housing ring – dark metallic with a subtle 3-D highlight.
                let alpha = if dist > outer_r - 0.5 {
                    (outer_r + 0.5 - dist).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                let ring_t = (dist - lens_r) / housing_width;
                let highlight = (1.0 - (ring_t - 0.4).abs() * 3.0).max(0.0) * 0.12;
                let grey = 0.10 + highlight;

                rgba[idx] = (grey * 255.0) as u8;
                rgba[idx + 1] = (grey * 255.0) as u8;
                rgba[idx + 2] = ((grey + 0.015) * 255.0) as u8; // slight cool tint
                rgba[idx + 3] = (alpha * 255.0) as u8;
            } else {
                // Inside the lens – layered Gaussian glow.
                let nr = dist / lens_r; // 0 at center, 1 at edge

                // Layer 1: wide ambient glow
                let ambient = (-1.5 * nr * nr).exp() * 0.25;
                // Layer 2: main glow (bulk of the colour)
                let glow = (-3.5 * nr * nr).exp() * 0.7;
                // Layer 3: tight centre highlight (white-hot core)
                let highlight = (-20.0 * nr * nr).exp() * 1.0;

                let total_glow = (ambient + glow) * b;
                let h = highlight * b;

                // Dark lens background (almost black) + glow layers
                let bg = 0.02;
                let r = (bg + base_r * total_glow + h).min(1.0);
                let g = (bg + base_g * total_glow + h * 0.7).min(1.0);
                let blue = (bg + base_b * total_glow + h * 0.5).min(1.0);

                // Smooth edge transition into the housing
                let alpha = if dist > lens_r - 1.0 {
                    (lens_r + 0.5 - dist).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                rgba[idx] = (r * 255.0).clamp(0.0, 255.0) as u8;
                rgba[idx + 1] = (g * 255.0).clamp(0.0, 255.0) as u8;
                rgba[idx + 2] = (blue * 255.0).clamp(0.0, 255.0) as u8;
                rgba[idx + 3] = (alpha * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }

    rgba
}
