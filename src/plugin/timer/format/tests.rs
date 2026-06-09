use super::format_time;

#[test]
fn zero() {
    assert_eq!(format_time(0.0), "0:00.000");
}

#[test]
fn negative_clamps_to_zero() {
    assert_eq!(format_time(-5.0), "0:00.000");
}

#[test]
fn five_and_a_half_seconds() {
    assert_eq!(format_time(5.5), "0:05.500");
}

#[test]
fn sixty_five_seconds() {
    assert_eq!(format_time(65.0), "1:05.000");
}

#[test]
fn one_hour_boundary() {
    assert_eq!(format_time(3600.0), "1:00:00.000");
}

#[test]
fn full_hour_minutes_seconds_millis() {
    assert_eq!(format_time(3661.234), "1:01:01.234");
}

#[test]
fn millis_rounding_up() {
    // 0.9995 rounds to 1000 ms = 1.000 s
    assert_eq!(format_time(0.9995), "0:01.000");
}

#[test]
fn millis_rounding_down() {
    assert_eq!(format_time(0.9994), "0:00.999");
}

#[test]
fn sub_second_millis_padding() {
    assert_eq!(format_time(0.001), "0:00.001");
}
