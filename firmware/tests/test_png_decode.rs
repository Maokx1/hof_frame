use firmware::png_decode::*;
use firmware::{conversion, frame};
use std::io::{self, Cursor, Read, Seek, SeekFrom};

#[test]
fn qualification_signature_only_upload_is_rejected_before_pixel_conversion() {
    let encoded = b"\x89PNG\r\n\x1a\n";
    assert!(firmware::upload::image_signature(encoded));
    let mut packed = vec![0x11; frame::FRAME_BYTES];
    assert!(decode(encoded, frame::Metadata::default(), &mut packed, || Ok(())).is_err());
    assert!(packed.iter().all(|byte| *byte == 0x11));
}

#[test]
fn wrong_output_size_is_rejected_before_decoding() {
    for len in [frame::FRAME_BYTES - 1, frame::FRAME_BYTES + 1] {
        let mut output = vec![0x11; len];
        assert!(matches!(
            decode(&[], Default::default(), &mut output, || {
                panic!("buffer size must be checked before decoding")
            }),
            Err(DecodeError::OverBudget)
        ));
        assert!(output.iter().all(|byte| *byte == 0x11));
    }
}

fn png(
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    bytes: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(bytes)
        .unwrap();
    encoded
}

#[test]
fn decodes_both_layouts_without_full_rgb_buffer() {
    for orientation in [frame::Orientation::Portrait, frame::Orientation::Landscape] {
        let (w, h) = conversion::dimensions(orientation);
        let encoded = png(
            w as u32,
            h as u32,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![255; w * h],
        );
        let mut output = vec![0; frame::FRAME_BYTES];
        let mut checkpoints = 0;
        decode(
            &encoded,
            frame::Metadata {
                orientation,
                ..Default::default()
            },
            &mut output,
            || {
                checkpoints += 1;
                Ok(())
            },
        )
        .unwrap();
        assert!(output.iter().all(|byte| *byte == 0x11));
        assert!(checkpoints >= h / 16);
    }
}

#[test]
fn alpha_and_sixteen_bit_are_supported() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    // Distinct high and low bytes expose incorrect 16-bit channel stripping.
    let opaque_black = [0x00, 0xff, 0xff, 0x00];
    let opaque_white = [0xff, 0x00, 0xff, 0x00];
    let transparent_black = [0x00, 0xff, 0x00, 0xff];
    let pixels = [opaque_black, opaque_white, transparent_black, opaque_white]
        .concat()
        .repeat(width * height / 4);
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::GrayscaleAlpha,
        png::BitDepth::Sixteen,
        &pixels,
    );
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
    for (index, byte) in output.iter().enumerate() {
        assert_eq!(*byte, [0x01, 0x11][index % 2], "byte {index}");
    }
}

#[test]
fn decodes_photo_sized_rgb_with_inflater_workspace() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let mut pixels = vec![0; width * height * 3];
    let mut random = 0x1234_5678_u32;
    for byte in &mut pixels {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        *byte = random as u8;
    }
    pixels[..12].copy_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &pixels,
    );
    assert!(encoded.len() > width * height * 2);
    assert!(encoded.len() <= firmware::upload::MAX_UPLOAD_BYTES);
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
    assert_eq!(&output[..2], &[0x36, 0x51]);
    assert!(frame::valid_pixels(&output));
}

#[test]
fn streams_exact_seven_mb_png_and_rejects_one_more_before_conversion() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let mut pixels = vec![0; width * height * 3];
    let mut random = 0x1234_5678_u32;
    for byte in &mut pixels {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        *byte = random as u8;
    }
    let mut encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &pixels,
    );
    assert!(encoded.len() > width * height * 2);
    let end = encoded.split_off(encoded.len() - 12);
    let padding = firmware::upload::MAX_UPLOAD_BYTES - encoded.len() - end.len() - 12;
    encoded.extend_from_slice(&(padding as u32).to_be_bytes());
    let chunk_start = encoded.len();
    encoded.extend_from_slice(b"tEXt");
    encoded.extend_from_slice(b"padding\0");
    encoded.resize(encoded.len() + padding - 8, b'x');
    let crc = frame::crc32(&encoded[chunk_start..]);
    encoded.extend_from_slice(&crc.to_be_bytes());
    encoded.extend_from_slice(&end);
    assert_eq!(encoded.len(), 7_000_000);
    let mut output = vec![0; frame::FRAME_BYTES];
    decode_reader(
        std::io::BufReader::with_capacity(4096, Cursor::new(&encoded)),
        encoded.len(),
        Default::default(),
        &mut output,
        || Ok(()),
    )
    .unwrap();
    assert!(frame::valid_pixels(&output));
    encoded.push(0);
    output.fill(0x11);
    assert!(matches!(
        decode_reader(
            std::io::BufReader::with_capacity(4096, Cursor::new(&encoded)),
            encoded.len(),
            Default::default(),
            &mut output,
            || panic!("oversize must fail before decoding"),
        ),
        Err(DecodeError::OverBudget)
    ));
    assert!(output.iter().all(|byte| *byte == 0x11));
}

#[test]
fn malformed_wrong_dimensions_and_deadline_are_rejected() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &vec![255; width * height],
    );
    let mut output = vec![0; frame::FRAME_BYTES];
    assert!(matches!(
        decode(&encoded, Default::default(), &mut output, || Err(
            DecodeError::DeadlineExceeded
        )),
        Err(DecodeError::DeadlineExceeded)
    ));
    assert!(
        decode(
            &encoded[..encoded.len() / 2],
            Default::default(),
            &mut output,
            || Ok(())
        )
        .is_err()
    );
    assert!(matches!(
        decode(b"\xff\xd8\xff", Default::default(), &mut output, || Ok(())),
        Err(DecodeError::Png(_))
    ));
    let small = png(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[0, 0, 0]);
    assert!(matches!(
        decode(&small, Default::default(), &mut output, || Ok(())),
        Err(DecodeError::UnsupportedLayout)
    ));
}

#[test]
fn deadline_during_conversion_stops_decoding() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &vec![255; width * height],
    );
    let mut output = vec![0x66; frame::FRAME_BYTES];
    let mut checkpoints = 0;
    let result = decode(&encoded, Default::default(), &mut output, || {
        checkpoints += 1;
        if checkpoints == 3 {
            Err(DecodeError::DeadlineExceeded)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(DecodeError::DeadlineExceeded)));
    assert_eq!(checkpoints, 3);
    assert!(output.contains(&0x11));
    assert!(output.contains(&0x66));
}

#[test]
fn staging_read_failure_is_preserved_as_a_png_io_error() {
    struct FailingRead {
        input: Cursor<Vec<u8>>,
        remaining: usize,
    }

    impl Read for FailingRead {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            if self.remaining == 0 {
                return Err(io::Error::other("staging read failed"));
            }
            let limit = buffer.len().min(self.remaining);
            let count = self.input.read(&mut buffer[..limit])?;
            self.remaining -= count;
            Ok(count)
        }
    }

    impl Seek for FailingRead {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.input.seek(position)
        }
    }

    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &vec![255; width * height],
    );
    let encoded_len = encoded.len();
    let reader = io::BufReader::with_capacity(
        64,
        FailingRead {
            input: Cursor::new(encoded),
            remaining: encoded_len / 2,
        },
    );
    let mut output = vec![0; frame::FRAME_BYTES];

    let error = decode_reader(reader, encoded_len, Default::default(), &mut output, || {
        Ok(())
    })
    .unwrap_err();
    assert!(
        matches!(
            &error,
            DecodeError::Png(png::DecodingError::IoError(source))
                if source.kind() == io::ErrorKind::Other && source.to_string() == "staging read failed"
        ),
        "{error:?}"
    );
}

#[test]
fn indexed_png_expands_palette() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 1, 2, 1].repeat(width * height / 4))
        .unwrap();
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
    for (index, byte) in output.iter().enumerate() {
        assert_eq!(*byte, [0x36, 0x56][index % 2], "byte {index}");
    }
}

#[test]
fn interlacing_animation_corruption_and_dimension_bombs_fail() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let original = png(
        width as u32,
        height as u32,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &vec![0; width * height],
    );
    let mut output = vec![0x11; frame::FRAME_BYTES];
    for (bad_width, interlaced) in [(width as u32, 1), (width as u32 + 1, 0), (100_000, 0)] {
        let mut encoded = original.clone();
        encoded[16..20].copy_from_slice(&bad_width.to_be_bytes());
        encoded[28] = interlaced;
        let crc = frame::crc32(&encoded[12..29]);
        encoded[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(
            decode(&encoded, Default::default(), &mut output, || Ok(())),
            Err(DecodeError::UnsupportedLayout)
        ));
        assert!(output.iter().all(|byte| *byte == 0x11));
    }
    let mut encoded = original;
    let idat = encoded
        .windows(4)
        .position(|bytes| bytes == b"IDAT")
        .unwrap();
    encoded[idat + 6] ^= 1;
    assert!(decode(&encoded, Default::default(), &mut output, || Ok(())).is_err());

    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_animated(1, 0).unwrap();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![0; width * height])
        .unwrap();
    assert!(decode(&encoded, Default::default(), &mut output, || Ok(())).is_err());
}
