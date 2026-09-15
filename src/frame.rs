//! Frame composition and ANSI/plain writers. See INTERFACES.md (frame section).
//!
//! Screen layout for a terminal of `w` x `h` cells:
//!
//! ```text
//!   row 0 ..= h-3 : map area (top-aligned); cells at x in 1..=w-2 (1-cell side
//!                   margins), backed by a braille dot grid of 2*(w-2) x 4*(h-2) dots
//!   row h-2       : blank separator margin
//!   row h-1       : clock line, centered, Style::Clock
//! ```
//!
//! Per cell, three dot layers are composed: land (scanline fill), the terminator
//! curve, and (optionally) the civil-twilight curve. Curve dots win the *style*
//! (`Terminator`) but land dots still merge into the glyph mask, so a terminator
//! crossing a continent shows both. Cells whose center dot falls outside the map
//! oval stay `Cell::BLANK`.
//!
//! Cross-module conventions relied on (per INTERFACES.md and the implemented
//! geo module):
//! - `geo::MapFrame::dot_to_lonlat` returns `(lon, lat)` in **radians** (the
//!   geo module is radians throughout); `lonlat_to_dot` takes degrees.
//! - `solar::sample_curve` yields `(lat_deg, lon_deg)` pairs (documented order).
//!
//! The theme maps each `Style` to a small documented set of SGR parameters (see
//! [`style_params`]); night shades carry the `dim` attribute and day land the
//! `bold` attribute so the map stays legible even without colors.

use crate::coast;
use crate::geo;
use crate::raster;
use crate::solar::{self, Shading, Sun};
use chrono::{DateTime, FixedOffset};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Blank,
    DaySea,
    DayLand,
    TwiSea,
    TwiLand,
    NightSea,
    NightLand,
    Terminator,
    Clock,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub sym: char,
    pub style: Style,
}
impl Cell {
    pub const BLANK: Cell = Cell {
        sym: ' ',
        style: Style::Blank,
    };
}
#[derive(Clone, PartialEq, Debug)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<Cell>, // row-major
}
impl Frame {
    /// New `w` x `h` frame filled with `fill`. Cells are row-major, `(x, y)` indexed.
    pub fn new(w: usize, h: usize, fill: Cell) -> Self {
        Frame {
            w,
            h,
            cells: vec![fill; w * h],
        }
    }
    /// Cell at column `x`, row `y`. Panics when out of range.
    pub fn get(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.w + x]
    }
    /// Overwrite the cell at column `x`, row `y`. Panics when out of range.
    pub fn set(&mut self, x: usize, y: usize, c: Cell) {
        self.cells[y * self.w + x] = c;
    }
}

pub struct RenderParams<'a> {
    pub w: usize,
    pub h: usize, // terminal size in cells
    pub lambda0_deg: f64,
    pub rings: &'a [coast::Ring],
    pub sun: Sun,
    pub draw_twilight_curve: bool,
    /// One-dot coastline outline around land contours (terrain-colored).
    pub draw_coastline: bool,
    pub ascii: bool,        // '#' land, '.' terminator instead of braille
    pub clock_line: String, // pre-rendered, may be truncated by composer
}

/// Compose a full frame: map area (rows 0..=h-3) plus the centered clock line
/// (row h-1). Pure: same params in, same frame out.
pub fn compose(p: &RenderParams) -> Frame {
    let mut frame = Frame::new(p.w, p.h, Cell::BLANK);

    compose_clock_row(&mut frame, &p.clock_line);

    // Map cells: screen columns 1..=w-2, screen rows 0..=h-3.
    let map_cols = p.w.saturating_sub(2);
    let map_rows = p.h.saturating_sub(2);
    if map_cols == 0 || map_rows == 0 {
        return frame; // degenerate terminal: clock line only
    }
    let dots_w = 2 * map_cols;
    let dots_h = 4 * map_rows;
    let mf = geo::MapFrame::new(dots_w, dots_h, p.lambda0_deg);

    // Layer 1: land fill (static per size + central meridian), plus the optional
    // coastline outline OR-ed in so it is styled as terrain (land style of the
    // cell's shading).
    let mut land = raster::Canvas::new(dots_w, dots_h);
    raster::draw_land(&mut land, &mf, p.rings);
    if p.draw_coastline {
        raster::draw_coastline(&mut land, &mf, p.rings);
    }

    // Layer 2: terminator curve (elevation 0); Layer 3: civil twilight (-6 deg).
    let mut terminator = raster::Canvas::new(dots_w, dots_h);
    let mut twilight = raster::Canvas::new(dots_w, dots_h);
    for &(lat, lon) in &solar::sample_curve(&p.sun, 0.0, 0.25) {
        if let Some((dx, dy)) = mf.lonlat_to_dot(lon, lat) {
            terminator.set_dot(dx, dy);
        }
    }
    if p.draw_twilight_curve {
        for &(lat, lon) in &solar::sample_curve(&p.sun, -6.0, 0.25) {
            if let Some((dx, dy)) = mf.lonlat_to_dot(lon, lat) {
                twilight.set_dot(dx, dy);
            }
        }
    }

    // Per-cell oval membership at dot granularity: which of the 2x4 braille dots of
    // each cell lie inside the map oval. This keeps the map border on the dot grid
    // (sub-cell detail) instead of stepping in whole cells. Same bit layout as
    // raster's CELL_BITS (left column dots 1,2,3,7; right column dots 4,5,6,8).
    const CELL_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    let mut oval_masks: Vec<u8> = vec![0u8; map_cols * map_rows];
    let mut shade_dots: Vec<(usize, usize)> = vec![(0, 0); map_cols * map_rows];
    for cy in 0..map_rows {
        for cx in 0..map_cols {
            let mut mask = 0u8;
            let mut shade: Option<(usize, usize)> = None;
            for (col, bits_col) in CELL_BITS.iter().enumerate() {
                for (row, bit) in bits_col.iter().enumerate() {
                    let (dx, dy) = (2 * cx + col, 4 * cy + row);
                    if mf.dot_to_lonlat(dx, dy).is_some() {
                        mask |= bit;
                        // Prefer the cell's center-ish dot (right column, third row)
                        // for the shading sample; any inside dot as fallback.
                        if shade.is_none() || (dx, dy) == (2 * cx + 1, 4 * cy + 2) {
                            shade = Some((dx, dy));
                        }
                    }
                }
            }
            let i = cy * map_cols + cx;
            oval_masks[i] = mask;
            shade_dots[i] = shade.unwrap_or((2 * cx + 1, 4 * cy + 2));
        }
    }

    for cy in 0..map_rows {
        for cx in 0..map_cols {
            let i = cy * map_cols + cx;
            let oval_mask = oval_masks[i];
            if oval_mask == 0 {
                continue; // entirely outside the map oval => stays BLANK
            }
            // Geographic location for shading. dot_to_lonlat speaks radians;
            // solar speaks degrees.
            let (lon_rad, lat_rad) = mf
                .dot_to_lonlat(shade_dots[i].0, shade_dots[i].1)
                .expect("shade dot was verified inside the oval");
            let (lon_deg, lat_deg) = (lon_rad.to_degrees(), lat_rad.to_degrees());
            // Land dots outside the oval (the scanline fills to the grid edge) drop.
            let land_mask = land.cell_mask(cx, cy) & oval_mask;
            let curve_mask = terminator.cell_mask(cx, cy) | twilight.cell_mask(cx, cy);
            let shading = solar::shading(solar::elevation_deg(lat_deg, lon_deg, &p.sun));
            let cell = if p.ascii {
                if curve_mask != 0 {
                    Cell {
                        sym: '.',
                        style: Style::Terminator,
                    }
                } else if land_mask != 0 {
                    Cell {
                        sym: '#',
                        style: land_style(shading),
                    }
                } else {
                    Cell {
                        sym: ' ',
                        style: sea_style(shading),
                    }
                }
            } else if curve_mask != 0 {
                // Curve dots take style precedence; land dots merge into the glyph.
                Cell {
                    sym: raster::braille(land_mask | curve_mask),
                    style: Style::Terminator,
                }
            } else if land_mask != 0 {
                Cell {
                    sym: raster::braille(land_mask),
                    style: land_style(shading),
                }
            } else {
                Cell {
                    sym: ' ',
                    style: sea_style(shading),
                }
            };
            frame.set(cx + 1, cy, cell);
        }
    }
    frame
}

/// Bottom row (`h-1`): the clock line, centered. When it does not fit, keep the
/// leading `w-2` characters (the trailing subsolar readout is dropped first) and
/// center that; the time itself survives on any usable width.
fn compose_clock_row(frame: &mut Frame, line: &str) {
    if frame.h == 0 {
        return;
    }
    let chars: Vec<char> = line.chars().collect();
    let kept: &[char] = if chars.len() <= frame.w {
        &chars
    } else {
        &chars[..frame.w.saturating_sub(2)]
    };
    let pad_left = (frame.w - kept.len()) / 2;
    let y = frame.h - 1;
    for (i, &ch) in kept.iter().enumerate() {
        frame.set(
            pad_left + i,
            y,
            Cell {
                sym: ch,
                style: Style::Clock,
            },
        );
    }
}

/// Format the clock line:
/// `Wed 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W`
/// Local civil time of `now`, its UTC offset, then the subsolar point with one
/// decimal and hemisphere letters (0 counts as N/E).
pub fn clock_line(now: DateTime<FixedOffset>, sun: &Sun) -> String {
    let secs = now.offset().local_minus_utc();
    let zone = if secs == 0 {
        "UTC".to_string()
    } else {
        let (sign, abs) = if secs < 0 { ('-', -secs) } else { ('+', secs) };
        let mins = abs / 60;
        format!("UTC{}{:02}:{:02}", sign, mins / 60, mins % 60)
    };
    let ns = if sun.decl_deg >= 0.0 { 'N' } else { 'S' };
    let ew = if sun.lon_deg >= 0.0 { 'E' } else { 'W' };
    format!(
        "{} {} · ☉ {:.1}°{} {:.1}°{}",
        now.format("%a %Y-%m-%d %H:%M:%S"),
        zone,
        sun.decl_deg.abs(),
        ns,
        sun.lon_deg.abs(),
        ew
    )
}

fn sea_style(shading: Shading) -> Style {
    match shading {
        Shading::Day => Style::DaySea,
        Shading::CivilTwilight => Style::TwiSea,
        Shading::Night => Style::NightSea,
    }
}

fn land_style(shading: Shading) -> Style {
    match shading {
        Shading::Day => Style::DayLand,
        Shading::CivilTwilight => Style::TwiLand,
        Shading::Night => Style::NightLand,
    }
}

/// Which SGR flavor a writer emits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    /// Bold/dim attributes only; never a color SGR (color=false terminals).
    Mono,
    /// Full 256-color theme.
    Full,
}

/// Style -> SGR parameter string (without the `ESC[` / `m` wrapper).
///
/// Palette constants (256-color indexes, picked for tasteful contrast):
/// - DaySea    : default foreground (plain reset)
/// - DayLand   : bold, fg 114 (soft green) — daylight land stands out
/// - TwiSea    : fg 244 (mid gray)
/// - TwiLand   : fg 246 (lighter gray; no bold/dim — the twilight middle ground)
/// - NightSea  : dim, fg 238 (dark gray)
/// - NightLand : dim, fg 244
/// - Terminator: bold, fg 214 (orange accent)
/// - Clock     : bold
///
/// In `Mono` mode only bold/dim survive (bold for day land / terminator / clock,
/// dim for night), so shading stays readable without any color support.
fn style_params(style: Style, mode: ColorMode) -> &'static str {
    match (style, mode) {
        (Style::Blank, _) | (Style::DaySea, _) => "0",
        (Style::DayLand, ColorMode::Mono) => "1",
        (Style::DayLand, ColorMode::Full) => "1;38;5;114",
        (Style::TwiSea, ColorMode::Mono) => "0",
        (Style::TwiSea, ColorMode::Full) => "38;5;244",
        (Style::TwiLand, ColorMode::Mono) => "0",
        (Style::TwiLand, ColorMode::Full) => "38;5;246",
        (Style::NightSea, ColorMode::Mono) => "2",
        (Style::NightSea, ColorMode::Full) => "2;38;5;238",
        (Style::NightLand, ColorMode::Mono) => "2",
        (Style::NightLand, ColorMode::Full) => "2;38;5;244",
        (Style::Terminator, ColorMode::Mono) => "1",
        (Style::Terminator, ColorMode::Full) => "1;38;5;214",
        (Style::Clock, _) => "1",
    }
}

fn push_sgr(out: &mut Vec<u8>, params: &str) {
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(params.as_bytes());
    out.push(b'm');
}

/// Append one cell, emitting an SGR only when the style's SGR parameters change
/// relative to `cur` (None == terminal is in the reset state). Every emitted SGR is
/// **self-contained**: `ESC[0;{params}m` (or a bare reset) rather than a delta.
/// SGR attributes are sticky — a bare `ESC[1m` or `ESC[38;5;Nm` after a
/// `dim`+color style would leave dim (or the old color) active — so transitions
/// must always begin from a known reset state. `mode == None` means "never emit
/// SGR" (plain-text output).
fn push_cell(
    out: &mut Vec<u8>,
    cell: Cell,
    cur: &mut Option<&'static str>,
    mode: Option<ColorMode>,
) {
    if let Some(m) = mode {
        let params = style_params(cell.style, m);
        // None == terminal still in the reset state, i.e. equivalent to "0".
        if cur.unwrap_or("0") != params {
            if params == "0" {
                push_sgr(out, "0");
            } else {
                let mut p = String::with_capacity(params.len() + 2);
                p.push_str("0;");
                p.push_str(params);
                push_sgr(out, &p);
            }
            *cur = Some(params);
        }
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(cell.sym.encode_utf8(&mut buf).as_bytes());
}

/// Diff writer: minimal ANSI bytes updating only changed runs of cells.
/// `prev == None` (or a size mismatch) emits a full frame (`ESC[H` then all rows).
/// Equal frames produce no output at all. With `color == false` only bold/dim
/// attributes are emitted, never color SGR. Non-empty output always ends with
/// an SGR reset.
pub fn render_ansi(prev: Option<&Frame>, next: &Frame, color: bool) -> Vec<u8> {
    let mode = if color {
        ColorMode::Full
    } else {
        ColorMode::Mono
    };
    let mut out: Vec<u8> = Vec::new();
    let mut cur: Option<&'static str> = None; // None == terminal in reset state

    match prev {
        Some(p) if p.w == next.w && p.h == next.h => {
            if p.cells == next.cells {
                return out; // nothing changed: emit nothing
            }
            // Minimal diff: contiguous runs of changed cells; runs never span rows.
            for y in 0..next.h {
                let mut x = 0;
                while x < next.w {
                    if next.get(x, y) == p.get(x, y) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < next.w && next.get(x, y) != p.get(x, y) {
                        x += 1;
                    }
                    // Absolute cursor move, 1-based coordinates.
                    let pos = format!("\x1b[{};{}H", y + 1, start + 1);
                    out.extend_from_slice(pos.as_bytes());
                    for i in start..x {
                        push_cell(&mut out, next.get(i, y), &mut cur, Some(mode));
                    }
                }
            }
        }
        _ => {
            // Full frame: home the cursor and emit every row.
            out.extend_from_slice(b"\x1b[H");
            for y in 0..next.h {
                for x in 0..next.w {
                    push_cell(&mut out, next.get(x, y), &mut cur, Some(mode));
                }
                if y + 1 < next.h {
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
    }

    if !out.is_empty() {
        out.extend_from_slice(b"\x1b[0m"); // always leave the terminal reset
    }
    out
}

/// One-shot plain output: full frame rows joined with `\n` plus a trailing
/// newline, SGR per style only when `color` (no cursor moves at all).
pub fn render_plain(f: &Frame, color: bool) -> Vec<u8> {
    let mode = if color { Some(ColorMode::Full) } else { None };
    let mut out: Vec<u8> = Vec::new();
    let mut cur: Option<&'static str> = None;
    for y in 0..f.h {
        for x in 0..f.w {
            push_cell(&mut out, f.get(x, y), &mut cur, mode);
        }
        if y + 1 < f.h {
            out.push(b'\n');
        }
    }
    if let Some(params) = cur {
        if params != "0" {
            push_sgr(&mut out, "0"); // close any open SGR before the final newline
        }
    }
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo;
    use crate::solar;
    use crate::solar::Sun;
    use chrono::{FixedOffset, TimeZone, Utc};

    const W: usize = 80;
    const H: usize = 24;

    /// Closed rectangular synthetic land ring covering the lat/lon box.
    fn box_ring(lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> coast::Ring {
        vec![
            (lat_min, lon_min),
            (lat_min, lon_max),
            (lat_max, lon_max),
            (lat_max, lon_min),
            (lat_min, lon_min),
        ]
    }

    fn params<'a>(
        w: usize,
        h: usize,
        rings: &'a [coast::Ring],
        sun: Sun,
        ascii: bool,
        twilight: bool,
        clock: &str,
    ) -> RenderParams<'a> {
        RenderParams {
            w,
            h,
            lambda0_deg: 0.0,
            rings,
            sun,
            draw_twilight_curve: twilight,
            draw_coastline: true,
            ascii,
            clock_line: clock.to_string(),
        }
    }

    /// Screen cell covering (lat, lon): same dot-grid frame compose() builds.
    fn probe(f: &Frame, lat: f64, lon: f64) -> Option<Cell> {
        let mf = geo::MapFrame::new(2 * (f.w - 2), 4 * (f.h - 2), 0.0);
        let (dx, dy) = mf.lonlat_to_dot(lon, lat)?;
        Some(f.get(dx / 2 + 1, dy / 4))
    }

    fn norm_lon(lon: f64) -> f64 {
        let l = (lon + 180.0).rem_euclid(360.0) - 180.0;
        if l == -180.0 {
            180.0
        } else {
            l
        }
    }

    fn is_day(c: Cell) -> bool {
        matches!(c.style, Style::DaySea | Style::DayLand)
    }
    fn is_night(c: Cell) -> bool {
        matches!(c.style, Style::NightSea | Style::NightLand)
    }
    fn is_land(c: Cell) -> bool {
        matches!(c.style, Style::DayLand | Style::TwiLand | Style::NightLand)
    }
    fn is_sea(c: Cell) -> bool {
        matches!(c.style, Style::DaySea | Style::TwiSea | Style::NightSea)
    }

    /// Count CSI sequences ending in 'H' (cursor positioning), e.g. ESC[6;11H, ESC[H.
    fn count_cursor_moves(out: &[u8]) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i + 1 < out.len() {
            if out[i] == 0x1b && out[i + 1] == b'[' {
                let mut j = i + 2;
                while j < out.len() && (out[j].is_ascii_digit() || out[j] == b';') {
                    j += 1;
                }
                if j < out.len() && out[j] == b'H' {
                    n += 1;
                }
                i = j + 1;
            } else {
                i += 1;
            }
        }
        n
    }

    // ---------------------------------------------------------------- Frame

    #[test]
    fn frame_new_get_set() {
        let mut f = Frame::new(3, 2, Cell::BLANK);
        assert_eq!(f.cells.len(), 6);
        assert_eq!(f.get(0, 0), Cell::BLANK);
        let c = Cell {
            sym: 'x',
            style: Style::Clock,
        };
        f.set(2, 1, c);
        assert_eq!(f.get(2, 1), c);
        assert_eq!(f.get(1, 1), Cell::BLANK); // neighbors untouched
    }

    // ------------------------------------------------------------ clock_line

    #[test]
    fn clock_line_exact_strings() {
        // Positive offset, both subsolar coordinates negative (S, W).
        let off = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = off.with_ymd_and_hms(2026, 9, 26, 14, 3, 22).unwrap();
        let sun = Sun {
            decl_deg: -12.34,
            lon_deg: -45.6,
        };
        assert_eq!(
            clock_line(now, &sun),
            "Sat 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W"
        );

        // Zero offset => bare " UTC".
        let utc = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2024, 6, 21, 12, 0, 0)
            .unwrap();
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 10.0,
        };
        assert_eq!(
            clock_line(utc, &sun),
            "Fri 2024-06-21 12:00:00 UTC · ☉ 23.4°N 10.0°E"
        );

        // Negative offset, positive subsolar coordinates.
        let west = FixedOffset::west_opt(5 * 3600).unwrap();
        let now = west.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap();
        let sun = Sun {
            decl_deg: 0.0,
            lon_deg: 135.0,
        };
        assert_eq!(
            clock_line(now, &sun),
            "Thu 2025-01-02 03:04:05 UTC-05:00 · ☉ 0.0°N 135.0°E"
        );
    }

    #[test]
    fn clock_line_zero_is_north_east() {
        let utc = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .unwrap();
        let sun = Sun {
            decl_deg: 0.0,
            lon_deg: 0.0,
        };
        assert!(clock_line(utc, &sun).ends_with("☉ 0.0°N 0.0°E"));
        let sun = Sun {
            decl_deg: -0.04,
            lon_deg: -0.02,
        };
        assert!(clock_line(utc, &sun).ends_with("☉ 0.0°S 0.0°W"));
    }

    // ------------------------------------------------- compose: clock row

    #[test]
    fn clock_row_centered_and_styled() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let line = "Sat 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W";
        let rings: Vec<coast::Ring> = vec![];
        let f = compose(&params(W, H, &rings, sun, false, false, line));
        let len = line.chars().count();
        let pad = (W - len) / 2;
        let row: String = (0..W).map(|x| f.get(x, H - 1).sym).collect();
        let expected = " ".repeat(pad) + line + &" ".repeat(W - len - pad);
        assert_eq!(row, expected);
        assert_eq!(f.get(pad, H - 1).style, Style::Clock); // text is Clock-styled
        assert_eq!(f.get(0, H - 1), Cell::BLANK); // padding is blank
        assert_eq!(f.get(W - 1, H - 1), Cell::BLANK);
    }

    #[test]
    fn clock_row_truncates_when_too_wide() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let line = "Wed 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W";
        let rings: Vec<coast::Ring> = vec![];
        let f = compose(&params(30, 3, &rings, sun, false, false, line));
        let kept: String = line.chars().take(28).collect(); // w - 2
        let pad = (30 - 28) / 2;
        let row: String = (0..30).map(|x| f.get(x, 2).sym).collect();
        let expected = " ".repeat(pad) + &kept + &" ".repeat(30 - 28 - pad);
        assert_eq!(row, expected);
    }

    // ------------------------------------------------ compose: shading

    #[test]
    fn june_solstice_polar_shading() {
        let t = Utc.with_ymd_and_hms(2024, 6, 21, 12, 0, 0).unwrap();
        let sun = solar::subsolar(t);
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let north = probe(&f, 80.0, 0.0).expect("80N/0E inside the map");
        let south = probe(&f, -80.0, 0.0).expect("80S/0E inside the map");
        assert!(
            is_day(north),
            "June solstice: 80N should be day, got {:?}",
            north
        );
        assert!(
            is_night(south),
            "June solstice: 80S should be night, got {:?}",
            south
        );
    }

    #[test]
    fn december_solstice_polar_shading() {
        let t = Utc.with_ymd_and_hms(2024, 12, 21, 12, 0, 0).unwrap();
        let sun = solar::subsolar(t);
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let north = probe(&f, 80.0, 0.0).expect("80N/0E inside the map");
        let south = probe(&f, -80.0, 0.0).expect("80S/0E inside the map");
        assert!(
            is_night(north),
            "December solstice: 80N should be night, got {:?}",
            north
        );
        assert!(
            is_day(south),
            "December solstice: 80S should be day, got {:?}",
            south
        );
    }

    #[test]
    fn equinox_shading_flips_across_terminator_meridians() {
        // At the equinox the terminator degenerates to the meridians
        // lon_subsolar +/- 90; day/night must flip across them.
        let t = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let sun = solar::subsolar(t);
        let rings: Vec<coast::Ring> = vec![];
        let f = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let lat = 20.0;
        // Day side: within 90 degrees of the subsolar meridian.
        assert!(is_day(
            probe(&f, lat, norm_lon(sun.lon_deg - 75.0)).expect("probe inside map")
        ));
        assert!(is_day(
            probe(&f, lat, norm_lon(sun.lon_deg + 75.0)).expect("probe inside map")
        ));
        // Night side: past the subsolar +/- 90 meridians.
        assert!(is_night(
            probe(&f, lat, norm_lon(sun.lon_deg - 105.0)).expect("probe inside map")
        ));
        assert!(is_night(
            probe(&f, lat, norm_lon(sun.lon_deg + 105.0)).expect("probe inside map")
        ));
    }

    // ------------------------------------------- compose: terminator layer

    #[test]
    fn terminator_cells_present_on_curve() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let mf = geo::MapFrame::new(2 * (W - 2), 4 * (H - 2), 0.0);
        let pts = solar::sample_curve(&sun, 0.0, 0.25);
        assert!(
            pts.len() > 100,
            "dense curve sampling expected, got {}",
            pts.len()
        );
        let mut mapped = 0usize;
        let mut terminator_cells = 0usize;
        for &(lat, lon) in &pts {
            if let Some((dx, dy)) = mf.lonlat_to_dot(lon, lat) {
                mapped += 1;
                let c = f.get(dx / 2 + 1, dy / 4);
                assert!(
                    c.style == Style::Terminator || c == Cell::BLANK,
                    "curve cell at ({lat},{lon}) is {c:?}, expected Terminator or Blank"
                );
                if c.style == Style::Terminator {
                    terminator_cells += 1;
                }
            }
        }
        assert!(mapped > 50, "most of the curve should map into the oval");
        assert!(
            terminator_cells >= 20,
            "expected many terminator cells, got {terminator_cells}"
        );
    }

    #[test]
    fn twilight_curve_drawn_when_enabled() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings: Vec<coast::Ring> = vec![];
        let mf = geo::MapFrame::new(2 * (W - 2), 4 * (H - 2), 0.0);

        let f = compose(&params(W, H, &rings, sun, false, true, "clock"));
        let mut twilight_styled = 0usize;
        for &(lat, lon) in &solar::sample_curve(&sun, -6.0, 0.25) {
            if let Some((dx, dy)) = mf.lonlat_to_dot(lon, lat) {
                if f.get(dx / 2 + 1, dy / 4).style == Style::Terminator {
                    twilight_styled += 1;
                }
            }
        }
        assert!(
            twilight_styled >= 20,
            "expected twilight-curve cells styled Terminator, got {twilight_styled}"
        );

        // Without the flag the -6 deg curve must not be drawn as terminator.
        let f2 = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let mut styled2 = 0usize;
        for &(lat, lon) in &solar::sample_curve(&sun, -6.0, 0.25) {
            if let Some((dx, dy)) = mf.lonlat_to_dot(lon, lat) {
                if f2.get(dx / 2 + 1, dy / 4).style == Style::Terminator {
                    styled2 += 1;
                }
            }
        }
        // The 0 deg curve is disjoint from the -6 deg curve at this declination
        // except for boundary cells near the polar tangents; allow that slop.
        assert!(
            styled2 < twilight_styled / 2,
            "{styled2} twilight cells styled without the flag"
        );
    }

    // ------------------------------------------------ compose: land layer

    #[test]
    fn synthetic_ring_renders_as_land() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let inside = probe(&f, 45.0, 10.0).expect("45N/10E inside the map");
        assert!(
            is_land(inside),
            "45N/10E inside the synthetic box should be land, got {:?}",
            inside.style
        );
        assert!(
            inside.sym != ' ' && ('\u{2800}'..='\u{28FF}').contains(&inside.sym),
            "land cell should be a non-blank braille glyph, got U+{:04X}",
            inside.sym as u32
        );
        let sea = probe(&f, 0.0, -140.0).expect("equator mid-Pacific inside the map");
        assert!(
            is_sea(sea),
            "mid-Pacific should be sea, got {:?}",
            sea.style
        );
        assert_eq!(sea.sym, ' ');
    }

    // ----------------------------------------------- compose: structure

    #[test]
    fn structural_invariants_both_sizes() {
        for &(w, h) in &[(80usize, 24usize), (120, 40)] {
            let sun = Sun {
                decl_deg: 23.44,
                lon_deg: 0.0,
            };
            let rings = vec![
                box_ring(30.0, 60.0, -10.0, 30.0),
                box_ring(-60.0, -30.0, 100.0, 160.0),
            ];
            let f = compose(&params(
                w,
                h,
                &rings,
                sun,
                false,
                true,
                "Sat 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W",
            ));
            assert_eq!(f.w, w);
            assert_eq!(f.h, h);
            assert_eq!(f.cells.len(), w * h);

            // Map rows: only braille or space; margins and separator row blank.
            for y in 0..h - 1 {
                for x in 0..w {
                    let c = f.get(x, y);
                    assert!(
                        c.sym == ' ' || ('\u{2800}'..='\u{28FF}').contains(&c.sym),
                        "({x},{y}): sym {:?} is not braille/space",
                        c.sym
                    );
                    if x == 0 || x == w - 1 || y == h - 2 {
                        assert_eq!(c, Cell::BLANK, "margin ({x},{y}) should be blank");
                    }
                }
            }

            // Plain output: h rows of exactly w chars, no control chars, no ESC.
            let plain = render_plain(&f, false);
            for &b in &plain {
                assert!(
                    b == b'\n' || (0x20..0x7f).contains(&b) || b >= 0x80,
                    "control byte 0x{b:02x} in plain output"
                );
            }
            let text = String::from_utf8(plain).unwrap();
            let lines: Vec<&str> = text.strip_suffix('\n').unwrap().split('\n').collect();
            assert_eq!(lines.len(), h);
            for line in &lines {
                assert_eq!(line.chars().count(), w);
            }
        }
    }

    #[test]
    fn ascii_mode_uses_hash_dot_space() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, true, false, "clock"));
        let mut hash = 0usize;
        let mut dot = 0usize;
        let mut space = 0usize;
        for y in 0..H - 2 {
            // map rows are 0..=h-3
            for x in 0..W {
                match f.get(x, y).sym {
                    '#' => hash += 1,
                    '.' => dot += 1,
                    ' ' => space += 1,
                    other => panic!("unexpected ascii symbol {other:?} at ({x},{y})"),
                }
            }
        }
        assert!(hash > 20, "expected land '#' cells, got {hash}");
        assert!(dot > 20, "expected terminator '.' cells, got {dot}");
        assert!(space > 100, "expected sea ' ' cells, got {space}");
        assert!(f
            .cells
            .iter()
            .any(|c| c.sym == '.' && c.style == Style::Terminator));
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        let sun = Sun {
            decl_deg: 10.0,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(0.0, 10.0, 0.0, 10.0)];
        for &(w, h) in &[(0usize, 0usize), (1, 1), (2, 2), (3, 3), (4, 2), (10, 1)] {
            let f = compose(&params(w, h, &rings, sun, false, false, "clock"));
            assert_eq!(f.w, w);
            assert_eq!(f.h, h);
            assert_eq!(f.cells.len(), w * h);
            let _ = render_plain(&f, false);
            let _ = render_ansi(None, &f, true);
        }
    }

    // ------------------------------------------------------ render_ansi

    #[test]
    fn ansi_diff_empty_when_frames_equal() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let p = params(W, H, &rings, sun, false, false, "clock");
        let f1 = compose(&p);
        let f2 = compose(&p);
        assert!(f1 == f2); // compose is deterministic
        assert!(render_ansi(Some(&f1), &f2, true).is_empty());
        assert!(render_ansi(Some(&f1), &f2, false).is_empty());
    }

    #[test]
    fn ansi_single_cell_change_is_one_run() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f1 = compose(&params(W, H, &rings, sun, false, false, "clock"));
        let mut f2 = f1.clone();
        f2.set(
            10,
            5,
            Cell {
                sym: '\u{2801}',
                style: Style::DayLand,
            },
        );
        let out = render_ansi(Some(&f1), &f2, true);
        assert!(!out.is_empty());
        assert_eq!(
            count_cursor_moves(&out),
            1,
            "one changed run must produce exactly one cursor move"
        );
        assert!(
            out.starts_with(b"\x1b[6;11H"),
            "expected ESC[6;11H prefix (row 5+1, col 10+1), got {:?}",
            String::from_utf8_lossy(&out[..out.len().min(24)])
        );
        assert!(out.ends_with(b"\x1b[0m"), "output must end with SGR reset");
        assert!(String::from_utf8(out).unwrap().contains('\u{2801}'));
    }

    #[test]
    fn ansi_adjacent_and_distant_changes() {
        let f1 = Frame::new(10, 4, Cell::BLANK);
        // Adjacent changes coalesce into a single run.
        let mut f2 = f1.clone();
        f2.set(
            2,
            1,
            Cell {
                sym: 'a',
                style: Style::Clock,
            },
        );
        f2.set(
            3,
            1,
            Cell {
                sym: 'b',
                style: Style::Clock,
            },
        );
        let out = render_ansi(Some(&f1), &f2, true);
        assert_eq!(count_cursor_moves(&out), 1, "adjacent cells form one run");
        assert!(out.starts_with(b"\x1b[2;3H"));

        // Distant changes produce two runs with absolute positioning.
        let mut f3 = f1.clone();
        f3.set(
            2,
            1,
            Cell {
                sym: 'a',
                style: Style::Clock,
            },
        );
        f3.set(
            7,
            3,
            Cell {
                sym: 'z',
                style: Style::NightSea,
            },
        );
        let out = render_ansi(Some(&f1), &f3, true);
        assert_eq!(count_cursor_moves(&out), 2);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\x1b[2;3H"));
        assert!(s.contains("\x1b[4;8H"));
    }

    #[test]
    fn ansi_full_frame_on_none_or_size_change() {
        let f = Frame::new(6, 3, Cell::BLANK);
        let out = render_ansi(None, &f, true);
        assert!(out.starts_with(b"\x1b[H"));
        assert!(out.ends_with(b"\x1b[0m"));
        assert_eq!(count_cursor_moves(&out), 1); // just the home ESC[H
        let s = String::from_utf8(out).unwrap();
        let body = s
            .strip_prefix("\x1b[H")
            .and_then(|r| r.strip_suffix("\x1b[0m"))
            .unwrap();
        assert_eq!(body, "      \r\n      \r\n      ");

        // Size mismatch against prev also takes the full-frame path.
        let bigger = Frame::new(8, 3, Cell::BLANK);
        assert!(render_ansi(Some(&f), &bigger, true).starts_with(b"\x1b[H"));
    }

    #[test]
    fn ansi_mono_has_attributes_but_no_color() {
        let mut f1 = Frame::new(4, 1, Cell::BLANK);
        f1.set(
            0,
            0,
            Cell {
                sym: '\u{2801}',
                style: Style::DayLand,
            },
        );
        f1.set(
            1,
            0,
            Cell {
                sym: '\u{2801}',
                style: Style::NightSea,
            },
        );
        f1.set(
            2,
            0,
            Cell {
                sym: 'x',
                style: Style::Terminator,
            },
        );
        let f2 = Frame::new(4, 1, Cell::BLANK);
        let out = render_ansi(Some(&f2), &f1, false);
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("38;5;"), "no color SGR in mono mode: {s}");
        assert!(
            s.contains("\x1b[0;1m"),
            "self-contained bold attribute present in mono mode: {s}"
        );
        assert!(
            s.contains("\x1b[0;2m"),
            "self-contained dim attribute present in mono mode: {s}"
        );
    }

    // ------------------------------------------- SGR terminal simulator

    /// Attributes a terminal holds for a character.
    #[derive(Clone, PartialEq, Debug, Default)]
    struct TermAttrs {
        bold: bool,
        dim: bool,
        fg: Option<u8>,
    }

    /// Minimal VT parser: tracks SGR state and a screen of (char, attrs-at-write).
    /// Exactly the state machine a real terminal applies — the point of these tests.
    struct Term {
        attrs: TermAttrs,
        cx: usize,
        cy: usize,
        screen: Vec<Vec<(char, TermAttrs)>>,
    }
    impl Term {
        fn new(w: usize, h: usize) -> Self {
            Term {
                attrs: TermAttrs::default(),
                cx: 0,
                cy: 0,
                screen: vec![vec![(' ', TermAttrs::default()); w]; h],
            }
        }
        fn feed(&mut self, bytes: &[u8]) {
            let s = String::from_utf8(bytes.to_vec()).unwrap();
            let mut it = s.chars().peekable();
            while let Some(c) = it.next() {
                match c {
                    '\x1b' => {
                        assert_eq!(it.next(), Some('['), "only CSI expected");
                        let mut seq = String::new();
                        let fin = loop {
                            let f = it.next().expect("unterminated CSI");
                            if f.is_ascii_alphabetic() {
                                break f;
                            }
                            seq.push(f);
                        };
                        match fin {
                            'm' => self.sgr(&seq),
                            'H' => {
                                let mut parts = seq.split(';');
                                let r: usize = parts.next().unwrap_or("").parse().unwrap_or(1);
                                let c: usize = parts.next().unwrap_or("").parse().unwrap_or(1);
                                self.cy = r - 1;
                                self.cx = c - 1;
                            }
                            _ => {}
                        }
                    }
                    '\r' => self.cx = 0,
                    '\n' => self.cy += 1,
                    other => {
                        self.screen[self.cy][self.cx] = (other, self.attrs.clone());
                        self.cx += 1;
                    }
                }
            }
        }
        /// Apply an SGR parameter list like a terminal does (sticky attributes).
        fn sgr(&mut self, seq: &str) {
            let params: Vec<i64> = seq
                .split(';')
                .map(|p| if p.is_empty() { 0 } else { p.parse().unwrap() })
                .collect();
            let mut i = 0;
            while i < params.len() {
                match params[i] {
                    0 => self.attrs = TermAttrs::default(),
                    1 => self.attrs.bold = true,
                    2 => self.attrs.dim = true,
                    22 => {
                        self.attrs.bold = false;
                        self.attrs.dim = false;
                    }
                    39 => self.attrs.fg = None,
                    38 => {
                        assert!(params[i + 1] == 5, "only 256-color SGR expected");
                        self.attrs.fg = Some(params[i + 2] as u8);
                        i += 2;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        /// The attrs a style must render with: a fresh parse of its param string
        /// starting from reset — self-contained by construction.
        fn expected(style: Style, color: bool) -> TermAttrs {
            let mut t = Term {
                attrs: TermAttrs::default(),
                cx: 0,
                cy: 0,
                screen: vec![],
            };
            let mode = if color { "full" } else { "mono" };
            let params = match (style, mode) {
                (Style::Blank, _) | (Style::DaySea, _) => "0",
                (Style::DayLand, "mono") => "1",
                (Style::DayLand, _) => "1;38;5;114",
                (Style::TwiSea, "mono") | (Style::TwiLand, "mono") => "0",
                (Style::TwiSea, _) => "38;5;244",
                (Style::TwiLand, _) => "38;5;246",
                (Style::NightSea, "mono") | (Style::NightLand, "mono") => "2",
                (Style::NightSea, _) => "2;38;5;238",
                (Style::NightLand, _) => "2;38;5;244",
                (Style::Terminator, "mono") => "1",
                (Style::Terminator, _) => "1;38;5;214",
                (Style::Clock, _) => "1",
            };
            let full = if params == "0" {
                "0".to_string()
            } else {
                format!("0;{params}")
            };
            t.sgr(&full);
            t.attrs
        }
    }

    /// Feed a frame sequence (full then diffs) through the simulator and require
    /// every screen cell to carry exactly the attributes its style mandates —
    /// across style flips, clock updates, both color modes. Regression test for
    /// the sticky-SGR bugs (stale dim/color bleeding into later runs).
    #[test]
    fn sgr_stream_renders_exact_attributes() {
        let rings = vec![
            box_ring(30.0, 60.0, -10.0, 30.0),
            box_ring(-60.0, -20.0, 100.0, 170.0),
        ];
        for &color in &[true, false] {
            let sun1 = Sun {
                decl_deg: 23.44,
                lon_deg: 0.0,
            };
            let sun2 = Sun {
                decl_deg: -23.44,
                lon_deg: 90.0,
            }; // flips shading globally
            let f1 = compose(&params(W, H, &rings, sun1, false, true, "AAAA 00:00:00 A"));
            let f2 = compose(&params(W, H, &rings, sun2, false, true, "BBBB 11:11:11 B"));
            let mut f3 = f2.clone();
            let cl = Cell {
                sym: '2',
                style: Style::Clock,
            };
            let pos = (0..W).find(|&x| f2.get(x, H - 1).sym == '1').unwrap();
            f3.set(pos, H - 1, cl); // clock-only second tick

            let mut term = Term::new(W, H);
            term.feed(&render_ansi(None, &f1, color));
            assert_screen(&term, &f1, color);
            term.feed(&render_ansi(Some(&f1), &f2, color));
            assert_screen(&term, &f2, color);
            term.feed(&render_ansi(Some(&f2), &f3, color));
            assert_screen(&term, &f3, color);
        }
    }

    fn assert_screen(term: &Term, f: &Frame, color: bool) {
        for y in 0..f.h {
            for x in 0..f.w {
                let (ch, attrs) = term.screen[y][x].clone();
                let cell = f.get(x, y);
                assert_eq!(ch, cell.sym, "screen char at ({x},{y})");
                assert_eq!(
                    attrs,
                    Term::expected(cell.style, color),
                    "attributes at ({x},{y}) style {:?}: stale SGR state leaked",
                    cell.style
                );
            }
        }
    }

    /// Direct regression for the reported clock bug: a diff run that touches dim
    /// night cells *before* the clock row must not leave dim/color on the clock.
    #[test]
    fn no_stale_dim_or_color_before_clock_run() {
        let f1 = Frame::new(12, 2, Cell::BLANK);
        let mut f2 = f1.clone();
        f2.set(
            0,
            0,
            Cell {
                sym: '\u{28ff}',
                style: Style::NightLand,
            },
        );
        f2.set(
            1,
            0,
            Cell {
                sym: '\u{28ff}',
                style: Style::TwiLand,
            },
        );
        f2.set(
            5,
            1,
            Cell {
                sym: '7',
                style: Style::Clock,
            },
        );
        let bytes = render_ansi(Some(&f1), &f2, true);
        let mut term = Term::new(12, 2);
        term.feed(&bytes);
        let attrs = term.screen[1][5].1.clone();
        assert_eq!(
            attrs,
            TermAttrs {
                bold: true,
                dim: false,
                fg: None
            },
            "clock must render plain bold, not the night run's dim/color"
        );
        let twi = term.screen[0][1].1.clone();
        assert_eq!(twi.fg, Some(246), "twilight land keeps its color");
        assert!(!twi.dim, "night dim must not leak into twilight land");
    }

    // ------------------------------------------- oval border granularity

    /// The map border must live on the dot grid: cells whose center is outside
    /// the oval but that contain inside-dots render (partial braille / styled
    /// sea), fully-outside cells stay BLANK, and no braille dot is ever set
    /// outside the oval. Regression for the whole-cell jagged border.
    #[test]
    fn map_border_is_dot_granular() {
        let cell_bits: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        for &(w, h) in &[(80usize, 24usize), (120, 40)] {
            let sun = Sun {
                decl_deg: 10.0,
                lon_deg: 0.0,
            };
            let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
            let f = compose(&params(w, h, &rings, sun, false, false, "clock"));
            let mf = geo::MapFrame::new(2 * (w - 2), 4 * (h - 2), 0.0);

            let mut partial = 0usize;
            for cy in 0..h - 2 {
                for cx in 0..w - 2 {
                    let mut oval = 0u8;
                    for (col, bits_col) in cell_bits.iter().enumerate() {
                        for (row, bit) in bits_col.iter().enumerate() {
                            if mf.dot_to_lonlat(2 * cx + col, 4 * cy + row).is_some() {
                                oval |= bit;
                            }
                        }
                    }
                    let cell = f.get(cx + 1, cy);
                    if oval == 0 {
                        assert_eq!(cell, Cell::BLANK, "({cx},{cy}) fully outside oval");
                    } else {
                        assert_ne!(
                            cell,
                            Cell::BLANK,
                            "({cx},{cy}) has inside dots: must render, not blank"
                        );
                        let popcount = oval.count_ones();
                        if popcount < 8 {
                            partial += 1;
                        }
                        // No braille dot outside the oval in any rendered glyph.
                        if let Some(mask) = (cell.sym as u32).checked_sub(0x2800) {
                            let mask = mask as u8;
                            assert_eq!(
                                mask & !oval,
                                0,
                                "({cx},{cy}) glyph has dots outside the oval: {:08b} vs {:08b}",
                                mask,
                                oval
                            );
                        }
                    }
                }
            }
            assert!(
                partial > 10,
                "expected many partial-border cells at {w}x{h}, got {partial}"
            );
        }
    }

    // ----------------------------------------------------- render_plain

    #[test]
    fn plain_color_false_has_no_esc() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, true, "clock"));
        let out = render_plain(&f, false);
        assert!(
            !out.contains(&0x1b),
            "plain color=false must contain no ESC"
        );
        assert!(out.ends_with(b"\n"));
    }

    #[test]
    fn plain_color_true_has_sgr_and_no_cursor_moves() {
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 0.0,
        };
        let rings = vec![box_ring(30.0, 60.0, -10.0, 30.0)];
        let f = compose(&params(W, H, &rings, sun, false, true, "clock"));
        let out = render_plain(&f, true);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("38;5;214"), "terminator accent color expected");
        assert!(
            s.contains("\x1b[0;1m"),
            "self-contained bold expected for day land / clock"
        );
        assert_eq!(
            count_cursor_moves(s.as_bytes()),
            0,
            "no cursor moves in plain output"
        );
        assert!(s.ends_with("\n"));
    }
}

#[cfg(test)]
mod coastline_frame_tests {
    use super::*;
    use crate::coast_data;
    use chrono::{TimeZone, Utc};

    /// With the coastline layer on (default), every land dot of the plain fill
    /// survives and the frame gains terrain-styled dots along the contours.
    #[test]
    fn coastline_layer_adds_and_never_removes() {
        let rings = crate::coast::decode(coast_data::LAND_DATA);
        let t = Utc.with_ymd_and_hms(2024, 6, 21, 12, 0, 0).unwrap();
        let sun = crate::solar::subsolar(t);
        let mut on = RenderParams {
            w: 100,
            h: 30,
            lambda0_deg: 0.0,
            rings: &rings,
            sun,
            draw_twilight_curve: false,
            draw_coastline: true,
            ascii: false,
            clock_line: "clock".to_string(),
        };
        let f_on = compose(&on);
        on.draw_coastline = false;
        let f_off = compose(&on);

        assert_ne!(f_on, f_off, "coastline toggle must change the frame");
        // Cell-wise: a cell that was land stays land; a cell that was sea may
        // become land (outline) but never the reverse; glyphs only gain dots.
        for y in 0..f_on.h {
            for x in 0..f_on.w {
                let (a, b) = (f_off.get(x, y), f_on.get(x, y));
                let land =
                    |s: Style| matches!(s, Style::DayLand | Style::TwiLand | Style::NightLand);
                if land(a.style) {
                    assert!(
                        land(b.style),
                        "coastline must not turn land into sea at ({x},{y})"
                    );
                    let (ma, mb) = (a.sym as u32 - 0x2800, b.sym as u32 - 0x2800);
                    assert_eq!(
                        ma & !mb,
                        0,
                        "glyph lost dots at ({x},{y}): {:08b} -> {:08b}",
                        ma,
                        mb
                    );
                }
            }
        }
        // Terrain coloring: gained cells (sea -> land) carry land styles of the
        // cell's own shading band — spot-check at least one DayLand gain.
        let gained_day = (0..f_on.h)
            .flat_map(|y| (0..f_on.w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                f_on.get(x, y).style == Style::DayLand
                    && !matches!(f_off.get(x, y).style, Style::DayLand)
            })
            .count();
        assert!(gained_day > 0, "expected terrain-colored coastline gains");
    }
}
