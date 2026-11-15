//! Loopback host for the browser shell.
//!
//! The page never receives upstream credentials or the media decryption key.
//! It only talks to this process. Without an explicit local fixture, the host
//! reports that no playback backend is configured and does not invent a catalog.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 32 * 1024 * 1024;
const FIXTURE_MARK: &str = "不是真实剧库";

struct State {
    dist: PathBuf,
    fixture: Option<Fixture>,
}

struct Fixture {
    root: PathBuf,
    label: String,
    items: Vec<Item>,
}

struct Item {
    id: u32,
    title: String,
    episodes: Vec<EpisodeFile>,
}

struct EpisodeFile {
    ep_num: u32,
    title: String,
    duration: u32,
    file: String,
}

#[derive(Deserialize)]
struct CatalogFile {
    label: String,
    items: Vec<CatalogItem>,
}

#[derive(Deserialize)]
struct CatalogItem {
    id: u32,
    title: String,
    episodes: Vec<CatalogEpisode>,
}

#[derive(Deserialize)]
struct CatalogEpisode {
    ep_num: u32,
    title: String,
    duration: u32,
    file: String,
}

fn main() {
    let mut args = env::args().skip(1);
    let mut dist = None;
    let mut fixture = None;
    let mut port = 17651u16;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dist" => dist = args.next(),
            "--fixture" => fixture = args.next(),
            "--port" => {
                let raw = args.next().unwrap_or_default();
                port = raw.parse().unwrap_or(0);
            }
            "--bind" => {
                eprintln!("hghubx-web 只监听 127.0.0.1，不接受 --bind");
                std::process::exit(2);
            }
            other => {
                eprintln!("无法识别的参数 {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(dist) = dist else {
        eprintln!("用法: hghubx-web --dist dist [--fixture 目录] [--port 17651]");
        std::process::exit(2);
    };
    let state = match load_state(PathBuf::from(dist), fixture.map(PathBuf::from)) {
        Ok(state) => Arc::new(state),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("runtime");
    runtime.block_on(async move {
        let listener = match bind_loopback(port) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let addr = listener.local_addr().expect("local addr");
        println!("ready http://{addr}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let (hold, shutdown) = oneshot::channel::<()>();
        serve(listener, state, shutdown).await;
        drop(hold);
    });
}

fn load_state(dist: PathBuf, fixture: Option<PathBuf>) -> Result<State, String> {
    let dist = dist.canonicalize().map_err(|error| format!("无法打开前端目录: {error}"))?;
    if !dist.join("web.html").is_file() {
        return Err("前端目录里没有 web.html。先运行 pnpm build。".to_string());
    }
    let fixture = match fixture {
        None => None,
        Some(path) => Some(load_fixture(&path)?),
    };
    Ok(State { dist, fixture })
}

fn load_fixture(path: &Path) -> Result<Fixture, String> {
    let root = path.canonicalize().map_err(|error| format!("无法打开示例目录: {error}"))?;
    let raw = fs::read(root.join("catalog.json")).map_err(|error| format!("无法读取 catalog.json: {error}"))?;
    let catalog: CatalogFile = serde_json::from_slice(&raw).map_err(|error| format!("catalog.json 无法解析: {error}"))?;
    if !catalog.label.contains(FIXTURE_MARK) {
        return Err("示例目录的 label 必须标明不是真实剧库".to_string());
    }
    if catalog.items.is_empty() {
        return Err("示例目录至少要有一条内容".to_string());
    }
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for item in catalog.items {
        if item.id == 0 || item.title.trim().is_empty() || !seen.insert(item.id) {
            return Err("示例条目的编号或标题无效".to_string());
        }
        if item.episodes.is_empty() {
            return Err(format!("示例 {} 没有选集", item.id));
        }
        let mut episodes = Vec::new();
        let mut episode_numbers = BTreeSet::new();
        for episode in item.episodes {
            if episode.ep_num == 0 || episode.title.trim().is_empty() || !episode_numbers.insert(episode.ep_num) {
                return Err("选集编号或标题无效".to_string());
            }
            if !safe_file_name(&episode.file) {
                return Err("选集文件名超出本地示例目录".to_string());
            }
            episodes.push(EpisodeFile {
                ep_num: episode.ep_num,
                title: episode.title,
                duration: episode.duration,
                file: episode.file,
            });
        }
        items.push(Item { id: item.id, title: item.title, episodes });
    }
    Ok(Fixture { root, label: catalog.label, items })
}

fn bind_loopback(port: u16) -> Result<TcpListener, String> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = std::net::TcpListener::bind(address).map_err(|error| format!("无法监听 {address}: {error}"))?;
    listener.set_nonblocking(true).map_err(|error| error.to_string())?;
    TcpListener::from_std(listener).map_err(|error| error.to_string())
}

async fn serve(listener: TcpListener, state: Arc<State>, mut shutdown: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((socket, peer)) = accepted else { continue; };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = handle_connection(socket, state).await;
                });
            }
        }
    }
}

async fn handle_connection(mut socket: TcpStream, state: Arc<State>) -> std::io::Result<()> {
    let mut header = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if header.windows(4).any(|window| window == b"\r\n\r\n") || header.len() > MAX_HEADER {
            break;
        }
        let read = socket.read(&mut chunk).await?;
        if read == 0 { break; }
        header.extend_from_slice(&chunk[..read]);
    }
    let Ok(text) = std::str::from_utf8(&header) else {
        return write_response(&mut socket, 400, "application/json", error_body("请求头不是 UTF-8")).await;
    };
    let Some((method, target, headers)) = parse_request(text) else {
        return write_response(&mut socket, 400, "application/json", error_body("无法解析请求")).await;
    };
    if method != "GET" {
        return write_response(&mut socket, 405, "application/json", error_body("只接受 GET")).await;
    }
    let port = socket.local_addr()?.port();
    if !host_allowed(&headers, port) || cross_site(&headers, port) {
        return write_response(&mut socket, 403, "application/json", error_body("拒绝非本机来源")).await;
    }
    let (status, content_type, body) = dispatch(&state, target);
    write_response(&mut socket, status, content_type, body).await
}

fn parse_request(text: &str) -> Option<(&str, &str, Vec<(&str, &str)>)> {
    let (head, _) = text.split_once("\r\n\r\n")?;
    let mut lines = head.split("\r\n");
    let mut request = lines.next()?.split_whitespace();
    let method = request.next()?;
    let target = request.next()?;
    if request.next() != Some("HTTP/1.1") { return None; }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim(), value.trim()));
    }
    Some((method, target, headers))
}

fn host_allowed(headers: &[(&str, &str)], port: u16) -> bool {
    let Some(host) = header(headers, "host") else { return false; };
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn cross_site(headers: &[(&str, &str)], port: u16) -> bool {
    if header(headers, "sec-fetch-site").is_some_and(|value| value.eq_ignore_ascii_case("cross-site")) {
        return true;
    }
    let Some(origin) = header(headers, "origin") else { return false };
    origin != format!("http://127.0.0.1:{port}") && origin != format!("http://localhost:{port}")
}

fn header<'a>(headers: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| *value)
}

fn dispatch(state: &State, target: &str) -> (u16, &'static str, Vec<u8>) {
    let path = target.split('?').next().unwrap_or(target);
    if target.contains('?') || path.contains('%') {
        return json_error(400, "地址里不能带查询或编码");
    }
    match path {
        "/hg/boot" => json_ok(boot_json(state)),
        "/hg/feed" => match &state.fixture {
            Some(fixture) => json_ok(feed_json(fixture)),
            None => json_error(409, "没有配置播放后端，不能列出内容"),
        },
        "/hg/media" | "/hg/videos" => json_error(404, "没有这个地址"),
        _ if path.starts_with("/hg/videos/") => video_route(state, path),
        _ if path.starts_with("/hg/media/") => media_route(state, &path["/hg/media/".len()..]),
        "/" | "/web.html" => read_static(state, "web.html"),
        _ => read_static(state, path.trim_start_matches('/')),
    }
}

fn video_route(state: &State, path: &str) -> (u16, &'static str, Vec<u8>) {
    let Some(fixture) = &state.fixture else {
        return json_error(409, "没有配置播放后端，不能读取内容");
    };
    let mut parts = path.trim_start_matches("/hg/videos/").split('/');
    let Some(id) = parse_id(parts.next().unwrap_or("")) else {
        return json_error(404, "没有这个内容");
    };
    let Some(item) = fixture.items.iter().find(|item| item.id == id) else {
        return json_error(404, "没有这个内容");
    };
    match (parts.next(), parts.next(), parts.next()) {
        (None, None, None) => json_ok(detail_json(item)),
        (Some("episodes"), Some(episode), None) => {
            let Some(ep_num) = parse_id(episode) else { return json_error(404, "没有这一集"); };
            let Some(episode) = item.episodes.iter().find(|episode| episode.ep_num == ep_num) else {
                return json_error(404, "没有这一集");
            };
            if contained_file(&fixture.root, &episode.file).is_none() {
                return json_error(404, "这一集没有可播放的本地文件");
            }
            json_ok(json!({
                "id": item.id,
                "ep_num": episode.ep_num,
                "duration": episode.duration,
                "title": episode.title,
                "video_url": format!("/hg/media/{}", episode.file),
            }))
        }
        _ => json_error(404, "没有这个地址"),
    }
}

fn media_route(state: &State, name: &str) -> (u16, &'static str, Vec<u8>) {
    let Some(fixture) = &state.fixture else {
        return json_error(409, "没有配置播放后端，不能读取媒体");
    };
    if name == "catalog.json" || !safe_file_name(name) {
        return json_error(404, "没有这个媒体文件");
    }
    let Some(path) = contained_file(&fixture.root, name) else {
        return json_error(404, "没有这个媒体文件");
    };
    let Ok(bytes) = fs::read(&path) else { return json_error(404, "没有这个媒体文件"); };
    if bytes.len() > MAX_BODY { return json_error(413, "媒体文件太大"); }
    if name.ends_with(".m3u8") {
        let text = String::from_utf8_lossy(&bytes);
        return match rewrite_playlist(&text) {
            Ok(playlist) => (200, "application/vnd.apple.mpegurl", playlist.into_bytes()),
            Err(message) => json_error(422, message),
        };
    }
    let content_type = if name.ends_with(".ts") { "video/mp2t" } else { "application/octet-stream" };
    (200, content_type, bytes)
}

fn boot_json(state: &State) -> Value {
    match &state.fixture {
        Some(fixture) => json!({ "kind": "fixture", "label": fixture.label }),
        None => json!({
            "kind": "unconfigured",
            "message": "没有可用的播放后端。需要已授权的列表、详情和播放地址。本服务只监听本机，不会访问未知服务器，也不会自动改用示例内容。示例只能用 web:fixture 显式启动。",
        }),
    }
}

fn feed_json(fixture: &Fixture) -> Value {
    let items: Vec<Value> = fixture.items.iter().map(|item| json!({
        "id": item.id,
        "title": item.title,
        "episode_count": item.episodes.len(),
        "total_episodes": item.episodes.len(),
        "cover": "",
        "tags": [],
        "score": 0,
        "hot": 0,
        "like_count": 0,
        "collect_count": 0,
        "comment_count": 0,
        "is_finished": true,
        "is_original": 0,
        "description": fixture.label,
    })).collect();
    json!({ "items": items, "pagination": { "page": 1, "pages": 1, "size": items.len(), "total": items.len() } })
}

fn detail_json(item: &Item) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "episode_count": item.episodes.len(),
        "episodes": item.episodes.iter().map(|episode| json!({
            "ep_num": episode.ep_num,
            "title": episode.title,
            "duration": episode.duration,
        })).collect::<Vec<_>>(),
    })
}

fn safe_file_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    first.is_ascii_alphanumeric()
        && name.len() <= 80
        && name.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
        && !name.contains("..")
}

fn contained_file(root: &Path, name: &str) -> Option<PathBuf> {
    if !safe_file_name(name) { return None; }
    let candidate = root.join(name);
    let file = candidate.canonicalize().ok()?;
    if file.starts_with(root) && file.is_file() { Some(file) } else { None }
}

fn rewrite_playlist(text: &str) -> Result<String, &'static str> {
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
        } else if trimmed.contains("://") || trimmed.starts_with('/') || trimmed.contains("..") || !safe_file_name(trimmed) {
            return Err("播放列表里有不允许的地址");
        } else {
            out.push_str("/hg/media/");
            out.push_str(trimmed);
        }
        out.push('\n');
    }
    Ok(out)
}

fn parse_id(value: &str) -> Option<u32> {
    if value.is_empty() || value.len() > 9 || !value.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
    value.parse().ok().filter(|id| *id > 0)
}

fn json_ok(value: Value) -> (u16, &'static str, Vec<u8>) {
    (200, "application/json", value.to_string().into_bytes())
}

fn json_error(status: u16, message: &str) -> (u16, &'static str, Vec<u8>) {
    (status, "application/json", error_body(message))
}

fn error_body(message: &str) -> Vec<u8> {
    json!({ "kind": "error", "message": message }).to_string().into_bytes()
}

fn read_static(state: &State, relative: &str) -> (u16, &'static str, Vec<u8>) {
    if relative.is_empty() || relative.contains("..") || relative.contains('\\') || relative.contains(':') {
        return json_error(404, "没有这个页面");
    }
    let candidate = state.dist.join(relative);
    let Ok(path) = candidate.canonicalize() else { return json_error(404, "没有这个页面"); };
    if !path.starts_with(&state.dist) || !path.is_file() { return json_error(404, "没有这个页面"); }
    let Ok(bytes) = fs::read(&path) else { return json_error(404, "没有这个页面"); };
    if bytes.len() > MAX_BODY { return json_error(413, "页面文件太大"); }
    let content_type = match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    (200, content_type, bytes)
}

async fn write_response(socket: &mut TcpStream, status: u16, content_type: &str, body: Vec<u8>) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; media-src 'self' blob:; connect-src 'self'; img-src 'self'; worker-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'\r\n\r\n",
        body.len(),
    );
    socket.write_all(header.as_bytes()).await?;
    socket.write_all(&body).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn playlist_stays_on_local_names() {
        let rewritten = rewrite_playlist("#EXTM3U\nseg001.ts\n").unwrap();
        assert_eq!(rewritten, "#EXTM3U\n/hg/media/seg001.ts\n");
        assert!(rewrite_playlist("https://example.test/a.ts\n").is_err());
        assert!(rewrite_playlist("../secret.ts\n").is_err());
    }

    #[test]
    fn file_names_reject_traversal() {
        assert!(safe_file_name("ep1.m3u8"));
        assert!(!safe_file_name("../catalog.json"));
        assert!(!safe_file_name("/etc/passwd"));
        assert!(!safe_file_name(""));
    }

    #[tokio::test]
    async fn unconfigured_boot_and_cross_site_rejection() {
        let root = std::env::temp_dir().join(format!("hghubx-web-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/web.html"), b"<p>web</p>").unwrap();
        let state = Arc::new(load_state(root.join("dist"), None).unwrap());
        let listener = bind_loopback(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
        let (tx, shutdown) = oneshot::channel();
        let task = tokio::spawn(serve(listener, state, shutdown));
        let boot = http(port, &format!("GET /hg/boot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")).await;
        assert!(boot.contains("unconfigured"));
        assert!(!boot.contains("离线示例"));
        let feed = http(port, &format!("GET /hg/feed HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")).await;
        assert!(feed.starts_with("HTTP/1.1 409"));
        let foreign = http(port, &format!("GET /hg/boot HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n")).await;
        assert!(foreign.starts_with("HTTP/1.1 403"));
        let cross = http(port, &format!("GET /hg/boot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nSec-Fetch-Site: cross-site\r\nConnection: close\r\n\r\n")).await;
        assert!(cross.starts_with("HTTP/1.1 403"));
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        let _ = fs::remove_dir_all(root);
    }

    async fn http(port: u16, request: &str) -> String {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut buffer = Vec::new();
        loop {
            let mut chunk = [0u8; 1024];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(size) => buffer.extend_from_slice(&chunk[..size]),
            }
            if buffer.len() > MAX_HEADER + MAX_BODY { break; }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    #[test]
    fn symlink_outside_fixture_is_not_served() {
        let root = std::env::temp_dir().join(format!("hghubx-web-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let outside = std::env::temp_dir().join(format!("hghubx-web-outside-{}", std::process::id()));
        fs::write(&outside, b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("leak.txt")).unwrap();
        assert!(contained_file(&root.canonicalize().unwrap(), "leak.txt").is_none());
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }
}

