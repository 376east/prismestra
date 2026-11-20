//! The portable editor's LCD draft has no device or runtime handle.
use crate::{
    app::scene_file::preview::Pixels,
    lcd::{media, overlay},
};
use image::{RgbaImage, imageops};
use serde_json::{Value, json};
use std::time::Instant;

const PREVIEW_SIDE: u32 = 120;
const FONT: &[u8] = include_bytes!("../../web/assets/SourceSans3.ttf");

pub(in crate::ui::windows::modern) struct Layout {
    frames: Vec<(RgbaImage, u32)>,
    renderer: overlay::Renderer,
    position: (u32, u32),
    request: u32,
    last_index: Option<usize>,
    started: Instant,
}

impl Layout {
    pub(super) fn new() -> Result<Self, String> {
        let mut frames = Vec::new();
        let mut end = 0;
        for frame in media::decoded_frames(media::BUILTIN_KALEIDOSCOPE_GIF.into())? {
            let (pixels, delay) = frame?;
            if frames.len() == media::MAX_FRAMES || pixels.width() > 256 || pixels.height() > 256 {
                return Err("Встроенный GIF превышает предел предпросмотра".into());
            }
            end += delay.clamp(50, 10_000);
            frames.push((pixels, end));
        }
        if frames.is_empty() {
            return Err("Во встроенном GIF нет кадров".into());
        }
        Ok(Self {
            frames,
            renderer: overlay::Renderer::new(FONT.to_vec())?,
            position: (0, 0),
            request: 0,
            last_index: None,
            started: Instant::now(),
        })
    }

    pub(super) fn validate_position(x: u32, y: u32) -> Result<(), String> {
        if x > overlay::LAYOUT_SIZE - overlay::CARD_WIDTH
            || y > overlay::LAYOUT_SIZE - overlay::CARD_HEIGHT
        {
            return Err("Карточка выходит за границы LCD".into());
        }
        Ok(())
    }

    pub(super) fn set_position(&mut self, request: u32, x: u32, y: u32) {
        self.position = (x, y);
        self.request = request;
        self.last_index = None;
    }

    pub(super) fn request(&self) -> u32 {
        self.request
    }

    pub(super) fn message(&mut self) -> Result<Option<Value>, String> {
        self.message_at(self.started.elapsed().as_millis())
    }

    fn message_at(&mut self, elapsed_ms: u128) -> Result<Option<Value>, String> {
        let duration = self.frames.last().unwrap().1;
        let time = (elapsed_ms % u128::from(duration)) as u32;
        let index = self.frames.partition_point(|(_, end)| *end <= time);
        if self.last_index == Some(index) {
            return Ok(None);
        }
        let side = overlay::LAYOUT_SIZE;
        let background = media::canvas(&self.frames[index].0, side, side, true, 0)?;
        let (x, y) = self.position;
        let frame = self.renderer.render_blocks(
            &background,
            &[overlay::Block {
                x,
                y,
                label: "CPU",
                celsius: 51.0,
            }],
        )?;
        let preview = imageops::resize(
            &frame,
            PREVIEW_SIDE,
            PREVIEW_SIDE,
            imageops::FilterType::Triangle,
        );
        let colors = preview
            .pixels()
            .map(|pixel| u32::from_be_bytes([0, pixel[0], pixel[1], pixel[2]]))
            .collect();
        let pixels = Pixels {
            width: PREVIEW_SIDE as usize,
            height: PREVIEW_SIDE as usize,
            colors,
        };
        self.last_index = Some(index);
        Ok(Some(
            json!({"kind":"scene_file_lcd_frame","request":self.request,"frame":pixels}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_animation_and_card_position_produce_revision_bound_preview_frames() {
        let mut layout = Layout::new().unwrap();
        Layout::validate_position(292, 404).unwrap();
        assert!(Layout::validate_position(293, 0).is_err());
        layout.set_position(17, 100, 100);
        let first = layout.message_at(0).unwrap().unwrap();
        assert_eq!(first["request"], 17);
        assert_eq!(
            first["frame"]["colors"].as_array().unwrap().len(),
            120 * 120
        );
        assert!(layout.message_at(0).unwrap().is_none());
        layout.set_position(18, 160, 200);
        layout.set_position(19, 240, 300);
        let moved = layout.message_at(0).unwrap().unwrap();
        assert_eq!(moved["request"], 19);
        let pixel = |frame: &Value, x: usize, y: usize| {
            frame["frame"]["colors"][y * 120 + x].as_u64().unwrap()
        };
        assert_ne!(pixel(&first, 50, 34), pixel(&moved, 50, 34));
        assert_ne!(pixel(&first, 84, 85), pixel(&moved, 84, 85));
        assert_eq!(pixel(&first, 119, 119), pixel(&moved, 119, 119));
        let animated = layout
            .message_at(u128::from(layout.frames[0].1))
            .unwrap()
            .unwrap();
        assert_eq!(animated["request"], 19);
        assert_ne!(animated["frame"], moved["frame"]);
    }
}
