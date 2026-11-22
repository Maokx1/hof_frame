use super::*;

#[test]
fn qualification_signature_only_upload_is_rejected_before_pixel_conversion() {
    let encoded = b"\x89PNG\r\n\x1a\n";
    assert!(crate::upload::image_signature(encoded));
    let mut packed = vec![0x11; frame::FRAME_BYTES];
    assert!(decode(encoded, frame::Metadata::default(), &mut packed, || Ok(())).is_err());
    assert!(packed.iter().all(|byte| *byte == 0x11));
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
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::GrayscaleAlpha,
        png::BitDepth::Sixteen,
        &vec![0; width * height * 4],
    );
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
    assert!(output.iter().all(|byte| *byte == 0x11));
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
    let encoded = png(
        width as u32,
        height as u32,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &pixels,
    );
    assert!(encoded.len() > width * height * 2);
    assert!(encoded.len() <= crate::upload::MAX_UPLOAD_BYTES);
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
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
    let padding = crate::upload::MAX_UPLOAD_BYTES - encoded.len() - end.len() - 12;
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
    assert!(decode_reader(
        std::io::BufReader::with_capacity(4096, Cursor::new(&encoded)),
        encoded.len(),
        Default::default(),
        &mut output,
        || panic!("oversize must fail before decoding"),
    )
    .is_err());
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
    assert!(decode(&encoded, Default::default(), &mut output, || Err(
        "deadline"
    ))
    .is_err());
    assert!(decode(
        &encoded[..encoded.len() / 2],
        Default::default(),
        &mut output,
        || Ok(())
    )
    .is_err());
    assert!(decode(b"\xff\xd8\xff", Default::default(), &mut output, || Ok(())).is_err());
    let small = png(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[0, 0, 0]);
    assert!(decode(&small, Default::default(), &mut output, || Ok(())).is_err());
}

#[test]
fn indexed_png_expands_palette() {
    let (width, height) = conversion::dimensions(frame::Metadata::default().orientation);
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_palette(vec![255, 0, 0]);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![0; width * height])
        .unwrap();
    let mut output = vec![0; frame::FRAME_BYTES];
    decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
    assert!(output.iter().all(|byte| *byte == 0x33));
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
    for (bad_width, interlaced) in [(width as u32 + 1, 1), (100_000, 0)] {
        let mut encoded = original.clone();
        encoded[16..20].copy_from_slice(&bad_width.to_be_bytes());
        encoded[28] = interlaced;
        let crc = frame::crc32(&encoded[12..29]);
        encoded[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(decode(&encoded, Default::default(), &mut output, || Ok(())).is_err());
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
