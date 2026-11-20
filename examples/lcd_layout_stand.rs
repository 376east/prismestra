//! Explicitly selected, hardware-free LCD layout and media preview.
use image::{
    AnimationDecoder, Frame, Rgb, RgbImage, RgbaImage,
    codecs::gif::{GifDecoder, GifEncoder, Repeat},
};
use pc_control::lcd::{media, overlay, protocol};
use std::{io::Cursor, path::PathBuf, time::Instant};

const PASSES: usize = 3;
const BLOCKS: [overlay::Block<'static>; 4] = [
    overlay::Block {
        x: 146,
        y: 46,
        label: "CPU",
        celsius: 56.0,
    },
    overlay::Block {
        x: 40,
        y: 202,
        label: "GPU",
        celsius: 48.0,
    },
    overlay::Block {
        x: 252,
        y: 202,
        label: "RAM",
        celsius: 42.0,
    },
    overlay::Block {
        x: 146,
        y: 358,
        label: "Liquid",
        celsius: 33.4,
    },
];

fn check_budget() -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = load_renderer()?;
    let side = overlay::LAYOUT_SIZE;
    let original = GifDecoder::new(Cursor::new(media::BUILTIN_KALEIDOSCOPE_GIF))?
        .into_frames()
        .next()
        .ok_or("Missing GIF frame")??;
    let backgrounds = [
        (
            "kaleidoscope",
            media::canvas(original.buffer(), side, side, true, 0)?,
        ),
        (
            "dark-gradient",
            RgbImage::from_fn(side, side, |x, y| Rgb([(x / 20) as u8, (y / 16) as u8, 18])),
        ),
        (
            "bright-grid",
            RgbImage::from_fn(side, side, |x, y| {
                if (x / 11 + y / 11).is_multiple_of(2) {
                    Rgb([255, 237, 16])
                } else {
                    Rgb([8, 14, 192])
                }
            }),
        ),
        (
            "fine-stipple",
            RgbImage::from_fn(side, side, |x, y| {
                // Deterministic high-frequency input to exercise the negative case.
                let bits = (y * side + x).wrapping_mul(2_654_435_761).rotate_left(7);
                Rgb([bits as u8, (bits >> 8) as u8, (bits >> 16) as u8])
            }),
        ),
        (
            "dark-stipple",
            RgbImage::from_fn(side, side, |x, y| {
                let bits = (y * side + x).wrapping_mul(2_654_435_761).rotate_left(7);
                Rgb([
                    (bits & 63) as u8,
                    ((bits >> 8) & 63) as u8,
                    ((bits >> 16) & 63) as u8,
                ])
            }),
        ),
    ];
    let corners = std::array::from_fn::<_, 4, _>(|index| {
        let x = if index % 2 == 0 {
            0
        } else {
            side - overlay::CARD_WIDTH
        };
        let y = if index < 2 {
            0
        } else {
            side - overlay::CARD_HEIGHT
        };
        overlay::Block {
            x,
            y,
            label: BLOCKS[index].label,
            celsius: BLOCKS[index].celsius,
        }
    });
    let mut checked = 0;
    let mut fallbacks = 0;
    for (background_name, background) in backgrounds {
        for (layout, blocks) in [
            ("preset", &BLOCKS[..]),
            ("corners", &corners[..]),
            ("single", &BLOCKS[..1]),
        ] {
            let started = Instant::now();
            let frame = renderer.encode_tt_blocks(&background, blocks, 0)?;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            protocol::validate_jpeg(&frame.jpeg)?;
            assert!(frame.jpeg.len() <= media::FRAME_BUDGET);
            let method = if frame.used_solid_background {
                "solid"
            } else {
                "original"
            };
            fallbacks += u32::from(frame.used_solid_background);
            checked += 1;
            println!(
                "{background_name:14} {layout:8} {:5} bytes {method:8} {elapsed:.1} ms",
                frame.jpeg.len()
            );
        }
    }
    println!("{checked} cases passed, {fallbacks} used the solid background");
    Ok(())
}

fn load_renderer() -> Result<overlay::Renderer, Box<dyn std::error::Error>> {
    let font = PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system font required")?)
        .join("Fonts/seguibl.ttf");
    Ok(overlay::Renderer::new(std::fs::read(font)?)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--budget" {
        return check_budget();
    }
    if args.len() != 2 || args[0] != "--kaleidoscope" {
        return Err("Usage: lcd_layout_stand --kaleidoscope <output-directory> | --budget".into());
    }
    let output = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&output)?;
    let source = media::BUILTIN_KALEIDOSCOPE_GIF;
    let started = Instant::now();
    let prepared = media::prepare(source.into(), 0, || false)?;
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut renderer = load_renderer()?;
    let mut preview = Vec::new();
    let mut durations_ms = Vec::new();
    let mut max_jpeg = 0;
    let mut over_frame_interval = 0;
    let mut source_dimensions = (0, 0);
    let side = overlay::LAYOUT_SIZE;
    {
        let mut encoder = GifEncoder::new_with_speed(&mut preview, 10);
        encoder.set_repeat(Repeat::Infinite)?;
        for pass in 0..PASSES {
            let mut decoder = GifDecoder::new(Cursor::new(source))?.into_frames();
            for (index, prepared_frame) in prepared.frames.iter().enumerate() {
                let started = Instant::now();
                let frame = decoder.next().ok_or("Missing GIF frame")??;
                source_dimensions = frame.buffer().dimensions();
                let delay = frame.delay();
                let canvas = media::canvas(frame.buffer(), side, side, true, 0)?;
                let rendered = renderer.render_blocks(&canvas, &BLOCKS)?;
                let jpeg = media::encode_jpeg(&rendered)?;
                protocol::validate_jpeg(&jpeg)?;
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                durations_ms.push(elapsed_ms);
                over_frame_interval += u64::from(elapsed_ms > f64::from(prepared_frame.delay_ms));
                max_jpeg = max_jpeg.max(jpeg.len());
                if pass == 0 {
                    if index == 0 {
                        rendered.save(output.join("preview.png"))?;
                    }
                    let frame = RgbaImage::from_fn(side, side, |x, y| {
                        let [r, g, b] = rendered.get_pixel(x, y).0;
                        image::Rgba([r, g, b, 255])
                    });
                    encoder.encode_frame(Frame::from_parts(frame, 0, 0, delay))?;
                }
            }
            if decoder.next().is_some() {
                return Err("Unexpected trailing GIF frame".into());
            }
        }
    }
    durations_ms.sort_by(f64::total_cmp);
    let p95 = durations_ms[(durations_ms.len() * 95).div_ceil(100) - 1];
    let loop_ms: u64 = prepared
        .frames
        .iter()
        .map(|frame| u64::from(frame.delay_ms))
        .sum();
    let report = serde_json::json!({
        "asset": "Animated Kaleidoscope",
        "source_dimensions": source_dimensions,
        "source_bytes": source.len(),
        "prepared_jpeg_bytes": prepared.frames.iter().map(|frame| frame.jpeg.len()).sum::<usize>(),
        "tt_jpeg_max_bytes": max_jpeg,
        "preview_gif_bytes": preview.len(),
        "frame_dimensions": [side, side],
        "frames": prepared.frames.len(),
        "loop_ms": loop_ms,
        "intended_fps": (prepared.frames.len() as f64) * 1000.0 / (loop_ms as f64),
        "prepare_ms": prepare_ms,
        "local_frame_samples": durations_ms.len(),
        "local_frame_p50_ms": durations_ms[durations_ms.len() / 2],
        "local_frame_p95_ms": p95,
        "local_frame_max_ms": durations_ms[durations_ms.len() - 1],
        "over_frame_interval": over_frame_interval,
        "hardware_writes": 0
    });
    std::fs::write(output.join("preview.gif"), preview)?;
    std::fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}
