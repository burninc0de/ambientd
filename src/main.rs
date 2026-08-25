use std::env;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process;
use std::thread;
use std::time::Duration;

const SENSOR_DIR: &str = "/sys/bus/iio/devices/iio:device0";
const BACKLIGHT_DEVICE: &str = "amdgpu_bl1";
const KBD_DEVICE: &str = "asus::kbd_backlight";

struct Args {
    interval_ms: u64,
    min_pct: f32,
    max_pct: f32,
    max_lux: f32,
    smooth: f32,
    hysteresis_pct: f32,
    device: String,
    sensor_dir: String,
    kbd_enabled: bool,
    kbd_on_lux: f32,
    kbd_off_lux: f32,
    kbd_device: String,
    // React to raw lux instead of the smoothed EMA for all keyboard decisions:
    // binary on/off decisions want hysteresis, not smoothing.
    kbd_raw_lux: bool,
    // Optional companion systemd unit (e.g. an idle-dimmer): stopped when the
    // room is bright, started again when it gets dark.
    kbd_service: Option<String>,
    nudge_enabled: bool,
    verbose: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            min_pct: 10.0,
            max_pct: 100.0,
            max_lux: 500.0,
            smooth: 0.2,
            hysteresis_pct: 2.0,
            device: BACKLIGHT_DEVICE.into(),
            sensor_dir: SENSOR_DIR.into(),
            kbd_enabled: true,
            kbd_on_lux: 0.5,
            kbd_off_lux: 2.0,
            kbd_device: KBD_DEVICE.into(),
            kbd_raw_lux: true,
            kbd_service: None,
            nudge_enabled: true,
            verbose: false,
        }
    }
}

// Load simple key=value config from $XDG_CONFIG_HOME/ambientd/config or ~/.config/ambientd/config
fn config_path() -> Option<String> {
    if let Ok(cfg_home) = env::var("XDG_CONFIG_HOME") {
        if !cfg_home.is_empty() {
            return Some(format!("{}/ambientd/config", cfg_home));
        }
    }
    if let Ok(home) = env::var("HOME") {
        return Some(format!("{}/.config/ambientd/config", home));
    }
    None
}

fn load_config(args: &mut Args) {
    if let Some(path) = config_path() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    match k {
                        "interval" | "interval_ms" => {
                            if let Ok(ms) = v.parse::<u64>() { args.interval_ms = ms; }
                        }
                        "min" => if let Ok(p) = v.parse() { args.min_pct = p; }
                        "max" => if let Ok(p) = v.parse() { args.max_pct = p; }
                        "max_lux" => if let Ok(l) = v.parse() { args.max_lux = l; }
                        "smooth" => if let Ok(s) = v.parse() { args.smooth = s; }
                        "hysteresis" => if let Ok(h) = v.parse() { args.hysteresis_pct = h; }
                        "device" => args.device = v.to_string(),
                        "sensor_dir" => args.sensor_dir = v.to_string(),
                        "kbd_on" => if let Ok(l) = v.parse() { args.kbd_on_lux = l; }
                        "kbd_off" => if let Ok(l) = v.parse() { args.kbd_off_lux = l; }
                        "kbd_device" => args.kbd_device = v.to_string(),
                        "kbd_raw_lux" | "kbd-raw-lux" => {
                            args.kbd_raw_lux = v != "0" && v.to_lowercase() != "false";
                        }
                        "kbd_service" | "kbd-service" => {
                            args.kbd_service = if v.is_empty() { None } else { Some(v.to_string()) }
                        }
                        "no_kbd" => args.kbd_enabled = v != "0" && v.to_lowercase() != "false",
                        "no_nudge" => args.nudge_enabled = v != "0" && v.to_lowercase() != "false",
                        _ => {}
                    }
                }
            }
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    load_config(&mut a);
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |what: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_exit(&format!("missing value for {what}")))
        };
        match arg.as_str() {
            "--interval" => a.interval_ms = val("interval").parse().unwrap_or_else(|_| usage_exit("bad --interval")),
            "--min" => a.min_pct = val("min").parse().unwrap_or_else(|_| usage_exit("bad --min")),
            "--max" => a.max_pct = val("max").parse().unwrap_or_else(|_| usage_exit("bad --max")),
            "--max-lux" => a.max_lux = val("max-lux").parse().unwrap_or_else(|_| usage_exit("bad --max-lux")),
            "--smooth" => a.smooth = val("smooth").parse().unwrap_or_else(|_| usage_exit("bad --smooth")),
            "--hysteresis" => a.hysteresis_pct = val("hysteresis").parse().unwrap_or_else(|_| usage_exit("bad --hysteresis")),
            "--device" => a.device = val("device"),
            "--sensor-dir" => a.sensor_dir = val("sensor-dir"),
            "--kbd-on" => a.kbd_on_lux = val("kbd-on").parse().unwrap_or_else(|_| usage_exit("bad --kbd-on")),
            "--kbd-off" => a.kbd_off_lux = val("kbd-off").parse().unwrap_or_else(|_| usage_exit("bad --kbd-off")),
            "--kbd-device" => a.kbd_device = val("kbd-device"),
            "--kbd-raw-lux" => {
                let v = val("kbd-raw-lux");
                a.kbd_raw_lux = v != "0" && v.to_lowercase() != "false";
            }
            "--kbd-service" => {
                let u = val("kbd-service");
                a.kbd_service = if u.is_empty() { None } else { Some(u) };
            }
            "--no-kbd" => a.kbd_enabled = false,
            "--no-nudge" => a.nudge_enabled = false,
            "-v" | "--verbose" => a.verbose = true,
            "-h" | "--help" => usage_exit(""),
            other => usage_exit(&format!("unknown arg: {other}")),
        }
    }
    if !(0.0..=1.0).contains(&a.smooth) {
        usage_exit("--smooth must be in 0..1");
    }
    if a.min_pct >= a.max_pct || !(0.0..=100.0).contains(&a.min_pct) {
        usage_exit("need 0 <= min < max <= 100");
    }
    if a.kbd_on_lux >= a.kbd_off_lux {
        usage_exit("need --kbd-on < --kbd-off (dead zone between thresholds)");
    }
    a
}

fn usage_exit(msg: &str) -> ! {
    eprintln!(
        "{}ambientd — auto brightness from ambient light sensor

USAGE: ambientd [OPTIONS]

OPTIONS:
  --interval <ms>       poll interval (default 1000)
  --min <pct>           minimum brightness percent (default 10)
  --max <pct>           maximum brightness percent (default 100)
  --max-lux <lux>       lux mapped to full brightness (default 500)
  --smooth <0..1>       EMA smoothing factor, lower = smoother (default 0.2)
  --hysteresis <pct>    only apply changes larger than this (default 2)
  --device <name>       backlight device (default amdgpu_bl1)
  --sensor-dir <path>   iio sensor dir (default {SENSOR_DIR})
  --kbd-on <lux>        keyboard backlight on at/below this lux (default 0.5)
  --kbd-off <lux>       keyboard backlight off at/above this lux (default 2)
  --kbd-device <name>   keyboard backlight device (default asus::kbd_backlight)
  --kbd-raw-lux <bool>  react to raw lux instead of smoothed EMA for keyboard
                        decisions (default true: instant room changes)
  --kbd-service <unit>  companion unit: stop when bright, start when dark
                        (e.g. asus-backlight-idle.service; default: none)
  --no-kbd              disable keyboard backlight automation
  --no-nudge            disable user-nudge baseline shifting

CONFIG FILE:
  ~/.config/ambientd/config (key=value per line, CLI flags override it)
  e.g. interval=500
  -v, --verbose         log every reading
",
        if msg.is_empty() { String::new() } else { format!("error: {msg}\n\n") }
    );
    process::exit(if msg.is_empty() { 0 } else { 2 });
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_lux(sensor_dir: &str) -> Option<f32> {
    let raw: f32 = read_trim(&format!("{sensor_dir}/in_illuminance_raw"))?.parse().ok()?;
    let scale: f32 = read_trim(&format!("{sensor_dir}/in_illuminance_scale"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    Some(raw * scale)
}

fn lux_to_pct(lux: f32, args: &Args) -> f32 {
    // Logarithmic response so dark-room adjustments are fine-grained.
    let l0 = 5.0; // knee below which we treat things as "pitch black"
    let t = (1.0 + lux / l0).ln() / (1.0 + args.max_lux / l0).ln();
    let pct = args.min_pct + (args.max_pct - args.min_pct) * t.clamp(0.0, 1.0);
    pct.round().clamp(args.min_pct, args.max_pct)
}

fn set_brightness(pct: u32, device: &str) -> std::io::Result<()> {
    let status = process::Command::new("brightnessctl")
        .args(["--device", device, "--quiet", "set", &format!("{pct}%")])
        .status()?;
    if !status.success() {
        eprintln!("brightnessctl exited with {status}");
    }
    Ok(())
}

fn set_kbd(on: bool, device: &str) -> std::io::Result<()> {
    let status = process::Command::new("brightnessctl")
        .args(["--device", device, "--quiet", "set", if on { "1" } else { "0" }])
        .status()?;
    if !status.success() {
        eprintln!("brightnessctl (kbd) exited with {status}");
    }
    Ok(())
}

fn systemctl_user(action: &str, unit: &str) {
    match process::Command::new("systemctl").args(["--user", action, unit]).status() {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("systemctl --user {action} {unit} exited with {s}"),
        Err(e) => eprintln!("systemctl --user {action} {unit}: {e}"),
    }
}

fn read_backlight(device: &str, which: &str) -> Option<f32> {
    read_trim(&format!("/sys/class/backlight/{device}/{which}"))?
        .parse::<f32>()
        .ok()
}

fn read_kbd_brightness(device: &str) -> Option<u32> {
    read_trim(&format!("/sys/class/leds/{device}/brightness"))?
        .parse()
        .ok()
}

fn runtime_dir() -> Option<String> {
    env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallback for sessions that don't export XDG_RUNTIME_DIR
            let uid = env::var("UID")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| unsafe { libc_getuid() });
            uid.map(|u| format!("/run/user/{u}"))
        })
}

#[cfg(target_os = "linux")]
unsafe fn libc_getuid() -> Option<u32> {
    // getuid() never fails; declare it directly to avoid a libc dependency
    extern "C" {
        fn getuid() -> u32;
    }
    Some(unsafe { getuid() })
}

#[cfg(not(target_os = "linux"))]
unsafe fn libc_getuid() -> Option<u32> {
    None
}

// Returns Some(listener) while holding the singleton lock — the caller MUST
// keep the returned listener alive for the process lifetime (dropping it
// closes the socket and lets other instances start). Returns None when
// another instance is already running.
fn acquire_singleton_lock() -> Option<Option<UnixListener>> {
    use std::io::ErrorKind;
    let Some(dir) = runtime_dir() else {
        eprintln!("cannot determine runtime dir (set XDG_RUNTIME_DIR); skipping singleton lock");
        return Some(None); // no lock available; proceed without one
    };
    let path = format!("{dir}/ambientd.lock");
    match UnixListener::bind(&path) {
        Ok(listener) => Some(Some(listener)),
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            // Socket file exists. If something is listening, it's a live instance.
            if UnixStream::connect(&path).is_ok() {
                eprintln!("another ambientd instance is already running (lock: {path})");
                return None;
            }
            // Nobody answers: stale socket left over from a crash. Remove and retry once.
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(listener) => Some(Some(listener)),
                Err(e) => {
                    eprintln!("cannot bind lock socket {path}: {e}");
                    Some(None)
                }
            }
        }
        Err(e) => {
            eprintln!("cannot bind lock socket {path}: {e}");
            Some(None)
        }
    }
}

fn main() {
    let args = parse_args();

    // _lock_guard holds the singleton socket for the whole run; do not bind with `_`.
    let _lock_guard = match acquire_singleton_lock() {
        Some(lock) => lock,
        None => process::exit(1),
    };

    if read_trim(&format!("{}/name", args.sensor_dir)).as_deref() != Some("als") {
        eprintln!(
            "warning: {} does not look like an als device, continuing anyway",
            args.sensor_dir
        );
    }

    println!("ambientd: polling every {}ms", args.interval_ms);

    let interval = Duration::from_millis(args.interval_ms);
    let mut ema: Option<f32> = None;
    let mut applied_pct: f32 = f32::MIN;
    // None = unknown (first evaluation applies state unconditionally)
    let mut kbd_on: Option<bool> = None;
    // User-nudge baseline shift (percent points added to the sensor curve)
    let bl_max = read_backlight(&args.device, "max_brightness").unwrap_or(0.0);
    let mut last_raw = read_backlight(&args.device, "brightness").unwrap_or(-1.0);
    let mut shift = 0.0f32;

    loop {
        match read_lux(&args.sensor_dir) {
            Some(lux) => {
                ema = Some(match ema {
                    None => lux,
                    Some(prev) => prev + args.smooth * (lux - prev),
                });
                let cur_ema = ema.unwrap();
                // Keyboard decisions run on raw lux by default: instant room
                // changes, with the Schmitt dead zone handling sensor noise.
                // Set kbd_raw_lux=false to fall back to the smoothed EMA.
                let kbd_lux = if args.kbd_raw_lux { lux } else { cur_ema };

                // Detect external brightness changes -> baseline shift
                let mut nudged = 0.0f32;
                if args.nudge_enabled && bl_max > 0.0 {
                    if let Some(cur) = read_backlight(&args.device, "brightness") {
                        if last_raw >= 0.0 && (cur - last_raw).abs() > 0.5 {
                            nudged = (cur - last_raw) * 100.0 / bl_max;
                            shift = (shift + nudged).clamp(-(args.max_pct - args.min_pct), args.max_pct - args.min_pct);
                        }
                        last_raw = cur;
                    }
                }

                let target = (lux_to_pct(cur_ema, &args) + shift).round().clamp(args.min_pct, args.max_pct);
                if nudged != 0.0 || (target - applied_pct).abs() >= args.hysteresis_pct {
                    let _ = set_brightness(target as u32, &args.device);
                    applied_pct = target;
                    if args.nudge_enabled && bl_max > 0.0 {
                        // Sync with the driver so our own write isn't misread as a nudge
                        if let Some(cur) = read_backlight(&args.device, "brightness") {
                            last_raw = cur;
                        }
                    }
                    println!(
                        "lux={:>7.1} ema={:>7.1} -> {}{}",
                        lux,
                        cur_ema,
                        target as u32,
                        if nudged != 0.0 {
                            format!("% (nudge {:+.0}, shift {:+.0})", nudged, shift)
                        } else {
                            "%".into()
                        }
                    );
                } else if args.verbose {
                    println!("lux={:>7.1} ema={:>7.1} -> {}% (hold)", lux, cur_ema, target as u32);
                }

                if args.kbd_enabled {
                    // Schmitt trigger: on below --kbd-on, off above --kbd-off,
                    // dead zone in between prevents flapping at the boundary.
                    let want_on = match kbd_on {
                        Some(true) => kbd_lux < args.kbd_off_lux,
                        _ => kbd_lux <= args.kbd_on_lux,
                    };
                    if kbd_on != Some(want_on) {
                        let _ = set_kbd(want_on, &args.kbd_device);
                        kbd_on = Some(want_on);
                        println!(
                            "lux={:>7.1} ema={:>7.1} -> keyboard backlight {}",
                            lux,
                            cur_ema,
                            if want_on { "on" } else { "off" }
                        );
                        // Companion daemon ownership: when the room is bright we
                        // stop it entirely (nothing left to re-light the keyboard
                        // behind our back); when it gets dark we hand control back
                        // so idle-dimming and input-restore work normally.
                        if let Some(unit) = &args.kbd_service {
                            let action = if want_on { "start" } else { "stop" };
                            systemctl_user(action, unit);
                            println!(
                                "lux={:>7.1} ema={:>7.1} -> {} {}",
                                lux, cur_ema, action, unit
                            );
                        }
                    }

                    // Stray-light watchdog: whenever policy says "dark", enforce
                    // it every tick regardless of who lit things up (lockscreen
                    // hooks, Fn keys, EC quirks). Cost: one sysfs read per tick.
                    if !want_on {
                        if let Some(b) = read_kbd_brightness(&args.kbd_device) {
                            if b > 0 {
                                let _ = set_kbd(false, &args.kbd_device);
                                println!(
                                    "lux={:>7.1} ema={:>7.1} -> keyboard backlight off (stray)",
                                    lux, cur_ema
                                );
                            }
                        }
                    }
                }
            }
            None => eprintln!("failed to read illuminance, retrying"),
        }
        thread::sleep(interval);
    }
}
