use std::io::{BufRead, Cursor, Seek};

use crate::{conversion, frame};

pub fn decode(
    encoded: &[u8],
    metadata: frame::Metadata,
    packed: &mut [u8],
    checkpoint: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), Box<dyn std::error::Error>> {
    decode_reader(
        Cursor::new(encoded),
        encoded.len(),
        metadata,
        packed,
        checkpoint,
    )
}

pub fn decode_reader(
    encoded: impl BufRead + Seek,
    encoded_len: usize,
    metadata: frame::Metadata,
    packed: &mut [u8],
    mut checkpoint: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if encoded_len > crate::upload::MAX_UPLOAD_BYTES || packed.len() != frame::FRAME_BYTES {
        return Err("encoded image or packed buffer exceeds the image budget".into());
    }
    checkpoint()?;
    let mut decoder = png::Decoder::new(encoded);
    decoder.set_limits(png::Limits { bytes: 128 * 1024 });
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let expected = conversion::dimensions(metadata.orientation);
    let info = reader.info();
    if (info.width as usize, info.height as usize) != expected
        || info.interlaced
        || info.animation_control.is_some()
    {
        return Err("requires a non-interlaced, static PNG at the selected display resolution; use the upload page to resize PNG/JPEG".into());
    }
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err("unsupported PNG output bit depth".into());
    }
    for y in 0..expected.1 {
        if y % 16 == 0 {
            checkpoint()?;
        }
        let row = reader.next_row()?.ok_or("truncated PNG rows")?;
        conversion::row(packed, metadata.orientation, y, row.data(), color.samples())?;
    }
    reader.finish()?;
    checkpoint()?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        let encoded = png(
            1200,
            1600,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Sixteen,
            &vec![0; 1200 * 1600 * 4],
        );
        let mut output = vec![0; frame::FRAME_BYTES];
        decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
        assert!(output.iter().all(|byte| *byte == 0x11));
    }

    #[test]
    fn decodes_photo_sized_rgb_with_inflater_workspace() {
        let mut pixels = vec![0; 1200 * 1600 * 3];
        let mut random = 0x1234_5678_u32;
        for (index, pixel) in pixels.chunks_exact_mut(3).enumerate() {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            pixel.copy_from_slice(&[random as u8, (index % 1200) as u8, (index / 1200) as u8]);
        }
        let encoded = png(
            1200,
            1600,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &pixels,
        );
        assert!(encoded.len() > 1024 * 1024);
        assert!(encoded.len() <= crate::upload::MAX_UPLOAD_BYTES);
        let mut output = vec![0; frame::FRAME_BYTES];
        decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
        assert!(frame::valid_pixels(&output));
    }

    #[test]
    fn streams_exact_seven_mb_png_and_rejects_one_more_before_conversion() {
        let mut pixels = vec![0; 1200 * 1600 * 3];
        let mut random = 0x1234_5678_u32;
        for byte in &mut pixels {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            *byte = random as u8;
        }
        let mut encoded = png(
            1200,
            1600,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &pixels,
        );
        assert!(encoded.len() > 4 * 1024 * 1024);
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
        let encoded = png(
            1200,
            1600,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![255; 1200 * 1600],
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
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 1200, 1600);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_palette(vec![255, 0, 0]);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&vec![0; 1200 * 1600])
            .unwrap();
        let mut output = vec![0; frame::FRAME_BYTES];
        decode(&encoded, Default::default(), &mut output, || Ok(())).unwrap();
        assert!(output.iter().all(|byte| *byte == 0x33));
    }

    #[test]
    fn interlacing_animation_corruption_and_dimension_bombs_fail() {
        let original = png(
            1200,
            1600,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![0; 1200 * 1600],
        );
        let mut output = vec![0x11; frame::FRAME_BYTES];
        for (width, interlaced) in [(1200_u32, 1), (100_000, 0)] {
            let mut encoded = original.clone();
            encoded[16..20].copy_from_slice(&width.to_be_bytes());
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
        let mut encoder = png::Encoder::new(&mut encoded, 1200, 1600);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_animated(1, 0).unwrap();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&vec![0; 1200 * 1600])
            .unwrap();
        assert!(decode(&encoded, Default::default(), &mut output, || Ok(())).is_err());
    }
}
