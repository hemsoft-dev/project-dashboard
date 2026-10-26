//! Shared loopback HTTP fixture. Accepted streams are explicitly blocking on Windows and Linux.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub struct Reply {
    pub code: u16,
    pub body: String,
}
pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        Self::with_auth(move |path, token| {
            assert_eq!(token, "fixture-read-token", "fixture bearer auth required");
            handler(path)
        })
    }

    pub fn with_auth(handler: impl Fn(&str, &str) -> Reply + Send + Sync + 'static) -> Self {
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
                        // Winsock accept inherits the listener's non-blocking mode; read_exact needs blocking I/O.
                        stream.set_nonblocking(false).unwrap();
                        let handler = handler.clone();
                        let seen = seen.clone();
                        connections.push(std::thread::spawn(move || {
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                                let mut byte = [0];
                                if stream.read_exact(&mut byte).is_err() {
                                    return;
                                }
                                request.push(byte[0]);
                            }
                            let request = String::from_utf8(request).unwrap();
                            let token = request
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("authorization")
                                        .then(|| value.trim().strip_prefix("Bearer ").expect("bearer scheme"))
                                })
                                .expect("fixture bearer auth required");
                            let path = request
                                .lines()
                                .next()
                                .unwrap()
                                .split_whitespace()
                                .nth(1)
                                .unwrap()
                                .to_owned();
                            seen.lock().unwrap().push(path.clone());
                            let reply = handler(&path, token);
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                reply.code,
                                reply.body.len(),
                                reply.body
                            );
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
            for thread in connections {
                thread.join().unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}
