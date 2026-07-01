//! Money is stored everywhere as an integer number of minor units (öre) to avoid
//! floating-point rounding error. These helpers convert to/from the decimal string
//! a human types or reads.

/// Parse a user-entered amount into minor units (öre). Accepts `.` or `,` as the
/// decimal separator (Swedish keyboards use `,`). Returns None on garbage or a
/// negative value. Truncates beyond two decimals.
pub fn parse_amount(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    let major: i64 = int_part.parse().ok()?;
    if major < 0 {
        return None;
    }
    let frac = match frac_part.len() {
        0 => 0,
        1 => frac_part.parse::<i64>().ok()? * 10,
        _ => frac_part.get(..2)?.parse::<i64>().ok()?,
    };
    Some(major * 100 + frac)
}

/// Render minor units (öre) as a plain decimal string, e.g. `12550` -> `"125.50"`.
pub fn format_amount(ore: i64) -> String {
    let sign = if ore < 0 { "-" } else { "" };
    let ore = ore.abs();
    format!("{sign}{}.{:02}", ore / 100, ore % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_amount("125"), Some(12500));
        assert_eq!(parse_amount("125.5"), Some(12550));
        assert_eq!(parse_amount("125.50"), Some(12550));
        assert_eq!(parse_amount("125,50"), Some(12550)); // swedish comma
        assert_eq!(parse_amount("0.99"), Some(99));
        assert_eq!(parse_amount(".5"), Some(50));
        assert_eq!(parse_amount("  10  "), Some(1000));
        assert_eq!(parse_amount("1.999"), Some(199)); // truncates
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_amount(""), None);
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount("-5"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(format_amount(12550), "125.50");
        assert_eq!(format_amount(5), "0.05");
        assert_eq!(format_amount(100), "1.00");
        assert_eq!(format_amount(-250), "-2.50");
    }
}
