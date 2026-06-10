use super::format_time;

#[test]
fn zero() {
    assert_eq!(format_time(0.0), "0:00.00");
}

#[test]
fn negative_clamps_to_zero() {
    assert_eq!(format_time(-5.0), "0:00.00");
}

#[test]
fn five_and_a_half_seconds() {
    assert_eq!(format_time(5.5), "0:05.50");
}

#[test]
fn sixty_five_seconds() {
    assert_eq!(format_time(65.0), "1:05.00");
}

#[test]
fn one_hour_boundary() {
    assert_eq!(format_time(3600.0), "1:00:00.00");
}

#[test]
fn full_hour_minutes_seconds_centis() {
    assert_eq!(format_time(3661.234), "1:01:01.23");
}

#[test]
fn centis_rounding_up() {
    // 0.9995 rounds to 100 cs = 1.00 s
    assert_eq!(format_time(0.9995), "0:01.00");
}

#[test]
fn centis_rounding_down() {
    assert_eq!(format_time(0.994), "0:00.99");
}

#[test]
fn sub_second_centis_padding() {
    assert_eq!(format_time(0.01), "0:00.01");
}
