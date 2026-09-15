# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `--interval 0` used to busy-spin the rotating loop at 100% CPU, and values above
  1000 ms were silently clamped to 1000: the flag is now validated to
  [10..1000] ms (out-of-range is a usage error, exit 2) and the range is
  documented in `--help`.
- Negative `--center` values (e.g. `--center -120`) were rejected by the argument
  parser as unknown flags — the documented [-180..180] range is now reachable for
  the whole western hemisphere.
- stdout write failures (e.g. `daylight --once | head` hitting EPIPE, or a lost
  TTY) were silently swallowed; they are now reported on stderr with exit code 1.
- SIGQUIT (Ctrl-\) terminated without restoring the terminal; it is now caught
  like INT/TERM/HUP: restore, then exit 131.
- The terminator/twilight curve left a small unclosed gap (~3 dots) at each
  poleward tip: the tangent latitude is now emitted as a merged branch pair, and
  the exact tangent row is admitted despite floating-point rounding of cos H.
- Solar fractional-year divisor is now 365 unconditionally (NOAA's calibration);
  the previous leap-year 366 roughly doubled declination error in leap years.
- The terminal size is re-queried after suspend/resume, so a resize performed
  while suspended repaints at the right size immediately.
- A persistent `crossterm::event::poll` error degraded into a busy loop; it now
  falls back to a bounded sleep.
- `NO_COLOR=""` (empty) used to disable color; per the no-color.org convention
  only non-empty values do now.

### Changed

- `datagen` (map regeneration) now validates its source: per-vertex latitude/
  longitude bounds, minimum ring size, and the rasterizer's pre-split-at-
  antimeridian data contract are enforced with loud errors instead of silently
  producing a corrupted map. The rasterizer likewise aborts on contract-violating
  edges rather than mis-rendering them.
- Performance while rotating: the land-polygon edge list is precomputed once at
  startup instead of per frame, and oval membership is computed once per compose
  instead of ~5x per dot.

### Documentation

- `INTERFACES.md` (the stale module-contract document) was removed: the
  source modules' own docs are the specification — the unit conventions live
  in the `geo`/`solar` headers, and `coast.rs`/`datagen.rs` document the exact
  wire format. A second copy of the contract had already drifted in five
  places and guaranteed more drift.
- `PLAN.md` is demoted to a frozen design record (banner added): it remains
  the source of the project's rationale and decision records, but the README
  and the source docs are the living documentation.
- README `--help` block replaced with the real output; exit codes now include
  QUIT→131; the idle-CPU claim notes the rotation-off qualifier; the PLAN.md
  sections that described v1.0 behavior inaccurately (solar accuracy and
  formulas, datagen splice target, rotation-time recomposition, CLI spec,
  test counts) were corrected in place before the freeze.

### Added

- Tests: SIGHUP→129 and SIGQUIT→131 exit codes, Ctrl-Z *key* suspend/resume,
  NO_COLOR / NO_COLOR="" / TERM=dumb color resolution on a TTY, `--interval`
  bounds, `--center` boundaries (both dateline meridians), the terminator tangent
  closure, the raster data-contract enforcement, and a registration test pinning
  raster's projection math to `geo`'s.

## [1.0.0] - 2026-09-15

### Added

- Terminal day/night world clock: Kavrayskiy VII projection world map in
  braille, shaded by real-time sun position (day / civil twilight / night)
  with a live terminator line, plus an ASCII fallback for limited fonts.
- Real-time clock line in the local timezone (or UTC with `--utc`),
  DST-correct across transitions while running.
- One-dot outline around the map oval, drawn in the terrain color
  (default on; `--no-outline`, `o` toggles live).
- Slow eastward map rotation, one full turn in ~6 minutes, redrawing
  every 100 ms for fluid motion (default on; `--no-rotate`, `a` toggles
  live; redraw interval tunable via `--interval`).
- Interactive keys: `q`/`Esc`/`Ctrl-C` quit, `Ctrl-Z` suspend/resume,
  `u` UTC toggle, `c` re-center to current timezone, `t` twilight curve,
  `o` map outline, `a` rotation, `r` repaint.
- `--center` to pin the central meridian, `--twilight` civil-twilight
  curve, `--color auto|always|never`, `--once` for one-shot rendering.
- `NO_COLOR` and `TERM=dumb` support with a dim/bold monochrome
  degradation path.
- Natural Earth 1:110m coastline data (public domain) embedded in the
  binary — no runtime downloads.
- AGPLv3 licensing; README with installation and usage documentation.

### Fixed

- Terminal color state corruption: clock characters randomly changing
  color and stale dim/brightness after night cells. All style
  transitions now emit self-contained SGR resets.
- Jagged whole-cell map borders: the oval border is now dot-granular.
- Inverted land fill above ~66–70°N and corrupted far-south (Antarctica)
  fills: the scanline fill was rewritten in raw coordinates and is
  verified against an independent point-in-polygon oracle.

### Performance

- While idle (rotation off) the process blocks on terminal input at
  ~0% CPU; frames are diff-rendered, only changed cells are written.
