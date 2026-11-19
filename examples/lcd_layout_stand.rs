//! Explicitly selected, hardware-free LCD layout and media preview.
use image::{
    AnimationDecoder, Frame, RgbaImage,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--kaleidoscope" {
        return Err("Usage: lcd_layout_stand --kaleidoscope <output-directory>".into());
    }
    let output = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&output)?;
    let source = media::BUILTIN_KALEIDOSCOPE_GIF;
    let started = Instant::now();
    let prepared = media::prepare(source.into(), 0, || false)?;
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    let font = PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system font required")?)
        .join("Fonts/seguibl.ttf");
    let mut renderer = overlay::Renderer::new(std::fs::read(font)?)?;
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
