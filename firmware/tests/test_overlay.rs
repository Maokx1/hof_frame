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
    let (black, white) = (Some(0), Some(1));
    let cases = [
        (Orientation::Landscape, Corner::TopLeft, (9, 9, 76, 36)),
        (Orientation::Landscape, Corner::TopRight, (723, 9, 790, 36)),
        (
            Orientation::Landscape,
            Corner::BottomLeft,
            (9, 443, 76, 470),
        ),
        (
            Orientation::Landscape,
            Corner::BottomRight,
            (723, 443, 790, 470),
        ),
        (Orientation::Portrait, Corner::TopLeft, (763, 9, 790, 76)),
        (
            Orientation::Portrait,
            Corner::TopRight,
            (763, 403, 790, 470),
        ),
        (Orientation::Portrait, Corner::BottomLeft, (9, 9, 36, 76)),
        (
            Orientation::Portrait,
            Corner::BottomRight,
            (9, 403, 36, 470),
        ),
    ];
    for (orientation, battery_corner, (left, top, right, bottom)) in cases {
        let overlay = StatusOverlay::new(
            Metadata {
                orientation,
                battery_corner,
                battery_indicator_enabled: true,
            },
            None,
        )
        .unwrap();
        for (x, y, expected) in [
            (left, top, black),
            (right, top, black),
            (left, bottom, black),
            (right, bottom, black),
            (left + 1, top + 1, white),
            (left - 1, top, None),
            (right + 1, top, None),
            (left, top - 1, None),
            (left, bottom + 1, None),
        ] {
            assert_eq!(
                overlay.pixel(x, y),
                expected,
                "{orientation:?} {battery_corner:?} at ({x}, {y})"
            );
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
fn disabled_indicator_has_no_pixels() {
    for metadata in [
        Metadata::default(),
        Metadata {
            orientation: Orientation::Portrait,
            battery_corner: Corner::BottomLeft,
            battery_indicator_enabled: false,
        },
    ] {
        let overlay = StatusOverlay::new(metadata, Some(50)).unwrap();
        assert!(overlay.pixels.is_empty());
        assert_eq!(overlay.pixel(0, 0), None);
        assert_eq!(overlay.pixel(frame::WIDTH - 1, frame::HEIGHT - 1), None);
    }
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
