use super::*;

#[test]
fn n16r8_partition_budget_supports_seven_mb_and_preserves_frames() {
    let mut previous_end = 0x9000;
    for line in include_str!("../partitions.csv").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        let offset = usize::from_str_radix(fields[3].trim_start_matches("0x"), 16).unwrap();
        let size = usize::from_str_radix(fields[4].trim_start_matches("0x"), 16).unwrap();
        assert_eq!(offset, previous_end, "partition {} gap/overlap", fields[0]);
        assert_eq!(offset % 4096, 0);
        assert_eq!(size % 4096, 0);
        match fields[0] {
            "nvs" => assert_eq!(size, 24 * 1024),
            "upload" => {
                assert_eq!(size, 7 * 1024 * 1024);
                assert!(size >= crate::upload::MAX_UPLOAD_BYTES);
            }
            "frames" => {
                assert_eq!(offset, 0xe00000);
                assert_eq!(size, STORAGE_BYTES);
            }
            "factory" => assert!(size > 4 * 1024 * 1024),
            "phy_init" => assert_eq!(size, 4096),
            other => panic!("unexpected partition {other}"),
        }
        previous_end = offset + size;
    }
    assert_eq!(previous_end, 16 * 1024 * 1024);
    assert!(PAYLOAD_OFFSET + FRAME_BYTES <= SLOT_BYTES);
}

#[test]
fn generated_storage_fixture_has_exact_length_and_supported_palette() {
    let fixture: Vec<_> = [0x01, 0x23, 0x56]
        .into_iter()
        .cycle()
        .take(FRAME_BYTES)
        .collect();
    assert_eq!(fixture.len(), FRAME_BYTES);
    assert!(valid_pixels(&fixture));
}

struct Flash {
    bytes: Vec<u8>,
    mutations: usize,
    fail_at: Option<usize>,
    torn: bool,
    read_error: bool,
}

impl Flash {
    fn new() -> Self {
        Self {
            bytes: vec![255; STORAGE_BYTES],
            mutations: 0,
            fail_at: None,
            torn: false,
            read_error: false,
        }
    }
}

impl FrameStorage for Flash {
    type Error = &'static str;

    fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if self.read_error {
            return Err("read");
        }
        bytes.copy_from_slice(&self.bytes[offset..offset + bytes.len()]);
        Ok(())
    }

    fn erase(&mut self, offset: usize, len: usize) -> Result<(), Self::Error> {
        self.mutations += 1;
        if self.fail_at == Some(self.mutations) {
            if self.torn {
                self.bytes[offset..offset + len / 2].fill(255);
            }
            return Err("erase");
        }
        self.bytes[offset..offset + len].fill(255);
        Ok(())
    }

    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error> {
        self.mutations += 1;
        let failed = self.fail_at == Some(self.mutations);
        let len = if failed {
            if self.torn {
                bytes.len() / 2
            } else {
                0
            }
        } else {
            bytes.len()
        };
        for (dst, src) in self.bytes[offset..offset + len].iter_mut().zip(bytes) {
            assert_eq!(*dst & src, *src, "flash must be erased before writing");
            *dst &= src;
        }
        if failed {
            Err("write")
        } else {
            Ok(())
        }
    }
}

#[test]
fn crc_matches_standard_check_vector() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn empty_flash_selects_generated_default() {
    let result = scan(&mut Flash::new()).unwrap();
    assert_eq!(result.current, None);
    assert_eq!(result.slots, [SlotStatus::Empty; 2]);
    assert_eq!(default_pixel(0, 0), 1);
    assert_eq!(default_pixel(WIDTH - 1, HEIGHT - 1), 1);
}

#[test]
fn full_frame_commit_and_reload_preserve_metadata_and_pixels() {
    let mut flash = Flash::new();
    let pixels = vec![0x35; FRAME_BYTES];
    let metadata = Metadata {
        orientation: Orientation::Landscape,
        battery_corner: Corner::BottomLeft,
    };
    let record = commit(&mut flash, &pixels, metadata).unwrap();
    assert_eq!(record.generation, 1);
    assert_eq!(scan(&mut flash).unwrap().current, Some(record));
    assert_eq!(
        &flash.bytes[record.payload_offset()..record.payload_offset() + FRAME_BYTES],
        pixels
    );
    assert_eq!(pixel(&pixels, 0, 0), Some(3));
    assert_eq!(pixel(&pixels, 1, 0), Some(5));
}

#[test]
fn replacement_alternates_slots_and_selects_newest_generation() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    for generation in 1..=3 {
        let record = commit(&mut flash, &pixels, Metadata::default()).unwrap();
        assert_eq!(record.generation, generation);
        assert_eq!(record.slot, ((generation - 1) % 2) as usize);
        assert_eq!(scan(&mut flash).unwrap().current, Some(record));
    }
}

#[test]
fn malformed_input_causes_no_flash_mutation() {
    let mut flash = Flash::new();
    for pixels in [
        vec![0x11; FRAME_BYTES - 1],
        vec![0x44; FRAME_BYTES],
        vec![0x1F; FRAME_BYTES],
    ] {
        assert_eq!(
            commit(&mut flash, &pixels, Metadata::default()),
            Err(FrameError::InvalidInput)
        );
    }
    assert_eq!(flash.mutations, 0);
}

#[test]
fn every_supported_palette_code_is_accepted() {
    for color in [0, 1, 2, 3, 5, 6] {
        assert!(valid_pixels(&[color * 17]));
    }
    for color in [4, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        assert!(!valid_pixels(&[color * 17]));
    }
}

#[test]
fn interrupted_erase_payload_header_and_publication_keep_old_frame() {
    let pixels = vec![0x11; FRAME_BYTES];
    let writes = FRAME_BYTES.div_ceil(CHUNK_BYTES);
    let mut original = Flash::new();
    let old = commit(&mut original, &pixels, Metadata::default()).unwrap();
    for operation in [1, 2, 2 + writes / 2, 1 + writes, 2 + writes, 3 + writes] {
        for torn in [false, true] {
            let mut flash = Flash {
                bytes: original.bytes.clone(),
                mutations: 0,
                fail_at: Some(operation),
                torn,
                read_error: false,
            };
            assert!(commit(&mut flash, &pixels, Metadata::default()).is_err());
            assert_eq!(
                scan(&mut flash).unwrap().current,
                Some(old),
                "operation={operation}, torn={torn}"
            );
        }
    }
}

#[test]
fn corrupt_new_payload_falls_back_to_previous_frame() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let old = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    let new = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    flash.bytes[new.payload_offset()] = 0x12;
    let result = scan(&mut flash).unwrap();
    assert_eq!(result.current, Some(old));
    assert_eq!(
        result.slots[new.slot],
        SlotStatus::Invalid(InvalidFrame::Checksum)
    );
}

#[test]
fn candidate_readback_failure_prevents_publication() {
    struct CorruptingFlash(Flash);
    impl FrameStorage for CorruptingFlash {
        type Error = &'static str;
        fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), Self::Error> {
            self.0.read(offset, bytes)
        }
        fn erase(&mut self, offset: usize, len: usize) -> Result<(), Self::Error> {
            self.0.erase(offset, len)
        }
        fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error> {
            self.0.write(offset, bytes)?;
            if offset == SLOT_BYTES + PAYLOAD_OFFSET {
                self.0.bytes[offset] = 0;
            }
            Ok(())
        }
    }
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let old = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    let mut storage = CorruptingFlash(flash);
    assert_eq!(
        commit(&mut storage, &pixels, Metadata::default()),
        Err(FrameError::Verification)
    );
    assert_eq!(scan(&mut storage).unwrap().current, Some(old));
    assert_eq!(
        &storage.0.bytes[SLOT_BYTES..SLOT_BYTES + HEADER_BYTES],
        &[255; HEADER_BYTES]
    );
}

#[test]
fn invalid_palette_is_rejected_even_with_matching_payload_checksum() {
    let mut flash = Flash::new();
    let record = FrameRecord {
        slot: 0,
        generation: 1,
        metadata: Metadata::default(),
        payload_crc: crc32(&vec![0x44; FRAME_BYTES]),
    };
    flash.bytes[..HEADER_BYTES].copy_from_slice(&header(record));
    flash.bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + FRAME_BYTES].fill(0x44);
    assert_eq!(
        scan(&mut flash).unwrap().slots[0],
        SlotStatus::Invalid(InvalidFrame::Palette)
    );
}

#[test]
fn every_header_byte_is_integrity_checked() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let record = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    for offset in 0..HEADER_BYTES {
        flash.bytes[offset] ^= 1;
        assert_eq!(
            scan(&mut flash).unwrap().current,
            None,
            "header offset={offset}"
        );
        flash.bytes[offset] ^= 1;
    }
    assert_eq!(scan(&mut flash).unwrap().current, Some(record));
}

#[test]
fn unsupported_metadata_is_rejected_even_with_valid_header_crc() {
    let record = FrameRecord {
        slot: 0,
        generation: 1,
        metadata: Metadata::default(),
        payload_crc: 0,
    };
    for (offset, value) in [(4, 2), (5, 9), (6, 9), (7, 8), (8, 0), (10, 0), (13, 0)] {
        let mut bytes = header(record);
        bytes[offset] = value;
        let checksum = crc32(&bytes[..28]);
        bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(parse_header(&bytes, 0), None);
    }
}

#[test]
fn storage_read_error_is_not_a_missing_frame_fallback() {
    let mut flash = Flash::new();
    flash.read_error = true;
    assert!(matches!(scan(&mut flash), Err(FrameError::Storage("read"))));
    assert_eq!(
        commit(&mut flash, &vec![0x11; FRAME_BYTES], Metadata::default()),
        Err(FrameError::Storage("read"))
    );
    assert_eq!(flash.mutations, 0);
}

#[test]
fn zero_and_exhausted_generations_are_not_wrapped() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let mut record = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    record.generation = 0;
    assert_eq!(parse_header(&header(record), 0), None);
    record.generation = u64::MAX;
    flash.bytes[..HEADER_BYTES].copy_from_slice(&header(record));
    let before = flash.mutations;
    assert_eq!(
        commit(&mut flash, &pixels, Metadata::default()),
        Err(FrameError::GenerationExhausted)
    );
    assert_eq!(flash.mutations, before);
}

#[test]
fn landscape_rotates_clockwise_into_native_portrait_geometry() {
    assert_eq!(
        native_coordinates(Orientation::Landscape, 0, 0),
        Some((1199, 0))
    );
    assert_eq!(
        native_coordinates(Orientation::Landscape, 1599, 0),
        Some((1199, 1599))
    );
    assert_eq!(
        native_coordinates(Orientation::Landscape, 0, 1199),
        Some((0, 0))
    );
    assert_eq!(
        native_coordinates(Orientation::Landscape, 1599, 1199),
        Some((0, 1599))
    );
    assert_eq!(native_coordinates(Orientation::Landscape, 1600, 0), None);
    assert_eq!(
        native_coordinates(Orientation::Portrait, 1199, 1599),
        Some((1199, 1599))
    );
    assert_eq!(pixel(&[0x11], 0, 0), None);
    assert_eq!(pixel(&vec![0x11; FRAME_BYTES], WIDTH, 0), None);
}
