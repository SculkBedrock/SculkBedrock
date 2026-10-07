//! Best-effort clipboard writes (zero third-party dependencies).
//!
//! - macOS: `pbcopy` (works in every terminal, including Terminal.app);
//! - Linux: `wl-copy` / `xclip` / `xsel` (first available in order);
//! - Windows: `clip`;
//! - Neither available: OSC 52 escape written straight to the terminal (reaches the local clipboard over SSH; needs terminal support,
//!   e.g. iTerm2/WezTerm/kitty/ghostty; legacy Terminal.app is unsupported).
//!
//! Each backend is probed once (cached in a `OnceLock`). External commands are handed to a background thread by the caller
//! (on Wayland the clipboard service needs a resident process, so `wait` would block forever); OSC 52 is written
//! directly by the caller on the thread owning stdout.

use std::sync::OnceLock;

/// Clipboard write backend (probed once, then cached).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Pbcopy,
    WlCopy,
    Xclip,
    Xsel,
    Clip,
    /// No external command: write OSC 52 straight to the terminal.
    Osc52,
}

/// Result reported back by the background copy thread.
pub struct CopyOutcome {
    /// Character count of the copied text (for the "copied N chars" notice).
    pub chars: usize,
    pub ok: bool,
}

/// Probe for an available backend (`PATH` lookup plus platform defaults).
pub fn backend() -> Backend {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    *BACKEND.get_or_init(|| {
        if cfg!(target_os = "macos") && path_has("pbcopy") {
            Backend::Pbcopy
        } else if cfg!(target_os = "windows") && path_has("clip") {
            Backend::Clip
        } else if path_has("wl-copy") {
            Backend::WlCopy
        } else if path_has("xclip") {
            Backend::Xclip
        } else if path_has("xsel") {
            Backend::Xsel
        } else if path_has("pbcopy") {
            Backend::Pbcopy
        } else if path_has("clip") {
            Backend::Clip
        } else {
            Backend::Osc52
        }
    })
}

/// Whether an executable exists on `PATH` (no spawn, pure directory lookup).
fn path_has(bin: &str) -> bool {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).any(|dir| {
        let full = dir.join(bin);
        // Append `.exe` on Windows.
        #[cfg(windows)]
        let full = if full.extension().is_none() {
            full.with_extension("exe")
        } else {
            full
        };
        full.is_file()
    })
}

/// Write to the clipboard via an external command (blocks the caller; run on a background thread).
pub fn copy_via_command(backend: Backend, text: &str) -> bool {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut cmd = match backend {
        Backend::Pbcopy => Command::new("pbcopy"),
        Backend::WlCopy => Command::new("wl-copy"),
        Backend::Xclip => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-in"]);
            c
        }
        Backend::Xsel => {
            let mut c = Command::new("xsel");
            c.args(["--clipboard", "--input"]);
            c
        }
        Backend::Clip => Command::new("clip"),
        Backend::Osc52 => return false,
    };
    let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return false,
    };
    if stdin.write_all(text.as_bytes()).is_err() {
        return false;
    }
    drop(stdin);
    // Note: `wl-copy` stays resident on Wayland, so the caller must run it on a background thread.
    child.wait().map(|status| status.success()).unwrap_or(false)
}

/// OSC 52 escape sequence (`\x1b]52;c;<base64>\x07`; text capped at 20k chars against flooding).
pub fn osc52_escape(text: &str) -> String {
    const MAX_CHARS: usize = 20_000;
    let clipped: String = text.chars().take(MAX_CHARS).collect();
    format!("\x1b]52;c;{}\x07", base64_encode(clipped.as_bytes()))
}

/// Minimal base64 encoder (standard alphabet, `=` padding; avoids a new dependency for OSC 52).
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, osc52_escape};

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Hi"), "SGk=");
        assert_eq!(base64_encode("你好".as_bytes()), "5L2g5aW9");
    }

    #[test]
    fn osc52_wraps_and_clips() {
        assert_eq!(osc52_escape("Hi"), "\x1b]52;c;SGk=\x07");
        // Truncate over-long input (flood guard).
        let long = "a".repeat(30_000);
        let esc = osc52_escape(&long);
        assert!(esc.len() < 30_000);
        assert!(esc.starts_with("\x1b]52;c;") && esc.ends_with('\x07'));
    }
}
