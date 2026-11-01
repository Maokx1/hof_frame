pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT / 2;
pub const SLOT_BYTES: usize = 1024 * 1024;
pub const STORAGE_BYTES: usize = 2 * SLOT_BYTES;
pub const PAYLOAD_OFFSET: usize = 4096;
const HEADER_BYTES: usize = 36;
const CHUNK_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Orientation {
    Portrait = 0,
    Landscape = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Corner {
    TopRight = 0,
    TopLeft = 1,
    BottomLeft = 2,
    BottomRight = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub orientation: Orientation,
    pub battery_corner: Corner,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            orientation: Orientation::Landscape,
            battery_corner: Corner::TopRight,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRecord {
    pub slot: usize,
    pub generation: u64,
    pub metadata: Metadata,
    pub payload_crc: u32,
}

impl FrameRecord {
    pub fn payload_offset(self) -> usize {
        self.slot * SLOT_BYTES + PAYLOAD_OFFSET
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidFrame {
    Header,
    Checksum,
    Palette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    Empty,
    Invalid(InvalidFrame),
    Valid(FrameRecord),
}

#[derive(Clone, Copy, Debug)]
pub struct Scan {
    pub slots: [SlotStatus; 2],
    pub current: Option<FrameRecord>,
}

pub trait FrameStorage {
    type Error;
    fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, offset: usize, len: usize) -> Result<(), Self::Error>;
    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError<E> {
    Storage(E),
    InvalidInput,
    GenerationExhausted,
    Verification,
}

pub fn crc32(bytes: &[u8]) -> u32 {
    !crc_update(u32::MAX, bytes)
}

fn crc_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    crc
}

fn valid_color(color: u8) -> bool {
    matches!(color, 0 | 1 | 2 | 3 | 5 | 6)
}

pub fn valid_pixels(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| valid_color(byte >> 4) && valid_color(byte & 15))
}

fn header(record: FrameRecord) -> [u8; HEADER_BYTES] {
    let mut bytes = [0; HEADER_BYTES];
    bytes[..4].copy_from_slice(b"HOF1");
    bytes[4] = 1;
    bytes[5] = record.metadata.orientation as u8;
    bytes[6] = record.metadata.battery_corner as u8;
    bytes[7] = 4;
    bytes[8..10].copy_from_slice(&(WIDTH as u16).to_le_bytes());
    bytes[10..12].copy_from_slice(&(HEIGHT as u16).to_le_bytes());
    bytes[12..16].copy_from_slice(&(FRAME_BYTES as u32).to_le_bytes());
    bytes[16..24].copy_from_slice(&record.generation.to_le_bytes());
    bytes[24..28].copy_from_slice(&record.payload_crc.to_le_bytes());
    let checksum = crc32(&bytes[..28]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
    bytes[32..].copy_from_slice(b"DONE");
    bytes
}

fn parse_header(bytes: &[u8; HEADER_BYTES], slot: usize) -> Option<FrameRecord> {
    if &bytes[..4] != b"HOF1"
        || bytes[4] != 1
        || bytes[7] != 4
        || u16::from_le_bytes(bytes[8..10].try_into().ok()?) as usize != WIDTH
        || u16::from_le_bytes(bytes[10..12].try_into().ok()?) as usize != HEIGHT
        || u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize != FRAME_BYTES
        || u32::from_le_bytes(bytes[28..32].try_into().ok()?) != crc32(&bytes[..28])
        || &bytes[32..] != b"DONE"
    {
        return None;
    }
    let orientation = match bytes[5] {
        0 => Orientation::Portrait,
        1 => Orientation::Landscape,
        _ => return None,
    };
    let battery_corner = match bytes[6] {
        0 => Corner::TopRight,
        1 => Corner::TopLeft,
        2 => Corner::BottomLeft,
        3 => Corner::BottomRight,
        _ => return None,
    };
    let generation = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    if generation == 0 {
        return None;
    }
    Some(FrameRecord {
        slot,
        generation,
        metadata: Metadata {
            orientation,
            battery_corner,
        },
        payload_crc: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
    })
}

fn validate_payload<S: FrameStorage>(
    storage: &mut S,
    record: FrameRecord,
) -> Result<Option<InvalidFrame>, FrameError<S::Error>> {
    let mut buffer = [0; CHUNK_BYTES];
    let mut crc = u32::MAX;
    let mut palette_valid = true;
    for offset in (0..FRAME_BYTES).step_by(CHUNK_BYTES) {
        let len = CHUNK_BYTES.min(FRAME_BYTES - offset);
        storage
            .read(record.payload_offset() + offset, &mut buffer[..len])
            .map_err(FrameError::Storage)?;
        crc = crc_update(crc, &buffer[..len]);
        palette_valid &= valid_pixels(&buffer[..len]);
    }
    Ok(if !palette_valid {
        Some(InvalidFrame::Palette)
    } else if !crc != record.payload_crc {
        Some(InvalidFrame::Checksum)
    } else {
        None
    })
}

pub fn scan<S: FrameStorage>(storage: &mut S) -> Result<Scan, FrameError<S::Error>> {
    let mut slots = [SlotStatus::Empty; 2];
    let mut current: Option<FrameRecord> = None;
    for (slot, status) in slots.iter_mut().enumerate() {
        let mut bytes = [0; HEADER_BYTES];
        storage
            .read(slot * SLOT_BYTES, &mut bytes)
            .map_err(FrameError::Storage)?;
        if bytes.iter().all(|byte| *byte == 255) {
            continue;
        }
        let Some(record) = parse_header(&bytes, slot) else {
            *status = SlotStatus::Invalid(InvalidFrame::Header);
            continue;
        };
        if let Some(reason) = validate_payload(storage, record)? {
            *status = SlotStatus::Invalid(reason);
            continue;
        }
        *status = SlotStatus::Valid(record);
        if current.is_none_or(|old| record.generation > old.generation) {
            current = Some(record);
        }
    }
    Ok(Scan { slots, current })
}

pub fn commit<S: FrameStorage>(
    storage: &mut S,
    pixels: &[u8],
    metadata: Metadata,
) -> Result<FrameRecord, FrameError<S::Error>> {
    if pixels.len() != FRAME_BYTES || !valid_pixels(pixels) {
        return Err(FrameError::InvalidInput);
    }
    let old = scan(storage)?.current;
    let generation = old
        .map_or(Some(1), |record| record.generation.checked_add(1))
        .ok_or(FrameError::GenerationExhausted)?;
    let record = FrameRecord {
        slot: old.map_or(0, |record| 1 - record.slot),
        generation,
        metadata,
        payload_crc: crc32(pixels),
    };
    let base = record.slot * SLOT_BYTES;
    storage
        .erase(base, SLOT_BYTES)
        .map_err(FrameError::Storage)?;
    for (index, chunk) in pixels.chunks(CHUNK_BYTES).enumerate() {
        storage
            .write(record.payload_offset() + index * CHUNK_BYTES, chunk)
            .map_err(FrameError::Storage)?;
    }
    if validate_payload(storage, record)?.is_some() {
        return Err(FrameError::Verification);
    }
    let bytes = header(record);
    storage
        .write(base, &bytes[..32])
        .map_err(FrameError::Storage)?;
    // Publication is last. A torn header/tag never replaces the other slot.
    storage
        .write(base + 32, &bytes[32..])
        .map_err(FrameError::Storage)?;
    let mut published = [0; HEADER_BYTES];
    storage
        .read(base, &mut published)
        .map_err(FrameError::Storage)?;
    if published != bytes {
        return Err(FrameError::Verification);
    }
    Ok(record)
}

pub fn pixel(packed: &[u8], x: usize, y: usize) -> Option<u8> {
    if packed.len() != FRAME_BYTES || x >= WIDTH || y >= HEIGHT {
        return None;
    }
    let index = y * WIDTH + x;
    let byte = packed[index / 2];
    Some(if index % 2 == 0 { byte >> 4 } else { byte & 15 })
}

pub fn native_coordinates(orientation: Orientation, x: usize, y: usize) -> Option<(usize, usize)> {
    match orientation {
        Orientation::Landscape if x < WIDTH && y < HEIGHT => Some((x, y)),
        // Rotate a portrait layout clockwise into the panel's native landscape orientation.
        Orientation::Portrait if x < HEIGHT && y < WIDTH => Some((WIDTH - 1 - y, x)),
        _ => None,
    }
}

pub fn default_pixel(_x: usize, _y: usize) -> u8 {
    1
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
