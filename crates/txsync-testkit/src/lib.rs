//! A tiny blocking HTTP mock server.
//!
//! `txsync`'s transports are synchronous (`reqwest::blocking`), so the tests use
//! a synchronous mock instead of pulling an async runtime in as a dev
//! dependency. Each route matches on method plus a path prefix and serves its
//! responses in order; the last response repeats, so "always 200" needs one
//! entry.
//!
//! Requests are recorded so a test can assert on the exact body a sink sent —
//! which is the only way to catch a currency conversion bug.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
    pub content_type: &'static str,
}

impl Response {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            content_type: "application/json",
        }
    }

    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            content_type: "application/json",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// `path` without the query string.
    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }
}

struct Route {
    method: String,
    path_prefix: String,
    responses: VecDeque<Response>,
}

#[derive(Default)]
struct State {
    routes: Vec<Route>,
    requests: Vec<Request>,
}

pub struct MockServer {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        listener
            .set_nonblocking(true)
            .expect("mock server nonblocking");

        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_state = Arc::clone(&state);
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle_connection(stream, &thread_state);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            state,
            stop,
            handle: Some(handle),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Register a route. Responses are served in order; the last one repeats.
    pub fn route(&self, method: &str, path_prefix: &str, responses: Vec<Response>) -> &Self {
        let mut state = self.state.lock().expect("mock server state");
        state.routes.push(Route {
            method: method.to_ascii_uppercase(),
            path_prefix: path_prefix.to_owned(),
            responses: responses.into(),
        });
        drop(state);
        self
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state
            .lock()
            .expect("mock server state")
            .requests
            .clone()
    }

    pub fn request_count(&self) -> usize {
        self.state.lock().expect("mock server state").requests.len()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, state: &Arc<Mutex<State>>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_nodelay(true)?;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];

    let header_end = loop {
        if let Some(position) = find_subslice(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_ascii_uppercase();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    let (path_only, query) = match path.split_once('?') {
        Some((path, query)) => (path.to_owned(), query.to_owned()),
        None => (path.clone(), String::new()),
    };

    let request = Request {
        method: method.clone(),
        path: path_only.clone(),
        query,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    };

    let response = {
        let mut state = state.lock().expect("mock server state");
        state.requests.push(request);
        let route = state
            .routes
            .iter_mut()
            .find(|route| route.method == method && path_only.starts_with(&route.path_prefix));
        match route {
            Some(route) if !route.responses.is_empty() => {
                if route.responses.len() > 1 {
                    route.responses.pop_front().expect("queued response")
                } else {
                    route.responses.front().cloned().expect("queued response")
                }
            }
            _ => Response::status(404, r#"{"error":"no route"}"#),
        }
    };

    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    };

    let payload = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        response.body
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
