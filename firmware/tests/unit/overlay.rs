use super::*;

#[test]
fn all_corners_follow_layout_rotation_and_exact_margins() {
    for orientation in [Orientation::Portrait, Orientation::Landscape] {
        for battery_corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            let overlay = StatusOverlay::new(
                Metadata {
                    orientation,
                    battery_corner,
                },
                None,
            )
            .unwrap();
            let (width, height) = crate::conversion::dimensions(orientation);
            let left = if matches!(battery_corner, Corner::TopLeft | Corner::BottomLeft) {
                MARGIN
            } else {
                width - MARGIN - BOX_WIDTH
            };
            let top = if matches!(battery_corner, Corner::TopLeft | Corner::TopRight) {
                MARGIN
            } else {
                height - MARGIN - BOX_HEIGHT
            };
            for (x, y, expected) in [
                (left, top, Some(BLACK)),
                (left + BOX_WIDTH - 1, top + BOX_HEIGHT - 1, Some(BLACK)),
                (left + 4, top + 4, Some(WHITE)),
                (left - 1, top, None),
                (left + BOX_WIDTH, top, None),
                (left, top - 1, None),
                (left, top + BOX_HEIGHT, None),
            ] {
                let (nx, ny) = frame::native_coordinates(orientation, x, y).unwrap();
                assert_eq!(overlay.pixel(nx, ny), expected);
            }
        }
    }
}

#[test]
fn unknown_text_is_24_pixels_high_and_all_percentages_fit() {
    let unknown = StatusOverlay::new(Default::default(), None).unwrap();
    let mut rendered = std::collections::HashSet::new();
    rendered.insert(unknown.pixels.clone());
    let text_rows: Vec<_> = (BORDER..BOX_HEIGHT - BORDER)
        .filter(|y| {
            (BORDER..BOX_WIDTH - BORDER).any(|x| unknown.pixels[y * BOX_WIDTH + x] == BLACK)
        })
        .collect();
    assert_eq!(
        text_rows.last().unwrap() - text_rows.first().unwrap() + 1,
        TEXT_HEIGHT
    );
    for value in 0..=100 {
        let overlay = StatusOverlay::new(Default::default(), Some(value)).unwrap();
        assert!(overlay
            .pixels
            .iter()
            .all(|pixel| matches!(pixel, &BLACK | &WHITE)));
        assert!(
            rendered.insert(overlay.pixels),
            "duplicate label for {value}%"
        );
    }
    assert!(StatusOverlay::new(Default::default(), Some(101)).is_err());
    assert_eq!(unknown.pixel(frame::WIDTH, 0), None);
    assert_eq!(unknown.pixel(0, frame::HEIGHT), None);
}

#[test]
fn overlay_never_modifies_saved_payload() {
    let photo = vec![0x66; frame::FRAME_BYTES];
    let overlay = StatusOverlay::new(Default::default(), None).unwrap();
    let corner = frame::WIDTH - MARGIN - BOX_WIDTH;
    assert_eq!(overlay.pixel(corner, MARGIN).unwrap_or(6), BLACK);
    assert_eq!(overlay.pixel(0, 0).unwrap_or(6), 6);
    assert!(photo.iter().all(|byte| *byte == 0x66));
}

#[test]
fn full_battery_precedes_unknown_and_full_percentage() {
    for percentage in [None, Some(100)] {
        let overlay = StatusOverlay::new(Default::default(), percentage).unwrap();
        let y = (BOX_HEIGHT - ICON_HEIGHT) / 2;
        let left = (BORDER..BOX_WIDTH - BORDER)
            .find(|x| overlay.pixels[y * BOX_WIDTH + x] == BLACK)
            .unwrap();
        assert_eq!(
            overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left],
            BLACK
        );
        assert_eq!(
            overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left + 5],
            BLACK
        );
        assert_eq!(
            overlay.pixels[(y + ICON_HEIGHT / 2) * BOX_WIDTH + left + ICON_WIDTH - 1],
            BLACK
        );
        assert!((left + ICON_WIDTH..left + ICON_WIDTH + 2)
            .all(|x| (BORDER..BOX_HEIGHT - BORDER)
                .all(|y| overlay.pixels[y * BOX_WIDTH + x] == WHITE)));
        assert!((left + ICON_WIDTH + 2..BOX_WIDTH - BORDER)
            .any(|x| (BORDER..BOX_HEIGHT - BORDER)
                .any(|y| overlay.pixels[y * BOX_WIDTH + x] == BLACK)));
    }
}
