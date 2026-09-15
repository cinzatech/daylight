//! Application state, run modes, and the event loop.
//!
//! Event-driven, no busy loops: while idle the process is blocked in
//! `crossterm::event::poll` (or a sleep to the next second when there is no
//! usable terminal input). Frames are only rebuilt when the minute (shading)
//! or second (clock) changes, or on resize / interactive toggles.

use crate::cli::{default_center_deg, Config};
use crate::coast;
use crate::frame::{self, Frame, RenderParams};
use crate::solar;
use crate::term::{self, TermGuard};
use chrono::{FixedOffset, Local, Utc};
use std::io::Write;

pub fn run(cfg: &Config) -> i32 {
    let stdout_tty = crossterm::tty::IsTty::is_tty(&std::io::stdout());
    let color = cfg.color.resolve(stdout_tty);
    let rings = coast::decode(crate::coast_data::LAND_DATA);

    if cfg.once || !stdout_tty {
        return run_once(cfg, color, &rings);
    }
    run_interactive(cfg, color, &rings)
}

/// One-shot render to stdout; exit 0.
fn run_once(cfg: &Config, color: bool, rings: &[coast::Ring]) -> i32 {
    let (w, h) = term::size();
    let now = Utc::now();
    let sun = solar::subsolar(now);
    let lambda0 = effective_center(cfg);
    let clock = clock_time(cfg, now);
    let f = frame::compose(&RenderParams {
        w: w as usize,
        h: h as usize,
        lambda0_deg: lambda0,
        rings,
        sun,
        draw_twilight_curve: cfg.twilight,
        draw_coastline: cfg.coastline,
        ascii: cfg.ascii,
        clock_line: frame::clock_line(clock, &sun),
    });
    let bytes = frame::render_plain(&f, color);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&bytes);
    let _ = out.flush();
    0
}

/// Central meridian: explicit `--center` > `--utc` (0°) > startup local offset.
fn effective_center(cfg: &Config) -> f64 {
    if let Some(c) = cfg.center_deg {
        c
    } else if cfg.utc {
        0.0
    } else {
        default_center_deg()
    }
}

/// The clock's timezone: UTC with `--utc`, otherwise the live local offset.
fn clock_time(cfg: &Config, now_utc: chrono::DateTime<Utc>) -> chrono::DateTime<FixedOffset> {
    if cfg.utc {
        now_utc.with_timezone(&FixedOffset::east_opt(0).unwrap())
    } else {
        now_utc.with_timezone(&Local).fixed_offset()
    }
}

struct App {
    lambda0_deg: f64,
    use_utc: bool,
    twilight: bool,
    coastline: bool,
    ascii: bool,
    color: bool,
    rings: Vec<coast::Ring>,
    size: (u16, u16),
    prev: Option<Frame>,
    last_minute: Option<i64>,
    last_second: Option<i64>,
}

impl App {
    /// Rebuild the frame and emit a diff (or full frame when forced/first).
    fn redraw(&mut self, now: chrono::DateTime<Utc>, full: bool) {
        let sun = solar::subsolar(now);
        let clock = if self.use_utc {
            now.with_timezone(&FixedOffset::east_opt(0).unwrap())
        } else {
            now.with_timezone(&Local).fixed_offset()
        };
        let f = frame::compose(&RenderParams {
            w: self.size.0 as usize,
            h: self.size.1 as usize,
            lambda0_deg: self.lambda0_deg,
            rings: &self.rings,
            sun,
            draw_twilight_curve: self.twilight,
            draw_coastline: self.coastline,
            ascii: self.ascii,
            clock_line: frame::clock_line(clock, &sun),
        });
        let prev = if full { None } else { self.prev.as_ref() };
        let bytes = frame::render_ansi(prev, &f, self.color);
        if !bytes.is_empty() {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
        }
        self.prev = Some(f);
    }

    fn handle_key(
        &mut self,
        key: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> Option<LoopCtl> {
        use crossterm::event::{KeyCode, KeyModifiers};
        match (key, mods) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(LoopCtl::Quit(130)),
            (KeyCode::Char('z'), KeyModifiers::CONTROL) => Some(LoopCtl::Suspend),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) | (KeyCode::Char('Q'), _) => {
                Some(LoopCtl::Quit(0))
            }
            (KeyCode::Char('u') | KeyCode::Char('U'), _) => {
                self.use_utc = !self.use_utc;
                Some(LoopCtl::Dirty)
            }
            (KeyCode::Char('c') | KeyCode::Char('C'), _) => {
                self.lambda0_deg = default_center_deg();
                Some(LoopCtl::DirtyFull)
            }
            (KeyCode::Char('t') | KeyCode::Char('T'), _) => {
                self.twilight = !self.twilight;
                Some(LoopCtl::DirtyFull)
            }
            (KeyCode::Char('o') | KeyCode::Char('O'), _) => {
                self.coastline = !self.coastline;
                Some(LoopCtl::DirtyFull)
            }
            (KeyCode::Char('r') | KeyCode::Char('R'), _) => Some(LoopCtl::DirtyFull),
            _ => None,
        }
    }
}

enum LoopCtl {
    Quit(i32),
    Suspend,
    Dirty,
    DirtyFull,
}

fn run_interactive(cfg: &Config, color: bool, rings: &[coast::Ring]) -> i32 {
    term::install_panic_hook();
    let mut guard = match TermGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("daylight: cannot initialize terminal: {e}");
            return 1;
        }
    };

    let mut app = App {
        lambda0_deg: effective_center(cfg),
        use_utc: cfg.utc,
        twilight: cfg.twilight,
        coastline: cfg.coastline,
        ascii: cfg.ascii,
        color,
        rings: rings.to_vec(),
        size: term::size(),
        prev: None,
        last_minute: None,
        last_second: None,
    };

    loop {
        let now = Utc::now();

        // Redraw only when something changed since our last drawn state.
        let minute_key = now.timestamp() / 60;
        let second_key = now.timestamp();
        let minute_changed = app.last_minute.map(|m| m != minute_key).unwrap_or(true);
        let second_changed = app.last_second.map(|s| s != second_key).unwrap_or(true);
        if app.prev.is_none() || minute_changed || second_changed {
            // First frame or resize => full repaint; shading recompute on minute change.
            let full = app.prev.is_none();
            app.redraw(now, full);
            app.last_minute = Some(minute_key);
            app.last_second = Some(second_key);
        }

        // Sleep/poll until the next whole second.
        let timeout = std::time::Duration::from_millis(1000 - now.timestamp_subsec_millis() as u64);
        if crossterm::event::poll(timeout).unwrap_or(false) {
            match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(k)) => {
                    if k.kind == crossterm::event::KeyEventKind::Press {
                        if let Some(ctl) = app.handle_key(k.code, k.modifiers) {
                            match ctl {
                                LoopCtl::Quit(code) => {
                                    guard.restore();
                                    return code;
                                }
                                LoopCtl::Suspend => {
                                    term::suspend(&mut guard);
                                    // Blocked in the kernel until SIGCONT.
                                    term::resume(&mut guard);
                                    app.prev = None;
                                }
                                LoopCtl::Dirty => {
                                    app.redraw(Utc::now(), false);
                                }
                                LoopCtl::DirtyFull => {
                                    app.prev = None;
                                }
                            }
                        }
                    }
                }
                Ok(crossterm::event::Event::Resize(_w, _h)) => {
                    app.size = term::size();
                    app.prev = None;
                }
                _ => {}
            }
        }

        // Signals.
        if let Some(sig) = term::take_exit_signal() {
            guard.restore();
            return term::exit_code_for_signal(sig);
        }
        if term::take_suspend_request() {
            term::suspend(&mut guard);
            term::resume(&mut guard);
            app.prev = None;
        }
        if term::take_resume_request() {
            app.prev = None;
        }
    }
}
