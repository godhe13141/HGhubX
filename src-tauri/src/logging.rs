//! Shared UTC timestamps and the always-on debug journal.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

static DEBUG_WRITER: Mutex<()> = Mutex::new(());

fn format_utc(at: OffsetDateTime) -> Result<String, String> {
    at.to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| "日志时间格式错误".to_string())
}

pub(crate) fn utc_timestamp() -> Result<String, String> {
    format_utc(OffsetDateTime::now_utc())
}

/// 诊断文本只留主机和失败类别。完整路径、查询串、用户信息和 Bearer 令牌都不落盘。
pub(crate) fn redact_diagnostics(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(marker) = rest.find("://") {
        let scheme_start = rest[..marker]
            .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '.' && c != '-')
            .map(|index| index + rest[..marker][index..].chars().next().unwrap().len_utf8())
            .unwrap_or(0);
        let scheme = &rest[scheme_start..marker];
        if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
            redacted.push_str(&rest[..marker + 3]);
            rest = &rest[marker + 3..];
            continue;
        }
        let after = &rest[marker + 3..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ','))
            .unwrap_or(after.len());
        let raw_url = &after[..end];
        let (had_userinfo, host_and_tail) = match raw_url.rfind('@') {
            Some(at) => (true, &raw_url[at + 1..]),
            None => (false, raw_url),
        };
        let host_end = host_and_tail
            .find(|c: char| matches!(c, '/' | '?' | '#'))
            .unwrap_or(host_and_tail.len());
        let host = &host_and_tail[..host_end];
        let tail = &host_and_tail[host_end..];
        let sensitive = had_userinfo
            || tail.contains('?')
            || tail.contains('#')
            || tail.trim_start_matches('/').contains('/');
        // A single slash is the origin itself. Anything longer can be a direct media path.
        let sensitive = sensitive || (tail.len() > 1 && tail != "/");
        redacted.push_str(&rest[..scheme_start]);
        redacted.push_str(scheme);
        redacted.push_str("://");
        redacted.push_str(host);
        if sensitive {
            redacted.push_str("/[redacted]");
        } else {
            redacted.push_str(tail);
        }
        rest = &after[end..];
    }
    redacted.push_str(rest);
    redact_assignments(&redacted)
}

fn redact_assignments(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let lower = message.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"bearer ") {
            let value = i + "bearer ".len();
            let mut end = value;
            while end < bytes.len() && !bytes[end].is_ascii_whitespace() && !matches!(bytes[end], b'"' | b'\'' | b'<' | b'>') {
                end += 1;
            }
            if end - value >= 8 {
                out.push_str(&message[last..i]);
                out.push_str("Bearer [redacted]");
                last = end;
                i = end;
                continue;
            }
        }
        const KEYS: [&str; 6] = ["auth_key=", "access_token=", "refresh_token=", "token=", "password=", "secret="];
        if let Some(key) = KEYS.iter().find(|key| bytes[i..].starts_with(key.as_bytes())) {
            let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if boundary {
                let value = i + key.len();
                let mut end = value;
                while end < bytes.len() && !matches!(bytes[end], b'&' | b'#' | b'"' | b'\'' | b' ' | b'\n' | b'\r' | b'<') {
                    end += 1;
                }
                out.push_str(&message[last..value]);
                out.push_str("[redacted]");
                last = end;
                i = end;
                continue;
            }
        }
        i += message[i..].chars().next().unwrap().len_utf8();
    }
    out.push_str(&message[last..]);
    out
}

pub(crate) fn append_debug(dir: &Path, message: &str) -> Result<(), String> {
    let _writer = DEBUG_WRITER.lock().map_err(|_| "日志锁失败")?;
    fs::create_dir_all(dir).map_err(|_| "无法创建日志目录")?;
    let message = redact_diagnostics(message);
    let at = utc_timestamp()?;
    let prefix = format!("[{at}] [pid={}] ", std::process::id());
    let mut record = String::new();
    // Stamp continuation lines too, including stack traces, HTML errors and pretty JSON.
    for line in message.lines() {
        record.push_str(&prefix);
        record.push_str(line);
        record.push('\n');
    }
    if record.is_empty() {
        record.push_str(&prefix);
        record.push('\n');
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("debug.log"))
        .and_then(|mut f| f.write_all(record.as_bytes()))
        .map_err(|_| "无法写入调试日志".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_dir() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hghubx-debug-log-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn timestamps_are_utc_even_when_input_has_another_offset() {
        let instant = OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .replace_nanosecond(123_456_789)
            .unwrap();
        for offset in [
            UtcOffset::UTC,
            UtcOffset::from_hms(8, 0, 0).unwrap(),
            UtcOffset::from_hms(-7, 0, 0).unwrap(),
        ] {
            assert_eq!(
                format_utc(instant.to_offset(offset)).unwrap(),
                "1970-01-01T00:00:00.123456789Z"
            );
        }
    }

    #[test]
    fn debug_errors_and_continuation_lines_have_utc_and_preserve_history() {
        let dir = test_dir();
        let path = dir.join("debug.log");
        fs::write(&path, "legacy entry without timestamp\n").unwrap();
        append_debug(&dir, "[api] api_feed → HTTP 503\r\n<html>\r\nerror\n").unwrap();
        append_debug(&dir, "[reject] failure\n    at handler").unwrap();
        append_debug(&dir, "").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "legacy entry without timestamp");
        assert_eq!(lines.len(), 7);
        for line in &lines[1..] {
            let (at, rest) = line.strip_prefix('[').unwrap().split_once("] ").unwrap();
            assert!(at.ends_with('Z'));
            assert!(at.contains('T'));
            assert!(rest.starts_with(&format!("[pid={}] ", std::process::id())));
            assert!(!line.contains('\r'));
        }
        assert!(lines[1].ends_with("[api] api_feed → HTTP 503"));
        assert!(lines[5].ends_with("    at handler"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_multiline_records_remain_together() {
        let dir = test_dir();
        let writers: Vec<_> = (0..4)
            .map(|worker| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for entry in 0..8 {
                        append_debug(
                            &dir,
                            &format!("start {worker}/{entry}\nend {worker}/{entry}"),
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let text = fs::read_to_string(dir.join("debug.log")).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 64);
        for pair in lines.chunks_exact(2) {
            assert_eq!(pair[1], pair[0].replace("start ", "end "));
            assert!(pair[0].contains("Z] [pid="));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn debug_log_drops_media_urls_userinfo_and_bearer_tokens() {
        let raw = "error sending request for url (https://user:secret@cdn.example/videos/a.m3u8?auth_key=supersecret&v=1) ← Bearer abcdefghijklmnop password=hunter2";
        let safe = redact_diagnostics(raw);
        assert!(safe.contains("cdn.example"));
        assert!(safe.contains("/[redacted]"));
        assert!(!safe.contains("supersecret") && !safe.contains("auth_key=") && !safe.contains("user:secret"));
        assert!(!safe.contains("abcdefghijklmnop") && !safe.contains("hunter2"));
        assert!(safe.contains("Bearer [redacted]") && safe.contains("password=[redacted]"));
        assert_eq!(redact_diagnostics("https://example.com/"), "https://example.com/");
        let dir = test_dir();
        append_debug(&dir, raw).unwrap();
        let stored = fs::read_to_string(dir.join("debug.log")).unwrap();
        assert!(!stored.contains("supersecret") && !stored.contains("hunter2"));
        assert!(stored.contains("cdn.example"));
        fs::remove_dir_all(dir).unwrap();
    }
}
