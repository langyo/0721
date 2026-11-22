//! Byte-size parsing for the `image-size-limit` configuration knob.
//!
//! Legacy semantics: `"8MiB"` style units (case-insensitive, `B`/`KiB`/
//! `MiB`/`GiB`/`TiB`, plus the plain `K`/`M`/`G` shorthands) or a bare
//! number string meaning raw bytes.

/// Parse a size string into bytes.
///
/// Whitespace between number and unit is tolerated (`"8 MiB"`).
///
/// # Examples
///
/// ```
/// # use i0721_proto::parse_size_limit;
/// assert_eq!(parse_size_limit("8MiB"), Some(8 * 1024 * 1024));
/// assert_eq!(parse_size_limit("1024"), Some(1024));
/// assert_eq!(parse_size_limit("1.5MiB"), Some(1_572_864));
/// assert_eq!(parse_size_limit("nope"), None);
/// ```
#[must_use]
pub fn parse_size_limit(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let split = value.find(|c: char| !c.is_ascii_digit() && c != '.');
    let (number, unit) = match split {
        Some(idx) => value.split_at(idx),
        None => (value, ""),
    };
    let number: f64 = number.trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = unit.trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1u64,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024_u64 * 1024 * 1024 * 1024,
        _ => return None,
    };
    let bytes = number * multiplier as f64;
    if bytes.is_finite() && bytes >= 0.0 && bytes <= u64::MAX as f64 {
        Some(bytes as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse_size_limit("8MiB"), Some(8 * 1024 * 1024));
        assert_eq!(parse_size_limit("8mib"), Some(8 * 1024 * 1024));
        assert_eq!(parse_size_limit("8 MiB"), Some(8 * 1024 * 1024));
        assert_eq!(parse_size_limit("2GiB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_limit("512KiB"), Some(512 * 1024));
        assert_eq!(parse_size_limit("4M"), Some(4 * 1024 * 1024));
        assert_eq!(parse_size_limit("1024"), Some(1024));
        assert_eq!(parse_size_limit("0"), Some(0));
        assert_eq!(parse_size_limit("0B"), Some(0));
        assert_eq!(parse_size_limit("1.5MiB"), Some(1_572_864));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_size_limit(""), None);
        assert_eq!(parse_size_limit("   "), None);
        assert_eq!(parse_size_limit("8 apples"), None);
        assert_eq!(parse_size_limit("MiB"), None);
        assert_eq!(parse_size_limit("-8MiB"), None);
        assert_eq!(parse_size_limit("8XiB"), None);
    }
}
