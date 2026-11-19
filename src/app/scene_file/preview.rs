//! Bounded software previews. Every projection reads the same composed frame.
use super::{Content, Edge, Region, SceneFile, UNIT};
use crate::{
    devices::{Canvas, Point},
    lighting::color::Rgb,
    runtime::scene::Renderer,
};
use serde::Serialize;

const MAX_PREVIEW_AXIS: usize = 64;
const SAMPLES_PER_EDGE: usize = 12;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Pixels {
    pub width: usize,
    pub height: usize,
    pub colors: Vec<u32>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Frame {
    pub canvas: Pixels,
    pub lcd: Option<Pixels>,
    pub strip: Option<Pixels>,
}

pub struct Preview {
    file: SceneFile,
    effects: Vec<Option<Renderer>>,
    sample_size: (usize, usize),
    coordinates: Canvas,
}

fn coordinate(index: usize, samples: usize, extent: u32) -> u32 {
    if samples == 1 {
        (extent - 1) / 2
    } else {
        (index as u32 * (extent - 1)) / (samples - 1) as u32
    }
}

impl Preview {
    pub fn new(file: SceneFile) -> Result<Self, String> {
        file.validate()?;
        let side = file.width.max(file.height) as usize;
        let width = (file.width as usize * MAX_PREVIEW_AXIS / side).max(1);
        let height = (file.height as usize * MAX_PREVIEW_AXIS / side).max(1);
        let mut points = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                points.push(Point {
                    device: String::new(),
                    led: points.len(),
                    x: coordinate(x, width, file.width),
                    y: coordinate(y, height, file.height),
                });
            }
        }
        let effects = file
            .layers
            .iter()
            .map(|layer| match &layer.content {
                Content::Color { .. } => Ok(None),
                Content::Effect { settings } => {
                    let mut settings = settings.clone();
                    settings.fps = 20;
                    Renderer::new(settings).map(Some)
                }
            })
            .collect::<Result<_, _>>()?;
        let coordinates = Canvas {
            width: file.width,
            height: file.height,
            points,
        };
        Ok(Self {
            file,
            effects,
            sample_size: (width, height),
            coordinates,
        })
    }

    pub fn step(&mut self) -> Result<Frame, String> {
        let (width, height) = self.sample_size;
        let mut canvas = vec![Rgb::default(); width * height];
        for (layer, renderer) in self.file.layers.iter().zip(self.effects.iter_mut()) {
            match (&layer.content, renderer) {
                (Content::Color { rgb }, _) => {
                    let color = Rgb::from_rrggbb(*rgb);
                    for pixel in &mut canvas {
                        *pixel = blend(*pixel, color, layer.opacity);
                    }
                }
                (_, Some(renderer)) => {
                    for (pixel, color) in canvas.iter_mut().zip(renderer.step(&self.coordinates)?) {
                        *pixel = blend(*pixel, color, layer.opacity);
                    }
                }
                _ => unreachable!("effect layers are compiled before rendering"),
            }
        }
        let canvas = Pixels {
            width,
            height,
            colors: canvas.into_iter().map(Rgb::rrggbb).collect(),
        };
        let lcd = self
            .file
            .outputs
            .lcd
            .as_ref()
            .map(|lcd| lcd_preview(&canvas, lcd.region));
        let strip = self
            .file
            .outputs
            .strip
            .as_ref()
            .map(|strip| strip_preview(&canvas, &strip.edges, strip.reverse));
        Ok(Frame { canvas, lcd, strip })
    }
}

fn blend(below: Rgb, above: Rgb, opacity: u8) -> Rgb {
    let opacity = u16::from(opacity);
    let channel = |below: u8, above: u8| {
        ((u16::from(below) * (100 - opacity) + u16::from(above) * opacity + 50) / 100) as u8
    };
    Rgb::new(
        channel(below.r, above.r),
        channel(below.g, above.g),
        channel(below.b, above.b),
    )
}

fn lcd_preview(canvas: &Pixels, region: Option<Region>) -> Pixels {
    let Some(r) = region else {
        return Pixels {
            width: canvas.width,
            height: canvas.height,
            colors: canvas.colors.clone(),
        };
    };
    let units = usize::from(UNIT);
    let left = usize::from(r.x) * canvas.width / units;
    let top = usize::from(r.y) * canvas.height / units;
    let right = (usize::from(r.x + r.width) * canvas.width).div_ceil(units);
    let bottom = (usize::from(r.y + r.height) * canvas.height).div_ceil(units);
    let mut colors = Vec::with_capacity((right - left) * (bottom - top));
    for row in canvas
        .colors
        .chunks_exact(canvas.width)
        .take(bottom)
        .skip(top)
    {
        colors.extend_from_slice(&row[left..right]);
    }
    Pixels {
        width: right - left,
        height: bottom - top,
        colors,
    }
}

fn strip_preview(canvas: &Pixels, edges: &[Edge], reverse: bool) -> Pixels {
    let mut colors = Vec::with_capacity(edges.len() * SAMPLES_PER_EDGE);
    for edge in edges {
        for i in 0..SAMPLES_PER_EDGE {
            let centered = |length: usize| (2 * i + 1) * length / (2 * SAMPLES_PER_EDGE);
            let (x, y) = match edge {
                Edge::Top => (centered(canvas.width), 0),
                Edge::Right => (canvas.width - 1, centered(canvas.height)),
                Edge::Bottom => (canvas.width - 1 - centered(canvas.width), canvas.height - 1),
                Edge::Left => (0, canvas.height - 1 - centered(canvas.height)),
            };
            colors.push(canvas.colors[y * canvas.width + x]);
        }
    }
    if reverse {
        colors.reverse();
    }
    Pixels {
        width: colors.len(),
        height: 1,
        colors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::scene_file::{Layer, Lcd, Strip},
        runtime::scene::example,
    };

    #[test]
    fn layers_and_projections_use_the_same_composed_pixels() {
        let mut model = SceneFile::from_effect(example(), 320, 180).unwrap();
        model.layers = vec![
            Layer {
                opacity: 100,
                content: Content::Color { rgb: 0x00_00_ff },
            },
            Layer {
                opacity: 50,
                content: Content::Color { rgb: 0xff_00_00 },
            },
        ];
        model.outputs.lcd = Some(Lcd {
            target: "screen".into(),
            region: Some(Region {
                x: 5_000,
                y: 0,
                width: 5_000,
                height: 5_000,
            }),
        });
        model.outputs.strip = Some(Strip {
            target: "edges".into(),
            edges: vec![Edge::Top, Edge::Right],
            reverse: true,
        });
        let frame = Preview::new(model).unwrap().step().unwrap();
        assert_eq!((frame.canvas.width, frame.canvas.height), (64, 36));
        let lcd = frame.lcd.unwrap();
        let strip = frame.strip.unwrap();
        assert_eq!((lcd.width, lcd.height), (32, 18));
        assert_eq!(strip.width, 24);
        assert!(
            frame
                .canvas
                .colors
                .iter()
                .chain(&lcd.colors)
                .chain(&strip.colors)
                .all(|color| *color == 0x80_00_80)
        );
    }

    #[test]
    fn crop_rounds_outwards_and_strip_respects_edge_order() {
        let canvas = Pixels {
            width: 3,
            height: 2,
            colors: vec![1, 2, 3, 4, 5, 6],
        };
        let lcd = lcd_preview(
            &canvas,
            Some(Region {
                x: 5_000,
                y: 5_000,
                width: 5_000,
                height: 5_000,
            }),
        );
        assert_eq!(lcd.colors, [5, 6]);
        let forward = strip_preview(&canvas, &[Edge::Right, Edge::Top], false);
        let backward = strip_preview(&canvas, &[Edge::Right, Edge::Top], true);
        assert_eq!(
            (
                forward.colors[0],
                forward.colors[11],
                forward.colors[12],
                forward.colors[23]
            ),
            (3, 6, 1, 3)
        );
        assert_eq!(
            backward.colors,
            forward.colors.iter().copied().rev().collect::<Vec<_>>()
        );
    }
}
