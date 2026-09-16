use std::fs;

/// Best-effort RSS from `/proc/self/statm` (pages → bytes).
pub fn rss_bytes() -> u64 {
    let Ok(text) = fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let page_size = rustix::param::page_size() as u64;
    pages.saturating_mul(page_size)
}
