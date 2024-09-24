use crossterm::terminal;
use std::env;
use std::io::IsTerminal;
use std::io::{self, Write};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::runtime::Runtime;
#[cfg(target_os = "windows")]
use winapi::um::wincon;

mod stdin;

/// Terminal
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Terminal {
    Screen,
    Tmux,
    XtermCompatible,
    Windows,
    Emacs,
}

/// 16bit RGB color
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u16,
    pub g: u16,
    pub b: u16,
}

/// Background theme
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// Error
#[derive(Error, Debug)]
pub enum Error {
    #[error("io error")]
    Io {
        #[from]
        source: io::Error,
    },
    #[error("parse error")]
    Parse(String),
    #[error("unsupported")]
    Unsupported,
    #[error("timeout")]
    Timeout,
}

/// get detected termnial
#[cfg(not(target_os = "windows"))]
pub fn terminal() -> Terminal {
    if env::var("INSIDE_EMACS").is_ok() {
        return Terminal::Emacs;
    }

    if env::var("TMUX").is_ok() {
        Terminal::Tmux
    } else {
        let is_screen = if let Ok(term) = env::var("TERM") {
            term.starts_with("screen")
        } else {
            false
        };
        if is_screen {
            Terminal::Screen
        } else {
            Terminal::XtermCompatible
        }
    }
}

/// get detected termnial
#[cfg(target_os = "windows")]
pub fn terminal() -> Terminal {
    if let Ok(term_program) = env::var("TERM_PROGRAM") {
        if term_program == "vscode" {
            return Terminal::XtermCompatible;
        }
    }

    if env::var("INSIDE_EMACS").is_ok() {
        return Terminal::Emacs;
    }

    // Windows Terminal is Xterm-compatible
    // https://github.com/microsoft/terminal/issues/3718
    if env::var("WT_SESSION").is_ok() {
        Terminal::XtermCompatible
    } else {
        Terminal::Windows
    }
}

/// get background color by `RGB`
#[cfg(not(target_os = "windows"))]
pub fn rgb(timeout: Duration) -> Result<Rgb, Error> {
    let term = terminal();
    let rgb = match term {
        Terminal::Emacs => Err(Error::Unsupported),
        _ => from_xterm(term, timeout),
    };
    let fallback = from_env_colorfgbg();
    if rgb.is_ok() {
        rgb
    } else if fallback.is_ok() {
        fallback
    } else {
        rgb
    }
}

/// get background color by `RGB`
#[cfg(target_os = "windows")]
pub fn rgb(timeout: Duration) -> Result<Rgb, Error> {
    let term = terminal();
    let rgb = match term {
        Terminal::Emacs => Err(Error::Unsupported),
        Terminal::XtermCompatible => from_xterm(term, timeout),
        _ => from_winapi(),
    };
    let fallback = from_env_colorfgbg();
    if rgb.is_ok() {
        rgb
    } else if fallback.is_ok() {
        fallback
    } else {
        rgb
    }
}

/// get background color by `RGB`
#[cfg(not(target_os = "windows"))]
pub fn latency(timeout: Duration) -> Result<Duration, Error> {
    let term = terminal();
    match term {
        Terminal::Emacs => Ok(Duration::from_millis(0)),
        _ => xterm_latency(timeout),
    }
}

/// get background color by `RGB`
#[cfg(target_os = "windows")]
pub fn latency(timeout: Duration) -> Result<Duration, Error> {
    let term = terminal();
    match term {
        Terminal::Emacs => Ok(Duration::from_millis(0)),
        Terminal::XtermCompatible => xterm_latency(timeout),
        _ => Ok(Duration::from_millis(0)),
    }
}

/// get background color by `Theme`
pub fn theme(timeout: Duration) -> Result<Theme, Error> {
    let rgb = rgb(timeout)?;

    // ITU-R BT.601
    let y = rgb.r as f64 * 0.299 + rgb.g as f64 * 0.587 + rgb.b as f64 * 0.114;

    if y > 32768.0 {
        Ok(Theme::Light)
    } else {
        Ok(Theme::Dark)
    }
}

fn from_xterm(term: Terminal, timeout: Duration) -> Result<Rgb, Error> {
    if !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
    {
        // Not a terminal, so don't try to read the current background color.
        return Err(Error::Unsupported);
    }

    // Query by XTerm control sequence
    let query = if term == Terminal::Tmux {
        "\x1bPtmux;\x1b\x1b]11;?\x07\x1b\\\x03"
    } else if term == Terminal::Screen {
        "\x1bP\x1b]11;?\x07\x1b\\\x03"
    } else {
        "\x1b]11;?\x1b\\"
    };

    let mut stderr = io::stderr();
    terminal::enable_raw_mode()?;
    write!(stderr, "{}", query)?;
    stderr.flush()?;

    let rt = Runtime::new()?;
    //let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let buffer: Result<_, Error> = rt.block_on(async {
        use tokio::io::AsyncReadExt;
        use tokio::time;
        let mut buffer = Vec::new();
        let mut stdin = stdin::stdin()?;
        let mut buf = [0; 1];
        let mut start = false;
        loop {
            if let Err(_) = time::timeout(timeout, stdin.read_exact(&mut buf)).await {
                return Err(Error::Timeout);
            }
            // response terminated by BEL(0x7)
            if start && (buf[0] == 0x7) {
                break;
            }
            // response terminated by ST(0x1b 0x5c)
            if start && (buf[0] == 0x1b) {
                // consume last 0x5c
                if let Err(_) = time::timeout(timeout, stdin.read_exact(&mut buf)).await {
                    return Err(Error::Timeout);
                }
                debug_assert_eq!(buf[0], 0x5c);
                break;
            }
            if start {
                buffer.push(buf[0]);
            }
            if buf[0] == b':' {
                start = true;
            }
        }
        Ok(buffer)
    });

    terminal::disable_raw_mode()?;

    // Should return by error after disable_raw_mode
    let buffer = buffer?;

    let s = String::from_utf8_lossy(&buffer);
    let (r, g, b) = decode_x11_color(&*s)?;
    Ok(Rgb { r, g, b })
}

fn from_env_colorfgbg() -> Result<Rgb, Error> {
    let var = env::var("COLORFGBG").map_err(|_| Error::Unsupported)?;
    let fgbg: Vec<_> = var.split(";").collect();
    let bg = fgbg.get(1).ok_or(Error::Unsupported)?;
    let bg = u8::from_str_radix(bg, 10).map_err(|_| Error::Parse(String::from(var)))?;

    // rxvt default color table
    let (r, g, b) = match bg {
        // black
        0 => (0, 0, 0),
        // red
        1 => (205, 0, 0),
        // green
        2 => (0, 205, 0),
        // yellow
        3 => (205, 205, 0),
        // blue
        4 => (0, 0, 238),
        // magenta
        5 => (205, 0, 205),

        // cyan
        6 => (0, 205, 205),
        // white
        7 => (229, 229, 229),
        // bright black
        8 => (127, 127, 127),
        // bright red
        9 => (255, 0, 0),
        // bright green
        10 => (0, 255, 0),
        // bright yellow
        11 => (255, 255, 0),
        // bright blue
        12 => (92, 92, 255),
        // bright magenta
        13 => (255, 0, 255),
        // bright cyan
        14 => (0, 255, 255),

        // bright white
        15 => (255, 255, 255),
        _ => (0, 0, 0),
    };

    Ok(Rgb {
        r: r * 256,
        g: g * 256,
        b: b * 256,
    })
}

fn xterm_latency(timeout: Duration) -> Result<Duration, Error> {
    // Query by XTerm control sequence
    let query = "\x1b[5n";

    let mut stderr = io::stderr();
    terminal::enable_raw_mode()?;
    write!(stderr, "{}", query)?;
    stderr.flush()?;

    let start = Instant::now();

    let rt = Runtime::new()?;
    //let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let ret: Result<_, Error> = rt.block_on(async {
        use tokio::io::AsyncReadExt;
        use tokio::time;
        let mut stdin = stdin::stdin()?;
        let mut buf = [0; 1];
        loop {
            if let Err(_) = time::timeout(timeout, stdin.read_exact(&mut buf)).await {
                return Err(Error::Timeout);
            }
            // response terminated by 'n'
            if buf[0] == b'n' {
                break;
            }
        }
        Ok(())
    });

    let end = start.elapsed();

    terminal::disable_raw_mode()?;

    let _ = ret?;

    Ok(end)
}

fn decode_x11_color(s: &str) -> Result<(u16, u16, u16), Error> {
    fn decode_hex(s: &str) -> Result<u16, Error> {
        let len = s.len() as u32;
        let mut ret = u16::from_str_radix(s, 16).map_err(|_| Error::Parse(String::from(s)))?;
        ret = ret << ((4 - len) * 4);
        Ok(ret)
    }

    let rgb: Vec<_> = s.split("/").collect();

    let r = rgb.get(0).ok_or_else(|| Error::Parse(String::from(s)))?;
    let g = rgb.get(1).ok_or_else(|| Error::Parse(String::from(s)))?;
    let b = rgb.get(2).ok_or_else(|| Error::Parse(String::from(s)))?;
    let r = decode_hex(r)?;
    let g = decode_hex(g)?;
    let b = decode_hex(b)?;

    Ok((r, g, b))
}

#[cfg(target_os = "windows")]
fn from_winapi() -> Result<Rgb, Error> {
    let info = unsafe {
        let handle = winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE);
        let mut info: wincon::CONSOLE_SCREEN_BUFFER_INFO = Default::default();
        wincon::GetConsoleScreenBufferInfo(handle, &mut info);
        info
    };

    let r = (wincon::BACKGROUND_RED & info.wAttributes) != 0;
    let g = (wincon::BACKGROUND_GREEN & info.wAttributes) != 0;
    let b = (wincon::BACKGROUND_BLUE & info.wAttributes) != 0;
    let i = (wincon::BACKGROUND_INTENSITY & info.wAttributes) != 0;

    let r: u8 = r as u8;
    let g: u8 = g as u8;
    let b: u8 = b as u8;
    let i: u8 = i as u8;

    let (r, g, b) = match (r, g, b, i) {
        (0, 0, 0, 0) => (0, 0, 0),
        (1, 0, 0, 0) => (128, 0, 0),
        (0, 1, 0, 0) => (0, 128, 0),
        (1, 1, 0, 0) => (128, 128, 0),
        (0, 0, 1, 0) => (0, 0, 128),
        (1, 0, 1, 0) => (128, 0, 128),
        (0, 1, 1, 0) => (0, 128, 128),
        (1, 1, 1, 0) => (192, 192, 192),
        (0, 0, 0, 1) => (128, 128, 128),
        (1, 0, 0, 1) => (255, 0, 0),
        (0, 1, 0, 1) => (0, 255, 0),
        (1, 1, 0, 1) => (255, 255, 0),
        (0, 0, 1, 1) => (0, 0, 255),
        (1, 0, 1, 1) => (255, 0, 255),
        (0, 1, 1, 1) => (0, 255, 255),
        (1, 1, 1, 1) => (255, 255, 255),
        _ => unreachable!(),
    };

    Ok(Rgb {
        r: r * 256,
        g: g * 256,
        b: b * 256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_x11_color() {
        let s = "0000/0000/0000";
        assert_eq!((0, 0, 0), decode_x11_color(s).unwrap());

        let s = "1111/2222/3333";
        assert_eq!((0x1111, 0x2222, 0x3333), decode_x11_color(s).unwrap());

        let s = "111/222/333";
        assert_eq!((0x1110, 0x2220, 0x3330), decode_x11_color(s).unwrap());

        let s = "11/22/33";
        assert_eq!((0x1100, 0x2200, 0x3300), decode_x11_color(s).unwrap());

        let s = "1/2/3";
        assert_eq!((0x1000, 0x2000, 0x3000), decode_x11_color(s).unwrap());
    }
}
