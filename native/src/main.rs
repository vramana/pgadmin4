//! pgAdmin 4's native desktop shell. It starts the Flask server as a sidecar, as the Electron
//! runtime in `runtime/` does, and shows its state in a Rune window. The native workspace (object
//! browser, Query Tool) is ported onto this shell screen by screen; until then "Open in browser"
//! hands the running server to the default browser.

mod sidecar;

use std::collections::VecDeque;
use std::process::Child;
use std::sync::{Mutex, OnceLock};

use rune::tok::{self, type_};
use rune::{App, AppHandle, AppSpec, Cx, El, ScrollSpec, Tx, col, el, row, text};

use sidecar::{Paths, Server};

const LOG_LINES: usize = 400;

#[derive(Debug, Default, PartialEq)]
enum Phase {
    #[default]
    Starting,
    Ready { url: String, millis: u128 },
    Failed { reason: String },
}

#[derive(Default)]
struct Ui {
    phase: Phase,
    /// Which launch the phase belongs to; answers from an older launch are dropped.
    attempt: u64,
    log: VecDeque<String>,
}

impl Ui {
    fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES { self.log.pop_front(); }
        self.log.push_back(line);
    }
}

/// The running server process and the handle that feeds the window, shared with launch threads.
struct Control {
    handle: AppHandle<Ui>,
    child: Mutex<Option<Child>>,
}

static CONTROL: OnceLock<Control> = OnceLock::new();

impl Control {
    fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Starts (or restarts) the server for `attempt` on a background thread.
    fn launch(&'static self, attempt: u64) {
        self.stop();
        std::thread::spawn(move || {
            let report = |phase: Phase| self.handle.update(move |s: &mut Ui, _| if s.attempt == attempt { s.phase = phase });
            let log = move |line: String| self.handle.update(move |s: &mut Ui, _| s.push_log(line));
            let paths = Paths::locate();
            let server = match sidecar::pick_port(None) {
                Ok(port) => Server::new(port),
                Err(e) => return report(Phase::Failed { reason: format!("no free port: {e}") }),
            };
            log(format!("Python: {}", paths.python.display()));
            log(format!("pgAdmin: {}", paths.pgadmin.display()));
            log(format!("Port: {}", server.port));
            let mut child = match sidecar::spawn(&paths, &server, log) {
                Ok(child) => child,
                Err(e) => return report(Phase::Failed { reason: format!("could not start {}: {e}", paths.python.display()) }),
            };
            let result = sidecar::wait_ready(&mut child, &server, sidecar::START_TIMEOUT);
            *self.child.lock().unwrap() = Some(child);
            match result {
                Ok(took) => report(Phase::Ready { url: server.start_url(), millis: took.as_millis() }),
                Err(reason) => { self.stop(); report(Phase::Failed { reason }) }
            }
        });
    }
}

fn retry(s: &mut Ui, _: &mut Tx) {
    s.attempt += 1;
    s.phase = Phase::Starting;
    s.log.clear();
    if let Some(control) = CONTROL.get() { control.launch(s.attempt); }
}

fn button(key: &'static str, label: &'static str, dark: bool) -> El {
    let t = tok::theme(dark);
    el().key(key).role("button").label(label).focusable(true)
        .px(14.0).padding(8.0).radius(6.0).fill(tok::accent(0, dark))
        .child(text(label).font(type_::LABEL).color(t.bg))
}

fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.env().system_dark;
    let t = tok::theme(dark);
    let (status, detail) = match &s.phase {
        Phase::Starting => ("Starting pgAdmin 4…".to_string(), String::new()),
        Phase::Ready { url, millis } => ("pgAdmin 4 is running".to_string(), format!("Server ready in {millis} ms at {}", url.split("/?").next().unwrap_or(url))),
        Phase::Failed { reason } => ("pgAdmin 4 could not start".to_string(), reason.clone()),
    };
    let mut actions = row().gap(8.0);
    match &s.phase {
        Phase::Ready { .. } => {
            actions = actions.child(button("open-browser", "Open in browser", dark).on_press_tx(|s: &mut Ui, tx: &mut Tx| {
                if let Phase::Ready { url, .. } = &s.phase { let _ = tx.open_url(url); }
            }));
        }
        Phase::Failed { .. } => actions = actions.child(button("retry", "Retry", dark).on_press_tx(retry)),
        Phase::Starting => {}
    }
    let lines = s.log.iter().enumerate().fold(col().gap(2.0), |c, (i, line)| {
        c.child(text(line.as_str()).key(format!("log-{i}")).font(type_::BODY.mono()).color(t.text_secondary))
    });
    let log = col().key("log").w_full().grow(1.0).min_h(0.0).padding(10.0).radius(8.0)
        .fill(t.card).border(1.0, t.card_border)
        .scroll(ScrollSpec { stick_bottom: true }).child(lines);
    col().w_full().h_full().padding(20.0).gap(12.0).fill(t.bg)
        .child(text(status).key("status").font(type_::TITLE).color(t.text))
        .child(text(detail).key("detail").font(type_::BODY).color(t.text_secondary))
        .child(actions)
        .child(log)
}

fn main() {
    let spec = AppSpec::<Ui> {
        min_size: Some((480.0, 360.0)),
        scenes: &[
            ("ready", |s| { s.phase = Phase::Ready { url: "http://127.0.0.1:5050/?key=k".into(), millis: 2400 }; s.push_log("Starting pgAdmin 4. Please navigate to http://127.0.0.1:5050 in your browser.".into()); }),
            ("failed", |s| { s.phase = Phase::Failed { reason: "the pgAdmin server exited during startup (exit status: 1)".into() }; s.push_log("ModuleNotFoundError: No module named 'flask'".into()); }),
        ],
        ..AppSpec::new("pgAdmin 4", (720.0, 480.0), Ui::default, view)
    };
    let app = App::new(spec).on_close(|_, _| { if let Some(c) = CONTROL.get() { c.stop(); } });
    let control = CONTROL.get_or_init(|| Control { handle: app.handle(), child: Mutex::new(None) });
    if !headless() { control.launch(0); }
    app.run();
    control.stop();
}

/// Headless runs (`--snap`, `--tree`, scripts) draw scenes and must not start a server.
fn headless() -> bool {
    std::env::args().any(|a| a.starts_with("--snap") || a.starts_with("--tree") || a.starts_with("--script"))
}
