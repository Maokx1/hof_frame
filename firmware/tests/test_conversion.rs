use firmware::conversion::*;
use firmware::frame::{self, Orientation, FRAME_BYTES};

#[test]
fn spectra6_palette_uses_ed2208_native_codes() {
    assert_eq!(
        PALETTE,
        [
            (0, [0, 0, 0]),
            (1, [255, 255, 255]),
            (2, [255, 255, 0]),
            (3, [255, 0, 0]),
            (5, [0, 0, 255]),
            (6, [0, 255, 0]),
        ]
    );
}

#[test]
fn white_and_transparency_stay_white() {
    for orientation in [Orientation::Portrait, Orientation::Landscape] {
        let (width, height) = dimensions(orientation);
        let mut output = vec![0; FRAME_BYTES];
        let pixels = vec![0; width * 4];
        for y in 0..height {
            row(&mut output, orientation, y, &pixels, 4).unwrap();
        }
        assert!(output.iter().all(|byte| *byte == 0x11));
    }
}

#[test]
fn palette_and_rotation_are_preserved() {
    for orientation in [Orientation::Portrait, Orientation::Landscape] {
        let (width, _) = dimensions(orientation);
        for (code, rgb) in PALETTE {
            let pixels: Vec<u8> = rgb
                .iter()
                .map(|v| *v as u8)
                .cycle()
                .take(width * 3)
                .collect();
            let mut output = vec![0x11; FRAME_BYTES];
            row(&mut output, orientation, 0, &pixels, 3).unwrap();
            for x in 0..width {
                let (nx, ny) = frame::native_coordinates(orientation, x, 0).unwrap();
                let index = ny * frame::WIDTH + nx;
                let actual = if index % 2 == 0 {
                    output[index / 2] >> 4
                } else {
                    output[index / 2] & 15
                };
                assert_eq!(actual, code);
            }
            assert!(frame::valid_pixels(&output));
        }
    }
}

#[test]
fn dithered_gray_contains_multiple_colors_and_bad_rows_fail() {
    let mut output = vec![0; FRAME_BYTES];
    let (width, height) = dimensions(Orientation::Landscape);
    let pixels = vec![128; width * 3];
    row(&mut output, Orientation::Landscape, 0, &pixels, 3).unwrap();
    assert!(output[..frame::WIDTH / 2]
        .iter()
        .any(|byte| byte >> 4 != byte & 15));
    assert!(row(&mut output, Orientation::Landscape, height, &pixels, 3).is_err());
    assert!(row(&mut output, Orientation::Landscape, 0, &pixels, 0).is_err());
}
