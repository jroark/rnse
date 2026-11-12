//! Browser front end.
//!
//! `rnse ui` runs the machine and serves a small page on a local port:
//! the unit's faceplate with the emulated screen behind its display
//! cut-out, the keys and knobs as click targets, and the firmware's debug
//! log underneath.  Everything is plain HTTP from the standard library;
//! the page polls for frames.
//!
//! The server only shuttles data between the page and [`Shared`]; the
//! thread that owns the [`Machine`](crate::machine::Machine) publishes
//! frames there and picks up input events.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const PAGE: &str = include_str!("../assets/ui.html");
const FACEPLATE: &[u8] = include_bytes!("../assets/faceplate.png");

/// Something the person at the page did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// A faceplate key went down or up.
    Key { name: String, down: bool },
    /// A knob was turned by `delta` detents (positive = clockwise).
    Knob { name: String, delta: i32 },
    /// Emulator-level controls.
    Ignition(bool),
    Disc(bool),
    Reset,
}

#[derive(Default)]
pub struct Shared {
    /// Latest frame, run-length encoded (see [`encode_frame`]).
    pub frame: Vec<u8>,
    pub frame_seq: u32,
    /// Pending input for the machine thread.
    pub events: VecDeque<UiEvent>,
    /// JSON object describing the machine state.
    pub status: String,
    /// Firmware debug output; `log_base` is the stream offset of `log[0]`.
    pub log: Vec<u8>,
    pub log_base: usize,
}

impl Shared {
    /// Append firmware output, keeping the last 256 KiB.
    pub fn push_log(&mut self, bytes: &[u8]) {
        self.log.extend_from_slice(bytes);
        const MAX: usize = 256 * 1024;
        if self.log.len() > MAX {
            let cut = self.log.len() - MAX;
            self.log.drain(..cut);
            self.log_base += cut;
        }
    }
}

/// Frame wire format: "RNSF", width and height (u16 LE), sequence number
/// (u32 LE), then runs of `[count, r, g, b]`.  User interface screens are
/// mostly flat colour, so this is small without needing a compressor.
pub fn encode_frame(w: usize, h: usize, rgb: &[u8], seq: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 4 + 16);
    out.extend_from_slice(b"RNSF");
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    let mut px = rgb.chunks_exact(3);
    if let Some(first) = px.next() {
        let mut cur = [first[0], first[1], first[2]];
        let mut run = 1u8;
        for p in px {
            if p == cur && run < 255 {
                run += 1;
            } else {
                out.push(run);
                out.extend_from_slice(&cur);
                cur = [p[0], p[1], p[2]];
                run = 1;
            }
        }
        out.push(run);
        out.extend_from_slice(&cur);
    }
    out
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{}\r\n",
        status,
        ctype,
        body.len(),
        extra
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn query<'a>(q: &'a str, key: &str) -> Option<&'a str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn handle(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, target) = match (parts.next(), parts.next()) {
            (Some(m), Some(t)) => (m.to_string(), t.to_string()),
            _ => return Ok(()),
        };
        let mut content_length = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        if content_length > 0 {
            let mut body = vec![0u8; content_length.min(65536)];
            reader.read_exact(&mut body)?;
        }
        let (path, q) = target.split_once('?').unwrap_or((target.as_str(), ""));
        match (method.as_str(), path) {
            ("GET", "/") => respond(&mut stream, "200 OK", "text/html; charset=utf-8", "", PAGE.as_bytes())?,
            ("GET", "/faceplate.png") => respond(&mut stream, "200 OK", "image/png", "", FACEPLATE)?,
            ("GET", "/frame") => {
                let since: u32 = query(q, "since").and_then(|v| v.parse().ok()).unwrap_or(u32::MAX);
                let body = {
                    let s = shared.lock().unwrap();
                    if s.frame.is_empty() || s.frame_seq == since {
                        None
                    } else {
                        Some(s.frame.clone())
                    }
                };
                match body {
                    Some(b) => respond(&mut stream, "200 OK", "application/octet-stream", "", &b)?,
                    None => respond(&mut stream, "204 No Content", "application/octet-stream", "", &[])?,
                }
            }
            ("GET", "/status") => {
                let body = shared.lock().unwrap().status.clone();
                respond(&mut stream, "200 OK", "application/json", "", body.as_bytes())?
            }
            ("GET", "/log") => {
                let from: usize = query(q, "from").and_then(|v| v.parse().ok()).unwrap_or(0);
                let (body, next) = {
                    let s = shared.lock().unwrap();
                    let start = from.max(s.log_base) - s.log_base;
                    let start = start.min(s.log.len());
                    (s.log[start..].to_vec(), s.log_base + s.log.len())
                };
                respond(&mut stream, "200 OK", "application/octet-stream", &format!("X-Next: {}\r\n", next), &body)?
            }
            ("POST", "/input") => {
                let ev = match query(q, "type") {
                    Some("key") => query(q, "name").map(|n| UiEvent::Key { name: n.to_string(), down: query(q, "down") == Some("1") }),
                    Some("knob") => query(q, "name").map(|n| UiEvent::Knob { name: n.to_string(), delta: query(q, "delta").and_then(|v| v.parse().ok()).unwrap_or(0) }),
                    Some("ignition") => Some(UiEvent::Ignition(query(q, "on") == Some("1"))),
                    Some("disc") => Some(UiEvent::Disc(query(q, "in") == Some("1"))),
                    Some("reset") => Some(UiEvent::Reset),
                    _ => None,
                };
                match ev {
                    Some(ev) => {
                        shared.lock().unwrap().events.push_back(ev);
                        respond(&mut stream, "200 OK", "text/plain", "", b"ok")?
                    }
                    None => respond(&mut stream, "400 Bad Request", "text/plain", "", b"bad input")?,
                }
            }
            _ => respond(&mut stream, "404 Not Found", "text/plain", "", b"not found")?,
        }
    }
}

/// Start serving on `addr` (e.g. "127.0.0.1:8080").  Returns the address
/// actually bound; connections are handled on background threads.
pub fn serve(addr: &str, shared: Arc<Mutex<Shared>>) -> std::io::Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let shared = shared.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, shared);
            });
        }
    });
    Ok(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encoding_round_trips() {
        let mut rgb = Vec::new();
        for i in 0..600usize {
            let c = if i < 300 { [1, 2, 3] } else { [(i % 7) as u8, 9, 9] };
            rgb.extend_from_slice(&c);
        }
        let enc = encode_frame(30, 20, &rgb, 7);
        assert_eq!(&enc[..4], b"RNSF");
        assert_eq!(u16::from_le_bytes([enc[4], enc[5]]), 30);
        assert_eq!(u32::from_le_bytes([enc[8], enc[9], enc[10], enc[11]]), 7);
        let mut dec = Vec::new();
        for run in enc[12..].chunks_exact(4) {
            for _ in 0..run[0] {
                dec.extend_from_slice(&run[1..]);
            }
        }
        assert_eq!(dec, rgb);
        // 300 identical pixels need two runs.
        assert_eq!(&enc[12..20], &[255, 1, 2, 3, 45, 1, 2, 3]);
    }

    #[test]
    fn query_parsing() {
        assert_eq!(query("type=key&name=NAV&down=1", "name"), Some("NAV"));
        assert_eq!(query("type=key", "down"), None);
    }

    #[test]
    fn serves_page_frames_and_input() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let addr = serve("127.0.0.1:0", shared.clone()).unwrap();
        let get = |req: &str| -> Vec<u8> {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            out
        };
        let page = get("GET / HTTP/1.1\r\n\r\n");
        assert!(page.starts_with(b"HTTP/1.1 200 OK"));
        assert!(String::from_utf8_lossy(&page).contains("faceplate.png"));

        assert!(get("GET /frame?since=0 HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 204"));
        {
            let mut s = shared.lock().unwrap();
            s.frame = encode_frame(2, 1, &[1, 1, 1, 2, 2, 2], 5);
            s.frame_seq = 5;
        }
        let f = get("GET /frame?since=4 HTTP/1.1\r\n\r\n");
        assert!(f.starts_with(b"HTTP/1.1 200 OK"));
        assert!(f.ends_with(&[1, 1, 1, 1, 1, 2, 2, 2]));
        assert!(get("GET /frame?since=5 HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 204"));

        assert!(get("POST /input?type=key&name=NAV&down=1 HTTP/1.1\r\nContent-Length: 0\r\n\r\n").starts_with(b"HTTP/1.1 200 OK"));
        assert!(get("POST /input?type=knob&name=main&delta=-2 HTTP/1.1\r\nContent-Length: 0\r\n\r\n").starts_with(b"HTTP/1.1 200 OK"));
        assert!(get("POST /input?type=nonsense HTTP/1.1\r\nContent-Length: 0\r\n\r\n").starts_with(b"HTTP/1.1 400"));
        let s = shared.lock().unwrap();
        assert_eq!(s.events[0], UiEvent::Key { name: "NAV".into(), down: true });
        assert_eq!(s.events[1], UiEvent::Knob { name: "main".into(), delta: -2 });
    }
}
