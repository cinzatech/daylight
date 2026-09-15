//! CLI parsing (clap derive) with conventional `--help`/`--version` and exit codes.
//!
//! clap conventions: `--help` exits 0, usage errors exit 2 with the message on stderr.
use clap::{Parser, ValueEnum};

/// A terminal day/night world clock: Kavrayskiy VII map, real-time terminator,
/// braille rendering.
#[derive(Parser, Debug)]
#[command(name = "daylight", version, about, long_about = None, after_help = INTERACTIVE_HELP)]
pub struct Args {
    /// Central meridian, degrees east [-180..180]. Takes precedence over
    /// `--utc` for the map meridian (the clock stays UTC).
    ///
    /// Default: 15° × UTC offset of the local timezone captured at startup,
    /// so your longitude sits near the map center.
    #[arg(
        long,
        value_name = "DEG",
        value_parser = parse_center,
        allow_negative_numbers = true
    )]
    pub center: Option<f64>,

    /// Clock in UTC; central meridian 0°.
    #[arg(long)]
    pub utc: bool,

    /// Also draw the civil twilight (−6°) curve.
    #[arg(long)]
    pub twilight: bool,

    /// Do not draw the one-dot outline around the map oval.
    #[arg(long)]
    pub no_outline: bool,

    /// Do not slowly rotate the map (one full turn in ~6 minutes).
    #[arg(long)]
    pub no_rotate: bool,

    /// Redraw interval in milliseconds while rotating [10..1000].
    #[arg(
        long = "interval",
        value_name = "MS",
        default_value_t = 100,
        value_parser = parse_interval
    )]
    pub interval_ms: u64,

    /// Render without braille ('#' land, '.' terminator) for limited fonts.
    #[arg(long)]
    pub ascii: bool,

    /// When to use color: auto | always | never.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
    pub color: ColorArg,

    /// Render one frame to stdout and exit (implied when stdout is not a TTY).
    #[arg(long)]
    pub once: bool,
}

const INTERACTIVE_HELP: &str = "Interactive keys (letter keys are case-insensitive):\n  q, Esc, Ctrl-C    quit (Ctrl-C exits 130)\n  Ctrl-Z            suspend (restore on resume)\n  u                 toggle UTC/local clock\n  c                 re-center map to current timezone\n  t                 toggle twilight curve\n  o                 toggle map-oval outline\n  a                 toggle slow rotation\n  r                 force repaint\n\nThe clock runs in the timezone current at startup (or UTC with --utc); only the\nmap center is pinned at startup — press c to re-center live.";

fn parse_center(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|_| format!("invalid number: {s}"))?;
    if (-180.0..=180.0).contains(&v) {
        Ok(v)
    } else {
        Err("must be within [-180, 180]".to_string())
    }
}

fn parse_interval(s: &str) -> Result<u64, String> {
    let v: u64 = s.parse().map_err(|_| format!("invalid number: {s}"))?;
    if (10..=1000).contains(&v) {
        Ok(v)
    } else {
        Err("must be within [10, 1000] milliseconds".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// User-specified central meridian in degrees east, if any.
    pub center_deg: Option<f64>,
    /// `--utc`: clock in UTC, central meridian 0°.
    pub utc: bool,
    pub twilight: bool,
    pub outline: bool,
    pub rotate: bool,
    pub interval_ms: u64,
    pub ascii: bool,
    pub color: ColorWhen,
    pub once: bool,
}

impl ColorWhen {
    /// Resolve `auto` against the environment. `NO_COLOR` disables color only
    /// when set to a non-empty value (the no-color.org convention), as does
    /// `TERM=dumb` or a missing `TERM`; otherwise color follows TTY-ness.
    pub fn resolve(self, stdout_is_tty: bool) -> bool {
        match self {
            ColorWhen::Always => true,
            ColorWhen::Never => false,
            ColorWhen::Auto => {
                if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                    return false;
                }
                match std::env::var("TERM").as_deref() {
                    Ok("dumb") | Err(_) => false,
                    Ok(_) => stdout_is_tty,
                }
            }
        }
    }
}

/// Parse argv using clap conventions (exiting on --help/--version/errors).
pub fn parse() -> Config {
    let args = Args::parse();
    Config {
        center_deg: args.center,
        utc: args.utc,
        twilight: args.twilight,
        outline: !args.no_outline,
        rotate: !args.no_rotate,
        interval_ms: args.interval_ms,
        ascii: args.ascii,
        color: match args.color {
            ColorArg::Auto => ColorWhen::Auto,
            ColorArg::Always => ColorWhen::Always,
            ColorArg::Never => ColorWhen::Never,
        },
        once: args.once,
    }
}

/// Default central meridian from the local UTC offset captured at startup:
/// `15° × offset-hours` (fractional offsets kept: UTC+05:30 → 82.5°), clamped
/// to [-180, 180].
pub fn default_center_deg() -> f64 {
    center_from_offset_secs(chrono::Local::now().offset().local_minus_utc())
}

/// Pure core of [`default_center_deg`]: `15° × offset-hours`, clamped.
pub fn center_from_offset_secs(offset_secs: i32) -> f64 {
    (15.0 * offset_secs as f64 / 3600.0).clamp(-180.0, 180.0)
}

#[cfg(test)]
mod tests {
    use super::center_from_offset_secs;

    #[test]
    fn center_from_offset_examples() {
        // Whole hours.
        assert!((center_from_offset_secs(0) - 0.0).abs() < 1e-12);
        assert!((center_from_offset_secs(2 * 3600) - 30.0).abs() < 1e-12);
        assert!((center_from_offset_secs(-5 * 3600) - (-75.0)).abs() < 1e-12);
        // Fractional offset kept as-is (India UTC+05:30).
        assert!((center_from_offset_secs(5 * 3600 + 1800) - 82.5).abs() < 1e-12);
        // Nepal UTC+05:45.
        assert!((center_from_offset_secs(5 * 3600 + 2700) - 86.25).abs() < 1e-12);
        // Clamp at the dateline (UTC+14 → 210° → 180°).
        assert!((center_from_offset_secs(14 * 3600) - 180.0).abs() < 1e-12);
        assert!((center_from_offset_secs(-12 * 3600) - (-180.0)).abs() < 1e-12);
    }
}
