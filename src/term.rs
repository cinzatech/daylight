//! Terminal backend: raw mode / alt screen guards, signal flags, suspend/resume,
//! panic hook. crossterm supplies modes + events; signal-hook supplies process signals.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

fn flag(cell: &OnceLock<Arc<AtomicBool>>) -> Arc<AtomicBool> {
    cell.get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

static EXIT_SIGINT: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static EXIT_SIGTERM: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static EXIT_SIGHUP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static SUSPEND_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static RESUME_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn install_signal_handlers() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGTSTP};
    use signal_hook::flag::register;
    let _ = register(SIGINT, flag(&EXIT_SIGINT));
    let _ = register(SIGTERM, flag(&EXIT_SIGTERM));
    let _ = register(SIGHUP, flag(&EXIT_SIGHUP));
    let _ = register(SIGTSTP, flag(&SUSPEND_FLAG));
    let _ = register(signal_hook::consts::SIGCONT, flag(&RESUME_FLAG));
}

/// Consume the pending exit signal, if any. Returns the signal number.
pub fn take_exit_signal() -> Option<i32> {
    if flag(&EXIT_SIGINT).swap(false, Ordering::SeqCst) {
        Some(signal_hook::consts::SIGINT)
    } else if flag(&EXIT_SIGTERM).swap(false, Ordering::SeqCst) {
        Some(signal_hook::consts::SIGTERM)
    } else if flag(&EXIT_SIGHUP).swap(false, Ordering::SeqCst) {
        Some(signal_hook::consts::SIGHUP)
    } else {
        None
    }
}

pub fn take_suspend_request() -> bool {
    flag(&SUSPEND_FLAG).swap(false, Ordering::SeqCst)
}

pub fn take_resume_request() -> bool {
    flag(&RESUME_FLAG).swap(false, Ordering::SeqCst)
}

/// Owns the terminal while the app runs. `Drop` restores everything, so every
/// exit path (normal, signal, panic via the hook) is symmetric.
pub struct TermGuard {
    active: bool,
}

impl TermGuard {
    /// Enter raw mode + alternate screen, hide cursor, disable line wrap, clear.
    pub fn enter() -> io::Result<Self> {
        install_signal_handlers();
        crossterm::terminal::enable_raw_mode()?;
        let mut out = io::stdout().lock();
        let _ = crossterm::execute!(
            out,
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::terminal::DisableLineWrap,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        );
        Ok(TermGuard { active: true })
    }

    /// Restore the terminal to the state we found it in.
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut out = io::stdout().lock();
        let _ = crossterm::execute!(
            out,
            crossterm::style::ResetColor,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::terminal::EnableLineWrap,
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = io::Write::flush(&mut out);
        let _ = crossterm::terminal::disable_raw_mode();
    }

    #[allow(dead_code)] // useful for callers/tests introspecting guard state
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Install a panic hook that restores the terminal before the default hook runs.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best effort: we cannot know which guard instance is alive, so perform
        // the raw restore sequence directly.
        let _ = crossterm::terminal::disable_raw_mode();
        let mut out = io::stdout().lock();
        let _ = crossterm::execute!(
            out,
            crossterm::style::ResetColor,
            crossterm::terminal::EnableLineWrap,
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
        prev(info);
    }));
}

/// Suspend: restore terminal, reset SIGTSTP to default, raise it.
/// The caller must re-init (and force a full redraw) on SIGCONT.
pub fn suspend(guard: &mut TermGuard) {
    guard.restore();
    // Re-raise SIGTSTP with its default disposition so the shell stops us.
    let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGTSTP);
}

/// Re-initialize after resume.
pub fn resume(guard: &mut TermGuard) {
    let _ = crossterm::terminal::enable_raw_mode();
    let mut out = io::stdout().lock();
    let _ = crossterm::execute!(
        out,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
        crossterm::terminal::DisableLineWrap,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    );
    guard.active = true;
}

/// Terminal size with a sane fallback.
pub fn size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// Map a caught signal number to the conventional exit code (128 + n).
pub fn exit_code_for_signal(sig: i32) -> i32 {
    128 + sig
}
