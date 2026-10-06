//! Human-readable sizes and dates for the file list.

use std::time::SystemTime;

/// Binary units (1 KiB = 1024 B), shown with the familiar short labels.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Local time as `YYYY-MM-DD HH:MM`.
pub fn format_time(time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024u64.pow(3)), "3.0 GB");
    }

    #[test]
    fn time_has_minute_precision() {
        let s = format_time(SystemTime::UNIX_EPOCH);
        assert_eq!(s.len(), "1970-01-01 00:00".len());
        assert!(s.starts_with("1970-01-01") || s.starts_with("1969-12-31"));
    }
}
