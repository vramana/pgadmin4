//! Launches the pgAdmin Flask server as a child process, as `runtime/src/js/pgadmin.js` does:
//! pick a port, make a one-time key, spawn `python -s pgAdmin4.py` with them in the
//! environment, and ping `/misc/ping` until it answers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long the server may take to answer its first ping (Electron's `connectionTimeout` default).
pub const START_TIMEOUT: Duration = Duration::from_secs(180);

/// Where the Python interpreter and `pgAdmin4.py` live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub python: PathBuf,
    pub pgadmin: PathBuf,
}

impl Paths {
    /// `PGADMIN_PYTHON` and `PGADMIN_WEB_FILE` win; otherwise a development checkout is assumed:
    /// `python3` on PATH and `web/pgAdmin4.py` next to this crate.
    pub fn locate() -> Paths {
        let python = std::env::var_os("PGADMIN_PYTHON").map(PathBuf::from).unwrap_or_else(|| {
            PathBuf::from(if cfg!(windows) { "python.exe" } else { "python3" })
        });
        let pgadmin = std::env::var_os("PGADMIN_WEB_FILE").map(PathBuf::from).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/pgAdmin4.py")
        });
        Paths { python, pgadmin }
    }
}

/// One launch of the server: its address, its key and the URLs built from them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub port: u16,
    pub key: String,
}

impl Server {
    pub fn new(port: u16) -> Server {
        Server { port, key: uuid::Uuid::new_v4().to_string() }
    }
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    /// The page that logs the desktop user in (the key is checked once, then a session cookie).
    pub fn start_url(&self) -> String {
        format!("{}/?key={}", self.base_url(), self.key)
    }
    pub fn ping_path(&self) -> String {
        format!("/misc/ping?key={}", self.key)
    }
}

/// A free port on 127.0.0.1 from the OS, or `fixed` when the user pinned one and it is free.
pub fn pick_port(fixed: Option<u16>) -> std::io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, fixed.unwrap_or(0)))?;
    Ok(listener.local_addr()?.port())
}

/// Spawns the server with its stdout and stderr piped; `on_line` gets every line from either.
pub fn spawn(paths: &Paths, server: &Server, on_line: impl Fn(String) + Send + Clone + 'static) -> std::io::Result<Child> {
    let mut command = Command::new(&paths.python);
    command
        .arg("-s")
        .arg(&paths.pgadmin)
        .env("PGADMIN_INT_PORT", server.port.to_string())
        .env("PGADMIN_INT_KEY", &server.key)
        .env("PGADMIN_SERVER_MODE", "OFF")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if cfg!(target_os = "macos") {
        // Finder and Dock launches get a minimal PATH without Homebrew's directories.
        command.env("PATH", with_dirs(std::env::var("PATH").unwrap_or_default(), &["/usr/local/bin", "/opt/homebrew/bin"]));
    }
    let mut child = command.spawn()?;
    if let Some(out) = child.stdout.take() { forward(out, on_line.clone()); }
    if let Some(err) = child.stderr.take() { forward(err, on_line); }
    Ok(child)
}

fn forward(stream: impl Read + Send + 'static, on_line: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            on_line(line);
        }
    });
}

fn with_dirs(path: String, dirs: &[&str]) -> String {
    let mut parts: Vec<&str> = path.split(':').filter(|p| !p.is_empty()).collect();
    for dir in dirs {
        if !parts.contains(dir) { parts.push(dir); }
    }
    parts.join(":")
}

/// True when `GET path` on 127.0.0.1:port answers 200.
pub fn ping(port: u16, path: &str) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() { return false; }
    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status).is_ok() && is_ok_status(&status)
}

fn is_ok_status(line: &str) -> bool {
    line.split_whitespace().nth(1) == Some("200")
}

/// Pings until the server answers, the child exits, or `timeout` passes. The interval starts at
/// 100 ms and doubles up to 1 s, as the Electron runtime's adaptive polling does.
pub fn wait_ready(child: &mut Child, server: &Server, timeout: Duration) -> Result<Duration, String> {
    let start = Instant::now();
    let mut interval = Duration::from_millis(100);
    loop {
        if ping(server.port, &server.ping_path()) { return Ok(start.elapsed()); }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the pgAdmin server exited during startup ({status})"));
        }
        if start.elapsed() >= timeout {
            return Err(format!("the pgAdmin server did not answer within {} s", timeout.as_secs()));
        }
        std::thread::sleep(interval);
        interval = (interval * 2).min(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_carry_port_and_key() {
        let s = Server { port: 5050, key: "k".into() };
        assert_eq!(s.start_url(), "http://127.0.0.1:5050/?key=k");
        assert_eq!(s.ping_path(), "/misc/ping?key=k");
    }

    #[test]
    fn keys_differ_per_launch() {
        assert_ne!(Server::new(1).key, Server::new(1).key);
    }

    #[test]
    fn picks_a_free_port() {
        let port = pick_port(None).unwrap();
        assert!(port > 0);
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
    }

    #[test]
    fn status_line_parsing() {
        assert!(is_ok_status("HTTP/1.1 200 OK\r\n"));
        assert!(!is_ok_status("HTTP/1.1 401 UNAUTHORIZED\r\n"));
        assert!(!is_ok_status(""));
    }

    #[test]
    fn homebrew_dirs_are_appended_once() {
        assert_eq!(with_dirs("/usr/bin:/opt/homebrew/bin".into(), &["/usr/local/bin", "/opt/homebrew/bin"]), "/usr/bin:/opt/homebrew/bin:/usr/local/bin");
    }

    #[test]
    fn ping_answers_true_only_for_200() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, stream) in listener.incoming().take(2).enumerate() {
                let mut stream = stream.unwrap();
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body = if i == 0 { "HTTP/1.1 200 OK\r\n\r\nPING" } else { "HTTP/1.1 401 UNAUTHORIZED\r\n\r\n" };
                stream.write_all(body.as_bytes()).unwrap();
            }
        });
        assert!(ping(port, "/misc/ping?key=a"));
        assert!(!ping(port, "/misc/ping?key=b"));
    }

    /// Starts the real server from this checkout. Needs pgAdmin's Python requirements:
    /// `PGADMIN_PYTHON=/path/to/venv/bin/python cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn launches_the_real_server() {
        let server = Server::new(pick_port(None).unwrap());
        let mut child = spawn(&Paths::locate(), &server, |line| eprintln!("server: {line}")).unwrap();
        let ready = wait_ready(&mut child, &server, Duration::from_secs(120));
        let _ = child.kill();
        let took = ready.unwrap();
        eprintln!("ready in {} ms", took.as_millis());
    }
}
