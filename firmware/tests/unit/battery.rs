use super::estimated_percentage;

#[test]
fn estimates_percentage_from_documented_nimh_pack_range() {
    assert_eq!(estimated_percentage(4_400), Some(0));
    assert_eq!(estimated_percentage(5_100), Some(50));
    assert_eq!(estimated_percentage(5_800), Some(100));
    assert_eq!(estimated_percentage(6_000), Some(100));
}

#[test]
fn rejects_voltage_outside_sensor_range() {
    assert_eq!(estimated_percentage(0), None);
    assert_eq!(estimated_percentage(6_501), None);
}
