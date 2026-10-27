//! A loopback pmd stub that records whole requests, bodies included, and a folder with both token files. Shared by
//! the write client's and the work items' tests; never connects to the real pmd.
#![allow(dead_code)]
use dashboard_core::write::PmdWriter;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const READ: &str = "fixture-read-token-r3ad";
pub const WRITE: &str = "fixture-write-token-wr1te";

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn token(&self) -> &str {
        self.header("authorization").unwrap().strip_prefix("Bearer ").unwrap()
    }
}
pub struct Reply {
    pub code: u16,
    pub body: String,
    pub headers: Vec<(&'static str, String)>,
    pub delay: Duration,
}
impl Reply {
    pub fn json(code: u16, value: Value) -> Self {
        Self {
            code,
            body: value.to_string(),
            headers: Vec::new(),
            delay: Duration::ZERO,
        }
    }
    pub fn error(code: u16, error: &str, message: &str) -> Self {
        Self::json(code, json!({"error": {"code": error, "message": message}}))
    }
}

/// A loopback server that reads whole requests, bodies included, and records them.
pub struct Stub {
    pub url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Stub {
    pub fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, seen) = (stop.clone(), requests.clone());
        let handler = Arc::new(handler);
        let thread = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let (handler, seen) = (handler.clone(), seen.clone());
                        connections.push(std::thread::spawn(move || {
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut head = Vec::new();
                            while !head.ends_with(b"\r\n\r\n") && head.len() < 16384 {
                                let mut byte = [0];
                                if stream.read_exact(&mut byte).is_err() {
                                    return;
                                }
                                head.push(byte[0]);
                            }
                            let head = String::from_utf8(head).unwrap();
                            let mut lines = head.lines();
                            let mut start = lines.next().unwrap().split_whitespace();
                            let method = start.next().unwrap().to_owned();
                            let path = start.next().unwrap().to_owned();
                            let headers: Vec<(String, String)> = lines
                                .filter_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    Some((name.trim().to_owned(), value.trim().to_owned()))
                                })
                                .collect();
                            let length = headers
                                .iter()
                                .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                                .map_or(0, |(_, v)| v.parse().unwrap());
                            let mut body = vec![0; length];
                            stream.read_exact(&mut body).unwrap();
                            let request = Request {
                                method,
                                path,
                                headers,
                                body: String::from_utf8(body).unwrap(),
                            };
                            seen.lock().unwrap().push(request.clone());
                            let reply = handler(&request);
                            std::thread::sleep(reply.delay);
                            let extra: String = reply.headers.iter().map(|(n, v)| format!("{n}: {v}\r\n")).collect();
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Stub\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{}",
                                reply.code,
                                reply.body.len(),
                                reply.body
                            );
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("stub accept: {e}"),
                }
            }
            for thread in connections {
                let _ = thread.join();
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

/// A temporary folder holding both token files.
pub struct Dir(pub PathBuf);
impl Dir {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-write-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("pmd-token"), READ).unwrap();
        std::fs::write(path.join("pmd-write-token"), format!("{WRITE}\n")).unwrap();
        Self(path)
    }
    pub fn writer(&self, stub: &Stub) -> PmdWriter {
        PmdWriter {
            url: stub.url.clone(),
            read_token_path: self.0.join("pmd-token"),
            write_token_path: Some(self.0.join("pmd-write-token")),
            timeout: Duration::from_secs(5),
        }
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
