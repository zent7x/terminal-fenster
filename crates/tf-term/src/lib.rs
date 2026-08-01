//! Terminal-Fenster terminal core.
//!
//! Owns everything that touches the user's terminal: capability detection, graphics
//! protocol encoding, raw-mode lifecycle, and input decoding.
//!
//! The single most important invariant in this crate: **the terminal must always be
//! restored.** A browser that leaves the tty in raw mode with mouse reporting enabled and
//! a stale image on screen has broken the user's shell, and they will have to blindly type
//! `reset`. Every path out -- normal exit, `?`, panic, SIGINT, SIGTERM, SIGHUP -- must go
//! through the restore logic. See [`tty::TtyGuard`].

pub mod b64;
pub mod caps;
pub mod cursor;
pub mod input;
pub mod kitty;
pub mod scroll;
pub mod transport;
pub mod tty;
pub mod unicode;

/// A rectangular region in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
    /// Smallest rect covering both.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
    /// Clamp to a bounding size, returning None if fully outside.
    pub fn clamp_to(&self, w: u32, h: u32) -> Option<Rect> {
        if self.x >= w || self.y >= h {
            return None;
        }
        let cw = self.w.min(w - self.x);
        let ch = self.h.min(h - self.y);
        if cw == 0 || ch == 0 {
            return None;
        }
        Some(Rect::new(self.x, self.y, cw, ch))
    }
}

/// Which drawing backend the renderer will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Kitty graphics protocol. True pixels, best performance.
    Kitty,
    /// Sixel (DEC). True pixels, palette-limited.
    Sixel,
    /// iTerm2 OSC 1337 inline images.
    Iterm2,
    /// Unicode half-block fallback. Low fidelity, works anywhere with truecolor.
    Unicode,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Kitty => "kitty",
            Backend::Sixel => "sixel",
            Backend::Iterm2 => "iterm2",
            Backend::Unicode => "unicode",
        }
    }
    /// True if this backend draws real pixels rather than approximating with glyphs.
    pub fn is_pixel_exact(&self) -> bool {
        !matches!(self, Backend::Unicode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union_handles_empty() {
        let empty = Rect::default();
        let a = Rect::new(10, 10, 5, 5);
        assert_eq!(empty.union(&a), a);
        assert_eq!(a.union(&empty), a);
    }

    #[test]
    fn rect_union_covers_both() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 5, 10, 30);
        assert_eq!(a.union(&b), Rect::new(0, 0, 30, 35));
    }

    #[test]
    fn rect_clamp_inside() {
        let r = Rect::new(5, 5, 100, 100);
        assert_eq!(r.clamp_to(50, 40), Some(Rect::new(5, 5, 45, 35)));
    }

    #[test]
    fn rect_clamp_fully_outside_is_none() {
        let r = Rect::new(80, 80, 10, 10);
        assert_eq!(r.clamp_to(50, 50), None);
    }
}
