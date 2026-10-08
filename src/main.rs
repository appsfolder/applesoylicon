use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GLYPHS: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@#$%^&*()+-=~|/\\{}[]";

#[derive(Clone, Copy)]
struct Palette {
    head: &'static str,
    near: &'static str,
    far: &'static str,
    mark: &'static str,
}

impl Palette {
    fn named(name: &str) -> Option<Self> {
        match name {
            "green" => Some(Self {
                head: "\x1b[1;97m",
                near: "\x1b[1;32m",
                far: "\x1b[2;32m",
                mark: "\x1b[1;97m",
            }),
            "amber" => Some(Self {
                head: "\x1b[1;97m",
                near: "\x1b[1;33m",
                far: "\x1b[2;33m",
                mark: "\x1b[1;97m",
            }),
            "ice" => Some(Self {
                head: "\x1b[1;97m",
                near: "\x1b[1;36m",
                far: "\x1b[2;36m",
                mark: "\x1b[1;97m",
            }),
            "soy" => Some(Self {
                head: "\x1b[1;93m",
                near: "\x1b[38;5;214m",
                far: "\x1b[38;5;94m",
                mark: "\x1b[1;97m",
            }),
            _ => None,
        }
    }
}

struct Config {
    palette: Palette,
    fps: u64,
    density: f64,
    message: String,
    seed: u64,
    direction: Direction,
    gyro: bool,
    gyro_offset: u16,
    gyro_mirror: GyroMirror,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GyroMirror {
    None,
    X,
    Y,
    Both,
}

impl GyroMirror {
    fn named(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Down,
    Up,
    Left,
    Right,
}

impl Direction {
    fn named(value: &str) -> Option<Self> {
        match value {
            "down" => Some(Self::Down),
            "up" => Some(Self::Up),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    fn dimensions(self, width: usize, height: usize) -> (usize, usize) {
        match self {
            Self::Down | Self::Up => (width, height),
            Self::Left | Self::Right => (height, width),
        }
    }
}

struct Sensor {
    readings: mpsc::Receiver<[f64; 3]>,
}

impl Sensor {
    fn start() -> io::Result<Self> {
        let device = find_accelerometer()?;
        let matrix = read_mount_matrix(&device).unwrap_or([
            1.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, //
            0.0, 0.0, 1.0,
        ]);
        let (tx, readings) = mpsc::channel();
        thread::spawn(move || {
            let mut filtered = [0.0_f64; 3];
            loop {
                let Some(raw) = read_acceleration(&device) else {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let mounted = apply_matrix(matrix, raw);
                for axis in 0..3 {
                    filtered[axis] = filtered[axis] * 0.62 + mounted[axis] * 0.38;
                }
                if tx.send(filtered).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        Ok(Self { readings })
    }
}

fn find_accelerometer() -> io::Result<PathBuf> {
    let entries = fs::read_dir("/sys/bus/iio/devices")?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("in_accel_x_raw").is_file()
            && path.join("in_accel_y_raw").is_file()
            && path.join("in_accel_z_raw").is_file()
        {
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no readable IIO accelerometer found",
    ))
}

fn read_number(path: &Path) -> Option<f64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_acceleration(device: &Path) -> Option<[f64; 3]> {
    Some([
        read_number(&device.join("in_accel_x_raw"))?,
        read_number(&device.join("in_accel_y_raw"))?,
        read_number(&device.join("in_accel_z_raw"))?,
    ])
}

fn read_mount_matrix(device: &Path) -> Option<[f64; 9]> {
    let text = fs::read_to_string(device.join("in_accel_mount_matrix")).ok()?;
    let values: Vec<f64> = text
        .split(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    values.try_into().ok()
}

fn apply_matrix(matrix: [f64; 9], vector: [f64; 3]) -> [f64; 3] {
    [
        matrix[0] * vector[0] + matrix[1] * vector[1] + matrix[2] * vector[2],
        matrix[3] * vector[0] + matrix[4] * vector[1] + matrix[5] * vector[2],
        matrix[6] * vector[0] + matrix[7] * vector[1] + matrix[8] * vector[2],
    ]
}

#[cfg(test)]
fn gravity_direction(x: f64, y: f64) -> Option<Direction> {
    let magnitude = x.abs().max(y.abs());
    if magnitude < 180.0 {
        return None;
    }
    if x.abs() > y.abs() * 1.12 {
        Some(if x > 0.0 {
            Direction::Right
        } else {
            Direction::Left
        })
    } else if y.abs() > x.abs() * 1.12 {
        Some(if y > 0.0 {
            Direction::Down
        } else {
            Direction::Up
        })
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct RainDrop {
    y: f64,
    speed: f64,
    length: usize,
    active: bool,
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn usize(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper.max(1)
    }
    fn unit(&mut self) -> f64 {
        self.next() as f64 / u64::MAX as f64
    }
}

struct Terminal {
    saved_stty: String,
}

impl Terminal {
    fn enter() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other(
                "applesoylicon needs an interactive terminal",
            ));
        }
        let saved = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()?;
        if !saved.status.success() {
            return Err(io::Error::other("could not read terminal state"));
        }
        let saved_stty = String::from_utf8_lossy(&saved.stdout).trim().to_owned();
        let status = Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err(io::Error::other("could not enter raw terminal mode"));
        }
        print!("\x1b[?1049h\x1b[?25l\x1b[2J");
        io::stdout().flush()?;
        Ok(Self { saved_stty })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        print!("\x1b[0m\x1b[2J\x1b[H\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
        let _ = Command::new("stty")
            .arg(&self.saved_stty)
            .stdin(Stdio::inherit())
            .status();
    }
}

trait IsTerminal {
    fn is_terminal(&self) -> bool;
}
impl IsTerminal for io::Stdin {
    fn is_terminal(&self) -> bool {
        unsafe { isatty(self.as_raw_fd()) == 1 }
    }
}
impl IsTerminal for io::Stdout {
    fn is_terminal(&self) -> bool {
        unsafe { isatty(self.as_raw_fd()) == 1 }
    }
}

unsafe extern "C" {
    fn isatty(fd: i32) -> i32;
    fn ioctl(fd: i32, request: usize, ...) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct WinSize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

fn terminal_size() -> (usize, usize) {
    let mut ws = WinSize::default();
    let ok = unsafe { ioctl(io::stdout().as_raw_fd(), 0x5413, &mut ws) } == 0;
    if ok && ws.col > 0 && ws.row > 0 {
        (ws.col as usize, ws.row as usize)
    } else {
        (80, 24)
    }
}

fn reset_drop(drop: &mut RainDrop, extent: usize, density: f64, rng: &mut Rng, random_y: bool) {
    drop.active = rng.unit() <= density;
    drop.y = if random_y {
        rng.usize(extent) as f64
    } else {
        -(rng.usize(extent.max(2) / 2 + 1) as f64)
    };
    drop.speed = 0.28 + rng.unit() * 0.72;
    drop.length = 7 + rng.usize(18);
}

fn config_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("applesoylicon/config"));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".config/applesoylicon/config"))
        .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is set".to_owned())
}

fn load_settings() -> BTreeMap<String, String> {
    let Ok(path) = config_path() else {
        return BTreeMap::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn validate_setting(key: &str, value: &str) -> Result<String, String> {
    match key {
        "palette" if Palette::named(value).is_some() => Ok(value.to_owned()),
        "fps" => value
            .parse::<u64>()
            .ok()
            .filter(|number| (5..=60).contains(number))
            .map(|_| value.to_owned())
            .ok_or_else(|| "fps must be an integer from 5 through 60".to_owned()),
        "density" => value
            .parse::<f64>()
            .ok()
            .filter(|number| (0.05..=1.0).contains(number))
            .map(|_| value.to_owned())
            .ok_or_else(|| "density must be a number from 0.05 through 1.0".to_owned()),
        "message" if !value.is_empty() && value.len() <= 40 && value.is_ascii() => {
            Ok(value.to_owned())
        }
        "seed" if value == "random" || value.parse::<u64>().is_ok() => Ok(value.to_owned()),
        "direction" if Direction::named(value).is_some() || value == "auto" => Ok(value.to_owned()),
        "gyro" => parse_bool(value)
            .map(|enabled| enabled.to_string())
            .ok_or_else(|| "gyro must be true or false".to_owned()),
        "gyro-offset" => value
            .parse::<u16>()
            .ok()
            .filter(|angle| matches!(angle, 0 | 90 | 180 | 270))
            .map(|_| value.to_owned())
            .ok_or_else(|| "gyro-offset must be 0, 90, 180, or 270".to_owned()),
        "gyro-mirror" if GyroMirror::named(value).is_some() => Ok(value.to_owned()),
        "gyro-mirror" => Err("gyro-mirror must be none, x, y, or both".to_owned()),
        "palette" => Err("palette must be green, amber, ice, or soy".to_owned()),
        "message" => Err("message must be 1-40 ASCII characters".to_owned()),
        unknown => Err(format!("unknown setting '{unknown}'")),
    }
}

fn save_settings(settings: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    let path = config_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "invalid configuration path".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create config directory: {error}"))?;
    let temporary = parent.join(format!(".config.tmp.{}", std::process::id()));
    let mut text = String::from("# applesoylicon persistent defaults\n");
    for (key, value) in settings {
        let _ = writeln!(text, "{key}={value}");
    }
    fs::write(&temporary, text).map_err(|error| format!("cannot write config: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("cannot replace config: {error}"))?;
    Ok(path)
}

fn run_setup(arguments: &[String]) -> Result<(), String> {
    let path = config_path()?;
    if arguments.is_empty() || arguments == ["show"] {
        println!("{}", path.display());
        let settings = load_settings();
        if settings.is_empty() {
            println!("(no saved settings)");
        } else {
            for (key, value) in settings {
                println!("{key}={value}");
            }
        }
        return Ok(());
    }
    if arguments == ["reset"] {
        match fs::remove_file(&path) {
            Ok(()) => println!("reset {}", path.display()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                println!("already reset {}", path.display())
            }
            Err(error) => return Err(format!("cannot reset config: {error}")),
        }
        return Ok(());
    }
    let mut settings = load_settings();
    for assignment in arguments {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected key=value, got '{assignment}'"))?;
        settings.insert(key.to_owned(), validate_setting(key, value)?);
    }
    let saved = save_settings(&settings)?;
    println!("saved {}", saved.display());
    for assignment in arguments {
        println!("{assignment}");
    }
    Ok(())
}

fn parse_args() -> Result<Option<Config>, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|argument| argument == "--setup") {
        run_setup(&args[1..])?;
        return Ok(None);
    }
    let saved = load_settings();
    let mut palette_name = saved
        .get("palette")
        .filter(|value| Palette::named(value).is_some())
        .cloned()
        .unwrap_or_else(|| "green".to_owned());
    let mut fps = saved
        .get("fps")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24_u64);
    let mut density = saved
        .get("density")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.72_f64);
    let mut message = saved
        .get("message")
        .cloned()
        .unwrap_or_else(|| "APPLESOYLICON".to_owned());
    let mut message_was_set = false;
    let saved_direction = saved.get("direction").map(String::as_str).unwrap_or("down");
    let mut direction = Direction::named(saved_direction).unwrap_or(Direction::Down);
    let mut gyro = saved_direction == "auto"
        || saved
            .get("gyro")
            .and_then(|value| parse_bool(value))
            .unwrap_or(false);
    let mut gyro_offset = saved
        .get("gyro-offset")
        .and_then(|value| value.parse().ok())
        .filter(|angle| matches!(angle, 0 | 90 | 180 | 270))
        .unwrap_or(0_u16);
    let mut gyro_mirror = saved
        .get("gyro-mirror")
        .and_then(|value| GyroMirror::named(value))
        .unwrap_or(GyroMirror::None);
    let random_seed = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    };
    let mut seed = saved
        .get("seed")
        .filter(|value| value.as_str() != "random")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(random_seed);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("applesoylicon {VERSION}");
                return Ok(None);
            }
            "--gyro" => gyro = true,
            "--palette" | "--fps" | "--density" | "--message" | "--seed" | "--direction"
            | "--gyro-offset" | "--gyro-mirror" => {
                let flag = args[i].clone();
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--palette" => palette_name = value.clone(),
                    "--fps" => {
                        fps = value
                            .parse()
                            .map_err(|_| "--fps must be an integer".to_owned())?
                    }
                    "--density" => {
                        density = value
                            .parse()
                            .map_err(|_| "--density must be a number".to_owned())?
                    }
                    "--message" => {
                        message = value.clone();
                        message_was_set = true;
                    }
                    "--direction" => {
                        if value == "auto" {
                            gyro = true;
                        } else {
                            direction = Direction::named(value).ok_or_else(|| {
                                "--direction must be down, up, left, right, or auto".to_owned()
                            })?;
                        }
                    }
                    "--gyro-offset" => {
                        gyro_offset = value
                            .parse()
                            .map_err(|_| "--gyro-offset must be 0, 90, 180, or 270".to_owned())?;
                        if !matches!(gyro_offset, 0 | 90 | 180 | 270) {
                            return Err("--gyro-offset must be 0, 90, 180, or 270".to_owned());
                        }
                    }
                    "--gyro-mirror" => {
                        gyro_mirror = GyroMirror::named(value).ok_or_else(|| {
                            "--gyro-mirror must be none, x, y, or both".to_owned()
                        })?;
                    }
                    "--seed" => {
                        seed = value
                            .parse()
                            .map_err(|_| "--seed must be an integer".to_owned())?
                    }
                    _ => unreachable!(),
                }
            }
            value if !value.starts_with('-') && !message_was_set => {
                message = value.to_owned();
                message_was_set = true;
            }
            unknown => return Err(format!("unknown option: {unknown}")),
        }
        i += 1;
    }
    if !(5..=60).contains(&fps) {
        return Err("--fps must be between 5 and 60".into());
    }
    if !(0.05..=1.0).contains(&density) {
        return Err("--density must be between 0.05 and 1.0".into());
    }
    if message.is_empty() || message.chars().count() > 40 || !message.is_ascii() {
        return Err("--message must be 1-40 ASCII characters".into());
    }
    let palette = Palette::named(&palette_name).ok_or_else(|| {
        format!("unknown palette '{palette_name}' (use green, amber, ice, or soy)")
    })?;
    Ok(Some(Config {
        palette,
        fps,
        density,
        message,
        seed,
        direction,
        gyro,
        gyro_offset,
        gyro_mirror,
    }))
}

fn print_help() {
    println!(
        "applesoylicon {VERSION} — terminal rain, zero consequences\n\nUSAGE:\n    applesoylicon [WORD] [OPTIONS]\n    applesoylicon --setup KEY=VALUE...\n    applesoylicon --setup [show|reset]\n\nOPTIONS:\n    --direction <DIR>   down, up, left, right, or auto [default: down]\n    --gyro              Physical rain driven by raw IIO acceleration\n    --gyro-offset <DEG> Rotate sensor vector: 0, 90, 180, or 270\n    --gyro-mirror <AXIS> none, x, y, or both [default: none]\n    --palette <NAME>    green, amber, ice, or soy [default: green]\n    --fps <N>           Frames per second, 5-60 [default: 24]\n    --density <N>       Rain density, 0.05-1.0 [default: 0.72]\n    --message <TEXT>    Alias for positional WORD [default: APPLESOYLICON]\n    --seed <N>          Deterministic random seed\n    --setup ...         Save, show, or reset persistent defaults\n    -h, --help          Print help\n    -V, --version       Print version\n\nSETUP KEYS:\n    palette fps density message seed direction gyro gyro-offset gyro-mirror\n\nKEYS:\n    q, Esc, Ctrl-C      Leave the rain\n    [ / ]               Decrease / increase density\n    r                   Rotate fixed direction clockwise\n    p                   Cycle palette\n    + / -               Change speed\n    d                   Toggle dense mode"
    );
}

fn run(mut config: Config) -> io::Result<()> {
    let _terminal = Terminal::enter()?;
    let sensor = if config.gyro {
        Some(Sensor::start().map_err(|error| {
            io::Error::other(format!(
                "gyro mode needs a readable Linux IIO accelerometer: {error}"
            ))
        })?)
    } else {
        None
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for byte in io::stdin().lock().bytes() {
            match byte {
                Ok(value) => {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let palettes = [
        Palette::named("green").unwrap(),
        Palette::named("amber").unwrap(),
        Palette::named("ice").unwrap(),
        Palette::named("soy").unwrap(),
    ];
    if let Some(sensor) = sensor {
        return run_physics(config, rx, sensor, palettes);
    }
    let mut palette_index = 0;
    let mut rng = Rng::new(config.seed);
    let (mut width, mut height) = terminal_size();
    let mut drops = make_drops(config.direction, width, height, config.density, &mut rng);
    let mut mark = new_mark(width, height, config.message.len(), &mut rng);
    let mut frame_time = Duration::from_millis(1000 / config.fps);
    loop {
        let frame_started = Instant::now();
        while let Ok(key) = rx.try_recv() {
            match key {
                b'q' | 27 | 3 => return Ok(()),
                b'p' => {
                    palette_index = (palette_index + 1) % palettes.len();
                    config.palette = palettes[palette_index];
                }
                b'r' => {
                    config.direction = match config.direction {
                        Direction::Down => Direction::Left,
                        Direction::Left => Direction::Up,
                        Direction::Up => Direction::Right,
                        Direction::Right => Direction::Down,
                    };
                    drops = make_drops(config.direction, width, height, config.density, &mut rng);
                    print!("\x1b[2J");
                }
                b'+' | b'=' => {
                    frame_time = frame_time
                        .saturating_sub(Duration::from_millis(4))
                        .max(Duration::from_millis(16))
                }
                b'-' | b'_' => {
                    frame_time =
                        (frame_time + Duration::from_millis(4)).min(Duration::from_millis(200))
                }
                b'd' => {
                    config.density = if config.density < 0.9 { 1.0 } else { 0.55 };
                    for d in &mut drops {
                        d.active = rng.unit() <= config.density;
                    }
                }
                b']' | b'}' => {
                    config.density = (config.density + 0.1).min(1.0);
                    for d in &mut drops {
                        d.active = rng.unit() <= config.density;
                    }
                }
                b'[' | b'{' => {
                    config.density = (config.density - 0.1).max(0.05);
                    for d in &mut drops {
                        d.active = rng.unit() <= config.density;
                    }
                }
                _ => {}
            }
        }
        let (new_width, new_height) = terminal_size();
        if (new_width, new_height) != (width, height) {
            (width, height) = (new_width, new_height);
            drops = make_drops(config.direction, width, height, config.density, &mut rng);
            mark = new_mark(width, height, config.message.len(), &mut rng);
            print!("\x1b[2J");
        }
        let mut out = String::with_capacity(width * 32);
        let (lanes, extent) = config.direction.dimensions(width, height);
        for (lane, drop) in drops.iter_mut().enumerate().take(lanes) {
            if !drop.active {
                continue;
            }
            let head = drop.y as isize;
            draw_cell(
                &mut out,
                head,
                lane,
                width,
                height,
                config.direction,
                config.palette.head,
                &mut rng,
            );
            for i in 1..=drop.length {
                let y = head - i as isize;
                let color = if i < drop.length / 3 {
                    config.palette.near
                } else {
                    config.palette.far
                };
                draw_cell(
                    &mut out,
                    y,
                    lane,
                    width,
                    height,
                    config.direction,
                    color,
                    &mut rng,
                );
            }
            let clear = head - drop.length as isize - 1;
            draw_blank(&mut out, clear, lane, width, height, config.direction);
            drop.y += drop.speed;
            if head - drop.length as isize > extent as isize {
                reset_drop(drop, extent, config.density, &mut rng, false);
            }
        }
        if mark.0 < height && mark.1 + config.message.len() <= width {
            let _ = write!(
                out,
                "\x1b[{};{}H{}{}",
                mark.0 + 1,
                mark.1 + 1,
                config.palette.mark,
                config.message
            );
        }
        print!("{out}");
        io::stdout().flush()?;
        thread::sleep(frame_time.saturating_sub(frame_started.elapsed()));
    }
}

struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    trail: VecDeque<(usize, usize)>,
}

fn particle_target(width: usize, height: usize, density: f64) -> usize {
    (((width * height) as f64 * density / 18.0).round() as usize).clamp(8, 900)
}

fn new_particle(width: usize, height: usize, rng: &mut Rng) -> Particle {
    Particle {
        x: rng.unit() * width.max(1) as f64,
        y: rng.unit() * height.max(1) as f64,
        vx: (rng.unit() - 0.5) * 0.16,
        vy: (rng.unit() - 0.5) * 0.08,
        trail: VecDeque::with_capacity(10),
    }
}

fn resize_particles(
    particles: &mut Vec<Particle>,
    width: usize,
    height: usize,
    density: f64,
    rng: &mut Rng,
) {
    let target = particle_target(width, height, density);
    particles.truncate(target);
    while particles.len() < target {
        particles.push(new_particle(width, height, rng));
    }
}

fn physics_acceleration(reading: [f64; 3]) -> (f64, f64) {
    let planar = reading[0].hypot(reading[1]);
    let total = planar.hypot(reading[2]).max(1.0);
    if planar < total * 0.035 {
        return (0.0, 0.0);
    }
    let tilt = (planar / total).clamp(0.0, 1.0);
    // Terminal cells are roughly twice as tall as they are wide.
    (
        reading[0] / planar * 0.22 * tilt,
        reading[1] / planar * 0.11 * tilt,
    )
}

fn rotate_sensor(reading: [f64; 3], offset: u16) -> [f64; 3] {
    match offset {
        90 => [-reading[1], reading[0], reading[2]],
        180 => [-reading[0], -reading[1], reading[2]],
        270 => [reading[1], -reading[0], reading[2]],
        _ => reading,
    }
}

fn mirror_sensor(mut reading: [f64; 3], mirror: GyroMirror) -> [f64; 3] {
    if matches!(mirror, GyroMirror::X | GyroMirror::Both) {
        reading[0] = -reading[0];
    }
    if matches!(mirror, GyroMirror::Y | GyroMirror::Both) {
        reading[1] = -reading[1];
    }
    reading
}

fn run_physics(
    mut config: Config,
    keys: mpsc::Receiver<u8>,
    sensor: Sensor,
    palettes: [Palette; 4],
) -> io::Result<()> {
    let mut palette_index = 0;
    let mut rng = Rng::new(config.seed);
    let (mut width, mut height) = terminal_size();
    let mut particles = Vec::new();
    resize_particles(&mut particles, width, height, config.density, &mut rng);
    let mut gravity = [0.0, 700.0, 0.0];
    let mut frame_time = Duration::from_millis(1000 / config.fps);
    let mut previous_cells: Vec<(usize, usize)> = Vec::new();
    let mut mark = new_mark(width, height, config.message.len(), &mut rng);

    loop {
        let frame_started = Instant::now();
        while let Ok(key) = keys.try_recv() {
            match key {
                b'q' | 27 | 3 => return Ok(()),
                b'p' => {
                    palette_index = (palette_index + 1) % palettes.len();
                    config.palette = palettes[palette_index];
                }
                b'+' | b'=' => {
                    frame_time = frame_time
                        .saturating_sub(Duration::from_millis(4))
                        .max(Duration::from_millis(16));
                }
                b'-' | b'_' => {
                    frame_time =
                        (frame_time + Duration::from_millis(4)).min(Duration::from_millis(200));
                }
                b']' | b'}' => {
                    config.density = (config.density + 0.1).min(1.0);
                    resize_particles(&mut particles, width, height, config.density, &mut rng);
                }
                b'[' | b'{' => {
                    config.density = (config.density - 0.1).max(0.05);
                    resize_particles(&mut particles, width, height, config.density, &mut rng);
                }
                b'd' => {
                    config.density = if config.density < 0.9 { 1.0 } else { 0.35 };
                    resize_particles(&mut particles, width, height, config.density, &mut rng);
                }
                _ => {}
            }
        }
        while let Ok(reading) = sensor.readings.try_recv() {
            gravity = reading;
        }

        let (new_width, new_height) = terminal_size();
        if (new_width, new_height) != (width, height) {
            (width, height) = (new_width, new_height);
            particles.clear();
            resize_particles(&mut particles, width, height, config.density, &mut rng);
            previous_cells.clear();
            mark = new_mark(width, height, config.message.len(), &mut rng);
            print!("\x1b[2J");
        }

        let corrected = mirror_sensor(
            rotate_sensor(gravity, config.gyro_offset),
            config.gyro_mirror,
        );
        let (ax, ay) = physics_acceleration(corrected);
        let mut out = String::with_capacity((particles.len() * 10 + previous_cells.len()) * 16);
        for (x, y) in previous_cells.drain(..) {
            let _ = write!(out, "\x1b[{};{}H ", y + 1, x + 1);
        }

        for particle in &mut particles {
            particle.vx = (particle.vx + ax) * 0.965;
            particle.vy = (particle.vy + ay) * 0.965;
            let speed = particle.vx.hypot(particle.vy);
            if speed > 2.8 {
                particle.vx *= 2.8 / speed;
                particle.vy *= 2.8 / speed;
            }
            particle.x += particle.vx;
            particle.y += particle.vy;

            let wrapped = particle.x < 0.0
                || particle.x >= width as f64
                || particle.y < 0.0
                || particle.y >= height as f64;
            particle.x = particle.x.rem_euclid(width.max(1) as f64);
            particle.y = particle.y.rem_euclid(height.max(1) as f64);
            if wrapped {
                particle.trail.clear();
            }

            let cell = (particle.x as usize, particle.y as usize);
            if particle.trail.back().copied() != Some(cell) {
                particle.trail.push_back(cell);
                while particle.trail.len() > 9 {
                    particle.trail.pop_front();
                }
            }
            for (index, &(x, y)) in particle.trail.iter().enumerate() {
                let color = if index + 1 == particle.trail.len() {
                    config.palette.head
                } else if index * 2 >= particle.trail.len() {
                    config.palette.near
                } else {
                    config.palette.far
                };
                let glyph = GLYPHS[rng.usize(GLYPHS.len())] as char;
                let _ = write!(out, "\x1b[{};{}H{}{}", y + 1, x + 1, color, glyph);
                previous_cells.push((x, y));
            }
        }
        if mark.0 < height && mark.1 + config.message.len() <= width {
            let _ = write!(
                out,
                "\x1b[{};{}H{}{}",
                mark.0 + 1,
                mark.1 + 1,
                config.palette.mark,
                config.message
            );
        }
        print!("{out}");
        io::stdout().flush()?;
        thread::sleep(frame_time.saturating_sub(frame_started.elapsed()));
    }
}

fn make_drops(
    direction: Direction,
    width: usize,
    height: usize,
    density: f64,
    rng: &mut Rng,
) -> Vec<RainDrop> {
    let (lanes, extent) = direction.dimensions(width, height);
    let mut drops = vec![
        RainDrop {
            y: 0.0,
            speed: 0.5,
            length: 12,
            active: true,
        };
        lanes
    ];
    for drop in &mut drops {
        reset_drop(drop, extent, density, rng, true);
    }
    drops
}

fn oriented_position(
    primary: isize,
    lane: usize,
    width: usize,
    height: usize,
    direction: Direction,
) -> Option<(usize, usize)> {
    let (x, y) = match direction {
        Direction::Down => (lane as isize, primary),
        Direction::Up => (lane as isize, height as isize - 1 - primary),
        Direction::Right => (primary, lane as isize),
        Direction::Left => (width as isize - 1 - primary, lane as isize),
    };
    (x >= 0 && x < width as isize && y >= 0 && y < height as isize)
        .then_some((x as usize, y as usize))
}

fn draw_cell(
    out: &mut String,
    primary: isize,
    lane: usize,
    width: usize,
    height: usize,
    direction: Direction,
    color: &str,
    rng: &mut Rng,
) {
    if let Some((x, y)) = oriented_position(primary, lane, width, height, direction) {
        let glyph = GLYPHS[rng.usize(GLYPHS.len())] as char;
        let _ = write!(out, "\x1b[{};{}H{}{}", y + 1, x + 1, color, glyph);
    }
}

fn draw_blank(
    out: &mut String,
    primary: isize,
    lane: usize,
    width: usize,
    height: usize,
    direction: Direction,
) {
    if let Some((x, y)) = oriented_position(primary, lane, width, height, direction) {
        let _ = write!(out, "\x1b[{};{}H ", y + 1, x + 1);
    }
}

fn new_mark(width: usize, height: usize, len: usize, rng: &mut Rng) -> (usize, usize) {
    let y = if height > 4 {
        2 + rng.usize(height - 3)
    } else {
        0
    };
    let x = if width > len + 4 {
        2 + rng.usize(width - len - 3)
    } else {
        0
    };
    (y, x)
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(Some(config)) => match run(config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("applesoylicon: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("applesoylicon: {error}\nTry 'applesoylicon --help'.");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_stream_is_repeatable() {
        let mut left = Rng::new(42);
        let mut right = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(left.next(), right.next());
        }
    }

    #[test]
    fn reset_drop_stays_in_sensible_ranges() {
        let mut rng = Rng::new(7);
        let mut drop = RainDrop {
            y: 0.0,
            speed: 0.0,
            length: 0,
            active: false,
        };
        reset_drop(&mut drop, 24, 1.0, &mut rng, true);
        assert!(drop.active);
        assert!((0.0..24.0).contains(&drop.y));
        assert!((0.28..=1.0).contains(&drop.speed));
        assert!((7..=24).contains(&drop.length));
    }

    #[test]
    fn hidden_message_fits_normal_terminal() {
        let mut rng = Rng::new(99);
        for _ in 0..100 {
            let (y, x) = new_mark(80, 24, 13, &mut rng);
            assert!(y < 24);
            assert!(x + 13 <= 80);
        }
    }

    #[test]
    fn directions_map_to_screen_edges() {
        assert_eq!(
            oriented_position(0, 2, 10, 5, Direction::Down),
            Some((2, 0))
        );
        assert_eq!(oriented_position(0, 2, 10, 5, Direction::Up), Some((2, 4)));
        assert_eq!(
            oriented_position(0, 2, 10, 5, Direction::Right),
            Some((0, 2))
        );
        assert_eq!(
            oriented_position(0, 2, 10, 5, Direction::Left),
            Some((9, 2))
        );
    }

    #[test]
    fn gravity_uses_dominant_axis_and_dead_zone() {
        assert_eq!(gravity_direction(800.0, 20.0), Some(Direction::Right));
        assert_eq!(gravity_direction(-800.0, 20.0), Some(Direction::Left));
        assert_eq!(gravity_direction(20.0, 800.0), Some(Direction::Down));
        assert_eq!(gravity_direction(20.0, -800.0), Some(Direction::Up));
        assert_eq!(gravity_direction(50.0, 40.0), None);
        assert_eq!(gravity_direction(700.0, 700.0), None);
    }

    #[test]
    fn physical_acceleration_preserves_raw_vector_direction() {
        let (right, down) = physics_acceleration([800.0, 400.0, 800.0]);
        assert!(right > 0.0);
        assert!(down > 0.0);
        assert!(right > down);
        assert_eq!(physics_acceleration([0.0, 0.0, 1000.0]), (0.0, 0.0));
    }

    #[test]
    fn density_controls_particle_count() {
        assert!(particle_target(80, 24, 0.9) > particle_target(80, 24, 0.2));
    }

    #[test]
    fn gyro_offset_rotates_clockwise() {
        let reading = [2.0, 1.0, 3.0];
        assert_eq!(rotate_sensor(reading, 0), [2.0, 1.0, 3.0]);
        assert_eq!(rotate_sensor(reading, 90), [-1.0, 2.0, 3.0]);
        assert_eq!(rotate_sensor(reading, 180), [-2.0, -1.0, 3.0]);
        assert_eq!(rotate_sensor(reading, 270), [1.0, -2.0, 3.0]);
    }

    #[test]
    fn gyro_mirror_flips_selected_screen_axes() {
        let reading = [2.0, 1.0, 3.0];
        assert_eq!(mirror_sensor(reading, GyroMirror::None), [2.0, 1.0, 3.0]);
        assert_eq!(mirror_sensor(reading, GyroMirror::X), [-2.0, 1.0, 3.0]);
        assert_eq!(mirror_sensor(reading, GyroMirror::Y), [2.0, -1.0, 3.0]);
        assert_eq!(mirror_sensor(reading, GyroMirror::Both), [-2.0, -1.0, 3.0]);
    }
}
