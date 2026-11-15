//! Linux 桌面端的媒体通道：把 [`crate::media::handle`] 挂到一个只监听
//! 127.0.0.1 的 HTTP 服务上。
//!
//! 为什么需要它：Tauri 的自定义协议只对 WebView 自己的资源加载生效，而
//! WebKitGTK 的 `<video>` 是交给 GStreamer 播放的，GStreamer 只认 http/https/file
//! 这类常规 URI，直接给它 `hg://localhost` 打不开；同时 WebKitGTK 4.1 默认不提供
//! MediaSource，hls.js 也用不上。Linux 端的播放只能让 GStreamer 的 hlsdemux
//! 原生完成，而它需要一个真正可取的 HTTP 地址——就是这个服务。
//!
//! 端口固定，前端 `src/api.ts` 的 `LOCAL` 必须与 [`ORIGIN`] 一致。
//!
//! 来源边界：校验 `Host`，拒绝带外来 `Origin` 的请求，也拒绝没有 Origin 但
//! `Sec-Fetch-Site: cross-site` 的浏览器嵌入请求。CORS 只回显已经接受的应用 Origin，
//! 不再使用 `*`。GStreamer 不发送 Origin 和 Fetch Metadata，所以这类本机非浏览器客户端
//! 仍然可以连接。这不是对抗同一用户其他进程的认证方案。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// 监听端口。前端硬编码同一个值，两边必须同时改。
pub const PORT: u16 = 17650;

/// 前端与 m3u8 分片改写共用的 origin。
pub const ORIGIN: &str = "http://127.0.0.1:17650";

/// 在后台线程里起服务。绑定失败只记日志（播放会退回自定义协议，大概率失败）。
pub fn start() {
    std::thread::spawn(|| {
        let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("[hg] 媒体端口 {PORT} 绑定失败：{err}");
                return;
            }
        };
        eprintln!("[hg] 媒体代理就绪：{ORIGIN}");
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                if let Err(err) = serve(stream) {
                    eprintln!("[hg] 媒体请求失败：{err}");
                }
            });
        }
    });
}

struct Accepted<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    range: Option<String>,
    origin: Option<String>,
}

/// 在接触上游之前决定这个回环请求能不能进媒体代理。
fn accept_request<'a>(head: &'a str, expected_host: &str) -> Result<Accepted<'a>, u16> {
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or("").split_whitespace();
    let method = request.next().ok_or(400u16)?;
    let target = request.next().ok_or(400u16)?;
    if request.next() != Some("HTTP/1.1") || request.next().is_some() {
        return Err(400);
    }
    if method != "GET" && method != "HEAD" {
        return Err(405);
    }
    if !target.starts_with('/') || target.starts_with("//") {
        return Err(400);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut host = None;
    let mut origin = None;
    let mut fetch_site = None;
    let mut range = None;
    let mut range_count = 0;
    let mut if_range = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        let value = value.trim();
        if value.chars().any(|c| c.is_control()) {
            return Err(400);
        }
        if name.eq_ignore_ascii_case("Host") {
            if host.is_some() {
                return Err(400);
            }
            host = Some(value);
        } else if name.eq_ignore_ascii_case("Origin") {
            if origin.is_some() {
                return Err(400);
            }
            origin = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("Sec-Fetch-Site") {
            fetch_site = Some(value);
        } else if name.eq_ignore_ascii_case("Range") {
            range_count += 1;
            range = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("If-Range") {
            if_range = true;
        }
    }
    if host != Some(expected_host) {
        return Err(403);
    }
    if let Some(origin) = origin.as_deref() {
        if !crate::allowed_frontend_origin(origin) {
            return Err(403);
        }
    } else if fetch_site.is_some_and(|site| site.eq_ignore_ascii_case("cross-site")) {
        return Err(403);
    }
    let range = if method == "GET" && range_count == 1 && !if_range {
        range
    } else {
        None
    };
    Ok(Accepted { method, path, query, range, origin })
}

fn serve(stream: TcpStream) -> std::io::Result<()> {
    serve_connection(stream, &format!("127.0.0.1:{PORT}"))
}

fn serve_connection(mut stream: TcpStream, expected_host: &str) -> std::io::Result<()> {
    let timeout = Some(std::time::Duration::from_secs(30));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = Vec::new();
    const MAX_HEADERS: usize = 16 * 1024;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        let room = MAX_HEADERS.saturating_add(1).saturating_sub(head.len());
        let take = available.len().min(room);
        let end = available[..take].windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
        let count = end.unwrap_or(take);
        head.extend_from_slice(&available[..count]);
        reader.consume(count);
        if head.len() > MAX_HEADERS {
            return write_plain(&mut stream, 431, "headers too large");
        }
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = match std::str::from_utf8(&head) {
        Ok(text) => text,
        Err(_) => return write_plain(&mut stream, 400, "bad request"),
    };
    let accepted = match accept_request(text, expected_host) {
        Ok(accepted) => accepted,
        Err(status) => return write_plain(&mut stream, status, "request rejected"),
    };
    let response = tauri::async_runtime::block_on(crate::media::handle(
        accepted.path,
        accepted.query,
        ORIGIN,
        accepted.range.as_deref(),
    ));
    let (parts, body) = response.into_parts();
    write_response(&mut stream, accepted.method, accepted.origin.as_deref(), parts, &body)
}

fn write_response(
    stream: &mut impl Write,
    method: &str,
    origin: Option<&str>,
    parts: tauri::http::response::Parts,
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = String::with_capacity(320);
    head.push_str(&format!(
        "HTTP/1.1 {} {}\r\n",
        parts.status.as_u16(),
        parts.status.canonical_reason().unwrap_or("OK")
    ));
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    for (name, value) in parts.headers.iter() {
        // This server sends a complete body and closes the connection. Do not
        // duplicate media::handle's length/CORS headers or forward framing for
        // an upstream connection (WebKit rejects ambiguous HTTP framing).
        if matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "access-control-allow-origin"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            head.push_str(&format!("{}: {}\r\n", name.as_str(), value));
        }
    }
    if let Some(origin) = origin {
        if !origin.chars().any(|c| c.is_control() || matches!(c, '\r' | '\n')) {
            head.push_str("Access-Control-Allow-Origin: ");
            head.push_str(origin);
            head.push_str("\r\nVary: Origin\r\n");
        }
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    if method != "HEAD" {
        stream.write_all(body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::{serve_connection, write_response, TcpListener, TcpStream};
    use tauri::http::Response;

    #[test]
    fn get_has_one_authoritative_length_and_cors_header() {
        let (parts, body) = Response::builder()
            .header("Content-Length", "999")
            .header("Transfer-Encoding", "chunked")
            .header("Connection", "keep-alive")
            .header("Access-Control-Allow-Origin", "*")
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .body(b"#EXTM3U\n".to_vec())
            .unwrap()
            .into_parts();
        let mut wire = Vec::new();
        write_response(&mut wire, "GET", Some("http://tauri.localhost"), parts, &body).unwrap();
        let wire = String::from_utf8(wire).unwrap();
        let (head, data) = wire.split_once("\r\n\r\n").unwrap();
        let headers = head.to_ascii_lowercase();
        assert_eq!(headers.matches("content-length:").count(), 1);
        assert!(headers.contains("content-length: 8\r\n"));
        assert_eq!(headers.matches("access-control-allow-origin:").count(), 1);
        assert!(headers.contains("access-control-allow-origin: http://tauri.localhost"));
        assert!(!headers.contains("access-control-allow-origin: *"));
        assert_eq!(headers.matches("connection:").count(), 1);
        assert!(headers.ends_with("connection: close"));
        assert!(!headers.contains("transfer-encoding:"));
        assert!(headers.contains("content-type: application/vnd.apple.mpegurl"));
        assert_eq!(data, "#EXTM3U\n");
    }

    #[test]
    fn head_keeps_representation_length_without_sending_body() {
        let (parts, body) = Response::builder()
            .status(206)
            .header("Content-Range", "bytes 0-3/20")
            .body(b"data".to_vec())
            .unwrap()
            .into_parts();
        let mut wire = Vec::new();
        write_response(&mut wire, "HEAD", None, parts, &body).unwrap();
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(wire.contains("Content-Length: 4\r\n"));
        assert!(wire.contains("content-range: bytes 0-3/20\r\n"));
        assert!(wire.ends_with("\r\n\r\n"));
        assert!(!wire.to_ascii_lowercase().contains("access-control-allow-origin: *"));
    }

    fn exchange(request: &str) -> String {
        use std::io::{Read, Write};
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = format!("127.0.0.1:{port}");
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            serve_connection(socket, &expected).unwrap();
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        client.write_all(request.replace("{port}", &port.to_string()).as_bytes()).unwrap();
        let mut body = String::new();
        let _ = client.read_to_string(&mut body);
        server.join().unwrap();
        body
    }

    #[test]
    fn socket_rejects_foreign_and_cross_site_requests_before_the_media_proxy() {
        let foreign = exchange(
            "GET /seg?u=http://127.0.0.1:1/secret-media HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(foreign.starts_with("HTTP/1.1 403"), "{foreign}");
        assert!(!foreign.contains("secret-media"));

        let cross_site = exchange(
            "GET /seg?u=http://127.0.0.1:1/secret-media HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nSec-Fetch-Site: cross-site\r\nConnection: close\r\n\r\n",
        );
        assert!(cross_site.starts_with("HTTP/1.1 403"), "{cross_site}");
        assert!(!cross_site.contains("secret-media"));

        let wrong_host = exchange(
            "GET /no-such HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(wrong_host.starts_with("HTTP/1.1 403"), "{wrong_host}");
    }

    #[test]
    fn socket_accepts_player_and_app_origins_without_a_wildcard() {
        let player = exchange(
            "GET /seg HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
        );
        assert!(player.starts_with("HTTP/1.1 400"), "{player}");
        assert!(player.contains("missing u"));
        assert!(!player.to_ascii_lowercase().contains("access-control-allow-origin: *"));

        let app = exchange(
            "GET /not-a-route?u=https://fixture.invalid/clip HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://tauri.localhost\r\nSec-Fetch-Site: cross-site\r\nConnection: close\r\n\r\n",
        );
        assert!(app.starts_with("HTTP/1.1 404"), "{app}");
        assert!(app.contains("not found"));
        assert!(!app.contains("fixture.invalid"));
        assert!(app.to_ascii_lowercase().contains("access-control-allow-origin: http://tauri.localhost"));
        assert!(!app.to_ascii_lowercase().contains("access-control-allow-origin: *"));
    }
}

fn write_plain(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let reason = tauri::http::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Error");
    let body = message.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}
