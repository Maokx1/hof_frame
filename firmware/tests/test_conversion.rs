use firmware::conversion::*;
use firmware::frame::{self, FRAME_BYTES, Orientation};

#[test]
fn converts_known_colors_to_packed_pixels_in_both_orientations() {
    let (native_width, native_height) = (800, 480);
    let colors = [
        ([0, 0, 0], 0),
        ([255, 255, 255], 1),
        ([255, 255, 0], 2),
        ([255, 0, 0], 3),
        ([0, 0, 255], 5),
        ([0, 255, 0], 6),
    ];
    for (orientation, width, height) in [
        (Orientation::Landscape, native_width, native_height),
        (Orientation::Portrait, native_height, native_width),
    ] {
        assert_eq!(dimensions(orientation), (width, height));
        let y = height / 3;
        let pixels: Vec<u8> = (0..width)
            .flat_map(|x| colors[x % colors.len()].0)
            .collect();
        let mut output = vec![0x44; FRAME_BYTES];
        row(&mut output, orientation, y, &pixels, 3).unwrap();

        for x in 0..width {
            let (native_x, native_y) = match orientation {
                Orientation::Landscape => (x, y),
                Orientation::Portrait => (native_width - 1 - y, x),
            };
            let index = native_y * native_width + native_x;
            let byte = output[index / 2];
            let actual = if index.is_multiple_of(2) {
                byte >> 4
            } else {
                byte & 15
            };
            assert_eq!(actual, colors[x % colors.len()].1, "{orientation:?} x={x}");
        }
        let (neighbor_x, neighbor_y) = match orientation {
            Orientation::Landscape => (0, y - 1),
            Orientation::Portrait => (native_width - y, 0),
        };
        assert_eq!(frame::pixel(&output, neighbor_x, neighbor_y), Some(4));
    }
}

#[test]
fn transparent_pixels_become_white_and_opaque_pixels_keep_their_color() {
    for (transparent, opaque, expected) in [
        (&[0, 0][..], &[0, 255][..], 0x10),
        (&[255, 0, 0, 0][..], &[255, 0, 0, 255][..], 0x13),
    ] {
        let mut pixels = Vec::new();
        for _ in 0..frame::WIDTH / 2 {
            pixels.extend_from_slice(transparent);
            pixels.extend_from_slice(opaque);
        }
        let mut output = vec![0x66; FRAME_BYTES];
        row(
            &mut output,
            Orientation::Landscape,
            0,
            &pixels,
            transparent.len(),
        )
        .unwrap();
        assert!(
            output[..frame::WIDTH / 2]
                .iter()
                .all(|byte| *byte == expected)
        );
        assert!(output[frame::WIDTH / 2..].iter().all(|byte| *byte == 0x66));
    }
}

#[test]
fn fully_transparent_image_is_white_in_both_orientations() {
    for (orientation, width, height) in [
        (Orientation::Landscape, 800, 480),
        (Orientation::Portrait, 480, 800),
    ] {
        let pixels = vec![0; width * 4];
        let mut output = vec![0; FRAME_BYTES];
        for y in 0..height {
            row(&mut output, orientation, y, &pixels, 4).unwrap();
        }
        assert!(output.iter().all(|byte| *byte == 0x11), "{orientation:?}");
    }
}

#[test]
fn partially_transparent_black_is_composited_over_white() {
    for pixel in [&[0, 128][..], &[0, 0, 0, 128][..]] {
        let mut output = vec![0; FRAME_BYTES];
        row(
            &mut output,
            Orientation::Landscape,
            0,
            &pixel.repeat(frame::WIDTH),
            pixel.len(),
        )
        .unwrap();
        assert_eq!(&output[..4], &[0x01; 4]);
    }
}

#[test]
fn grayscale_dithering_depends_on_pixel_position_and_brightness() {
    for (gray, expected_rows) in [
        (
            110,
            [[0x00, 0x01], [0x10, 0x10], [0x01, 0x00], [0x10, 0x10]],
        ),
        (
            160,
            [[0x01, 0x01], [0x11, 0x11], [0x01, 0x01], [0x11, 0x11]],
        ),
    ] {
        let pixels = vec![gray; frame::WIDTH];
        let mut output = vec![0; FRAME_BYTES];
        for (y, expected) in expected_rows.iter().enumerate() {
            row(&mut output, Orientation::Landscape, y, &pixels, 1).unwrap();
            let start = y * frame::WIDTH / 2;
            for (byte_index, byte) in output[start..start + frame::WIDTH / 2].iter().enumerate() {
                assert_eq!(
                    *byte,
                    expected[byte_index % 2],
                    "gray={gray}, y={y}, byte={byte_index}"
                );
            }
        }
    }
}

#[test]
fn invalid_rows_return_typed_errors_without_modifying_output() {
    let (width, height) = dimensions(Orientation::Landscape);
    let pixels = vec![0; width * 3];
    let mut output = vec![0x11; FRAME_BYTES];

    for (y, input, channels) in [
        (height, pixels.as_slice(), 3),
        (0, pixels.as_slice(), 0),
        (0, pixels.as_slice(), 5),
        (0, &pixels[..pixels.len() - 1], 3),
    ] {
        assert_eq!(
            row(&mut output, Orientation::Landscape, y, input, channels),
            Err(ConversionError::InvalidRow)
        );
        assert!(output.iter().all(|byte| *byte == 0x11));
    }
    assert_eq!(
        row(
            &mut output[..FRAME_BYTES - 1],
            Orientation::Landscape,
            0,
            &pixels,
            3,
        ),
        Err(ConversionError::InvalidRow)
    );
    assert!(output.iter().all(|byte| *byte == 0x11));
}
