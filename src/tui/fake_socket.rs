//! Shared scriptable fake herdr socket for wire-level tests.
//!
//! A real Unix-domain-socket server speaking herdr's NDJSON protocol:
//! canned results per method, every request recorded, and pushed events
//! written to every open `events.subscribe` connection — in whatever frame
//! shape the test supplies, so tests can feed herdr's real wire shapes.
//! Additive: the inline fakes in `socket.rs` tests stay as they are.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    replies: HashMap<String, Value>,
    requests: Vec<Value>,
    /// Write halves of live subscription connections.
    subscribers: Vec<UnixStream>,
    /// Total subscriptions ever opened (monotonic, for waiting).
    subscriptions_opened: usize,
}

pub struct FakeHerdrSocket {
    _dir: tempfile::TempDir,
    path: PathBuf,
    shared: Arc<(Mutex<State>, Condvar)>,
    stop: Arc<AtomicBool>,
}

impl FakeHerdrSocket {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared2, stop2) = (shared.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { return };
                let shared = shared2.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        FakeHerdrSocket {
            _dir: dir,
            path,
            shared,
            stop,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Script the `result` returned for `method`. Unscripted methods get a
    /// herdr-style `not_found` error.
    pub fn reply(&self, method: &str, result: Value) {
        self.lock().replies.insert(method.to_string(), result);
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<Value> {
        self.lock().requests.clone()
    }

    /// Requests whose `method` matches.
    pub fn requests_for(&self, method: &str) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|r| r["method"] == method)
            .collect()
    }

    /// Block until at least `n` subscriptions have been opened in total.
    pub fn wait_for_subscriptions(&self, n: usize) {
        let (m, cv) = &*self.shared;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut st = m.lock().unwrap();
        while st.subscriptions_opened < n {
            let left = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for {n} subscriptions"));
            st = cv.wait_timeout(st, left).unwrap().0;
        }
    }

    /// Write one frame, verbatim, to every live subscription connection.
    pub fn push(&self, frame: Value) {
        let mut st = self.lock();
        st.subscribers
            .retain_mut(|w| writeln!(w, "{frame}").and_then(|_| w.flush()).is_ok());
    }

    /// End every live subscription (the client sees EOF).
    pub fn close_subscriptions(&self) {
        for s in self.lock().subscribers.drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for FakeHerdrSocket {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.close_subscriptions();
        // Wake the blocking accept so the server thread can exit.
        let _ = UnixStream::connect(&self.path);
    }
}

fn serve(stream: UnixStream, shared: &(Mutex<State>, Condvar)) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { return };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = req["id"].clone();
        let method = req["method"].as_str().unwrap_or_default().to_string();
        let (m, cv) = shared;
        let mut st = m.lock().unwrap();
        st.requests.push(req);
        if method == "events.subscribe" {
            let ack = json!({"id": id, "result": {"type": "subscription_started"}});
            if writeln!(writer, "{ack}").is_err() {
                return;
            }
            if let Ok(w) = writer.try_clone() {
                st.subscribers.push(w);
            }
            st.subscriptions_opened += 1;
            cv.notify_all();
            // herdr closes a connection that sends a second subscribe;
            // stop reading so this connection is push-only from here on.
            return;
        }
        let resp = match st.replies.get(&method) {
            Some(result) => json!({"id": id, "result": result}),
            None => json!({"id": id, "error": {"code": "not_found", "message": method}}),
        };
        drop(st);
        if writeln!(writer, "{resp}").is_err() {
            return;
        }
    }
}
