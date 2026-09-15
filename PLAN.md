# PLAN — `daylight`: a terminal day/night world clock

> **Deviation (user-directed, §15 rule):** R5 (single-file `src/main.rs`) is dropped.
> The application uses proper modularization under `src/` (`cli`, `solar`, `geo`, `coast`,
> `coast_data`, `raster`, `frame`, `term`, `app`), with integration tests in `tests/`.
> Everything else in this plan stands unchanged.

A small, single-file Rust application that renders an ASCII/Unicode map of the Earth in the
**Kavrayskiy VII** projection, shaded by real-time **daylight** (terminator + civil twilight),
with the **current date and time** displayed in the user's timezone. Rendering uses
**8-dot braille patterns** (U+2800–U+28FF) for sub-character-cell resolution. The program is
event-driven (no busy loops), updates only when needed, and behaves like a well-mannered
UNIX citizen.

---

## 1. Requirements (distilled from the brief)

| # | Requirement |
| --- | --- |
| R1 | World map in Kavrayskiy VII projection |
| R2 | Daylight lines (terminator; twilight as an option) driven by the computer's internal clock |
| R3 | Current date and time displayed, in the timezone current at program start |
| R4 | Sub-cursor rendering via 8-dot braille characters ("8-pin morse") |
| R5 | Entire application implemented in a single Rust file (`src/main.rs`) |
| R6 | Clean, maintainable code; lean resource usage; no busy loops; updates only as necessary |
| R7 | Polite terminal behavior, exactly as expected of UNIX programs |
| R8 | Config flags/options with the usual conventions (`--help`, `--version`, exit codes) |
| R9 | Appropriately tested — **execution of this plan ends only when the tests demonstrate correct rendering with all features** |
| R10 | Rust is the only language present anywhere in the repository |

---

## 2. Constraints and interpretations

- **Single file.** All application and test code lives in `src/main.rs` (inline `#[cfg(test)]`
  modules). `Cargo.toml` is cargo-required metadata, not application code. A small
  **Rust** developer tool `examples/datagen.rs` regenerates the embedded map data inside
  `src/main.rs` (see §7) — it is build tooling, not part of the application, and it is Rust.
- **Rust only.** No Python/JS/shell scripts, no Makefile, no CI YAML. All orchestration is
  plain `cargo` commands; all verification tooling is Rust (`cargo test`, an `examples/`
  binary, dev-dependencies).
- **UNIX focus.** The signal layer is `cfg(unix)`; crossterm itself is cross-platform, but
  Windows is explicitly out of scope.
- **Terminology.** "8-pin morse characters" = Unicode **braille patterns** U+2800–U+28FF,
  each cell packing a 2-wide × 4-tall dot matrix.
- **"Clock centered on the current timezone at the time of starting"** is interpreted as:
  (a) the displayed clock runs in the local timezone (via `chrono`), and
  (b) the map's **central meridian** is set from the UTC offset captured once at startup
  (≈ 15° × offset-hours), so the user's longitude sits near the map center.
  Both are overridable (`--center`, `--utc`). See §13.

---

## 3. Dependencies

| Crate | Kind | Why | Why it's trusted / why hand-rolled alternative loses |
| --- | --- | --- | --- |
| `chrono` | runtime | Calendar/timezone math, `Local` offset, formatting | The standard, widely maintained time crate; reimplementing tzdb/DST is a liability |
| `clap` (derive) | runtime | CLI flags with conventional `--help`/`--version`, exit code 2 on usage error, validation | De-facto standard; hand-rolling conventions correctly is fiddly, low-value code |
| `crossterm` | runtime | Raw mode, alt screen, cursor/wrap control, terminal size, key/resize events — including all escape-sequence parsing | The standard terminal I/O crate (ratatui's own backend), actively maintained; exactly the wheel not worth reinventing |
| `signal-hook` | runtime | SIGINT/TERM/HUP/TSTP/CONT → clean shutdown, suspend/resume | The standard signal-handling crate; crossterm covers SIGWINCH (as `Event::Resize`) but not process signals |
| `serde_json` + `serde` | dev only (`datagen` example) | Parse Natural Earth GeoJSON once at data-generation time | Not in the shipped binary |
| `portable-pty` | dev only (tests) | Spawn the app in a pseudo-terminal from tests | From the wezterm project, actively maintained; lets us *prove* terminal politeness in Rust |

**Deliberately NOT used (per user direction):**

- **No TUI/widget framework** (`ratatui`/`tui-rs`). The app is one full-screen canvas plus
  one text line; a widget/layout framework buys nothing we need. `crossterm` supplies the
  terminal mechanics (modes, screen, input, resize); frame composition and diffing — the
  parts specific to our braille canvas — stay ~200 lines of our own code (§8–§9).
- **No async runtime.** crossterm's synchronous API is all the app needs.
- **No solar-position crate.** Existing crates are stale or heavyweight; the NOAA low-precision
  algorithm is ~50 lines with authoritative documentation (§6.2).
- **No map-data crate.** None reputable bundles coastlines; we embed public-domain data (§7).

---

## 4. Architecture — single-file layout (`src/main.rs`)

Ordered top-to-bottom; each module is a plain `mod` block with a comment header. Estimated
~1,800–2,400 lines including tests and the generated data constant.

```text
src/main.rs
├── docs        module-level design notes, data provenance, regeneration instructions
├── cli         clap derive Args → Config
├── solar       NOAA declination/equation-of-time, subsolar point, sun elevation,
│               terminator curve sampling
├── geo         Kavrayskiy VII forward/inverse, central-meridian rotation, map frame math
├── coast       generated `LAND_DATA` const + decoder → polygons (lat/lon rings)
├── raster      dot-grid canvas: geographic scanline land fill, dot plotting, braille packing
├── frame       Cell/Frame buffers, theme/colors, frame composition (map + clock line),
│               Frame→ANSI diff writer (interactive) and plain writer (one-shot)
├── term        crossterm setup/teardown guards, signal flags, event loop,
│               winsize query, panic hook
├── app         App state, update scheduling (what to recompute when), run modes
├── main        arg parsing, mode dispatch, error handling, exit codes
└── tests       #[cfg(test)] mod tests — unit, self-spawn CLI, and pty integration tests
```

Data flow (all pure functions, no hidden state — this is what makes it testable):

```text
(now: DateTime<Utc>, cfg: Config, size: Size)
   → solar::subsolar_point(now)                      (δ, λs)
   → raster::render_map(size, λ0, polygons, δ, λs)   → dot layers
   → frame::compose(...)                             → Frame (cells: char+fg+bg)
   → term::write_diff(prev, next)                    → ANSI bytes to stdout
```

---

## 5. Filesystem layout

```text
Cargo.toml
src/main.rs               the entire application (+ inline tests)
examples/datagen.rs       Rust tool: NE GeoJSON → encoded const, spliced into src/main.rs
data/ne_110m_land.geojson committed source data (public domain, provenance in §7)
PLAN.md
```

`.gitignore`: `/target`. No README required (`--help` is the doc); may add later.

---

## 6. The math

### 6.1 Kavrayskiy VII (verified: Wikipedia, citing Snyder 1993, *Flattening the Earth*, p. 202)

λ, φ in radians; λ measured from the chosen central meridian:

```text
forward:  x = (3λ/2) · sqrt(1/3 − (φ/π)²)        y = φ
inverse:  φ = y                                   λ = 2x / (3 · sqrt(1/3 − (y/π)²))
```

Properties we test against:

- y is exactly latitude (parallels are straight, evenly spaced).
- At the equator: |x| = (√3/2)·λ, so full equator width = √3·π ≈ 5.441.
- At the poles (|φ| = π/2): |x| = 0.5 · equator value → **pole line is exactly half the
  equator length**.
- `(φ/π)² ≤ 0.25 < 1/3`, so the square root never hits zero → inverse is total and cheap.
- Symmetric: f(−λ) = −f(λ), f(−φ) = f(φ) (test inputs probe both).

Map frame: rotate longitude by −λ₀ (central meridian), normalize to (−π, π]. Because
terminal cells are ≈ 1:2 (w:h) and braille packs 2×4 dots per cell, **dot spacing is
approximately square**; scale the map uniformly to fit the canvas with a 1-cell margin.
Screen y grows downward → flip. Cells whose inverse lands outside the map oval
(|x| > (√3/2)·√(1/3−(φ/π)²)·π-equivalent bound) are left blank.

### 6.2 Solar position — NOAA "General Solar Position Calculations" (gml.noaa.gov)

Low-precision (±0.01° declination, valid across ~1950–2050, far beyond visual need):

```text
γ      = 2π/365 · (day_of_year − 1 + (UTC_hours − 12)/24)     (366 in leap years)
eqtime = 229.18 · (0.000075 + 0.001868 cos γ − 0.032077 sin γ
                  − 0.014615 cos 2γ − 0.040849 sin 2γ)          [minutes]
δ      = 0.006918 − 0.399912 cos γ + 0.070257 sin γ − 0.006758 cos 2γ
         + 0.000907 sin 2γ − 0.002697 cos 3γ + 0.00148 sin 3γ   [radians]

subsolar point:  lat = δ
                 lon = −15° · (UTC_decimal_hours + eqtime/60), normalized to (−180, 180]
```

Sun elevation at (φ, λ) — spherical Earth, hour angle H = λ − lon_subsolar (degrees):

```text
sin α = sin φ · sin δ + cos φ · cos δ · cos H
day: α ≥ 0 · civil twilight: −6° ≤ α < 0 · night: α < −6°
```

Terminator curve (α = 0 locus) for crisp drawing: for each φ,
`cos H₀ = −tan φ · tan δ`; if |tan φ · tan δ| ≤ 1 the terminator passes through
λ = lon_subsolar ± H₀ (two branches joining at the polar-circle tangent); otherwise
that latitude is in polar day/night. The `--twilight` flag draws the same curve for
α = −6°. Curve samples are plotted as braille dots (§8). At equinox (δ ≈ 0) the branches
degenerate to the meridians lon_subsolar ± 90° — an explicit test case.

### 6.3 Precision and types

`f64` everywhere. Trig in radians; degrees only at the CLI/UX boundary. No `unsafe` math,
no global mutable state; all functions take values and return values (pure → testable).

---

## 7. Map data — provenance, encoding, regeneration (all Rust)

- **Source:** Natural Earth **1:110m land** polygons (`ne_110m_land.geojson` from the
  official `nvkelso/natural-earth-vector` repository). **Public domain** — safe to commit
  and embed. 110m generalization (~km scale) is ideal for a braille world map
  (one dot ≈ 0.5–1° at typical sizes).
- **Committed:** `data/ne_110m_land.geojson` (data, not code; ~a few hundred KB) so builds
  and regeneration never touch the network.
- **Encoding** (produced by `examples/datagen.rs`, spliced into `src/main.rs` between
  `// @generated BEGIN` / `// @generated END` markers as a single `static LAND_DATA: &str`):
  vertices rounded to 2 decimals (±0.005° ≈ 550 m, below source resolution), per-ring
  zigzag varint deltas of centidegrees, base85-encoded. Expected ~10–20 KB of string for
  ~5–7k vertices (verified at M3; if it exceeds 40 KB, revisit quantization).
- **Regeneration** (documented in the file header comment):
  `cargo run --example datagen -- data/ne_110m_land.geojson`
  The tool parses GeoJSON (`serde_json`), encodes, and rewrites the marked region of
  `src/main.rs` in place. Idempotent; diff-friendly (single line, `#[rustfmt::skip]`).
- **Decoder** (in-app, ~40 lines): base85 → varints → Δ-centidegrees → `Vec<Polygon>`,
  where `Polygon = Vec<(f64 lat, f64 lon)>` rings. Even-odd semantics.
  Natural Earth polygons are already split at the antimeridian; after central-meridian
  rotation, segments whose Δλ exceeds 180° are split at ±180° during rendering (§8).

---

## 8. Rendering pipeline

**Canvas.** For terminal size W×H cells: dot grid of (2W)×(4H) dot positions... minus a
1-cell border and a reserved bottom line for the clock. Each character cell owns an 8-bit
braille mask:

```text
dot bits (Unicode braille):  col 0 (left)  rows 0..3 → 0x01 0x02 0x04 0x40   (dots 1,2,3,7)
                             col 1 (right) rows 0..3 → 0x08 0x10 0x20 0x80   (dots 4,5,6,8)
char = '\u{2800}' | mask
```

**Land fill — geographic scanline (exact, fast).** Because parallels are straight lines in
this projection, every dot *row* is a single latitude φ_row. For each dot row:

1. Compute crossings of every polygon edge with latitude φ_row (ray-cast, eastward
   even-odd rule; vertex-exact rows nudged by a tiny ε — standard scanline trick).
2. Sort crossings; fill spans pairwise, converting each span's longitudes to x via the
   forward formula (x is linear in λ at fixed φ) and thus to dot columns.
3. Dateline: segments spanning >180° after rotation are split at ±180° before crossing tests.
   Antarctica/pole handling falls out of even-odd on the committed rings (probe-tested).

Complexity per (re)build: O(rows × edges + fills); only on resize. Map *never* needs
recomputation per tick — only shading/terminator do (§10).

**Layers composed per cell:**

1. **Background shading** (day / civil-twilight band / night) from sun elevation at the
   cell's inverse-projected center. Parallels are rows → per-row φ, sin φ, cos φ precomputed
   once → per-cell cost is one `cos H` multiply-add. O(cells) per minute.
2. **Land dots** from the scanline fill (static per size+λ₀).
3. **Terminator dots** (and `--twilight` −6° curve) — plotted as braille dots along the
   analytic curve, distinct fg color.
4. **Clock line** (bottom, centered): `Wed 2026-09-26 14:03:22 UTC+02:00 · ☉ 12.3°S 45.6°W`
   (local time; subsolar point readout). Degrades gracefully on narrow terminals
   (drop subsolar → drop weekday → time only). Right edge: nothing; map occupies all else.

**Colors.** Theme (fixed, tasteful): day sea = default bg, day land = bright fg;
night = dimmed fg + dark bg; twilight band = intermediate; terminator = accent. Dim/bold
SGR carries night/day even on color-less terminals. 256-color indexes when `TERM` suggests
support, basic 16 otherwise, mono when `NO_COLOR` or `TERM=dumb` or `--color never`
(shading then relies on dim attribute; mono fallback still legible: land dots vs. blank sea

- terminator dots). `--ascii` flag renders without braille (`#` land, `.` terminator) for
fonts lacking braille glyphs.

**Frame + diff writer.** `Frame = Vec<Cell{sym, fg, bg, attrs}>`. On each update the app
builds the next `Frame`, then emits only runs of changed cells: absolute cursor positioning
only when runs are discontiguous, SGR transitions only on change, one buffered write
through a single locked-stdout flush.
This is "only updates as necessary" at the wire level. One-shot mode writes the whole frame
as plain rows (ANSI colors only if enabled).

---

## 9. Terminal backend (`crossterm` + `signal-hook`)

**Enter/leave symmetry.** On interactive start (stdout is a TTY and not `--once`):
`enable_raw_mode()`, then `EnterAlternateScreen`, hide cursor, `DisableLineWrap`, clear —
all via crossterm commands through one locked stdout handle. On exit (every path): the
reverse of each plus an SGR reset and `disable_raw_mode()`.

**Guards.** `TermGuard` (Drop restores), plus a `panic!` hook that restores and then defers
to the previous hook — terminal state survives panics. Suspension (Ctrl-Z key or SIGTSTP):
restore → reset SIGTSTP to default → raise it (the recipe from signal-hook's own docs);
on SIGCONT: re-init raw mode + screen, invalidate the frame cache, full redraw.

**Signals.** `signal-hook` for INT, TERM, HUP, TSTP, CONT. Raw mode turns off ISIG, so
Ctrl-C/Ctrl-Z arrive as ordinary crossterm key events and take the same code paths as
external signals; `kill`-delivered signals flip atomic flags checked on every loop wake
(≤ 1 s latency — the loop wakes each second regardless). Exit codes after restore:
SIGINT→130, SIGTERM→143, SIGHUP→129 (128+n convention). SIGWINCH is handled inside
crossterm and surfaced as `Event::Resize`.

**Input.** crossterm owns all terminal input decoding — key events, modifiers, and
lone-Esc vs. escape-sequence disambiguation. No hand-rolled input parsing anywhere.

**Event loop (single thread).**

```text
loop {
    draw_if_dirty();
    timeout = until next whole second (clock shows seconds);    // ≤ 1 s, usually < 1 s
    if crossterm::event::poll(timeout)? {                       // blocks; no busy loop
        match crossterm::event::read()? {
            Key(q/Esc/Ctrl-C) → quit
            Key(Ctrl-Z)       → suspend
            Key(u/c/t/r)      → toggle actions · Resize(w,h) → rebuild + full frame
        }
    }
    if signal flags set → quit / suspend / resume handling
    now = Utc::now();
    if minute(now) changed → recompute shading + terminator
    if second(now) changed → update clock cells (+ subsolar text)
}
```

If there is no usable controlling terminal, `event::poll` is skipped in favor of sleeping
to the next second — still zero busy-waiting; keys are unavailable, signals still honored.

Recompute budget per second wake is O(cells) trig (sub-millisecond in release) and only
touches the frame when something actually changed (second/minute/resize). While idle the
process is blocked in `event::poll`/sleep — 0% CPU, proven by test (§14).

**Sizes and TTY quirks.** Size via `crossterm::terminal::size()`, re-queried on
`Event::Resize`; 0-valued rows/cols → fall back 80×24. Degenerate sizes (rows < 3) →
clock line only. Never write to the last column with autowrap disabled; never ring the
bell; never capture the mouse. TTY detection via `crossterm::tty::IsTty`.

**Non-TTY stdout.** One-shot frame to stdout (plain; colored only with `--color always`),
exit 0. `--once` forces the same path even on a TTY.

---

## 10. Resource contract ("lean")

| Concern | Contract |
| --- | --- |
| CPU idle | Blocked in crossterm `event::poll` (or sleep in timer-only mode); ~0% between events. Test asserts < 100 ms CPU over a 2.5 s idle window. |
| CPU active | Full frame rebuild < 5 ms (release) at 120×40; per-second update touches O(changed cells). |
| Syscalls | One buffered `write(2)` per frame diff; no repeated polling of time (`now` sampled once per wake). |
| Memory | Static data ~20 KB; frame buffers 2×W×H cells reused (no per-tick allocation after warm-up). |
| Startup | No I/O beyond terminal setup + data decode; < 100 ms cold start. |
| Allocation | All per-tick work reuses preallocated buffers (`Vec::clear()` + reuse). |

---

## 11. Terminal politeness contract (acceptance-tested where possible)

- Alternate screen used and exited; user scrollback untouched.
- Cursor hidden and restored; no cursor litter between frames (diff moves cursor only to changed runs).
- Raw mode entered/exited symmetrically; restored on quit, signals, and panics.
- `^Z`/`fg` job control works (restore on TSTP, re-init + full repaint on CONT).
- Exit codes: 0 clean; 2 usage (clap); 1 runtime error; 130/143/129 for INT/TERM/HUP.
- Errors/diagnostics → stderr, rendering → stdout; nothing extraneous on either.
- Honors `NO_COLOR`, `TERM=dumb`, non-TTY stdout (one-shot), missing stdin (no keys, still runs).
- No bell, no mouse capture, no title changes; autowrap disabled only while we own the screen.
- `--help` exits 0 and is accurate; `--version` prints `daylight <semver>`.

---

## 12. CLI specification (clap derive, conventional behavior)

```text
daylight [OPTIONS]

Options:
      --center <DEG>   Central meridian, degrees east [-180..180].
                       Default: 15° × UTC offset of the local timezone captured at startup.
      --utc            Clock in UTC; central meridian 0° (shorthand for the default center).
      --twilight       Also draw the civil twilight (−6°) curve.
      --ascii          Render without braille ('#' land, '.' terminator) for limited fonts.
      --color <WHEN>   auto | always | never  [default: auto]
      --once           Render one frame to stdout and exit (implied when stdout is not a TTY).
  -h, --help           Print help
  -V, --version        Print version
```

Interactive keys: `q`, `Esc`, `Ctrl-C` quit · `Ctrl-Z` suspend · `u` toggle UTC/local clock · `c` re-center to
current timezone · `t` toggle twilight · `r` force repaint. `--help` documents them.

---

## 13. Timezone centering (decision record)

Captured **once at startup**: `chrono::Local::now().offset()` →
(a) default `--center` = clamp(15° × offset_hours, −180..180);
(b) the clock line's display timezone. The clock itself renders from live `chrono::Local`
(so a DST transition while running still shows correct civil time); only the map center is
pinned at startup (a map that jumps meridians mid-session would be jarring; `c` re-centers
live). `--utc` / `--center` override both. Rationale documented here and in `--help`.

---

## 14. Testing plan (all tests in `src/main.rs`, Rust-only tooling)

### 14.1 Unit — pure functions

| Area | Tests (fixed inputs, no wall clock) |
| --- | --- |
| `geo` | forward reference values (hand-computed from §6.1: equator/pole half-widths = √3·π/2 and half of it; x at λ=π/2, φ=0 ≈ 1.360); `y == φ`; forward/inverse round-trip over a fixed grid incl. poles, dateline, ±center rotations; odd/even symmetry. |
| `solar` | declination at 2000-06-21 ≈ +23.44° ±0.3, 2000-12-21 ≈ −23.44° ±0.3, equinox ≈ 0 ±0.5; equation of time ≈ −14 min (Feb 11) / +16 min (Nov 3) ±1; subsolar lon at 12:00 UTC ≈ 0 ±4°, at 18:00 UTC ≈ −90 ±4°; elevation ≈ 90° at subsolar point; polar night at December solstice +80° lat. |
| `coast` | decode → expected ring/polygon counts, bbox ⊆ [−90..90]×[−180..180], vertex-count header integrity. |
| `raster` | braille bit layout (dot1 → U+2801, dot8 → U+2880, all-dots → U+28FF); land probes: Rome, Cairo, Beijing, Sydney, Buenos Aires, Anchorage, Antarctic plateau (land); Gulf of Guinea, mid-Pacific, N-Atlantic, Indian Ocean (sea); land fraction of map oval within an acceptance band tuned at M4 (sanity ≈ 25–35%); concave-polygon scanline case; dateline-splitting case; 1×1 and 200×60 grids don't panic. |
| `frame` | composed frame at **fixed instants** (UTC + pinned offset, fixed size): June solstice 12:00 UTC → probe cells: (80°N,0°) day, (80°S,0°) night; December solstice reversed; equinox → shading flips across lon_subsolar ± 90° meridians; terminator braille dots present on the analytic curve (sampled points' cells non-blank); clock line exact string match `Wed 2024-06-19 ...` style incl. `--utc` and offset cases; structural invariants: every plain-text row same width, map rows contain only braille/space, no control bytes; two sizes (80×24, 120×40) both pass. |
| `diff` | no output when frame unchanged; single-cell change emits a single changed run. |

### 14.2 Self-spawn CLI tests (`std::env::current_exe()`, piped stdio)

`--help` → exit 0 + usage text; `--version` → semver line; `--center 999` → exit 2 + clap
error on stderr; plain run with piped stdout → one frame + exit 0; `--color always` frame
contains SGR, `--color never` contains none.

### 14.3 PTY integration tests (`portable-pty`, cfg(unix))

- Launch in pty → alt-screen enter + braille glyphs present; send `q` → exit 0, output tail
  contains restore sequences (`?1049l`, `?25h`), termios restored (verified from parent via
  pty attributes).
- Send Ctrl-C byte → clean exit 130 with restore sequences.
- `kill(SIGTERM)` → exit 143 with restore sequences.
- Resize pty → app repaints at new size (frame width changes, no crash).
- **Idle CPU**: sample `/proc/<pid>/stat` utime+stime across a 2.5 s idle window
  (cfg linux) → delta < 100 ms — proves no busy loop.
- `SIGTSTP`/`SIGCONT`: suspend → restore sequences seen; resume → repaint; still quits cleanly.

### 14.4 Acceptance matrix (maps requirements → evidence)

| Req | Evidence |
| --- | --- |
| R1 map | land probe tests + `frame` invariants + visual check (M8) |
| R2 daylight | solstice/equinox shading probes; terminator-dot tests; live pty run |
| R3 clock/tz | clock-line exact-match tests (local, `--utc`, `--center`); pty run |
| R4 braille | bit-layout tests; map-rows-braille-only invariant |
| R5 single file | repo layout (§5) |
| R6 lean | idle-CPU pty test; no-diff no-write test; per-tick O(cells) by construction |
| R7 politeness | §14.3 suite + §11 checklist |
| R8 CLI | §14.2 suite |
| R9/R10 | entire suite is `cargo test` / `cargo clippy` / `cargo fmt` — Rust only |

Final visual gate (human, once, at M8): run interactively in a real terminal; compare shape
against reference Kavrayskiy VII (Snyder/Wikipedia figure); confirm continents, terminator
position vs. an independent source (e.g., timeanddate terminator), clock correctness,
twilight flag, `--ascii`, `--once` piped to a file.

---

## 15. Implementation milestones (each ends green: build + tests + fmt + clippy)

| # | Milestone | Deliverable | Verification |
| --- | --- | --- | --- |
| M0 | Scaffold | `cargo` bin crate, deps, `.gitignore`, module skeletons, CI-less gates documented | `cargo build && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings` |
| M1 | `geo` | Kavrayskiy VII fwd/inv + frame math | §14.1 geo tests |
| M2 | `solar` | NOAA decl/EoT, subsolar, elevation, terminator sampling | §14.1 solar tests |
| M3 | Data | commit `data/ne_110m_land.geojson`; `examples/datagen.rs`; `LAND_DATA` spliced; decoder | datagen idempotent; decoder tests; const ≤ 40 KB |
| M4 | `raster` | scanline fill + braille packing + dot canvas | §14.1 raster tests (tune land-fraction band) |
| M5 | `frame` | shading, terminator layer, clock line, theme, plain writer, diff writer | §14.1 frame + diff tests |
| M6 | CLI + one-shot | clap wiring, `--once`, exit codes, self-spawn tests | §14.2 |
| M7 | `term` + `app` loop | crossterm raw mode/alt screen/events, signal-hook lifecycle, diff writer wired, job control, panic hook | §14.3 pty suite |
| M8 | Polish & acceptance | `--ascii`, degradation ladder, NO_COLOR/dumb, perf pass (§10), docs in `--help` | full matrix §14.4 + visual gate |

Commit per milestone. If a milestone's tests expose design flaws, update this plan file in
the same commit (decision-record discipline).

---

## 16. Definition of Done

- [x] `cargo test` green — every test named in §14 exists and passes (77 unit + 8 self-spawn CLI + 6 pty = 91).
- [x] `cargo fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean.
- [x] Fixed-instant frame tests prove: recognizable land (probes), correct day/night
      asymmetry at both solstices and equinox, terminator dots on the analytic curve,
      correct clock string (local, `--utc`, `--center`).
- [x] PTY suite proves: clean quit, Ctrl-C/SIGTERM exits with codes 130/143 and terminal
      fully restored, resize repaints, suspend/resume works, idle CPU < 100 ms / 2.5 s.
- [x] `--once` piped output is a correct standalone frame; `--help`/`--version` conventional.
- [x] No language other than Rust anywhere in the repo (grep-audited: no `.py/.js/.sh`,
      no Makefile, no CI YAML).
- [ ] Visual gate §14.4 passed by a human in a real terminal *(pending — requires a human;
      the piped `--once` frame and pty captures show a correct Kavrayskiy VII world map,
      terminator, and clock line, but the formal human sign-off remains open).*
- [x] PLAN.md updated with any deviations recorded (single-file → modular, this file header).

---

## 17. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Braille glyphs missing/broken in some fonts | `--ascii` fallback; documented in `--help` |
| Cell aspect ≠ 1:2 in exotic terminals | uniform scale keeps shape recognizable; constant documented; acceptable for v1 |
| Dateline/polar scanline bugs (rotated Antarctica, Bering Strait) | dedicated raster tests (§14.1) incl. `--center 180` and `--center 90` cases |
| Signal latency ≤ 1 s (flags checked per wake) | by design — the loop wakes every second anyway (§9) |
| crossterm event source unavailable (no controlling terminal) | startup probe → timer-only loop; keys disabled, signals still honored (§9) |
| tz changes mid-run (travel, DST) | clock follows live `Local`; center pinned by design (§13), `c` re-centers |
| Land-fraction test brittle across data regenerations | band calibrated once at M4 against committed data (data is pinned in-repo → deterministic) |
| `portable-pty` / `/proc` platform gaps | pty suite `cfg(unix)`, CPU test `cfg(target_os = "linux")`; other assertions OS-independent |
| Encoded data larger than budget | quantization/encoding revisited at M3 (options: 1-decimal quantization of 110m interior points — still ≥ source resolution) |
| Performance regression | per-frame rebuild budget asserted in a (release-mode) timing smoke test with generous margin |

## 18. Out of scope (v1)

Windows; city labels; other projections; nautical/astronomical twilight; mouse/pan/zoom;
network features; i18n; publishing to crates.io. All are natural follow-ups; none affect
the architecture.
