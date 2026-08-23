# ambientd

A small zero-dependency Rust daemon that automatically adjusts screen (and
keyboard) brightness based on the ambient light sensor (ALS) on any Linux
system. It is **not** tied to a compositor or desktop environment: everything
happens at the kernel/logind level, so it behaves identically under GNOME, KDE,
sway, Hyprland, i3, or a bare TTY console.

It polls the IIO ambient light sensor exposed by the AMD Sensor Fusion Hub,
smooths the readings, maps them onto a logarithmic brightness curve, and drives
the backlight through `brightnessctl` (which uses logind, so it works
unprivileged).

## Requirements

- Linux with an IIO ambient light sensor exposed in sysfs
  (`/sys/bus/iio/devices/*/in_illuminance_raw`)
- `brightnessctl` and a logind seat session (`loginctl` should list a session
  for your user)
- systemd, only for the convenience units installed by `install.sh` — the
  binary itself has no such dependency

Confirmed working on an **ASUS Zenbook 14 OLED (UM3406HA)** running
Omarchy/Hyprland. Everything is configurable, so other ALS-equipped laptops
should work too — see `--sensor-dir`, `--device`, and `--kbd-device` below.

## Hardware

| Component | Value |
|---|---|
| Sensor | `als` on `/sys/bus/iio/devices/iio:device0` |
| Backing device | AMD Sensor Fusion Hub (`lspci` `63:00.7`), HID-over-PCIe `0020:1022:0001.0002` |
| Illuminance channel | `in_illuminance_raw`, scale `0.100000000` (lux = raw x 0.1) |
| Extra channels | chromaticity x/y, color temperature, intensity |
| Backlight | `amdgpu_bl1`, max `400000`, driven via `brightnessctl --device amdgpu_bl1` |

## Build

No crates, no network access needed:

```bash
cargo build --release
# binary at target/release/ambientd
```

## Install

One-shot setup (builds, installs the binary and units, starts everything):

```bash
./install.sh
```

The sudo-gated parts (suspend-recovery unit) degrade to printed instructions
when no terminal is available for the password prompt — re-run the script in a
terminal later. Undo everything with `./uninstall.sh`.

What it sets up:

- `~/.local/bin/ambientd` — release binary
- `~/.config/systemd/user/ambientd.service` — user unit (`Restart=on-failure`),
  enabled at login
- `/etc/systemd/system/als-reload.service` — root unit that reloads the ALS
  driver after suspend/resume (see Troubleshooting)

## Usage

```
USAGE: ambientd [OPTIONS]

OPTIONS:
  --interval <ms>       poll interval (default 1000)
  --min <pct>           minimum brightness percent (default 10)
  --max <pct>           maximum brightness percent (default 100)
  --max-lux <lux>       lux mapped to full brightness (default 500)
  --smooth <0..1>       EMA smoothing factor, lower = smoother (default 0.2)
  --hysteresis <pct>    only apply changes larger than this (default 2)
  --device <name>       backlight device (default amdgpu_bl1)
  --sensor-dir <path>   iio sensor dir (default /sys/bus/iio/devices/iio:device0)
  --kbd-on <lux>        keyboard backlight on at/below this lux (default 0.5)
  --kbd-off <lux>       keyboard backlight off at/above this lux (default 2)
  --kbd-device <name>   keyboard backlight device (default asus::kbd_backlight)
  --no-kbd              disable keyboard backlight automation
  --no-nudge            disable user-nudge baseline shifting
  -v, --verbose         log every reading
  -h, --help            show help
```

Example — snappier indoor profile (the default floor is already 10%):

```bash
ambientd --max-lux 300 --smooth 0.3
```

### Config file

Every option can also be set persistently in `~/.config/ambientd/config` as
simple `key=value` lines (comments with `#`). CLI flags take precedence.
Honors `$XDG_CONFIG_HOME`. **Changes require a restart:**
`systemctl --user restart ambientd`.

A fully annotated template with every key and its default lives in
[`config.example`](config.example) — copy it as a starting point:

```bash
cp config.example ~/.config/ambientd/config
$EDITOR ~/.config/ambientd/config
systemctl --user restart ambientd
```

```
# poll the sensor twice per second, snappier smoothing
interval=500
smooth=0.3
hysteresis=1.5
no_kbd=false
```

Recognized keys mirror the flags: `interval`, `min`, `max`, `max_lux`,
`smooth`, `hysteresis`, `device`, `sensor_dir`, `kbd_on`, `kbd_off`,
`kbd_device`, `kbd_service`, `no_kbd`, `no_nudge` (boolean keys accept
`true`/`false`).

### Mapping curve

Brightness follows a logarithmic response so dark-room changes are fine-grained
and bright-room changes are coarse:

```
pct = min + (max - min) * ln(1 + lux/5) / ln(1 + max_lux/5)
```

clamped to `[min, max]`. Readings are smoothed with an exponential moving
average (`ema += smooth * (lux - ema)`), and the backlight is only touched when
the target differs from the last applied value by at least `--hysteresis`
percent, which prevents constant flicker.

### User nudges (baseline shift)

External brightness changes — Fn keys, `brightnessctl` in your own bindings,
anything — are detected each tick by watching
`/sys/class/backlight/<device>/brightness`. Instead of fighting the user, the
daemon absorbs the difference as a persistent **baseline shift** added on top of
the sensor curve:

```
target = clamp(lux_curve(ema) + shift)
```

So pressing brightness-up in a dark room keeps the screen brighter than the
sensor alone would choose, while ambient changes still modulate around your
preference. Nudged ticks bypass hysteresis for immediate feedback. The shift is
clamped to `±(max - min)` and resets on restart. Disable with `--no-nudge`.

### Keyboard backlight

The keyboard backlight (`asus::kbd_backlight`, levels 0-3) is automated with a
Schmitt trigger on the smoothed lux: it turns **on** at or below `--kbd-on`
(default 0.5 lux) and **off** at or above `--kbd-off` (default 2 lux). The dead
zone between the two thresholds prevents flapping when sitting near one value.
Transitions are logged:

```
lux=    0.2 ema=    0.2 -> keyboard backlight on
lux=   31.0 ema=   22.4 -> keyboard backlight off
```

Disable with `--no-kbd` if you prefer Fn-key control only.

## How it works

1. Read `/sys/bus/iio/devices/iio:device0/in_illuminance_raw` and multiply by
   `in_illuminance_scale` to get lux.
2. EMA-smooth over successive polls.
3. Map lux to a brightness percentage via the log curve.
4. If the percentage moved beyond the hysteresis threshold, run:
   `brightnessctl --device amdgpu_bl1 --quiet set N%`

`iio-sensor-proxy` is **not** required; the daemon reads sysfs directly. If it
happens to be installed and running, that's fine too — the two don't interfere.

## Troubleshooting

### Sensor reads 0 constantly (dead sensor)

Symptom: `cat .../in_illuminance_raw` returns `0` **instantly**. A healthy ALS
blocks ~50-300 ms on a fresh sample before answering; instant replies mean the
firmware never delivered a sample.

Confirm with timing:

```bash
time cat /sys/bus/iio/devices/iio:device0/in_illuminance_raw
```

Some AMD SFH systems (notably Framework laptops and various Zenbook models)
suffer a known failure mode where the kernel repeatedly logs:

```
hid-sensor-hub 0020:1022:0001.0002: Event data for report 4 was too short (18 vs 14)
```

and every consumer reads 0: raw sysfs, buffered capture, and
`monitor-sensor` (`iio-sensor-proxy`) alike. See kernel Bugzilla
[217762](https://bugzilla.kernel.org/show_bug.cgi?id=217762) ("Regression: ALS/ACS
stops working on amd-sfh") and the equivalent Framework Laptop reports — same
instant-zero symptom, fixed by reloading the driver.

Fix — just reload the ALS driver (the SFH hub itself stays loaded):

```bash
sudo modprobe -r hid_sensor_als
sudo modprobe hid_sensor_als

# then verify: first read should take ~200 ms and return nonzero
for i in $(seq 1 10); do time cat /sys/bus/iio/devices/iio*/in_illuminance_raw; sleep 1; done
```

The `als-reload.service` unit above automates this after every suspend.

Note on module dependencies: on some systems `amd_sfh` is held in use by
`amd_pmf` (itself held by `amdxdna`, the NPU driver), so unloading the whole
SFH stack requires removing all three — unnecessary here, since reloading only
`hid_sensor_als` is sufficient.

### Verifying the sensor through the desktop stack

```bash
monitor-sensor
# expect: === Has ambient light sensor (value: X.XXXXXX, unit: lux)
```

If that shows 0.000000 while raw sysfs reads nonzero, restart iio-sensor-proxy;
ambientd is unaffected either way.

### Watching the daemon

When running as the installed user unit (default after `./install.sh`):

```bash
systemctl --user status ambientd
journalctl --user -u ambientd -f        # follow live readings
```

Log lines look like:

```
lux=   42.0 ema=   38.7 -> 55%
lux=   41.0 ema=   39.2 -> 55% (hold)
```

A singleton guard (Unix socket at `/run/user/$UID/ambientd.lock`) makes any
second instance exit immediately — multiple copies fight over the backlight and
produce sawtooth brightness oscillation.

### Brightness does not change at all

Check that `brightnessctl` works for your user outside the daemon:

```bash
brightnessctl --device amdgpu_bl1 --quiet set 50%
```

If that fails, you are probably not in a logind seat session (`loginctl` /
`loginctl show-session $XDG_SESSION_ID`).

## Files

```
├── Cargo.toml / Cargo.lock
├── README.md
├── LICENSE
├── install.sh / uninstall.sh
├── config.example             # annotated config file template
├── als-reload.service        # root unit: reload ALS driver after resume
├── ambientd.user.service     # user unit template
└── src/main.rs               # entire implementation, std-only
~/.local/bin/ambientd                     # installed release binary
~/.config/systemd/user/ambientd.service   # installed user unit
/etc/systemd/system/als-reload.service    # installed root unit
```
