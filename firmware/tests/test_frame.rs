use firmware::frame::*;

#[test]
fn n16r8_partition_budget_supports_seven_mb_and_preserves_frames() {
    let mut previous_end = 0x9000;
    let mut found_upload = false;
    let mut found_frames = false;
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
                found_upload = true;
                assert_eq!((fields[1], fields[2]), ("data", "undefined"));
                assert_eq!(size, 7 * 1024 * 1024);
                assert!(size >= firmware::upload::MAX_UPLOAD_BYTES);
            }
            "frames" => {
                found_frames = true;
                assert_eq!((fields[1], fields[2]), ("data", "undefined"));
                assert_eq!(offset, 0xe00000);
                assert_eq!(size, STORAGE_BYTES);
            }
            "factory" => assert!(size > 4 * 1024 * 1024),
            "phy_init" => assert_eq!(size, 4096),
            other => panic!("unexpected partition {other}"),
        }
        previous_end = offset + size;
    }
    assert!(found_upload && found_frames);
    assert_eq!(previous_end, 16 * 1024 * 1024);
    const { assert!(PAYLOAD_OFFSET + FRAME_BYTES <= SLOT_BYTES) };
}

struct Flash {
    bytes: Vec<u8>,
    mutations: usize,
    fail_at: Option<usize>,
    torn: bool,
    read_error: bool,
    read_error_after_mutations: Option<usize>,
    corrupt_write_at: Option<usize>,
}

impl Flash {
    fn new() -> Self {
        Self {
            bytes: vec![255; STORAGE_BYTES],
            mutations: 0,
            fail_at: None,
            torn: false,
            read_error: false,
            read_error_after_mutations: None,
            corrupt_write_at: None,
        }
    }
}

impl FrameStorage for Flash {
    type Error = &'static str;

    fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if self.read_error
            || self
                .read_error_after_mutations
                .is_some_and(|count| self.mutations >= count)
        {
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
            if self.corrupt_write_at == Some(offset) && !bytes.is_empty() {
                self.bytes[offset] = 0;
            }
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
    let mut pixels = vec![0x11; 192_000];
    pixels[0] = 0x35;
    pixels[399] = 0x26;
    pixels[400] = 0x50;
    pixels[191_999] = 0x16;
    let metadata = Metadata {
        orientation: Orientation::Portrait,
        battery_corner: Corner::BottomLeft,
        battery_indicator_enabled: true,
    };
    let record = commit(&mut flash, &pixels, metadata).unwrap();
    assert_eq!(record.generation, 1);
    assert_eq!(record.slot, 0);
    assert_eq!(record.metadata, metadata);
    assert_eq!(scan(&mut flash).unwrap().current, Some(record));
    let stored = &flash.bytes[4096..4096 + 192_000];
    assert_eq!(stored, pixels);
    for (x, y, color) in [
        (0, 0, 3),
        (1, 0, 5),
        (798, 0, 2),
        (799, 0, 6),
        (0, 1, 5),
        (1, 1, 0),
        (798, 479, 1),
        (799, 479, 6),
    ] {
        assert_eq!(pixel(stored, x, y), Some(color), "pixel ({x}, {y})");
    }
}

#[test]
fn replacement_alternates_slots_and_selects_newest_generation() {
    let mut flash = Flash::new();
    for (index, (color, metadata)) in [
        (
            0x11,
            Metadata {
                orientation: Orientation::Portrait,
                battery_corner: Corner::TopLeft,
                battery_indicator_enabled: true,
            },
        ),
        (
            0x33,
            Metadata {
                orientation: Orientation::Landscape,
                battery_corner: Corner::BottomRight,
                battery_indicator_enabled: false,
            },
        ),
        (
            0x66,
            Metadata {
                orientation: Orientation::Portrait,
                battery_corner: Corner::BottomLeft,
                battery_indicator_enabled: false,
            },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let pixels = vec![color; FRAME_BYTES];
        let record = commit(&mut flash, &pixels, metadata).unwrap();
        assert_eq!(record.generation, index as u64 + 1);
        assert_eq!(record.slot, index % 2);
        assert_eq!(record.metadata, metadata);
        let current = scan(&mut flash).unwrap().current.unwrap();
        assert_eq!(current, record);
        assert_eq!(current.metadata, metadata);
        assert_eq!(
            &flash.bytes[current.payload_offset()..current.payload_offset() + FRAME_BYTES],
            pixels
        );
    }
}

#[test]
fn malformed_input_causes_no_flash_mutation() {
    let mut flash = Flash::new();
    for pixels in [
        vec![0x11; FRAME_BYTES - 1],
        vec![0x44; FRAME_BYTES],
        vec![0x41; FRAME_BYTES],
        vec![0x14; FRAME_BYTES],
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
        assert!(!valid_pixels(&[(color << 4) | 1]));
        assert!(!valid_pixels(&[0x10 | color]));
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
                read_error_after_mutations: None,
                corrupt_write_at: None,
            };
            let expected = if operation == 1 { "erase" } else { "write" };
            assert_eq!(
                commit(&mut flash, &pixels, Metadata::default()),
                Err(FrameError::Storage(expected)),
                "operation={operation}, torn={torn}"
            );
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
fn corrupt_publication_tag_fails_verification_and_preserves_previous_frame() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let old = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    let candidate_slot = 1 - old.slot;
    flash.corrupt_write_at = Some(candidate_slot * SLOT_BYTES + 32);

    assert_eq!(
        commit(&mut flash, &pixels, Metadata::default()),
        Err(FrameError::Verification)
    );
    let result = scan(&mut flash).unwrap();
    assert_eq!(result.current, Some(old));
    assert_eq!(
        result.slots[candidate_slot],
        SlotStatus::Invalid(InvalidFrame::Header)
    );
}

#[test]
fn candidate_read_error_prevents_publication() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let old = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    flash.read_error_after_mutations = Some(flash.mutations + 1);

    assert_eq!(
        commit(&mut flash, &pixels, Metadata::default()),
        Err(FrameError::Storage("read"))
    );
    flash.read_error_after_mutations = None;
    let result = scan(&mut flash).unwrap();
    assert_eq!(result.current, Some(old));
    assert_eq!(result.slots[1 - old.slot], SlotStatus::Empty);
}

#[test]
fn published_header_read_error_reports_failure_even_if_new_frame_is_valid() {
    let mut flash = Flash::new();
    let pixels = vec![0x11; FRAME_BYTES];
    let old = commit(&mut flash, &pixels, Metadata::default()).unwrap();
    let writes = FRAME_BYTES.div_ceil(CHUNK_BYTES);
    flash.read_error_after_mutations = Some(flash.mutations + 1 + writes + 2);

    assert_eq!(
        commit(&mut flash, &pixels, Metadata::default()),
        Err(FrameError::Storage("read"))
    );
    flash.read_error_after_mutations = None;
    let result = scan(&mut flash).unwrap();
    assert_eq!(result.slots[old.slot], SlotStatus::Valid(old));
    assert!(matches!(
        result.current,
        Some(record) if record.slot == 1 - old.slot && record.generation == old.generation + 1
    ));
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
    for (offset, value) in [
        (4, 2),
        (5, 9),
        (6, 4),
        (6, 9),
        (7, 8),
        (8, 0),
        (10, 0),
        (13, 0),
    ] {
        let mut bytes = header(record);
        bytes[offset] = value;
        let checksum = crc32(&bytes[..28]);
        bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(parse_header(&bytes, 0), None);
    }
}

#[test]
fn frame_header_preserves_all_layouts_and_legacy_indicator_encoding() {
    let legacy_header = [
        b'H', b'O', b'F', b'1', // Magic
        1, 0, 2, 4, // Version, portrait, bottom-left, bits per pixel
        0x20, 0x03, 0xe0, 0x01, // 800 x 480
        0x00, 0xee, 0x02, 0x00, // 192,000-byte payload
        1, 0, 0, 0, 0, 0, 0, 0, // Generation
        123, 0, 0, 0, // Payload checksum
        0x28, 0x39, 0xfd, 0x09, // Header checksum
        b'D', b'O', b'N', b'E', // Published
    ];
    let legacy_record = FrameRecord {
        slot: 1,
        generation: 1,
        metadata: Metadata {
            orientation: Orientation::Portrait,
            battery_corner: Corner::BottomLeft,
            battery_indicator_enabled: true,
        },
        payload_crc: 123,
    };
    assert_eq!(parse_header(&legacy_header, 1), Some(legacy_record));
    assert_eq!(header(legacy_record), legacy_header);

    for (orientation, orientation_code) in [(Orientation::Portrait, 0), (Orientation::Landscape, 1)]
    {
        for (corner, corner_code) in [
            (Corner::TopRight, 0),
            (Corner::TopLeft, 1),
            (Corner::BottomLeft, 2),
            (Corner::BottomRight, 3),
        ] {
            for enabled in [false, true] {
                let record = FrameRecord {
                    slot: 1,
                    generation: 1,
                    metadata: Metadata {
                        orientation,
                        battery_corner: corner,
                        battery_indicator_enabled: enabled,
                    },
                    payload_crc: 123,
                };
                let encoded = header(record);
                assert_eq!(encoded[5], orientation_code);
                assert_eq!(encoded[6], corner_code | if enabled { 0 } else { 0x80 });
                assert_eq!(parse_header(&encoded, 1), Some(record));
            }
        }
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
fn landscape_is_native_and_portrait_rotates_clockwise_into_it() {
    assert_eq!(
        native_coordinates(Orientation::Landscape, 0, 0),
        Some((0, 0))
    );
    assert_eq!(
        native_coordinates(Orientation::Landscape, 799, 479),
        Some((799, 479))
    );
    assert_eq!(
        native_coordinates(Orientation::Portrait, 0, 0),
        Some((799, 0))
    );
    assert_eq!(
        native_coordinates(Orientation::Portrait, 479, 799),
        Some((0, 479))
    );
    assert_eq!(native_coordinates(Orientation::Landscape, 800, 0), None);
    assert_eq!(native_coordinates(Orientation::Portrait, 480, 0), None);
    assert_eq!(pixel(&[0x11], 0, 0), None);
    assert_eq!(pixel(&vec![0x11; FRAME_BYTES], WIDTH, 0), None);
}
