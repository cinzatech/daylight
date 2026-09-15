# daylight

A terminal day/night world clock: an ASCII/Unicode world map in the **Kavrayskiy VII**
projection, shaded by real-time **daylight** (terminator + optional civil twilight),
rendered with **8-dot braille characters** for sub-character-cell resolution, with the
current date and time in your timezone.

    git clone … && cd daylight
    cargo run

## Installation

Requires a Rust toolchain (1.74+). Build and install the release binary:

```sh
cargo install --path .
```

or just run it in place:

```sh
cargo run --release
```

No network access, config files, or external data are needed at runtime — the
Natural Earth 1:110m coastline data (public domain) is embedded in the binary
(~18 KB encoded).

## Usage

Run `daylight` in a terminal. `daylight --help`:

```
Terminal day/night world clock: Kavrayskiy VII map, real-time terminator, braille rendering

Usage: daylight [OPTIONS]

Options:
      --center <DEG>   Central meridian, degrees east [-180..180].
                       Default: 15° × UTC offset of the local timezone captured at startup.
      --utc            Clock in UTC; central meridian 0°
      --twilight       Also draw the civil twilight (−6°) curve
      --no-coastline   Do not draw the one-dot coastline outline
      --ascii          Render without braille ('#' land, '.' terminator) for limited fonts
      --color <WHEN>   When to use color: auto | always | never  [default: auto]
      --once           Render one frame to stdout and exit (implied when stdout is not a TTY)
  -h, --help           Print help
  -V, --version        Print version
```

### Interactive keys

| Key | Action |
| --- | --- |
| `q`, `Esc` | quit |
| `Ctrl-C` | quit (exit code 130) |
| `Ctrl-Z` | suspend; the terminal is restored and repainted on resume |
| `u` | toggle UTC/local clock |
| `c` | re-center the map to the current timezone |
| `t` | toggle the civil twilight curve |
| `o` | toggle the coastline outline |
| `r` | force repaint |

### Examples

```sh
daylight                  # interactive; centered on your timezone
daylight --utc            # UTC clock, Greenwich-centered map
daylight --center 90      # map centered on 90°E
daylight --twilight       # also draw the −6° twilight curve
daylight --no-coastline   # area fill only, no outline
daylight --ascii          # '#'/'.' glyphs instead of braille
daylight --once > map.txt # one static frame (also automatic when piping)
```

## Behavior notes

- The clock runs in the timezone current at startup; DST transitions while running
  are still shown correctly. Only the map's central meridian is pinned at startup —
  press `c` to re-center live.
- Honors `NO_COLOR` and `TERM=dumb`; degrades to dim/bold attributes without color.
- Well-mannered terminal citizen: alternate screen, symmetric raw-mode
  setup/teardown (also on panics and signals), no bell, no mouse capture, scrollback
  untouched. Exit codes: 0 clean, 2 usage error, 1 runtime error, 130/143/129 for
  INT/TERM/HUP.
- Lean: while idle the process is blocked on terminal input (~0% CPU); frames are
  diffed and only changed cells are written.

## Development

```sh
cargo test                          # unit + CLI self-spawn + PTY integration tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Map data can be regenerated from `data/ne_110m_land.geojson`:

```sh
cargo run --example datagen -- data/ne_110m_land.geojson
```

See [PLAN.md](PLAN.md) for the full design and requirements. Linux/UNIX only.
