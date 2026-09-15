//! PTY integration tests (PLAN.md §14.3, cfg(unix)): spawn the app in a
//! pseudo-terminal via `portable-pty` and prove terminal politeness —
//! alternate screen + cursor restore on every exit path, exit codes for
//! quit / Ctrl-C / SIGTERM, resize repaint, negligible idle CPU, and
//! SIGTSTP/SIGCONT job control.
//!
//! Robustness: output is mirrored by a reader thread into a shared buffer;
//! every wait polls that buffer on a short cadence with generous deadlines
//! (10 s), and children are always reaped — SIGKILLed if still alive when a
//! deadline passes — so a failing expectation fails the test instead of
//! hanging it. The app's signal-flag latency is ≤ 1 s by design (PLAN §9),
//! which the timeouts absorb.

#![cfg(unix)]

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const COLS: u16 = 100;
const ROWS: u16 = 30;
const POLL: Duration = Duration::from_millis(50);
const TIMEOUT: Duration = Duration::from_secs(10);

// ------------------------------------------------------------- helpers

/// True if `bytes` contains any braille pattern char (U+2800..=U+28FF).
fn has_braille(bytes: &[u8]) -> bool {
    bytes
        .windows(3)
        .any(|w| w[0] == 0xE2 && (0xA0..=0xA3).contains(&w[1]) && (0x80..=0xBF).contains(&w[2]))
}

fn count_of(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// A short, printable tail of the collected output for failure messages.
fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(240);
    String::from_utf8_lossy(&bytes[start..]).replace('\x1b', "ESC")
}

fn assert_restore_sequences(out: &[u8], ctx: &str) {
    assert!(
        count_of(out, b"\x1b[?1049l") >= 1,
        "{ctx}: expected LeaveAlternateScreen (ESC[?1049l) in output; tail: {}",
        tail(out)
    );
    assert!(
        count_of(out, b"\x1b[?25h") >= 1,
        "{ctx}: expected cursor Show (ESC[?25h) in output; tail: {}",
        tail(out)
    );
}

/// Deliver a signal to `pid` via the external `kill(1)` utility (kept out of
/// the dependency tree; no libc crate needed).
fn kill_signal(pid: u32, sig: &str) {
    let status = Command::new("kill")
        .arg("-s")
        .arg(sig)
        .arg(pid.to_string())
        .status()
        .unwrap_or_else(|e| panic!("failed to run kill -s {sig} {pid}: {e}"));
    assert!(status.success(), "kill -s {sig} {pid} failed: {status}");
}

/// Length of the SGR escape sequence starting at `b` (`ESC [ params m`),
/// or None if this is some other escape sequence.
fn sgr_len(b: &[u8]) -> Option<usize> {
    if b.len() < 3 || b[0] != 0x1b || b[1] != b'[' {
        return None;
    }
    let mut i = 2;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b';') {
        i += 1;
    }
    (i < b.len() && b[i] == b'm').then_some(i + 1)
}

/// Rows of the most recent *full* frame (`ESC[H` + rows joined by CRLF, as
/// emitted by `frame::render_ansi` with `prev == None`) in the raw pty
/// output, with SGR attribute sequences stripped. A frame is delimited by
/// the next non-SGR escape sequence (a clock diff's cursor move, a mode
/// change, the following frame's `ESC[H`) or by the end of the buffer.
/// Callers must match on exact row count and width so a frame that is still
/// being written can never satisfy the expectation.
fn last_full_frame_rows(buf: &[u8]) -> Option<Vec<String>> {
    let mut frames: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut rows: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut in_frame = false;
    let mut i = 0;
    while i < buf.len() {
        if !in_frame {
            if buf[i..].starts_with(b"\x1b[H") {
                in_frame = true;
                rows.clear();
                cur.clear();
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        if buf[i..].starts_with(b"\r\n") {
            rows.push(std::mem::take(&mut cur));
            i += 2;
            continue;
        }
        if buf[i] == 0x1b {
            if let Some(len) = sgr_len(&buf[i..]) {
                i += len; // style change inside the frame: not content
                continue;
            }
            // Any other escape sequence delimits the end of a full frame.
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            frames.push(std::mem::take(&mut rows));
            in_frame = false;
            continue; // re-examine this escape outside frame mode
        }
        cur.push(buf[i]);
        i += 1;
    }
    if in_frame {
        if !cur.is_empty() {
            rows.push(cur);
        }
        frames.push(rows);
    }
    frames.last().map(|rows| {
        rows.iter()
            .map(|r| String::from_utf8_lossy(r).into_owned())
            .collect()
    })
}

// -------------------------------------------------------------- session

/// A running app instance in a pty, its output mirrored into a shared buffer
/// by a reader thread (which also records EOF/EIO so tests can drain before
/// reaping the child).
struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Arc<Mutex<Vec<u8>>>,
    reader_done: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
    reaped: bool,
}

impl Session {
    fn launch(args: &[&str]) -> Session {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_daylight"));
        cmd.args(args);
        cmd.env("TERM", "xterm-256color"); // other env is inherited
        let child = pair
            .slave
            .spawn_command(cmd)
            .expect("spawn daylight in pty");
        // Drop our slave end so the master read hits EOF/EIO when the child
        // exits (otherwise it would block forever).
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().expect("clone pty reader");
        let writer = pair.master.take_writer().expect("take pty writer");
        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_done = Arc::new(AtomicBool::new(false));
        let sink = Arc::clone(&output);
        let done = Arc::clone(&reader_done);
        let handle = std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break, // EOF (or EIO once the child is gone)
                    Ok(n) => sink
                        .lock()
                        .expect("output lock")
                        .extend_from_slice(&buf[..n]),
                }
            }
            done.store(true, Ordering::SeqCst);
        });
        Session {
            master: pair.master,
            writer,
            child,
            output,
            reader_done,
            reader: Some(handle),
            reaped: false,
        }
    }

    fn output(&self) -> Vec<u8> {
        self.output.lock().expect("output lock").clone()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().expect("flush pty writer");
    }

    fn pid(&self) -> u32 {
        self.child.process_id().expect("child pid")
    }

    /// Wait for the reader to see EOF (all output drained), then reap the
    /// child and return its exit code. If it is still alive when `timeout`
    /// expires, it is SIGKILLed so the test fails instead of hanging.
    fn exit_code(&mut self, timeout: Duration) -> u32 {
        let deadline = Instant::now() + timeout;
        while !self.reader_done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        if !self.reader_done.load(Ordering::SeqCst) {
            let _ = self.child.kill(); // fail fast, do not hang the suite
        }
        let status = self.child.wait().expect("reap pty child");
        self.reaped = true;
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        status.exit_code()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Never leave a child (or a blocked reader) behind, even on panic.
        if !self.reaped {
            let _ = self.child.kill();
        }
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

/// Poll the shared output until `pred` returns Some, or the deadline passes.
fn wait_until<T>(
    s: &Session,
    mut pred: impl FnMut(&[u8]) -> Option<T>,
    timeout: Duration,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        let buf = s.output();
        if let Some(v) = pred(&buf) {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

fn wait_for(s: &Session, needle: &[u8], timeout: Duration) -> bool {
    wait_until(s, |buf| (count_of(buf, needle) > 0).then_some(()), timeout).is_some()
}

fn wait_for_count(s: &Session, needle: &[u8], n: usize, timeout: Duration) -> bool {
    wait_until(s, |buf| (count_of(buf, needle) >= n).then_some(()), timeout).is_some()
}

// --------------------------------------------------------------- tests

#[test]
fn quit_restores_terminal() {
    let mut s = Session::launch(&[]);
    assert!(
        wait_for(&s, b"\x1b[?1049h", TIMEOUT),
        "no EnterAlternateScreen at startup; tail: {}",
        tail(&s.output())
    );
    assert!(
        has_braille(&s.output()),
        "no braille glyphs in the first frame; tail: {}",
        tail(&s.output())
    );

    s.send(b"q");
    let code = s.exit_code(TIMEOUT);
    assert_eq!(code, 0, "'q' must exit cleanly with code 0");
    assert_restore_sequences(&s.output(), "quit");
}

#[test]
fn ctrl_c_byte_exits_130() {
    // PLAN §9/§14.3: raw mode turns off ISIG, so the Ctrl-C byte arrives as
    // a key event that must take the same path as an external SIGINT:
    // restore the terminal, then exit with the conventional 128+2 = 130.
    let mut s = Session::launch(&[]);
    assert!(
        wait_for(&s, b"\x1b[?1049h", TIMEOUT),
        "app did not start; tail: {}",
        tail(&s.output())
    );

    s.send(&[0x03]); // Ctrl-C
    let code = s.exit_code(TIMEOUT);
    assert_restore_sequences(&s.output(), "ctrl-c");
    assert_eq!(
        code, 130,
        "Ctrl-C must exit 130 (128+SIGINT, PLAN §14.3); the app returned {code}"
    );
}

#[test]
fn sigterm_exits_143() {
    let mut s = Session::launch(&[]);
    assert!(
        wait_for(&s, b"\x1b[?1049h", TIMEOUT),
        "app did not start; tail: {}",
        tail(&s.output())
    );

    kill_signal(s.pid(), "TERM");
    let code = s.exit_code(TIMEOUT);
    assert_restore_sequences(&s.output(), "sigterm");
    assert_eq!(
        code, 143,
        "SIGTERM must exit 143 (128+15, PLAN §11); the app returned {code}"
    );
}

#[test]
fn resize_repaints_at_new_size() {
    let mut s = Session::launch(&[]);

    // First full frame at the initial 100x30: every row exactly `COLS` wide.
    let w = COLS as usize;
    let h = ROWS as usize;
    let first = wait_until(
        &s,
        |buf| {
            last_full_frame_rows(buf)
                .filter(|rows| rows.len() == h && rows.iter().all(|r| r.chars().count() == w))
        },
        TIMEOUT,
    )
    .unwrap_or_else(|| {
        panic!(
            "no complete {COLS}x{ROWS} first frame; tail: {}",
            tail(&s.output())
        )
    });
    assert!(
        first
            .iter()
            .any(|r| r.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))),
        "first frame must contain braille"
    );

    // Resize the pty; the app must repaint at the new size without crashing.
    s.master
        .resize(PtySize {
            rows: 40,
            cols: 130,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize pty master to 130x40");
    let second = wait_until(
        &s,
        |buf| {
            last_full_frame_rows(buf)
                .filter(|rows| rows.len() == 40 && rows.iter().all(|r| r.chars().count() == 130))
        },
        TIMEOUT,
    )
    .unwrap_or_else(|| {
        panic!(
            "no complete 130x40 repaint after resize; tail: {}",
            tail(&s.output())
        )
    });
    assert!(
        second
            .iter()
            .any(|r| r.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))),
        "resized frame must still contain braille"
    );

    // And still quits cleanly afterwards.
    s.send(b"q");
    assert_eq!(s.exit_code(TIMEOUT), 0, "must quit cleanly after a resize");
}

/// utime+stime (in clock ticks) of `pid` from /proc/<pid>/stat.
#[cfg(target_os = "linux")]
fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

#[test]
#[cfg(target_os = "linux")]
fn idle_cpu_is_negligible() {
    // PLAN §10/§14.3: the event loop blocks in poll — under 100 ms of CPU
    // over a 2.5 s idle window. /proc ticks are 100/s on stock kernels, so
    // the budget is 10 ticks.
    let mut s = Session::launch(&[]);
    assert!(
        wait_for(&s, b"\x1b[?1049h", TIMEOUT),
        "app did not start; tail: {}",
        tail(&s.output())
    );

    let pid = s.pid();
    let before = cpu_ticks(pid).expect("read /proc/<pid>/stat before the idle window");
    std::thread::sleep(Duration::from_millis(2500));
    let after = cpu_ticks(pid).expect("read /proc/<pid>/stat after the idle window");
    let delta = after.saturating_sub(before);
    assert!(
        delta <= 10,
        "idle CPU over 2.5 s was {delta} ticks (>= 100 ms at 100 Hz) — busy loop?"
    );

    s.send(b"q");
    assert_eq!(s.exit_code(TIMEOUT), 0);
}

/// Process state character from /proc/<pid>/stat ("T" = stopped).
#[cfg(target_os = "linux")]
fn proc_state(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().next().map(str::to_string)
}

#[test]
fn sigtstp_sigcont_roundtrip() {
    // SIGTSTP: restore the terminal and stop (job control); SIGCONT:
    // re-init the screen and repaint; the app still quits cleanly after.
    let mut s = Session::launch(&[]);
    assert!(
        wait_for(&s, b"\x1b[?1049h", TIMEOUT),
        "app did not start; tail: {}",
        tail(&s.output())
    );
    assert!(has_braille(&s.output()), "first frame must contain braille");

    let pid = s.pid();
    kill_signal(pid, "TSTP");

    // The loop notices the signal flag within ~1 s (PLAN §9), then restores
    // the terminal before raising the default SIGTSTP.
    assert!(
        wait_for(&s, b"\x1b[?1049l", TIMEOUT),
        "no terminal restore on SIGTSTP; tail: {}",
        tail(&s.output())
    );
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if proc_state(pid).as_deref() == Some("T") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child was not actually stopped after SIGTSTP (state: {:?})",
                proc_state(pid)
            );
            std::thread::sleep(POLL);
        }
    }

    kill_signal(pid, "CONT");
    // Resume re-enters the alternate screen and forces a full repaint.
    assert!(
        wait_for_count(&s, b"\x1b[?1049h", 2, TIMEOUT),
        "no alt-screen re-enter after SIGCONT; tail: {}",
        tail(&s.output())
    );
    assert!(
        wait_for_count(&s, b"\x1b[H", 2, TIMEOUT),
        "no full repaint after SIGCONT; tail: {}",
        tail(&s.output())
    );

    s.send(b"q");
    assert_eq!(
        s.exit_code(TIMEOUT),
        0,
        "must still quit cleanly after suspend/resume"
    );
    assert_restore_sequences(&s.output(), "quit after suspend/resume");
}
