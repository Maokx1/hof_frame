use crate::frame::{self, Corner, Metadata, Orientation};

pub const BOX_WIDTH: usize = 120;
pub const BOX_HEIGHT: usize = 48;
pub const MARGIN: usize = 16;
const BORDER: usize = 2;
const TEXT_HEIGHT: usize = 24;
const WHITE: u8 = 1;
const BLACK: u8 = 0;
const ICON_WIDTH: usize = 17;
const ICON_HEIGHT: usize = 11;
const ICON_GAP: usize = 5;
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
            Some(value) if value <= 100 => format!("{value}%"),
            Some(_) => return Err("battery percentage exceeds 100".into()),
            None => "?%".into(),
        };
        let font = fontdue::Font::from_bytes(
            include_bytes!("../assets/status-font.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .map_err(str::to_owned)?;
        let mut size = 32.0 * TEXT_HEIGHT as f32 / font.metrics('?', 32.0).height as f32;
        let text_budget = BOX_WIDTH - 2 * (BORDER + PADDING) - ICON_WIDTH - ICON_GAP;
        while text
            .chars()
            .map(|ch| font.metrics(ch, size).advance_width.ceil() as usize)
            .sum::<usize>()
            > text_budget
            || text
                .chars()
                .map(|ch| font.metrics(ch, size).height)
                .max()
                .unwrap_or(0)
                > TEXT_HEIGHT
        {
            size -= 0.5;
            if size <= 0.0 {
                return Err("status text cannot fit beside the battery icon".into());
            }
        }
        let glyphs: Vec<_> = text.chars().map(|ch| font.rasterize(ch, size)).collect();
        let width: usize = glyphs
            .iter()
            .map(|(m, _)| m.advance_width.ceil() as usize)
            .sum();
        let ascent = glyphs
            .iter()
            .map(|(m, _)| m.height as i32 + m.ymin)
            .max()
            .ok_or("empty status text")?;
        let descent = glyphs
            .iter()
            .map(|(m, _)| m.ymin)
            .min()
            .ok_or("empty status text")?;
        let height = usize::try_from(ascent - descent).map_err(|_| "invalid font bounds")?;
        let content_width = ICON_WIDTH + ICON_GAP + width;
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
                if x < BORDER || x >= BOX_WIDTH - BORDER || y < BORDER || y >= BOX_HEIGHT - BORDER {
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
        let mut pen = (icon_left + ICON_WIDTH + ICON_GAP) as i32;
        let baseline = ((BOX_HEIGHT - height) / 2) as i32 + ascent;
        // The panel has no gray: ordered coverage preserves outline-rasterized edges.
        const COVERAGE: [[u16; 4]; 4] =
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for (metrics, bitmap) in glyphs {
            let left = pen + metrics.xmin;
            let top = baseline - metrics.height as i32 - metrics.ymin;
            for y in 0..metrics.height {
                for x in 0..metrics.width {
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
                    if u16::from(bitmap[y * metrics.width + x]) > COVERAGE[dy % 4][dx % 4] * 16 + 7
                    {
                        pixels[dy * BOX_WIDTH + dx] = BLACK;
                    }
                }
            }
            pen += metrics.advance_width.ceil() as i32;
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
            Orientation::Portrait => (native_x, native_y),
            Orientation::Landscape => (native_y, frame::WIDTH - 1 - native_x),
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
mod tests {
    use super::*;

    #[test]
    fn all_corners_follow_layout_rotation_and_exact_margins() {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            for battery_corner in [
                Corner::TopLeft,
                Corner::TopRight,
                Corner::BottomLeft,
                Corner::BottomRight,
            ] {
                let overlay = StatusOverlay::new(
                    Metadata {
                        orientation,
                        battery_corner,
                    },
                    None,
                )
                .unwrap();
                let (width, height) = crate::conversion::dimensions(orientation);
                let left = if matches!(battery_corner, Corner::TopLeft | Corner::BottomLeft) {
                    MARGIN
                } else {
                    width - MARGIN - BOX_WIDTH
                };
                let top = if matches!(battery_corner, Corner::TopLeft | Corner::TopRight) {
                    MARGIN
                } else {
                    height - MARGIN - BOX_HEIGHT
                };
                for (x, y, expected) in [
                    (left, top, Some(BLACK)),
                    (left + BOX_WIDTH - 1, top + BOX_HEIGHT - 1, Some(BLACK)),
                    (left + 4, top + 4, Some(WHITE)),
                    (left - 1, top, None),
                    (left + BOX_WIDTH, top, None),
                    (left, top - 1, None),
                    (left, top + BOX_HEIGHT, None),
                ] {
                    let (nx, ny) = frame::native_coordinates(orientation, x, y).unwrap();
                    assert_eq!(overlay.pixel(nx, ny), expected);
                }
            }
        }
    }

    #[test]
    fn unknown_text_is_24_pixels_high_and_all_percentages_fit() {
        let unknown = StatusOverlay::new(Default::default(), None).unwrap();
        let text_rows: Vec<_> = (BORDER..BOX_HEIGHT - BORDER)
            .filter(|y| {
                (BORDER..BOX_WIDTH - BORDER).any(|x| unknown.pixels[y * BOX_WIDTH + x] == BLACK)
            })
            .collect();
        assert_eq!(
            text_rows.last().unwrap() - text_rows.first().unwrap() + 1,
            TEXT_HEIGHT
        );
        for value in 0..=100 {
            let overlay = StatusOverlay::new(Default::default(), Some(value)).unwrap();
            assert!(overlay
                .pixels
                .iter()
                .all(|pixel| matches!(pixel, &BLACK | &WHITE)));
        }
        assert!(StatusOverlay::new(Default::default(), Some(101)).is_err());
        assert_eq!(unknown.pixel(frame::WIDTH, 0), None);
        assert_eq!(unknown.pixel(0, frame::HEIGHT), None);
    }

    #[test]
    fn overlay_never_modifies_saved_payload() {
        let photo = vec![0x66; frame::FRAME_BYTES];
        let overlay = StatusOverlay::new(Default::default(), None).unwrap();
        let corner = frame::WIDTH - MARGIN - BOX_WIDTH;
        assert_eq!(overlay.pixel(corner, MARGIN).unwrap_or(6), BLACK);
        assert_eq!(overlay.pixel(0, 0).unwrap_or(6), 6);
        assert!(photo.iter().all(|byte| *byte == 0x66));
    }

    #[test]
    fn full_battery_precedes_unknown_and_full_percentage() {
        for percentage in [None, Some(100)] {
            let overlay = StatusOverlay::new(Default::default(), percentage).unwrap();
            let y = (BOX_HEIGHT - ICON_HEIGHT) / 2;
            let left = (BORDER..BOX_WIDTH - BORDER)
                .find(|x| overlay.pixels[y * BOX_WIDTH + x] == BLACK)
                .unwrap();
            assert_eq!(
                overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left],
                BLACK
            );
            assert_eq!(
                overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left + 5],
                BLACK
            );
            assert_eq!(
                overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left + ICON_WIDTH - 1],
                BLACK
            );
            assert!((left + ICON_WIDTH + ICON_GAP..BOX_WIDTH - BORDER)
                .any(|x| (BORDER..BOX_HEIGHT - BORDER)
                    .any(|y| overlay.pixels[y * BOX_WIDTH + x] == BLACK)));
        }
    }
}
