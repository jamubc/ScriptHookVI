use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use fontdue::{Font, FontSettings, Metrics};

pub struct Image {
    pub width: u32,
    pub height: u32,
    key: u64,
    pixels: Vec<u32>,
}

#[derive(Clone)]
pub enum Cmd {
    Rect { x: i32, y: i32, w: i32, h: i32, from: u32, to: u32, horizontal: bool, radius: i32 },
    Frame { x: i32, y: i32, w: i32, h: i32, color: u32, thickness: f32, radius: i32 },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, color: u32, thickness: f32 },
    Circle { x: f32, y: f32, radius: f32, color: u32 },
    Triangle { points: [(f32, f32); 3], color: u32 },
    Text { x: i32, y: i32, size: f32, color: u32, font: usize, spacing: f32, tabular: bool, text: String },
    Image { x: i32, y: i32, w: i32, h: i32, tint: u32, image: Arc<Image> },
    Clip { x: i32, y: i32, w: i32, h: i32 },
}

pub fn digest(cmds: &[Cmd], extra: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    extra.hash(&mut hasher);
    for cmd in cmds {
        match cmd {
            Cmd::Rect { x, y, w, h, from, to, horizontal, radius } => {
                (0u8, x, y, w, h, from, to, horizontal, radius).hash(&mut hasher)
            }
            Cmd::Text { x, y, size, color, font, spacing, tabular, text } => {
                (1u8, x, y, size.to_bits(), color, font, spacing.to_bits(), tabular, text).hash(&mut hasher)
            }
            Cmd::Frame { x, y, w, h, color, thickness, radius } => {
                (2u8, x, y, w, h, color, thickness.to_bits(), radius).hash(&mut hasher)
            }
            Cmd::Line { x1, y1, x2, y2, color, thickness } => {
                (3u8, [*x1, *y1, *x2, *y2, *thickness].map(f32::to_bits), color).hash(&mut hasher)
            }
            Cmd::Circle { x, y, radius, color } => (4u8, [*x, *y, *radius].map(f32::to_bits), color).hash(&mut hasher),
            Cmd::Triangle { points, color } => {
                (5u8, points.map(|(x, y)| (x.to_bits(), y.to_bits())), color).hash(&mut hasher)
            }
            Cmd::Image { x, y, w, h, tint, image } => (6u8, x, y, w, h, tint, image.key).hash(&mut hasher),
            Cmd::Clip { x, y, w, h } => (7u8, x, y, w, h).hash(&mut hasher),
        }
    }
    hasher.finish() | 1
}

fn fonts() -> &'static [Font; 3] {
    static FONTS: OnceLock<[Font; 3]> = OnceLock::new();
    FONTS.get_or_init(|| {
        let load = |bytes: &[u8]| Font::from_bytes(bytes, FontSettings::default()).expect("embedded font");
        [
            load(include_bytes!("../assets/regular.ttf")),
            load(include_bytes!("../assets/bold.ttf")),
            load(include_bytes!("../assets/mono.ttf")),
        ]
    })
}

pub fn font_index(name: Option<&str>) -> usize {
    match name {
        Some("bold") => 1,
        Some("mono") => 2,
        _ => 0,
    }
}

fn digit_width(face: &Font, size: f32) -> f32 {
    ('0'..='9').map(|c| face.metrics(c, size).advance_width).fold(0.0, f32::max)
}

fn place(ch: char, advance: f32, cell: Option<f32>) -> (f32, f32) {
    match cell {
        Some(cell) if ch.is_ascii_digit() => (cell, (cell - advance) / 2.0),
        _ => (advance, 0.0),
    }
}

pub fn text_width(text: &str, size: f32, font: usize, spacing: f32, tabular: bool) -> f32 {
    let face = &fonts()[font.min(2)];
    let cell = tabular.then(|| digit_width(face, size));
    let (width, count) = text
        .chars()
        .fold((0.0, 0), |(w, n), c| (w + place(c, face.metrics(c, size).advance_width, cell).0, n + 1));
    width + spacing * (count.max(1) - 1) as f32
}

pub fn fit(text: &str, size: f32, font: usize, spacing: f32, tabular: bool, max: f32) -> String {
    if text_width(text, size, font, spacing, tabular) <= max {
        return text.to_string();
    }
    let dots = text_width("…", size, font, 0.0, false);
    if dots > max {
        return String::new();
    }
    let face = &fonts()[font.min(2)];
    let cell = tabular.then(|| digit_width(face, size));
    let (mut width, mut end) = (0.0, 0);
    for (i, ch) in text.char_indices() {
        width += place(ch, face.metrics(ch, size).advance_width, cell).0 + spacing;
        if width + dots > max {
            break;
        }
        end = i + ch.len_utf8();
    }
    format!("{}…", text[..end].trim_end())
}

pub fn line_height(size: f32, font: usize) -> f32 {
    fonts()[font.min(2)]
        .horizontal_line_metrics(size)
        .map_or(size, |m| m.ascent - m.descent)
}

fn premultiply(color: u32) -> u32 {
    let a = color >> 24;
    let ch = |shift: u32| ((((color >> shift) & 0xff) * a + 127) / 255) << shift;
    a << 24 | ch(16) | ch(8) | ch(0)
}

pub fn decode_png(bytes: &[u8]) -> std::result::Result<Image, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let pixels: Vec<u32> = buf[..info.buffer_size()]
        .chunks_exact(info.color_type.samples())
        .map(|p| {
            let gray = |v: u8| u32::from(v) * 0x01_0101;
            premultiply(match *p {
                [v] => 0xff00_0000 | gray(v),
                [v, a] => u32::from(a) << 24 | gray(v),
                [r, g, b] => 0xff00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
                [r, g, b, a] => u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
                _ => 0,
            })
        })
        .collect();
    if pixels.len() != (info.width * info.height) as usize {
        return Err("unsupported pixel layout".into());
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (info.width, info.height, &pixels).hash(&mut hasher);
    Ok(Image { width: info.width, height: info.height, key: hasher.finish(), pixels })
}

#[derive(Default)]
pub struct Renderer {
    glyphs: HashMap<(usize, char, u32), (Metrics, Vec<u8>)>,
}

fn blend(pixel: &mut u32, color: u32, coverage: u32) {
    let a = (color >> 24) * coverage / 255;
    if a == 0 {
        return;
    }
    let keep = 255 - a;
    let d = *pixel;
    let mix = |shift: u32| {
        let src = if shift == 24 { a } else { ((color >> shift) & 0xff) * a / 255 };
        (src + ((d >> shift) & 0xff) * keep / 255).min(255) << shift
    };
    *pixel = mix(24) | mix(16) | mix(8) | mix(0);
}

fn blend_premultiplied(pixel: &mut u32, color: u32) {
    let a = color >> 24;
    if a == 0 {
        return;
    }
    let keep = 255 - a;
    let d = *pixel;
    let mix = |shift: u32| (((color >> shift) & 0xff) + ((d >> shift) & 0xff) * keep / 255).min(255) << shift;
    *pixel = mix(24) | mix(16) | mix(8) | mix(0);
}

fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32).min(255) << shift
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

fn fade(color: u32, amount: f32) -> u32 {
    ((((color >> 24) as f32 * amount).round() as u32) << 24) | (color & 0x00ff_ffff)
}

fn rounded(fx: f32, fy: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (fx - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (fy - h / 2.0).abs() - (h / 2.0 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn frame(pixels: &mut [u32], width: i32, clip: [i32; 4], (x, y, w, h): (i32, i32, i32, i32), color: u32, thickness: f32, radius: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = radius.clamp(0, w.min(h) / 2) as f32;
    let t = thickness.clamp(0.0, w.min(h) as f32 / 2.0);
    if t.is_nan() || t <= 0.0 {
        return;
    }
    let edge = t.max(r).ceil() as i32 + 1;
    let (x0, y0) = (x.max(clip[0]), y.max(clip[1]));
    let (x1, y1) = (x.saturating_add(w).min(clip[2]), y.saturating_add(h).min(clip[3]));
    for py in y0..y1 {
        let fy = (py - y) as f32 + 0.5;
        let spans = if py - y >= edge && y + h - py > edge {
            [(x0, x1.min(x + edge)), (x0.max(x + w - edge).max(x + edge), x1)]
        } else {
            [(x0, x1), (0, 0)]
        };
        for (a, b) in spans {
            for px in a..b {
                let d = rounded((px - x) as f32 + 0.5, fy, w as f32, h as f32, r);
                let coverage = (0.5 - d).clamp(0.0, 1.0) - (0.5 - d - t).clamp(0.0, 1.0);
                blend(&mut pixels[(py * width + px) as usize], color, (coverage * 255.0) as u32);
            }
        }
    }
}

fn convex(pixels: &mut [u32], width: i32, clip: [i32; 4], points: &[(f32, f32)], color: u32) {
    let n = points.len();
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    if area.is_nan() || area.abs() <= 1e-3 {
        return;
    }
    let edges: Vec<(f32, f32, f32)> = (0..n)
        .filter_map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let len = ex.hypot(ey) * area.signum();
            (len != 0.0).then(|| {
                let (nx, ny) = (-ey / len, ex / len);
                (nx, ny, nx * a.0 + ny * a.1)
            })
        })
        .collect();
    let top = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let bottom = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    let y0 = ((top - 1.0).floor() as i32).max(clip[1]);
    let y1 = ((bottom + 1.0).ceil() as i32).min(clip[3]);
    'rows: for py in y0..y1 {
        let fy = py as f32 + 0.5;
        let (mut lo, mut hi) = (clip[0] as f32, (clip[2] - 1) as f32);
        for &(nx, ny, c) in &edges {
            let k = c - 0.5 - ny * fy;
            if nx > 1e-6 {
                lo = lo.max(k / nx - 0.5);
            } else if nx < -1e-6 {
                hi = hi.min(k / nx - 0.5);
            } else if k > 0.0 {
                continue 'rows;
            }
        }
        for px in lo.ceil() as i32..=hi.floor() as i32 {
            let fx = px as f32 + 0.5;
            let d = edges.iter().map(|&(nx, ny, c)| nx * fx + ny * fy - c).fold(f32::MAX, f32::min);
            blend(&mut pixels[(py * width + px) as usize], color, ((d + 0.5).clamp(0.0, 1.0) * 255.0) as u32);
        }
    }
}

fn circle(pixels: &mut [u32], width: i32, clip: [i32; 4], (cx, cy): (f32, f32), r: f32, color: u32) {
    if r.is_nan() || r <= 0.0 {
        return;
    }
    let reach = r + 0.5;
    let y0 = ((cy - reach).floor() as i32).max(clip[1]);
    let y1 = ((cy + reach).ceil() as i32).min(clip[3]);
    for py in y0..y1 {
        let dy = py as f32 + 0.5 - cy;
        let span = reach * reach - dy * dy;
        if span <= 0.0 {
            continue;
        }
        let half = span.sqrt();
        let lo = ((cx - half - 0.5).ceil() as i32).max(clip[0]);
        let hi = ((cx + half - 0.5).floor() as i32).min(clip[2] - 1);
        for px in lo..=hi {
            let coverage = (reach - (px as f32 + 0.5 - cx).hypot(dy)).clamp(0.0, 1.0);
            blend(&mut pixels[(py * width + px) as usize], color, (coverage * 255.0) as u32);
        }
    }
}

fn picture(pixels: &mut [u32], width: i32, clip: [i32; 4], (x, y, w, h): (i32, i32, i32, i32), tint: u32, image: &Image) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (iw, ih) = (image.width as i32, image.height as i32);
    let (x0, y0) = (x.max(clip[0]), y.max(clip[1]));
    let (x1, y1) = (x.saturating_add(w).min(clip[2]), y.saturating_add(h).min(clip[3]));
    let sample = |i: i32, scale: f32, limit: i32| {
        let s = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (limit - 1) as f32);
        let base = s.floor();
        (base as usize, (base as i32 + 1).min(limit - 1) as usize, ((s - base) * 256.0) as u32)
    };
    let columns: Vec<_> = (x0..x1).map(|px| sample(px - x, iw as f32 / w as f32, iw)).collect();
    let tint = premultiply(tint);
    let stride = iw as usize;
    for py in y0..y1 {
        let (r0, r1, wy) = sample(py - y, ih as f32 / h as f32, ih);
        let (upper, lower) = (&image.pixels[r0 * stride..][..stride], &image.pixels[r1 * stride..][..stride]);
        for (px, &(c0, c1, wx)) in (x0..x1).zip(&columns) {
            let mix = |shift: u32| {
                let ch = |p: u32| (p >> shift) & 0xff;
                let top = ch(upper[c0]) * (256 - wx) + ch(upper[c1]) * wx;
                let bottom = ch(lower[c0]) * (256 - wx) + ch(lower[c1]) * wx;
                let v = (top * (256 - wy) + bottom * wy) >> 16;
                ((v * (ch(tint) + 1)) >> 8) << shift
            };
            blend_premultiplied(&mut pixels[(py * width + px) as usize], mix(24) | mix(16) | mix(8) | mix(0));
        }
    }
}

impl Renderer {
    pub fn render(&mut self, cmds: &[Cmd], size: (i32, i32), pixels: &mut [u32], background: u32) {
        pixels.fill(background);
        let mut clip = [0, 0, size.0, size.1];
        for cmd in cmds {
            match cmd {
                Cmd::Rect { x, y, w, h, from, to, horizontal, radius } => {
                    if *w <= 0 || *h <= 0 {
                        continue;
                    }
                    let (x0, y0) = ((*x).max(clip[0]), (*y).max(clip[1]));
                    let (x1, y1) = (x.saturating_add(*w).min(clip[2]), y.saturating_add(*h).min(clip[3]));
                    let r = (*radius).clamp(0, (*w).min(*h) / 2) as f32;
                    let across = *horizontal && from != to;
                    for py in y0..y1 {
                        let row = if from == to || *horizontal {
                            *from
                        } else {
                            lerp(*from, *to, (py - y) as f32 / (*h - 1).max(1) as f32)
                        };
                        let fy = (py - y) as f32 + 0.5;
                        let cy = if fy < r { r - fy } else if fy > *h as f32 - r { fy - (*h as f32 - r) } else { 0.0 };
                        for px in x0..x1 {
                            let mut coverage = 255;
                            if cy > 0.0 {
                                let fx = (px - x) as f32 + 0.5;
                                let cx = if fx < r { r - fx } else if fx > *w as f32 - r { fx - (*w as f32 - r) } else { 0.0 };
                                if cx > 0.0 {
                                    let d = (cx * cx + cy * cy).sqrt();
                                    coverage = ((r + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32;
                                }
                            }
                            let color = if across { lerp(*from, *to, (px - x) as f32 / (*w - 1).max(1) as f32) } else { row };
                            blend(&mut pixels[(py * size.0 + px) as usize], color, coverage);
                        }
                    }
                }
                Cmd::Frame { x, y, w, h, color, thickness, radius } => {
                    frame(pixels, size.0, clip, (*x, *y, *w, *h), *color, *thickness, *radius)
                }
                Cmd::Line { x1, y1, x2, y2, color, thickness } => {
                    let (mut dx, mut dy) = (x2 - x1, y2 - y1);
                    let len = dx.hypot(dy);
                    (dx, dy) = if len > 0.0 { (dx / len, dy / len) } else { (1.0, 0.0) };
                    let (t, color) = if *thickness < 1.0 { (1.0, fade(*color, thickness.max(0.0))) } else { (*thickness, *color) };
                    let (hx, hy) = (dx * t / 2.0, dy * t / 2.0);
                    let (a, b) = ((x1 + 0.5 - hx, y1 + 0.5 - hy), (x2 + 0.5 + hx, y2 + 0.5 + hy));
                    let quad = [(a.0 - hy, a.1 + hx), (b.0 - hy, b.1 + hx), (b.0 + hy, b.1 - hx), (a.0 + hy, a.1 - hx)];
                    convex(pixels, size.0, clip, &quad, color);
                }
                Cmd::Circle { x, y, radius, color } => circle(pixels, size.0, clip, (*x, *y), *radius, *color),
                Cmd::Triangle { points, color } => convex(pixels, size.0, clip, points, *color),
                Cmd::Image { x, y, w, h, tint, image } => picture(pixels, size.0, clip, (*x, *y, *w, *h), *tint, image),
                Cmd::Clip { x, y, w, h } => {
                    clip = [(*x).max(0), (*y).max(0), x.saturating_add(*w).min(size.0), y.saturating_add(*h).min(size.1)];
                }
                Cmd::Text { x, y, size: px_size, color, font, spacing, tabular, text } => {
                    let index = (*font).min(2);
                    let face = &fonts()[index];
                    let ascent = face.horizontal_line_metrics(*px_size).map_or(*px_size, |m| m.ascent);
                    let baseline = *y as f32 + ascent;
                    let cell = tabular.then(|| digit_width(face, *px_size));
                    let mut pen = *x as f32;
                    for ch in text.chars() {
                        let (metrics, bitmap) = self
                            .glyphs
                            .entry((index, ch, px_size.to_bits()))
                            .or_insert_with(|| face.rasterize(ch, *px_size));
                        let (advance, shift) = place(ch, metrics.advance_width, cell);
                        let gx = (pen + shift + metrics.xmin as f32).round() as i32;
                        let gy = (baseline - metrics.ymin as f32 - metrics.height as f32).round() as i32;
                        for row in 0..metrics.height as i32 {
                            let py = gy + row;
                            if py < clip[1] || py >= clip[3] {
                                continue;
                            }
                            for col in 0..metrics.width as i32 {
                                let px = gx + col;
                                if px < clip[0] || px >= clip[2] {
                                    continue;
                                }
                                let coverage = u32::from(bitmap[(row * metrics.width as i32 + col) as usize]);
                                blend(&mut pixels[(py * size.0 + px) as usize], *color, coverage);
                            }
                        }
                        pen += advance + spacing;
                    }
                }
            }
        }
    }
}
