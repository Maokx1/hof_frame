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

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverlayError {
    /// The battery percentage is above 100.
    #[error("battery percentage {0} exceeds 100")]
    InvalidPercentage(u8),
    /// The embedded status font cannot be parsed or measured.
    #[error("invalid status font")]
    InvalidFont,
    /// The embedded status font has no glyph for a status character.
    #[error("status font is missing {0:?}")]
    MissingGlyph(char),
    /// A status glyph could not be rasterized.
    #[error("cannot render status glyph {0:?}")]
    UnrenderableGlyph(char),
    /// The status text has no glyphs to lay out.
    #[error("empty status text")]
    EmptyText,
    /// The status text cannot fit inside the battery box.
    #[error("status text does not fit the battery box")]
    TextTooLarge,
    /// A glyph would draw over the battery box border.
    #[error("status glyph clips the battery box")]
    GlyphClipped,
    /// The overlay pixel buffer could not be allocated.
    #[error("cannot allocate status overlay pixels")]
    OutOfMemory,
}

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
    pub fn new(metadata: Metadata, percentage: Option<u8>) -> Result<Self, OverlayError> {
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
            return Err(OverlayError::TextTooLarge);
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
fn status_text(percentage: Option<u8>) -> Result<String, OverlayError> {
    match percentage {
        Some(value) if value <= 100 => Ok(format!(" {value}%")),
        Some(value) => Err(OverlayError::InvalidPercentage(value)),
        None => Ok(" ?%".into()),
    }
}

/// Resolve glyphs, fit their size, and calculate the shared text baseline.
fn layout_text(text: &str) -> Result<TextLayout, OverlayError> {
    let font = FontRef::from_index(include_bytes!("../assets/status-font.ttf"), 0)
        .ok_or(OverlayError::InvalidFont)?;
    let charmap = font.charmap();
    let ids = text
        .chars()
        .map(|ch| {
            let id = charmap.map(ch);
            if id == 0 {
                Err(OverlayError::MissingGlyph(ch))
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
        .ok_or(OverlayError::UnrenderableGlyph('?'))?;
    if question.placement.height == 0 {
        return Err(OverlayError::InvalidFont);
    }
    let size = 32.0 * TEXT_HEIGHT as f32 / question.placement.height as f32;
    let (glyphs, width) = fit_glyphs(font, &ids, &mut context, &renderer, size)?;
    let ascent = glyphs
        .iter()
        .map(|(_, image)| image.as_ref().map_or(0, |image| image.placement.top))
        .max()
        .ok_or(OverlayError::EmptyText)?;
    let descent = glyphs
        .iter()
        .map(|(_, image)| {
            image.as_ref().map_or(0, |image| {
                image.placement.top - image.placement.height as i32
            })
        })
        .min()
        .ok_or(OverlayError::EmptyText)?;
    let height = usize::try_from(ascent - descent).map_err(|_| OverlayError::InvalidFont)?;
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
) -> Result<(Glyphs, usize), OverlayError> {
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
                    .ok_or(OverlayError::UnrenderableGlyph(ch))?;
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
            return Err(OverlayError::TextTooLarge);
        }
    }
}

/// Allocate the white status box and draw its black border.
fn bordered_box() -> Result<Vec<u8>, OverlayError> {
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(BOX_WIDTH * BOX_HEIGHT)
        .map_err(|_| OverlayError::OutOfMemory)?;
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
fn draw_text(pixels: &mut [u8], layout: TextLayout, left: usize) -> Result<(), OverlayError> {
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
fn draw_glyph(pixels: &mut [u8], image: &Image, left: i32, top: i32) -> Result<(), OverlayError> {
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
                return Err(OverlayError::GlyphClipped);
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
