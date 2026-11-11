use crate::frame::{self, Corner, Metadata, Orientation};
use swash::{
    scale::{Render, ScaleContext, Source},
    FontRef,
};

pub const BOX_WIDTH: usize = 120;
pub const BOX_HEIGHT: usize = 48;
pub const MARGIN: usize = 16;
const BORDER: usize = 2;
const TEXT_HEIGHT: usize = 24;
const WHITE: u8 = 1;
const BLACK: u8 = 0;
const ICON_WIDTH: usize = 17;
const ICON_HEIGHT: usize = 11;
const PADDING: usize = 5;

pub struct StatusOverlay {
    metadata: Metadata,
    left: usize,
    top: usize,
    pixels: Vec<u8>,
}

impl StatusOverlay {
    pub fn new(metadata: Metadata, percentage: Option<u8>) -> Result<Self, String> {
        let text = match percentage {
            Some(value) if value <= 100 => format!(" {value}%"),
            Some(_) => return Err("battery percentage exceeds 100".into()),
            None => " ?%".into(),
        };
        let font = FontRef::from_index(include_bytes!("../assets/status-font.ttf"), 0)
            .ok_or("invalid status font")?;
        let charmap = font.charmap();
        let glyph_ids: Vec<_> = text
            .chars()
            .map(|ch| {
                let id = charmap.map(ch);
                if id == 0 {
                    Err(format!("status font is missing {ch:?}"))
                } else {
                    Ok((ch, id))
                }
            })
            .collect::<Result<_, _>>()?;
        let mut context = ScaleContext::new();
        let renderer = Render::new(&[Source::Outline]);
        let question = renderer
            .render(
                &mut context.builder(font).size(32.0).build(),
                charmap.map('?'),
            )
            .ok_or("cannot render status font")?;
        if question.placement.height == 0 {
            return Err("invalid status font bounds".into());
        }
        let mut size = 32.0 * TEXT_HEIGHT as f32 / question.placement.height as f32;
        let text_budget = BOX_WIDTH - 2 * (BORDER + PADDING) - ICON_WIDTH;
        let (glyphs, width) = loop {
            let metrics = font.glyph_metrics(&[]).scale(size);
            let mut scaler = context.builder(font).size(size).build();
            let mut glyphs = Vec::with_capacity(glyph_ids.len());
            let mut width = 0;
            let mut max_height = 0;
            for &(ch, id) in &glyph_ids {
                let advance = metrics.advance_width(id).ceil() as usize;
                let image = if ch == ' ' {
                    None
                } else {
                    let image = renderer
                        .render(&mut scaler, id)
                        .ok_or_else(|| format!("cannot render status glyph {ch:?}"))?;
                    max_height = max_height.max(image.placement.height as usize);
                    Some(image)
                };
                glyphs.push((advance, image));
                width += advance;
            }
            if width <= text_budget && max_height <= TEXT_HEIGHT {
                break (glyphs, width);
            }
            size -= 0.5;
            if size <= 0.0 {
                return Err("status text cannot fit beside the battery icon".into());
            }
        };
        let ascent = glyphs
            .iter()
            .map(|(_, image)| image.as_ref().map_or(0, |image| image.placement.top))
            .max()
            .ok_or("empty status text")?;
        let descent = glyphs
            .iter()
            .map(|(_, image)| {
                image.as_ref().map_or(0, |image| {
                    image.placement.top - image.placement.height as i32
                })
            })
            .min()
            .ok_or("empty status text")?;
        let height = usize::try_from(ascent - descent).map_err(|_| "invalid font bounds")?;
        let content_width = ICON_WIDTH + width;
        if content_width > BOX_WIDTH - 2 * (BORDER + PADDING) || height > BOX_HEIGHT - 2 * BORDER {
            return Err("status text does not fit the battery box".into());
        }
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(BOX_WIDTH * BOX_HEIGHT)
            .map_err(|e| e.to_string())?;
        pixels.resize(BOX_WIDTH * BOX_HEIGHT, WHITE);
        for y in 0..BOX_HEIGHT {
            for x in 0..BOX_WIDTH {
                if !(BORDER..BOX_WIDTH - BORDER).contains(&x)
                    || !(BORDER..BOX_HEIGHT - BORDER).contains(&y)
                {
                    pixels[y * BOX_WIDTH + x] = BLACK;
                }
            }
        }
        let icon_left = (BOX_WIDTH - content_width) / 2;
        let icon_top = (BOX_HEIGHT - ICON_HEIGHT) / 2;
        for y in 0..ICON_HEIGHT {
            for x in 0..ICON_WIDTH {
                let body = x < ICON_WIDTH - 2;
                let terminal = x >= ICON_WIDTH - 2 && (3..ICON_HEIGHT - 3).contains(&y);
                if body || terminal {
                    pixels[(icon_top + y) * BOX_WIDTH + icon_left + x] = BLACK;
                }
            }
        }
        let mut pen = (icon_left + ICON_WIDTH) as i32;
        let baseline = ((BOX_HEIGHT - height) / 2) as i32 + ascent;
        // The panel has no gray: ordered coverage preserves outline-rasterized edges.
        const COVERAGE: [[u16; 4]; 4] =
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for (advance, image) in glyphs {
            if let Some(image) = image {
                let left = pen + image.placement.left;
                let top = baseline - image.placement.top;
                let width = image.placement.width as usize;
                for y in 0..image.placement.height as usize {
                    for x in 0..width {
                        let dx = left + x as i32;
                        let dy = top + y as i32;
                        if dx < BORDER as i32
                            || dx >= (BOX_WIDTH - BORDER) as i32
                            || dy < BORDER as i32
                            || dy >= (BOX_HEIGHT - BORDER) as i32
                        {
                            return Err("status glyph clips the battery box".into());
                        }
                        let dx = dx as usize;
                        let dy = dy as usize;
                        if u16::from(image.data[y * width + x]) > COVERAGE[dy % 4][dx % 4] * 16 + 7
                        {
                            pixels[dy * BOX_WIDTH + dx] = BLACK;
                        }
                    }
                }
            }
            pen += advance as i32;
        }
        let (width, height) = crate::conversion::dimensions(metadata.orientation);
        let left = match metadata.battery_corner {
            Corner::TopLeft | Corner::BottomLeft => MARGIN,
            Corner::TopRight | Corner::BottomRight => width - MARGIN - BOX_WIDTH,
        };
        let top = match metadata.battery_corner {
            Corner::TopLeft | Corner::TopRight => MARGIN,
            Corner::BottomLeft | Corner::BottomRight => height - MARGIN - BOX_HEIGHT,
        };
        Ok(Self {
            metadata,
            left,
            top,
            pixels,
        })
    }

    pub fn pixel(&self, native_x: usize, native_y: usize) -> Option<u8> {
        if native_x >= frame::WIDTH || native_y >= frame::HEIGHT {
            return None;
        }
        let (x, y) = match self.metadata.orientation {
            Orientation::Landscape => (native_x, native_y),
            Orientation::Portrait => (native_y, frame::WIDTH - 1 - native_x),
        };
        let x = x.checked_sub(self.left)?;
        let y = y.checked_sub(self.top)?;
        if x >= BOX_WIDTH || y >= BOX_HEIGHT {
            return None;
        }
        Some(self.pixels[y * BOX_WIDTH + x])
    }
}

#[cfg(test)]
#[path = "../tests/unit/overlay.rs"]
mod tests;
