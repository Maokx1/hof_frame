//! Small read-only status overlay, rendered in layout coordinates.

use crate::frame::{self, Corner, Metadata, Orientation};
use swash::{
    scale::{image::Image, Render, ScaleContext, Source},
    FontRef,
};

pub const BOX_WIDTH: usize = 68;
pub const BOX_HEIGHT: usize = 28;
pub const MARGIN: usize = 9;
/// Width of the overlay border in pixels.
pub const BORDER: usize = 1;
/// Target text height in pixels.
pub const TEXT_HEIGHT: usize = 14;
/// Native white palette code.
pub const WHITE: u8 = 1;
/// Native black palette code.
pub const BLACK: u8 = 0;
/// Battery icon width in pixels.
pub const ICON_WIDTH: usize = 10;
/// Battery icon height in pixels.
pub const ICON_HEIGHT: usize = 7;
const PADDING: usize = 3;
// The panel has no gray: ordered coverage preserves outline-rasterized edges.
const COVERAGE: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

type Glyphs = Vec<(usize, Option<Image>)>;

struct TextLayout {
    glyphs: Glyphs,
    width: usize,
    ascent: i32,
    height: usize,
}

pub struct StatusOverlay {
    metadata: Metadata,
    left: usize,
    top: usize,
    /// Rendered overlay pixels in layout coordinates.
    pub pixels: Vec<u8>,
}

impl StatusOverlay {
    /// Build an optional status box without modifying the underlying frame.
    pub fn new(metadata: Metadata, percentage: Option<u8>) -> Result<Self, String> {
        if !metadata.battery_indicator_enabled {
            return Ok(Self {
                metadata,
                left: 0,
                top: 0,
                pixels: Vec::new(),
            });
        }
        let text = status_text(percentage)?;
        let layout = layout_text(&text)?;
        let content_width = ICON_WIDTH + layout.width;
        if content_width > BOX_WIDTH - 2 * (BORDER + PADDING)
            || layout.height > BOX_HEIGHT - 2 * BORDER
        {
            return Err("status text does not fit the battery box".into());
        }
        let mut pixels = bordered_box()?;
        let icon_left = (BOX_WIDTH - content_width) / 2;
        draw_icon(&mut pixels, icon_left);
        draw_text(&mut pixels, layout, icon_left + ICON_WIDTH)?;
        let (left, top) = box_position(metadata);
        Ok(Self {
            metadata,
            left,
            top,
            pixels,
        })
    }

    /// Return an overlay pixel in native panel coordinates, or no overlay.
    pub fn pixel(&self, native_x: usize, native_y: usize) -> Option<u8> {
        if !self.metadata.battery_indicator_enabled
            || native_x >= frame::WIDTH
            || native_y >= frame::HEIGHT
        {
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

/// Format display-only battery data and reject percentages above 100.
fn status_text(percentage: Option<u8>) -> Result<String, String> {
    match percentage {
        Some(value) if value <= 100 => Ok(format!(" {value}%")),
        Some(_) => Err("battery percentage exceeds 100".into()),
        None => Ok(" ?%".into()),
    }
}

/// Resolve glyphs, fit their size, and calculate the shared text baseline.
fn layout_text(text: &str) -> Result<TextLayout, String> {
    let font = FontRef::from_index(include_bytes!("../assets/status-font.ttf"), 0)
        .ok_or("invalid status font")?;
    let charmap = font.charmap();
    let ids = text
        .chars()
        .map(|ch| {
            let id = charmap.map(ch);
            if id == 0 {
                Err(format!("status font is missing {ch:?}"))
            } else {
                Ok((ch, id))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
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
    let size = 32.0 * TEXT_HEIGHT as f32 / question.placement.height as f32;
    let (glyphs, width) = fit_glyphs(font, &ids, &mut context, &renderer, size)?;
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
    Ok(TextLayout {
        glyphs,
        width,
        ascent,
        height,
    })
}

/// Shrink rendered glyphs until both the height and icon-adjacent width fit.
fn fit_glyphs(
    font: FontRef<'_>,
    ids: &[(char, u16)],
    context: &mut ScaleContext,
    renderer: &Render<'_>,
    mut size: f32,
) -> Result<(Glyphs, usize), String> {
    let budget = BOX_WIDTH - 2 * (BORDER + PADDING) - ICON_WIDTH;
    loop {
        let metrics = font.glyph_metrics(&[]).scale(size);
        let mut scaler = context.builder(font).size(size).build();
        let mut glyphs = Vec::with_capacity(ids.len());
        let mut width = 0;
        let mut max_height = 0;
        for &(ch, id) in ids {
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
        if width <= budget && max_height <= TEXT_HEIGHT {
            return Ok((glyphs, width));
        }
        size -= 0.5;
        if size <= 0.0 {
            return Err("status text cannot fit beside the battery icon".into());
        }
    }
}

/// Allocate the white status box and draw its black border.
fn bordered_box() -> Result<Vec<u8>, String> {
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
    Ok(pixels)
}

/// Draw the fixed battery symbol, not a measured fill level.
fn draw_icon(pixels: &mut [u8], left: usize) {
    let top = (BOX_HEIGHT - ICON_HEIGHT) / 2;
    for y in 0..ICON_HEIGHT {
        for x in 0..ICON_WIDTH {
            let body = x < ICON_WIDTH - 2;
            let terminal = x >= ICON_WIDTH - 2 && (3..ICON_HEIGHT - 3).contains(&y);
            if body || terminal {
                pixels[(top + y) * BOX_WIDTH + left + x] = BLACK;
            }
        }
    }
}

/// Place glyphs on a centered baseline beside the icon.
fn draw_text(pixels: &mut [u8], layout: TextLayout, left: usize) -> Result<(), String> {
    let mut pen = left as i32;
    let baseline = ((BOX_HEIGHT - layout.height) / 2) as i32 + layout.ascent;
    for (advance, image) in layout.glyphs {
        if let Some(image) = image {
            draw_glyph(
                pixels,
                &image,
                pen + image.placement.left,
                baseline - image.placement.top,
            )?;
        }
        pen += advance as i32;
    }
    Ok(())
}

/// Dither glyph coverage to panel colors, rejecting any border clipping.
fn draw_glyph(pixels: &mut [u8], image: &Image, left: i32, top: i32) -> Result<(), String> {
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
            if u16::from(image.data[y * width + x]) > COVERAGE[dy % 4][dx % 4] * 16 + 7 {
                pixels[dy * BOX_WIDTH + dx] = BLACK;
            }
        }
    }
    Ok(())
}

/// Calculate corner placement in the photo's layout coordinate system.
fn box_position(metadata: Metadata) -> (usize, usize) {
    let (width, height) = crate::conversion::dimensions(metadata.orientation);
    let left = match metadata.battery_corner {
        Corner::TopLeft | Corner::BottomLeft => MARGIN,
        Corner::TopRight | Corner::BottomRight => width - MARGIN - BOX_WIDTH,
    };
    let top = match metadata.battery_corner {
        Corner::TopLeft | Corner::TopRight => MARGIN,
        Corner::BottomLeft | Corner::BottomRight => height - MARGIN - BOX_HEIGHT,
    };
    (left, top)
}
