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
#[path = "../tests/unit/png_decode.rs"]
mod tests;
