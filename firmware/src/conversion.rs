use crate::frame::{self, Orientation, FRAME_BYTES};

const PALETTE: [(u8, [i32; 3]); 6] = [
    (0, [0, 0, 0]),
    (1, [255, 255, 255]),
    (5, [255, 0, 0]),
    (6, [255, 255, 0]),
    (2, [0, 255, 0]),
    (3, [0, 0, 255]),
];
const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

pub fn dimensions(orientation: Orientation) -> (usize, usize) {
    match orientation {
        Orientation::Portrait => (frame::HEIGHT, frame::WIDTH),
        Orientation::Landscape => (frame::WIDTH, frame::HEIGHT),
    }
}

pub fn row(
    output: &mut [u8],
    orientation: Orientation,
    y: usize,
    pixels: &[u8],
    channels: usize,
) -> Result<(), &'static str> {
    let (width, height) = dimensions(orientation);
    if output.len() != FRAME_BYTES
        || y >= height
        || !(1..=4).contains(&channels)
        || pixels.len() != width * channels
    {
        return Err("invalid conversion row dimensions");
    }
    for (x, pixel) in pixels.chunks_exact(channels).enumerate() {
        let alpha = if channels == 2 || channels == 4 {
            i32::from(pixel[channels - 1])
        } else {
            255
        };
        let mut rgb = [0; 3];
        for (channel, value) in rgb.iter_mut().enumerate() {
            let source = i32::from(pixel[if channels <= 2 { 0 } else { channel }]);
            *value = (source * alpha + 255 * (255 - alpha) + 127) / 255;
            if *value != 0 && *value != 255 {
                *value = (*value + (BAYER[y % 4][x % 4] * 2 - 15) * 4).clamp(0, 255);
            }
        }
        let color = PALETTE
            .iter()
            .min_by_key(|(_, candidate)| {
                rgb.iter()
                    .zip(candidate)
                    .map(|(a, b)| (a - b).pow(2))
                    .sum::<i32>()
            })
            .expect("nonempty palette")
            .0;
        let (native_x, native_y) =
            frame::native_coordinates(orientation, x, y).expect("validated layout coordinates");
        let index = native_y * frame::WIDTH + native_x;
        let byte = &mut output[index / 2];
        if index % 2 == 0 {
            *byte = (*byte & 15) | color << 4;
        } else {
            *byte = (*byte & 240) | color;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/conversion.rs"]
mod tests;
