//! Compact188x76 card with vector icons. Independent Rust rasterizer, not Qt pixel parity.
//! Font bytes are supplied locally by the host; proprietary system fonts are not bundled.
use image::{
    Rgb, RgbImage, RgbaImage,
    imageops::{self, FilterType},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub const LAYOUT_SIZE: u32 = 480;
pub const CARD_WIDTH: u32 = 188;
pub const CARD_HEIGHT: u32 = 76;
pub const MAX_BLOCKS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardPosition {
    pub x: u32,
    pub y: u32,
}

impl CardPosition {
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.x > LAYOUT_SIZE - CARD_WIDTH || self.y > LAYOUT_SIZE - CARD_HEIGHT {
            return Err("Карточка выходит за границы LCD.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeStyle {
    #[default]
    Reading,
    Arc,
    Bar,
    Flame,
}

impl GaugeStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub const OPTIONS: [(Self, &str, &str); 4] = [
        (Self::Reading, "reading", "Число и значок"),
        (Self::Arc, "arc", "Круговая шкала"),
        (Self::Bar, "bar", "Столбик"),
        (Self::Flame, "flame", "Высота пламени"),
    ];
}

/// Top-left position in the upright 480×480 LCD view.
pub struct Block<'a> {
    pub x: u32,
    pub y: u32,
    pub label: &'a str,
    pub celsius: f64,
}

pub struct TtLayoutFrame {
    pub jpeg: Vec<u8>,
    /// True when encoding needed a plain background to keep the cards readable.
    pub used_solid_background: bool,
}

const TT_FALLBACK_BACKGROUND: Rgb<u8> = Rgb([16, 16, 20]);

pub struct Renderer {
    font: fontdue::Font,
    glyphs: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
    style: GaugeStyle,
    card_position: CardPosition,
}
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let mut p = PathBuilder::new();
    let k = r * 0.5522848;
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish().unwrap()
}
fn line(p: &mut PathBuilder, x: f32, y: f32, x2: f32, y2: f32) {
    p.move_to(x, y);
    p.line_to(x2, y2);
}
fn icon(label: &str) -> tiny_skia::Path {
    let mut p = PathBuilder::new();
    if label.starts_with("CPU") {
        p.push_path(&rounded(5., 5., 16., 16., 2.));
        p.push_rect(tiny_skia::Rect::from_xywh(9., 9., 8., 8.).unwrap());
        for n in [8., 13., 18.] {
            line(&mut p, n, 1., n, 5.);
            line(&mut p, n, 21., n, 25.);
            line(&mut p, 1., n, 5., n);
            line(&mut p, 21., n, 25., n);
        }
    } else if label.contains("GPU") {
        p.push_path(&rounded(1., 6., 24., 15., 2.));
        p.push_circle(8., 13.5, 3.5);
        p.push_circle(18., 13.5, 3.5);
        line(&mut p, 5., 21., 5., 24.);
        line(&mut p, 8., 24., 19., 24.);
    } else if label.starts_with("RAM") {
        p.push_path(&rounded(1., 6., 24., 13., 1.5));
        for x in [4., 10., 16.] {
            p.push_path(&rounded(x, 9., 5., 6., 0.5));
        }
        for x in [4., 7., 10., 13., 16., 19., 22.] {
            line(&mut p, x, 19., x, 22.);
        }
    } else if label == "Жидкость" || label.to_lowercase().contains("liquid") {
        p.move_to(13., 1.);
        p.cubic_to(10., 7., 4., 12., 4., 17.);
        p.cubic_to(4., 28., 22., 28., 22., 17.);
        p.cubic_to(22., 12., 16., 7., 13., 1.);
        p.move_to(8.1, 17.2);
        p.cubic_to(8.3, 20.5, 10., 22., 13., 22.);
    } else {
        p.push_path(&rounded(10., 2., 6., 16., 3.));
        p.push_circle(13., 20., 5.);
        line(&mut p, 13., 7., 13., 19.);
    }
    p.finish().unwrap()
}
impl Renderer {
    pub fn new(font_bytes: Vec<u8>) -> Result<Self, String> {
        if font_bytes.is_empty() || font_bytes.len() > 32 * 1024 * 1024 {
            return Err("Invalid font size".into());
        }
        let font = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .map_err(str::to_owned)?;
        if font.horizontal_line_metrics(22.).is_none() {
            return Err("Font has no horizontal metrics".into());
        }
        Ok(Self {
            font,
            glyphs: HashMap::new(),
            style: GaugeStyle::default(),
            card_position: CardPosition { x: 146, y: 350 },
        })
    }
    pub fn with_style(mut self, style: GaugeStyle) -> Self {
        self.style = style;
        self
    }
    pub(crate) fn with_card_position(mut self, position: CardPosition) -> Result<Self, String> {
        position.validate()?;
        self.card_position = position;
        Ok(self)
    }
    fn width(&self, text: &str) -> f32 {
        text.chars()
            .map(|c| self.font.metrics(c, 22.).advance_width)
            .sum()
    }
    fn title(&self, label: &str) -> String {
        if self.width(label) <= 118. {
            return label.into();
        }
        let ellipsis = self.width("…");
        let mut result = String::new();
        let mut width = 0.;
        for c in label.chars() {
            let next = self.font.metrics(c, 22.).advance_width;
            if width + next + ellipsis > 118. {
                break;
            }
            result.push(c);
            width += next;
        }
        result.push('…');
        result
    }
    fn text(&mut self, pixmap: &mut Pixmap, text: &str, left: i32, y: i32) {
        let mut x = left as f32 + (118. - self.width(text)) / 2.;
        let metrics = self.font.horizontal_line_metrics(22.).unwrap();
        let baseline = y as f32 + (34. - (metrics.ascent - metrics.descent)) / 2. + metrics.ascent;
        for c in text.chars() {
            if !self.glyphs.contains_key(&c) {
                if self.glyphs.len() >= 256 {
                    self.glyphs.clear();
                }
                self.glyphs.insert(c, self.font.rasterize(c, 22.));
            }
            let (metrics, coverage) = &self.glyphs[&c];
            let gx = x.round() as i32 + metrics.xmin;
            let gy = baseline.round() as i32 - metrics.ymin - metrics.height as i32;
            for dy in 0..metrics.height {
                for dx in 0..metrics.width {
                    let px = gx + dx as i32;
                    let py = gy + dy as i32;
                    if !(left..left + 118).contains(&px) || !(y..y + 34).contains(&py) {
                        continue;
                    }
                    let a = u16::from(coverage[dy * metrics.width + dx]);
                    let offset = ((py * LAYOUT_SIZE as i32 + px) * 4) as usize;
                    let out = &mut pixmap.data_mut()[offset..offset + 4];
                    for channel in out.iter_mut().take(3) {
                        *channel = (a + (u16::from(*channel) * (255 - a) + 127) / 255) as u8;
                    }
                    out[3] = (a + (u16::from(out[3]) * (255 - a) + 127) / 255) as u8;
                }
            }
            x += metrics.advance_width;
        }
    }
    /// Render from back to front in upright view coordinates. Omit unavailable
    /// readings before calling. The caller owns persistence and device rotation.
    pub fn render_blocks(
        &mut self,
        background: &RgbImage,
        blocks: &[Block<'_>],
    ) -> Result<RgbImage, String> {
        if background.dimensions() != (LAYOUT_SIZE, LAYOUT_SIZE) || blocks.len() > MAX_BLOCKS {
            return Err("LCD layout requires a 480×480 canvas and at most four blocks".into());
        }
        let mut layer = Pixmap::new(LAYOUT_SIZE, LAYOUT_SIZE).ok_or("Cannot allocate overlay")?;
        for block in blocks {
            if block.x > LAYOUT_SIZE - CARD_WIDTH || block.y > LAYOUT_SIZE - CARD_HEIGHT {
                return Err("Temperature block exceeds LCD canvas".into());
            }
            let name =
                normalized_label(block.label, block.celsius).ok_or("Invalid temperature block")?;
            self.paint_card(&mut layer, block.x, block.y, &name, block.celsius);
        }
        let mut view = background.clone();
        blend(&mut view, layer)?;
        Ok(view)
    }

    /// Encode an upright layout for TT. On size exhaustion alone, retry once
    /// with a solid background while retaining all positioned cards.
    pub fn encode_tt_blocks(
        &mut self,
        background: &RgbImage,
        blocks: &[Block<'_>],
        device_rotation: i32,
    ) -> Result<TtLayoutFrame, String> {
        let angle = super::media::rotation(device_rotation)?;
        let encode = |view| {
            let device_frame = match angle {
                90 => imageops::rotate90(&view),
                180 => imageops::rotate180(&view),
                270 => imageops::rotate270(&view),
                _ => view,
            };
            super::media::encode_jpeg_within_budget(&device_frame)
        };
        let view = self.render_blocks(background, blocks)?;
        if let Some(jpeg) = encode(view)? {
            return Ok(TtLayoutFrame {
                jpeg,
                used_solid_background: false,
            });
        }
        let plain = RgbImage::from_pixel(LAYOUT_SIZE, LAYOUT_SIZE, TT_FALLBACK_BACKGROUND);
        let view = self.render_blocks(&plain, blocks)?;
        let jpeg = encode(view)?.ok_or("Solid LCD layout exceeds the 20000-byte TT JPEG budget")?;
        Ok(TtLayoutFrame {
            jpeg,
            used_solid_background: true,
        })
    }

    pub fn render(
        &mut self,
        background: &RgbImage,
        angle: i32,
        label: &str,
        celsius: f64,
    ) -> Result<RgbImage, String> {
        let (width, height) = background.dimensions();
        if !(128..=2048).contains(&width) || !(128..=2048).contains(&height) {
            return Err("Invalid background dimensions".into());
        }
        let Ok(angle) = super::media::rotation(angle) else {
            return Ok(background.clone());
        };
        let Some(name) = normalized_label(label, celsius) else {
            return Ok(background.clone());
        };
        let mut view = match angle {
            90 => imageops::rotate270(background),
            180 => imageops::rotate180(background),
            270 => imageops::rotate90(background),
            _ => background.clone(),
        };
        let mut layer = Pixmap::new(LAYOUT_SIZE, LAYOUT_SIZE).ok_or("Cannot allocate overlay")?;
        let CardPosition { x, y } = self.card_position;
        self.paint_card(&mut layer, x, y, &name, celsius);
        blend(&mut view, layer)?;
        Ok(match angle {
            90 => imageops::rotate90(&view),
            180 => imageops::rotate180(&view),
            270 => imageops::rotate270(&view),
            _ => view,
        })
    }

    fn paint_card(&mut self, layer: &mut Pixmap, x: u32, y: u32, name: &str, celsius: f64) {
        let panel = rounded(
            x as f32,
            y as f32,
            CARD_WIDTH as f32,
            CARD_HEIGHT as f32,
            10.,
        );
        let mut paint = Paint::default();
        paint.set_color_rgba8(16, 16, 14, 202);
        layer.fill_path(
            &panel,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        paint.set_color_rgba8(255, 247, 231, 32);
        layer.stroke_path(
            &panel,
            &paint,
            &Stroke {
                width: 1.,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
        paint.set_color_rgba8(255, 255, 255, 255);
        let transform =
            Transform::from_row(48. / 26., 0., 0., 48. / 26., x as f32 + 8., y as f32 + 14.);
        if self.style == GaugeStyle::Reading {
            layer.stroke_path(
                &icon(name),
                &paint,
                &Stroke {
                    width: 1.8,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                },
                transform,
                None,
            );
        } else {
            paint_gauge(layer, self.style, celsius.round(), transform);
        }
        self.text(layer, &self.title(name), x as i32 + 62, y as i32 + 4);
        // Actual Qt QString fixed0 uses ties away from zero and suppresses -0.
        let reading = format!("{}°C", celsius.round() as i32);
        self.text(layer, &reading, x as i32 + 62, y as i32 + 38);
    }
}

fn gauge_shape(style: GaugeStyle, fill: f32) -> Option<tiny_skia::Path> {
    if fill <= 0. {
        return None;
    }
    let mut path = PathBuilder::new();
    match style {
        GaugeStyle::Reading => return None,
        GaugeStyle::Arc => {
            let steps = (fill * 32.).ceil() as u32;
            for point in 0..=steps {
                let angle =
                    (0.75 + 1.5 * fill * point as f32 / steps as f32) * std::f32::consts::PI;
                let x = 13. + 10. * angle.cos();
                let y = 13. + 10. * angle.sin();
                if point == 0 {
                    path.move_to(x, y);
                } else {
                    path.line_to(x, y);
                }
            }
        }
        GaugeStyle::Bar => {
            let height = 22. * fill;
            let cap = (height * 0.5).min(2.);
            return Some(rounded(8., 24. - height, 10., height, cap));
        }
        GaugeStyle::Flame => {
            let height = 5. + 17. * fill;
            let tip = 24. - height;
            path.move_to(13., 24.);
            path.cubic_to(1., 24., 3., 24. - height * 0.45, 10., tip + height * 0.23);
            path.cubic_to(11., tip + height * 0.38, 13., tip + height * 0.25, 16., tip);
            path.cubic_to(24., tip + height * 0.46, 26., 24., 13., 24.);
            path.close();
        }
    }
    path.finish()
}

fn paint_gauge(layer: &mut Pixmap, style: GaugeStyle, value: f64, transform: Transform) {
    // This is a fixed visual scale, independent of alert or sensor thresholds.
    let level = ((value + 20.) / 140.).clamp(0., 1.) as f32;
    let accent = match style {
        GaugeStyle::Flame => [255, 139, 71, 255],
        _ => [104, 221, 225, 255],
    };
    for (fill, rgba) in [(1., [255, 255, 255, 41]), (level, accent)] {
        let Some(path) = gauge_shape(style, fill) else {
            continue;
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        if style == GaugeStyle::Arc {
            let stroke = Stroke {
                width: 2.8,
                line_cap: LineCap::Round,
                ..Stroke::default()
            };
            layer.stroke_path(&path, &paint, &stroke, transform, None);
        } else {
            layer.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
    }
}

fn normalized_label(raw: &str, celsius: f64) -> Option<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    ((!name.is_empty())
        && name.encode_utf16().count() <= 4096
        && (-20.0..=150.0).contains(&celsius))
    .then_some(name)
}

fn blend(view: &mut RgbImage, layer: Pixmap) -> Result<(), String> {
    let rgba = RgbaImage::from_raw(LAYOUT_SIZE, LAYOUT_SIZE, layer.take())
        .ok_or("Invalid overlay raster")?;
    let rgba = if view.dimensions() == (LAYOUT_SIZE, LAYOUT_SIZE) {
        rgba
    } else {
        imageops::resize(&rgba, view.width(), view.height(), FilterType::Triangle)
    };
    // Layer channels are premultiplied. Never multiply source color by alpha twice.
    for (x, y, pixel) in view.enumerate_pixels_mut() {
        let source = rgba.get_pixel(x, y).0;
        let a = u16::from(source[3]);
        *pixel = Rgb([0, 1, 2].map(|i| {
            (u16::from(source[i]) + (u16::from(pixel[i]) * (255 - a) + 127) / 255).min(255) as u8
        }));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn renderer() -> Renderer {
        let path = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("Fonts/seguibl.ttf");
        Renderer::new(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn positioned_layout_uses_the_existing_card_and_respects_bounds() {
        let mut renderer = renderer();
        let background = RgbImage::from_pixel(LAYOUT_SIZE, LAYOUT_SIZE, Rgb([45, 42, 49]));
        let mut block = Block {
            x: 146,
            y: 350,
            label: "CPU",
            celsius: 56.0,
        };
        let original = renderer.render(&background, 0, "CPU", 56.0).unwrap();
        assert_eq!(
            original,
            renderer
                .render_blocks(&background, std::slice::from_ref(&block))
                .unwrap()
        );
        block.x = 30;
        block.y = 40;
        let repositioned = renderer
            .render_blocks(&background, std::slice::from_ref(&block))
            .unwrap();
        assert_ne!(repositioned.get_pixel(48, 58), background.get_pixel(48, 58));
        assert_eq!(
            repositioned.get_pixel(165, 365),
            background.get_pixel(165, 365)
        );
        block.x = u32::MAX;
        assert!(renderer.render_blocks(&background, &[block]).is_err());
    }

    #[test]
    fn gauges_change_shape_with_the_reading_and_stay_aligned_after_rotation() {
        let background = RgbImage::new(LAYOUT_SIZE, LAYOUT_SIZE);
        let mut glyphs = Vec::new();
        for style in [GaugeStyle::Arc, GaugeStyle::Bar, GaugeStyle::Flame] {
            let mut renderer = renderer().with_style(style);
            let low = renderer.render(&background, 0, "CPU", 25.).unwrap();
            let high = renderer.render(&background, 0, "CPU", 90.).unwrap();
            let glyph = |image: &RgbImage| imageops::crop_imm(image, 154, 364, 48, 48).to_image();
            assert_ne!(glyph(&low), glyph(&high), "{style:?}");
            assert!(!glyphs.contains(&glyph(&high)), "{style:?}");
            glyphs.push(glyph(&high));
            let rotated = renderer.render(&background, 90, "CPU", 90.).unwrap();
            assert_eq!(imageops::rotate270(&rotated), high);
        }
    }

    #[test]
    fn tt_layout_keeps_fitting_frames_and_preserves_cards_on_budget_fallback() {
        let mut renderer = renderer();
        let layout = [Block {
            x: 20,
            y: 100,
            label: "GPU",
            celsius: 47.0,
        }];
        let plain = RgbImage::from_pixel(LAYOUT_SIZE, LAYOUT_SIZE, TT_FALLBACK_BACKGROUND);
        let expected = renderer.render_blocks(&plain, &layout).unwrap();
        let expected = super::super::media::encode_jpeg(&imageops::rotate90(&expected)).unwrap();
        let fitting = renderer.encode_tt_blocks(&plain, &layout, 90).unwrap();
        assert!(!fitting.used_solid_background);
        assert_eq!(fitting.jpeg, expected);

        let grid = RgbImage::from_fn(LAYOUT_SIZE, LAYOUT_SIZE, |x, y| {
            if (x / 11 + y / 11).is_multiple_of(2) {
                Rgb([255, 237, 16])
            } else {
                Rgb([8, 14, 192])
            }
        });
        let view = renderer.render_blocks(&grid, &layout).unwrap();
        assert!(
            super::super::media::encode_jpeg_within_budget(&imageops::rotate90(&view))
                .unwrap()
                .is_none()
        );
        let fallback = renderer.encode_tt_blocks(&grid, &layout, 90).unwrap();
        assert!(fallback.used_solid_background);
        assert_eq!(fallback.jpeg, expected);
        assert!(renderer.encode_tt_blocks(&grid, &layout, 45).is_err());
    }
    #[test]
    fn compact_icons_and_invalid_values_preserve_background() {
        let mut r = renderer();
        let image = RgbImage::from_pixel(480, 480, Rgb([100, 40, 10]));
        for label in ["CPU", "GPU", "RAM", "Liquid"] {
            let result = r.render(&image, 0, label, 42.).unwrap();
            assert_eq!(result.get_pixel(100, 100), image.get_pixel(100, 100));
            assert_ne!(result, image);
            assert_eq!(r.render(&image, 0, label, f64::NAN).unwrap(), image);
        }
        assert_eq!(r.render(&image, 45, "CPU", 42.).unwrap(), image);
        assert_eq!(r.render(&image, 0, "  ", 42.).unwrap(), image);
    }
    #[test]
    fn rotations_preserve_viewing_orientation_and_preview_geometry() {
        let mut r = renderer();
        let image = RgbImage::from_pixel(480, 480, Rgb([100, 40, 10]));
        let normal = r.render(&image, 0, "Liquid", 33.4).unwrap();
        for angle in [90, 180, 270] {
            let rotated = match angle {
                90 => imageops::rotate90(&image),
                180 => imageops::rotate180(&image),
                _ => imageops::rotate270(&image),
            };
            let output = r.render(&rotated, angle, "Liquid", 33.4).unwrap();
            let expected = match angle {
                90 => imageops::rotate90(&normal),
                180 => imageops::rotate180(&normal),
                _ => imageops::rotate270(&normal),
            };
            assert_eq!(output, expected);
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("work/lcd-stand");
        std::fs::create_dir_all(&path).unwrap();
        let mut grid = RgbImage::new(960, 960);
        for (i, label) in ["CPU", "GPU", "RAM", "Liquid"].iter().enumerate() {
            let output = r.render(&image, 0, label, 33.4 + i as f64 * 10.).unwrap();
            imageops::replace(
                &mut grid,
                &output,
                (i % 2 * 480) as i64,
                (i / 2 * 480) as i64,
            );
        }
        grid.save(path.join("overlay-icons.png")).unwrap();
    }
}
