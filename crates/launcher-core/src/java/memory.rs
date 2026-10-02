//! How much memory Minecraft may take: limits from the machine's RAM and
//! normalised `-Xmx` arguments. `-Xmx` without a unit is bytes, and the default
//! `-Xmx` is added whenever none is given.

use serde_json::Value;

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const MIN_HEAP_GB: u64 = 1;
pub const SYSTEM_RESERVE_GB: u64 = 2;
pub const ABSOLUTE_MAX_HEAP_GB: u64 = 32;
pub const FALLBACK_TOTAL_GB: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryLimits {
    pub total_gb: u64,
    pub available_gb: Option<u64>,
    pub min_heap_gb: u64,
    pub max_heap_gb: u64,
    pub recommended_heap_gb: u64,
}

fn max_heap_for(total_gb: u64) -> u64 {
    let safe = match total_gb {
        0..=2 => 1,
        3..=4 => total_gb - 1,
        _ => total_gb - SYSTEM_RESERVE_GB,
    };
    safe.clamp(MIN_HEAP_GB, ABSOLUTE_MAX_HEAP_GB)
}

fn recommended_for(total_gb: u64) -> u64 {
    match total_gb {
        0..=4 => 2,
        5..=8 => 4,
        9..=16 => 6,
        17..=32 => 8,
        _ => 12,
    }
}

impl MemoryLimits {
    /// Limits for a machine with `total` bytes of RAM (unknown or zero: 8 GiB).
    pub fn from_bytes(total: Option<u64>, available: Option<u64>) -> MemoryLimits {
        let total_gb = total.filter(|b| *b > 0).map_or(FALLBACK_TOTAL_GB, |b| b / GIB).max(MIN_HEAP_GB);
        let max_heap_gb = max_heap_for(total_gb);
        MemoryLimits {
            total_gb,
            available_gb: available.map(|b| b / GIB),
            min_heap_gb: MIN_HEAP_GB,
            max_heap_gb,
            recommended_heap_gb: recommended_for(total_gb).min(max_heap_gb).max(MIN_HEAP_GB),
        }
    }

    pub fn detect() -> MemoryLimits {
        let (total, available) = detect_memory_bytes();
        MemoryLimits::from_bytes(total, available)
    }

    /// `gb` within [min, max].
    pub fn clamp(&self, gb: u64) -> u64 {
        gb.clamp(self.min_heap_gb, self.max_heap_gb)
    }
}

impl From<MemoryLimits> for launcher_shared::MemoryInfo {
    fn from(limits: MemoryLimits) -> launcher_shared::MemoryInfo {
        launcher_shared::MemoryInfo {
            total_gb: limits.total_gb,
            min_heap_gb: limits.min_heap_gb,
            max_heap_gb: limits.max_heap_gb,
            recommended_heap_gb: limits.recommended_heap_gb,
        }
    }
}

/// A memory amount in whole GiB. A bare number is GiB (config values); `-Xmx`/`-Xms` with `k`,
/// `m` or `g` rounds up; `-Xmx` without a unit is bytes. Zero and junk: `None`.
pub fn parse_memory_gb(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
        return raw.parse::<u64>().ok().filter(|gb| *gb > 0);
    }
    let rest = raw.strip_prefix("-Xmx").or_else(|| raw.strip_prefix("-Xms"))?;
    let (digits, unit) = match rest.chars().last()? {
        c if c.is_ascii_alphabetic() => (&rest[..rest.len() - 1], Some(c.to_ascii_lowercase())),
        _ => (rest, None),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    let divisor = match unit {
        Some('g') => 1,
        Some('m') => 1024,
        Some('k') => 1024 * 1024,
        None => GIB,
        Some(_) => return None,
    };
    Some(amount.div_ceil(divisor).max(1))
}

/// A config value: a number of GiB or a text `parse_memory_gb` understands.
pub fn parse_memory_value(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_f64().filter(|gb| *gb >= 1.0).map(|gb| gb as u64),
        Value::String(text) => parse_memory_gb(text),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedJvm {
    pub arguments: Vec<String>,
    pub max_gb: Option<u64>,
    pub changed: bool,
    pub removed_initial_heap: bool,
}

/// One `-Xmx` first (the last one given, else `fallback_max_gb`), clamped to the machine; `-Xms`
/// and blanks dropped. Other repeats stay (`--add-opens` comes once per package).
pub fn sanitize_jvm_arguments(
    arguments: &[String],
    fallback_max_gb: Option<u64>,
    limits: &MemoryLimits,
) -> SanitizedJvm {
    let original: Vec<String> =
        arguments.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    let mut extra: Vec<String> = Vec::new();
    let mut selected = None;
    let mut removed_initial_heap = false;
    for argument in &original {
        let lower = argument.to_ascii_lowercase();
        if lower.starts_with("-xmx") {
            if let Some(gb) = parse_memory_gb(argument) {
                selected = Some(gb);
            }
            continue;
        }
        if lower.starts_with("-xms") {
            removed_initial_heap = true;
            continue;
        }
        extra.push(argument.clone());
    }
    let max_gb = selected.or(fallback_max_gb).map(|gb| limits.clamp(gb));
    let mut sanitized = Vec::with_capacity(extra.len() + 1);
    if let Some(gb) = max_gb {
        sanitized.push(format!("-Xmx{gb}G"));
    }
    sanitized.extend(extra);
    SanitizedJvm { changed: sanitized != original, arguments: sanitized, max_gb, removed_initial_heap }
}

#[cfg(windows)]
fn detect_memory_bytes() -> (Option<u64>, Option<u64>) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: the struct is zeroed with its length set, as the API requires.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return (None, None);
    }
    (Some(status.ullTotalPhys), Some(status.ullAvailPhys))
}

#[cfg(target_os = "linux")]
fn detect_memory_bytes() -> (Option<u64>, Option<u64>) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else { return (None, None) };
    let field = |name: &str| {
        text.lines()
            .find_map(|line| {
                line.strip_prefix(name)?.strip_prefix(':')?.split_whitespace().next()?.parse::<u64>().ok()
            })
            .map(|kib| kib * 1024)
    };
    (field("MemTotal"), field("MemAvailable"))
}

#[cfg(target_os = "macos")]
fn detect_memory_bytes() -> (Option<u64>, Option<u64>) {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: `hw.memsize` is a u64 and `len` holds its size.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut bytes as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    ((status == 0).then_some(bytes), None)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn detect_memory_bytes() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn limits(total_gb: u64) -> MemoryLimits {
        MemoryLimits::from_bytes(Some(total_gb * GIB), None)
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn limits_follow_the_original_table() {
        let table = [(1, 1, 1), (2, 1, 1), (4, 3, 2), (8, 6, 4), (16, 14, 6), (32, 30, 8), (64, 32, 12)];
        for (total, max, recommended) in table {
            let l = limits(total);
            assert_eq!(
                (l.total_gb, l.max_heap_gb, l.recommended_heap_gb),
                (total, max, recommended),
                "{total} GiB"
            );
        }
        let unknown = MemoryLimits::from_bytes(None, None);
        assert_eq!((unknown.total_gb, unknown.max_heap_gb), (FALLBACK_TOTAL_GB, 6));
        assert_eq!(MemoryLimits::from_bytes(Some(0), None).total_gb, FALLBACK_TOTAL_GB);
        assert_eq!(MemoryLimits::from_bytes(Some(GIB / 2), Some(GIB)).available_gb, Some(1));
        assert_eq!((limits(16).clamp(0), limits(16).clamp(64)), (1, 14));
    }

    #[test]
    fn memory_values_parse_like_the_original_except_unitless_bytes() {
        let cases = [
            ("6", Some(6)),
            ("0", None),
            ("-Xmx4G", Some(4)),
            ("-Xmx4g", Some(4)),
            ("-Xmx4096M", Some(4)),
            ("-Xmx4097m", Some(5)),
            ("-Xmx2097152k", Some(2)),
            ("-Xmx4294967296", Some(4)),
            ("-Xmx512", Some(1)),
            ("-Xms2G", Some(2)),
            ("-Xmx4.5G", None),
            ("-Xmx4T", None),
            ("-Xmx", None),
            ("-XX:+UseG1GC", None),
            ("abc", None),
        ];
        for (raw, gb) in cases {
            assert_eq!(parse_memory_gb(raw), gb, "{raw}");
        }
        assert_eq!(parse_memory_value(&json!(6)), Some(6));
        assert_eq!(parse_memory_value(&json!(0.5)), None);
        assert_eq!(parse_memory_value(&json!("8")), Some(8));
        assert_eq!(parse_memory_value(&json!(true)), None);
    }

    #[test]
    fn repeated_arguments_other_than_memory_are_kept() {
        let given =
            args(&["--add-opens", "a/b=ALL-UNNAMED", "--add-opens", "c/d=ALL-UNNAMED", "-Xmx2G", "-Xmx3G"]);
        let s = sanitize_jvm_arguments(&given, None, &limits(16));
        assert_eq!(
            s.arguments,
            args(&["-Xmx3G", "--add-opens", "a/b=ALL-UNNAMED", "--add-opens", "c/d=ALL-UNNAMED"])
        );
    }

    #[test]
    fn sanitizing_keeps_one_safe_xmx() {
        let l = limits(16);
        let s = sanitize_jvm_arguments(&args(&["-Xmx4G", "-XX:+UseG1GC", "-Xms1G", "  "]), Some(6), &l);
        assert_eq!(s.arguments, args(&["-Xmx4G", "-XX:+UseG1GC"]));
        assert_eq!((s.max_gb, s.changed, s.removed_initial_heap), (Some(4), true, true));
        let fallback = sanitize_jvm_arguments(&args(&["-XX:+UseG1GC"]), Some(6), &l);
        assert_eq!(
            fallback.arguments,
            args(&["-Xmx6G", "-XX:+UseG1GC"]),
            "fallback even with other arguments"
        );
        assert_eq!(sanitize_jvm_arguments(&args(&["-Xmx64G"]), None, &l).arguments, args(&["-Xmx14G"]));
        assert_eq!(
            sanitize_jvm_arguments(&args(&["-Xmx2G", "-Xmx3G"]), None, &l).arguments,
            args(&["-Xmx3G"])
        );
        assert_eq!(sanitize_jvm_arguments(&args(&["-Xmx8589934592"]), None, &l).arguments, args(&["-Xmx8G"]));
        let untouched = sanitize_jvm_arguments(&args(&["-Xmx4G"]), Some(6), &l);
        assert_eq!((untouched.arguments, untouched.changed), (args(&["-Xmx4G"]), false));
        let empty = sanitize_jvm_arguments(&[], None, &l);
        assert_eq!((empty.arguments.len(), empty.changed, empty.max_gb), (0, false, None));
    }

    #[test]
    fn this_machine_reports_sane_limits() {
        let l = MemoryLimits::detect();
        assert!(l.total_gb >= 1);
        assert!(l.min_heap_gb <= l.recommended_heap_gb && l.recommended_heap_gb <= l.max_heap_gb);
    }

    #[test]
    fn the_ui_gets_the_same_limits() {
        assert_eq!(
            launcher_shared::MemoryInfo::from(limits(16)),
            launcher_shared::MemoryInfo {
                total_gb: 16,
                min_heap_gb: 1,
                max_heap_gb: 14,
                recommended_heap_gb: 6
            }
        );
    }
}
