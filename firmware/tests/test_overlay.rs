use firmware::frame::{self, Corner, Metadata, Orientation};
use firmware::overlay::*;

fn enabled_metadata() -> Metadata {
    Metadata {
        battery_indicator_enabled: true,
        ..Metadata::default()
    }
}

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
                    battery_indicator_enabled: true,
                },
                None,
            )
            .unwrap();
            let (width, height) = firmware::conversion::dimensions(orientation);
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
fn unknown_text_fits_reduced_height_and_all_percentages_fit() {
    let unknown = StatusOverlay::new(enabled_metadata(), None).unwrap();
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
        let overlay = StatusOverlay::new(enabled_metadata(), Some(value)).unwrap();
        assert!(overlay
            .pixels
            .iter()
            .all(|pixel| matches!(pixel, &BLACK | &WHITE)));
        assert!(
            rendered.insert(overlay.pixels),
            "duplicate label for {value}%"
        );
    }
    assert!(matches!(
        StatusOverlay::new(enabled_metadata(), Some(101)),
        Err(OverlayError::InvalidPercentage(101))
    ));
    assert_eq!(unknown.pixel(frame::WIDTH, 0), None);
    assert_eq!(unknown.pixel(0, frame::HEIGHT), None);
}

#[test]
fn default_metadata_disables_battery_indicator() {
    assert!(!Metadata::default().battery_indicator_enabled);
    let overlay = StatusOverlay::new(Metadata::default(), None).unwrap();

    assert!(overlay.pixels.is_empty());
}

#[test]
fn disabled_indicator_has_no_pixels() {
    let metadata = Metadata {
        battery_indicator_enabled: false,
        ..Metadata::default()
    };
    let overlay = StatusOverlay::new(metadata, Some(50)).unwrap();

    assert!(overlay.pixels.is_empty());
    assert_eq!(overlay.pixel(0, 0), None);
    assert_eq!(overlay.pixel(frame::WIDTH - 1, frame::HEIGHT - 1), None);
}

#[test]
fn indicator_size_is_scaled_for_the_7_3_inch_panel() {
    assert!(BOX_WIDTH * 5 <= 120 * 3);
    assert!(BOX_HEIGHT * 5 <= 48 * 3);
}

#[test]
fn overlay_never_modifies_saved_payload() {
    let photo = vec![0x66; frame::FRAME_BYTES];
    let overlay = StatusOverlay::new(enabled_metadata(), None).unwrap();
    let corner = frame::WIDTH - MARGIN - BOX_WIDTH;
    assert_eq!(overlay.pixel(corner, MARGIN).unwrap_or(6), BLACK);
    assert_eq!(overlay.pixel(0, 0).unwrap_or(6), 6);
    assert!(photo.iter().all(|byte| *byte == 0x66));
}

#[test]
fn full_battery_precedes_unknown_and_full_percentage() {
    for percentage in [None, Some(100)] {
        let overlay = StatusOverlay::new(enabled_metadata(), percentage).unwrap();
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
