use std::env;
use std::fmt::Write as FmtWrite;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
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

fn reset_drop(drop: &mut RainDrop, height: usize, density: f64, rng: &mut Rng, random_y: bool) {
    drop.active = rng.unit() <= density;
    drop.y = if random_y {
        rng.usize(height) as f64
    } else {
        -(rng.usize(height.max(2) / 2 + 1) as f64)
    };
    drop.speed = 0.28 + rng.unit() * 0.72;
    drop.length = 7 + rng.usize(18);
}

fn parse_args() -> Result<Option<Config>, String> {
    let mut palette_name = "green".to_owned();
    let mut fps = 24_u64;
    let mut density = 0.72_f64;
    let mut message = "APPLESOYLICON".to_owned();
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let args: Vec<String> = env::args().skip(1).collect();
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
            "--palette" | "--fps" | "--density" | "--message" | "--seed" => {
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
                    "--message" => message = value.clone(),
                    "--seed" => {
                        seed = value
                            .parse()
                            .map_err(|_| "--seed must be an integer".to_owned())?
                    }
                    _ => unreachable!(),
                }
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
    }))
}

fn print_help() {
    println!(
        "applesoylicon {VERSION} — terminal rain, zero consequences\n\nUSAGE:\n    applesoylicon [OPTIONS]\n\nOPTIONS:\n    --palette <NAME>    green, amber, ice, or soy [default: green]\n    --fps <N>           Frames per second, 5-60 [default: 24]\n    --density <N>       Active-column ratio, 0.05-1.0 [default: 0.72]\n    --message <TEXT>    Hidden message, 1-40 ASCII chars [default: APPLESOYLICON]\n    --seed <N>          Deterministic random seed\n    -h, --help          Print help\n    -V, --version       Print version\n\nKEYS:\n    q, Esc, Ctrl-C      Leave the rain\n    p                   Cycle palette\n    + / -               Change speed\n    d                   Toggle dense mode"
    );
}

fn run(mut config: Config) -> io::Result<()> {
    let _terminal = Terminal::enter()?;
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
    let mut palette_index = 0;
    let mut rng = Rng::new(config.seed);
    let (mut width, mut height) = terminal_size();
    let mut drops = vec![
        RainDrop {
            y: 0.0,
            speed: 0.5,
            length: 12,
            active: true
        };
        width
    ];
    for drop in &mut drops {
        reset_drop(drop, height, config.density, &mut rng, true);
    }
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
                _ => {}
            }
        }
        let (new_width, new_height) = terminal_size();
        if (new_width, new_height) != (width, height) {
            (width, height) = (new_width, new_height);
            drops = vec![
                RainDrop {
                    y: 0.0,
                    speed: 0.5,
                    length: 12,
                    active: true
                };
                width
            ];
            for drop in &mut drops {
                reset_drop(drop, height, config.density, &mut rng, true);
            }
            mark = new_mark(width, height, config.message.len(), &mut rng);
            print!("\x1b[2J");
        }
        let mut out = String::with_capacity(width * 32);
        let mut hit_mark = false;
        for (x, drop) in drops.iter_mut().enumerate() {
            if !drop.active {
                continue;
            }
            let head = drop.y as isize;
            draw_cell(&mut out, head, x, height, config.palette.head, &mut rng);
            for i in 1..=drop.length {
                let y = head - i as isize;
                let color = if i < drop.length / 3 {
                    config.palette.near
                } else {
                    config.palette.far
                };
                draw_cell(&mut out, y, x, height, color, &mut rng);
            }
            let clear = head - drop.length as isize - 1;
            if clear >= 0 && clear < height as isize {
                let _ = write!(out, "\x1b[{};{}H ", clear + 1, x + 1);
            }
            if mark.0 == head as usize && x >= mark.1 && x < mark.1 + config.message.len() {
                hit_mark = true;
            }
            drop.y += drop.speed;
            if head - drop.length as isize > height as isize {
                reset_drop(drop, height, config.density, &mut rng, false);
            }
        }
        if hit_mark {
            mark = new_mark(width, height, config.message.len(), &mut rng);
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

fn draw_cell(out: &mut String, y: isize, x: usize, height: usize, color: &str, rng: &mut Rng) {
    if y >= 0 && y < height as isize {
        let glyph = GLYPHS[rng.usize(GLYPHS.len())] as char;
        let _ = write!(out, "\x1b[{};{}H{}{}", y + 1, x + 1, color, glyph);
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
}
