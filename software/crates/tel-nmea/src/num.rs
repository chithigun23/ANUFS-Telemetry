//! Checked decimal parsing helpers. Every function returns `None` on malformed
//! input and never panics or overflows.

/// Longest accepted numeric field. Keeps every intermediate inside `i64`.
const MAX_FIELD_LEN: usize = 18;

/// Value of an ASCII decimal digit.
pub(crate) fn digit(b: u8) -> Option<u8> {
    if b.is_ascii_digit() {
        b.checked_sub(b'0')
    } else {
        None
    }
}

/// Value of an ASCII hexadecimal digit (upper or lower case).
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => b.checked_sub(b'0'),
        b'A'..=b'F' => b.checked_sub(b'A').and_then(|v| v.checked_add(10)),
        b'a'..=b'f' => b.checked_sub(b'a').and_then(|v| v.checked_add(10)),
        _ => None,
    }
}

/// Parse an unsigned integer made only of digits.
pub(crate) fn parse_u32(field: &[u8]) -> Option<u32> {
    if field.is_empty() || field.len() > 9 {
        return None;
    }
    let mut acc: u32 = 0;
    for &b in field {
        acc = acc.checked_mul(10)?.checked_add(u32::from(digit(b)?))?;
    }
    Some(acc)
}

/// Parse a decimal number such as `-12.345` into a value scaled by `10^scale`.
///
/// Extra fractional digits beyond `scale` are truncated toward zero. At least
/// one digit is required. A leading `-` is accepted; `+` is not.
pub(crate) fn parse_fixed(field: &[u8], scale: u32) -> Option<i64> {
    if field.is_empty() || field.len() > MAX_FIELD_LEN {
        return None;
    }
    let (neg, rest) = match field.split_first() {
        Some((&b'-', rest)) => (true, rest),
        _ => (false, field),
    };
    let mut int_part: i64 = 0;
    let mut frac_part: i64 = 0;
    let mut frac_digits: u32 = 0;
    let mut seen_digit = false;
    let mut in_frac = false;
    for &b in rest {
        if b == b'.' {
            if in_frac {
                return None;
            }
            in_frac = true;
            continue;
        }
        let d = i64::from(digit(b)?);
        seen_digit = true;
        if in_frac {
            if frac_digits < scale {
                frac_part = frac_part.checked_mul(10)?.checked_add(d)?;
                frac_digits = frac_digits.checked_add(1)?;
            }
        } else {
            int_part = int_part.checked_mul(10)?.checked_add(d)?;
        }
    }
    if !seen_digit {
        return None;
    }
    // Pad missing fractional digits so the fraction is always `scale` wide.
    while frac_digits < scale {
        frac_part = frac_part.checked_mul(10)?;
        frac_digits = frac_digits.checked_add(1)?;
    }
    let factor = 10_i64.checked_pow(scale)?;
    let magnitude = int_part.checked_mul(factor)?.checked_add(frac_part)?;
    if neg {
        magnitude.checked_neg()
    } else {
        Some(magnitude)
    }
}

/// Parse an NMEA latitude or longitude (`ddmm.mmmm` / `dddmm.mmmm`) into
/// 1e-9 degrees (unsigned magnitude). `deg_digits` is 2 for latitude and 3 for
/// longitude. Minutes must be below 60 and degrees at most `max_deg`.
pub(crate) fn parse_coord(field: &[u8], deg_digits: usize, max_deg: i64) -> Option<i64> {
    if field.len() < deg_digits.checked_add(2)? || field.len() > MAX_FIELD_LEN {
        return None;
    }
    let (deg_str, min_str) = field.split_at_checked(deg_digits)?;
    let mut degrees: i64 = 0;
    for &b in deg_str {
        degrees = degrees.checked_mul(10)?.checked_add(i64::from(digit(b)?))?;
    }
    if degrees > max_deg {
        return None;
    }
    // Minutes in 1e-9 minutes; require the integer part to be exactly 2 digits.
    let dot = min_str.iter().position(|&b| b == b'.');
    let int_len = dot.unwrap_or(min_str.len());
    if int_len != 2 {
        return None;
    }
    let minutes_e9 = parse_fixed(min_str, 9)?;
    if !(0..60_000_000_000).contains(&minutes_e9) {
        return None;
    }
    // Round to nearest 1e-9 degree.
    let min_as_deg = minutes_e9.checked_add(30)?.checked_div(60)?;
    let total = degrees.checked_mul(1_000_000_000)?.checked_add(min_as_deg)?;
    if total > max_deg.checked_mul(1_000_000_000)? {
        return None;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_basic() {
        assert_eq!(parse_fixed(b"0.860", 3), Some(860));
        assert_eq!(parse_fixed(b"1.124780", 6), Some(1_124_780));
        assert_eq!(parse_fixed(b"-0.337", 3), Some(-337));
        assert_eq!(parse_fixed(b"54.481", 3), Some(54_481));
        assert_eq!(parse_fixed(b"5", 2), Some(500));
        assert_eq!(parse_fixed(b"5.", 2), Some(500));
        assert_eq!(parse_fixed(b".5", 2), Some(50));
    }

    #[test]
    fn fixed_truncates_extra_digits() {
        assert_eq!(parse_fixed(b"1.23456", 3), Some(1_234));
        assert_eq!(parse_fixed(b"-1.23999", 2), Some(-123));
    }

    #[test]
    fn fixed_rejects_garbage() {
        for bad in [&b""[..], b".", b"-", b"1.2.3", b"1e5", b"+1", b"--1", b"1,5", b"abc", b" 1"] {
            assert_eq!(parse_fixed(bad, 3), None, "{:?}", bad);
        }
    }

    #[test]
    fn fixed_rejects_overlong_and_overflow() {
        assert_eq!(parse_fixed(b"1234567890123456789", 0), None);
        assert_eq!(parse_fixed(b"999999999999999999", 6), None); // 18 digits * 1e6 overflows
    }

    #[test]
    fn u32_bounds() {
        assert_eq!(parse_u32(b"0"), Some(0));
        assert_eq!(parse_u32(b"123456789"), Some(123_456_789));
        assert_eq!(parse_u32(b"1234567890"), None);
        assert_eq!(parse_u32(b""), None);
        assert_eq!(parse_u32(b"12a"), None);
        assert_eq!(parse_u32(b"-1"), None);
    }

    #[test]
    fn coord_spec_example() {
        // $GNGGA ... 3149.332006,N,11706.913200,E
        // 31 deg + 49.332006 min = 31.8222001 deg
        assert_eq!(parse_coord(b"3149.332006", 2, 90), Some(31_822_200_100));
        // 117 deg + 06.913200 min = 117.1152200 deg
        assert_eq!(parse_coord(b"11706.913200", 3, 180), Some(117_115_220_000));
    }

    #[test]
    fn coord_rejects_bad() {
        assert_eq!(parse_coord(b"9100.000000", 2, 90), None); // 91 deg
        assert_eq!(parse_coord(b"3160.000000", 2, 90), None); // 60.0 min
        assert_eq!(parse_coord(b"314.9", 2, 90), None);
        assert_eq!(parse_coord(b"", 2, 90), None);
        assert_eq!(parse_coord(b"abcd.efg", 2, 90), None);
        assert_eq!(parse_coord(b"9000.000001", 2, 90), None); // beyond max
    }

    #[test]
    fn coord_without_fraction() {
        assert_eq!(parse_coord(b"3149", 2, 90), Some(31_816_666_667));
    }

    #[test]
    fn coord_max_ok() {
        assert_eq!(parse_coord(b"9000.000000", 2, 90), Some(90_000_000_000));
        assert_eq!(parse_coord(b"18000.000000", 3, 180), Some(180_000_000_000));
    }

    #[test]
    fn hex_digits() {
        assert_eq!(hex_digit(b'0'), Some(0));
        assert_eq!(hex_digit(b'a'), Some(10));
        assert_eq!(hex_digit(b'F'), Some(15));
        assert_eq!(hex_digit(b'g'), None);
    }
}
