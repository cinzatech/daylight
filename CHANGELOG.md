# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
