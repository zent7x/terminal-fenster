//! Terminal input decoding: SGR mouse, Kitty keyboard protocol, legacy keys, focus, paste.
//!
//! This is a byte-stream state machine. Input arrives in arbitrary chunks -- a single escape
//! sequence can be split across two `read()` calls -- so the decoder must buffer and only
//! consume complete sequences. Every `decode` call returns the events it could fully parse
//! and leaves any partial tail in the buffer.
//!
//! # The ESC ambiguity
//!
//! A lone `ESC` byte is genuinely ambiguous in legacy encoding: it may be the Escape key, or
//! the first byte of a sequence that has not arrived yet. Terminals resolve this with a
//! timeout. We expose that as [`Decoder::flush_pending_escape`] so the caller owns the
//! policy rather than burying a `sleep` in the parser. The Kitty keyboard protocol removes
//! this ambiguity entirely, which is a major reason we prefer it.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

impl Modifiers {
    /// Decode a Kitty/xterm modifier parameter. The wire value is `bitmask + 1`.
    pub fn from_kitty_param(v: u32) -> Self {
        let b = v.saturating_sub(1);
        Self {
            shift: b & 1 != 0,
            alt: b & 2 != 0,
            ctrl: b & 4 != 0,
            meta: b & 8 != 0,
        }
    }
    pub fn any(&self) -> bool {
        self.shift || self.alt || self.ctrl || self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Move,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Key {
        code: KeyCode,
        mods: Modifiers,
        kind: KeyEventKind,
        /// Text the terminal associated with this key, when it reports it (Kitty flag 16).
        text: Option<String>,
    },
    Mouse {
        kind: MouseKind,
        button: MouseButton,
        /// Coordinates. Pixel-precise when SGR-pixels (mode 1016) is active, otherwise
        /// cell coordinates that the caller must scale.
        x: u32,
        y: u32,
        mods: Modifiers,
    },
    FocusGained,
    FocusLost,
    Paste(String),
    /// Bytes we could not interpret. Surfaced rather than silently dropped so that
    /// unknown-sequence bugs are observable instead of mysterious.
    Unknown(Vec<u8>),
}

#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    in_paste: bool,
    paste_buf: Vec<u8>,
    /// True when SGR-pixels (1016) is active, so coordinates are already pixels.
    pub pixel_mouse: bool,
}

impl Decoder {
    pub fn new(pixel_mouse: bool) -> Self {
        Self { pixel_mouse, ..Default::default() }
    }

    /// Feed bytes and drain all complete events.
    pub fn decode(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        let mut out = VecDeque::new();
        loop {
            match self.step() {
                Step::Event(e) => out.push_back(e),
                Step::NeedMore => break,
                Step::Consumed => continue,
            }
        }
        out.into()
    }

    /// Number of bytes still buffered awaiting more input.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Resolve a lone buffered `ESC` as the Escape key. The caller decides the timeout.
    pub fn flush_pending_escape(&mut self) -> Option<Event> {
        if self.buf.len() == 1 && self.buf[0] == 0x1b {
            self.buf.clear();
            return Some(Event::Key {
                code: KeyCode::Escape,
                mods: Modifiers::default(),
                kind: KeyEventKind::Press,
                text: None,
            });
        }
        None
    }

    fn step(&mut self) -> Step {
        if self.buf.is_empty() {
            return Step::NeedMore;
        }

        if self.in_paste {
            return self.step_paste();
        }

        if self.buf[0] != 0x1b {
            return self.step_plain();
        }
        if self.buf.len() < 2 {
            return Step::NeedMore; // ambiguous lone ESC
        }
        match self.buf[1] {
            b'[' => self.step_csi(),
            b'O' => self.step_ss3(),
            _ => {
                // ESC <char> = Alt+char in legacy encoding.
                let (ch, len) = match decode_utf8(&self.buf[1..]) {
                    Some(v) => v,
                    None => return Step::NeedMore,
                };
                self.buf.drain(..1 + len);
                Step::Event(Event::Key {
                    code: KeyCode::Char(ch),
                    mods: Modifiers { alt: true, ..Default::default() },
                    kind: KeyEventKind::Press,
                    text: None,
                })
            }
        }
    }

    fn step_paste(&mut self) -> Step {
        const END: &[u8] = b"\x1b[201~";
        if let Some(pos) = find(&self.buf, END) {
            self.paste_buf.extend_from_slice(&self.buf[..pos]);
            self.buf.drain(..pos + END.len());
            self.in_paste = false;
            let text = String::from_utf8_lossy(&self.paste_buf).into_owned();
            self.paste_buf.clear();
            return Step::Event(Event::Paste(text));
        }
        // Keep a tail that could contain a partial terminator.
        if self.buf.len() > END.len() {
            let keep = END.len() - 1;
            let take = self.buf.len() - keep;
            self.paste_buf.extend_from_slice(&self.buf[..take]);
            self.buf.drain(..take);
        }
        Step::NeedMore
    }

    fn step_plain(&mut self) -> Step {
        let b = self.buf[0];
        // C0 controls first: these are unambiguous.
        let ev = match b {
            b'\r' | b'\n' => Some((KeyCode::Enter, Modifiers::default())),
            b'\t' => Some((KeyCode::Tab, Modifiers::default())),
            0x7f | 0x08 => Some((KeyCode::Backspace, Modifiers::default())),
            0x01..=0x1a => {
                // Ctrl+letter. 0x09/0x0a/0x0d already handled above.
                let ch = (b'a' + (b - 1)) as char;
                Some((KeyCode::Char(ch), Modifiers { ctrl: true, ..Default::default() }))
            }
            _ => None,
        };
        if let Some((code, mods)) = ev {
            self.buf.drain(..1);
            return Step::Event(Event::Key { code, mods, kind: KeyEventKind::Press, text: None });
        }
        match decode_utf8(&self.buf) {
            Some((ch, len)) => {
                self.buf.drain(..len);
                Step::Event(Event::Key {
                    code: KeyCode::Char(ch),
                    mods: Modifiers::default(),
                    kind: KeyEventKind::Press,
                    text: Some(ch.to_string()),
                })
            }
            None => Step::NeedMore,
        }
    }

    fn step_csi(&mut self) -> Step {
        // Find the final byte in the range 0x40..=0x7e.
        let mut end = None;
        for (i, &b) in self.buf.iter().enumerate().skip(2) {
            if (0x40..=0x7e).contains(&b) {
                end = Some(i);
                break;
            }
        }
        let end = match end {
            Some(e) => e,
            None => return Step::NeedMore,
        };
        let seq: Vec<u8> = self.buf[..=end].to_vec();
        let final_byte = seq[end];
        let body = &seq[2..end];

        // Bracketed paste start.
        if body == b"200" && final_byte == b'~' {
            self.buf.drain(..=end);
            self.in_paste = true;
            return Step::Consumed;
        }
        // Focus events.
        if body.is_empty() && (final_byte == b'I' || final_byte == b'O') {
            self.buf.drain(..=end);
            return Step::Event(if final_byte == b'I' { Event::FocusGained } else { Event::FocusLost });
        }
        // SGR mouse: CSI < btn ; col ; row (M|m)
        if body.first() == Some(&b'<') && (final_byte == b'M' || final_byte == b'm') {
            let parts = split_params(&body[1..]);
            if parts.len() >= 3 {
                let ev = decode_sgr_mouse(parts[0], parts[1], parts[2], final_byte == b'M');
                self.buf.drain(..=end);
                return match ev {
                    Some(e) => Step::Event(e),
                    None => Step::Consumed,
                };
            }
        }
        // Kitty keyboard: CSI unicode-key[:alt] ; mods[:event-type] [; text] u
        if final_byte == b'u' {
            let ev = decode_kitty_key(body);
            self.buf.drain(..=end);
            return match ev {
                Some(e) => Step::Event(e),
                None => Step::Consumed,
            };
        }
        // Legacy special keys, optionally with a modifier parameter.
        if let Some(ev) = decode_legacy_csi(body, final_byte) {
            self.buf.drain(..=end);
            return Step::Event(ev);
        }
        self.buf.drain(..=end);
        Step::Event(Event::Unknown(seq))
    }

    fn step_ss3(&mut self) -> Step {
        if self.buf.len() < 3 {
            return Step::NeedMore;
        }
        let c = self.buf[2];
        let code = match c {
            b'A' => KeyCode::Up,
            b'B' => KeyCode::Down,
            b'C' => KeyCode::Right,
            b'D' => KeyCode::Left,
            b'H' => KeyCode::Home,
            b'F' => KeyCode::End,
            b'P' => KeyCode::F(1),
            b'Q' => KeyCode::F(2),
            b'R' => KeyCode::F(3),
            b'S' => KeyCode::F(4),
            _ => {
                let seq = self.buf[..3].to_vec();
                self.buf.drain(..3);
                return Step::Event(Event::Unknown(seq));
            }
        };
        self.buf.drain(..3);
        Step::Event(Event::Key {
            code,
            mods: Modifiers::default(),
            kind: KeyEventKind::Press,
            text: None,
        })
    }
}

enum Step {
    Event(Event),
    Consumed,
    NeedMore,
}

fn decode_sgr_mouse(btn: u32, x: u32, y: u32, press: bool) -> Option<Event> {
    let mods = Modifiers {
        shift: btn & 4 != 0,
        alt: btn & 8 != 0,
        ctrl: btn & 16 != 0,
        meta: false,
    };
    let motion = btn & 32 != 0;
    let wheel = btn & 64 != 0;
    let low = btn & 3;

    let (kind, button) = if wheel {
        match low {
            0 => (MouseKind::WheelUp, MouseButton::None),
            1 => (MouseKind::WheelDown, MouseButton::None),
            2 => (MouseKind::WheelLeft, MouseButton::None),
            _ => (MouseKind::WheelRight, MouseButton::None),
        }
    } else {
        let button = match low {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        if motion {
            (MouseKind::Move, button)
        } else if press {
            (MouseKind::Down, button)
        } else {
            (MouseKind::Up, button)
        }
    };
    Some(Event::Mouse { kind, button, x, y, mods })
}

fn decode_kitty_key(body: &[u8]) -> Option<Event> {
    // key[:shifted:base] ; mods[:event] ; text-codepoints
    let sections: Vec<&[u8]> = body.split(|&b| b == b';').collect();
    if sections.is_empty() {
        return None;
    }
    let key_parts: Vec<&[u8]> = sections[0].split(|&b| b == b':').collect();
    let keynum: u32 = parse_u32(key_parts[0])?;

    let (mods, kind) = if sections.len() > 1 && !sections[1].is_empty() {
        let mp: Vec<&[u8]> = sections[1].split(|&b| b == b':').collect();
        let m = parse_u32(mp[0]).map(Modifiers::from_kitty_param).unwrap_or_default();
        let k = if mp.len() > 1 {
            match parse_u32(mp[1]) {
                Some(2) => KeyEventKind::Repeat,
                Some(3) => KeyEventKind::Release,
                _ => KeyEventKind::Press,
            }
        } else {
            KeyEventKind::Press
        };
        (m, k)
    } else {
        (Modifiers::default(), KeyEventKind::Press)
    };

    let text = if sections.len() > 2 && !sections[2].is_empty() {
        let s: String = sections[2]
            .split(|&b| b == b':')
            .filter_map(parse_u32)
            .filter_map(char::from_u32)
            .collect();
        if s.is_empty() { None } else { Some(s) }
    } else {
        None
    };

    // Functional keys use the Kitty private-use codepoint block; the rest are literal
    // Unicode codepoints of the key's base layout character.
    let code = match keynum {
        13 => KeyCode::Enter,
        9 => KeyCode::Tab,
        127 => KeyCode::Backspace,
        27 => KeyCode::Escape,
        57359 => KeyCode::F(1),
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        7 => KeyCode::Home,
        8 => KeyCode::End,
        _ => KeyCode::Char(char::from_u32(keynum)?),
    };
    Some(Event::Key { code, mods, kind, text })
}

fn decode_legacy_csi(body: &[u8], final_byte: u8) -> Option<Event> {
    let params = split_params(body);
    let mods = if params.len() >= 2 {
        Modifiers::from_kitty_param(params[1])
    } else {
        Modifiers::default()
    };
    let code = match final_byte {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        b'~' => match params.first().copied()? {
            1 | 7 => KeyCode::Home,
            2 => KeyCode::Insert,
            3 => KeyCode::Delete,
            4 | 8 => KeyCode::End,
            5 => KeyCode::PageUp,
            6 => KeyCode::PageDown,
            11..=15 => KeyCode::F((params[0] - 10) as u8),
            17..=21 => KeyCode::F((params[0] - 11) as u8),
            23..=26 => KeyCode::F((params[0] - 12) as u8),
            _ => return None,
        },
        _ => return None,
    };
    Some(Event::Key { code, mods, kind: KeyEventKind::Press, text: None })
}

fn split_params(body: &[u8]) -> Vec<u32> {
    body.split(|&b| b == b';').filter_map(parse_u32).collect()
}

fn parse_u32(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(v)
}

/// Decode one UTF-8 scalar, returning None if the buffer holds only a partial sequence.
fn decode_utf8(b: &[u8]) -> Option<(char, usize)> {
    if b.is_empty() {
        return None;
    }
    let len = match b[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return Some((char::REPLACEMENT_CHARACTER, 1)), // invalid lead byte
    };
    if b.len() < len {
        return None; // partial: wait for more bytes
    }
    match std::str::from_utf8(&b[..len]) {
        Ok(s) => s.chars().next().map(|c| (c, len)),
        Err(_) => Some((char::REPLACEMENT_CHARACTER, 1)),
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec() -> Decoder {
        Decoder::new(true)
    }

    #[test]
    fn plain_ascii_is_a_char_key() {
        let ev = dec().decode(b"a");
        assert_eq!(
            ev,
            vec![Event::Key {
                code: KeyCode::Char('a'),
                mods: Modifiers::default(),
                kind: KeyEventKind::Press,
                text: Some("a".into())
            }]
        );
    }

    #[test]
    fn ctrl_letter_decodes() {
        let ev = dec().decode(&[0x03]); // Ctrl+C
        match &ev[0] {
            Event::Key { code, mods, .. } => {
                assert_eq!(*code, KeyCode::Char('c'));
                assert!(mods.ctrl);
            }
            _ => panic!("expected key"),
        }
    }

    #[test]
    fn sgr_mouse_press_and_release() {
        let mut d = dec();
        let ev = d.decode(b"\x1b[<0;100;200M");
        assert_eq!(
            ev,
            vec![Event::Mouse {
                kind: MouseKind::Down,
                button: MouseButton::Left,
                x: 100,
                y: 200,
                mods: Modifiers::default()
            }]
        );
        let ev = d.decode(b"\x1b[<0;100;200m");
        match &ev[0] {
            Event::Mouse { kind, .. } => assert_eq!(*kind, MouseKind::Up),
            _ => panic!(),
        }
    }

    #[test]
    fn sgr_mouse_wheel_directions() {
        let mut d = dec();
        match &d.decode(b"\x1b[<64;5;5M")[0] {
            Event::Mouse { kind, .. } => assert_eq!(*kind, MouseKind::WheelUp),
            _ => panic!(),
        }
        match &d.decode(b"\x1b[<65;5;5M")[0] {
            Event::Mouse { kind, .. } => assert_eq!(*kind, MouseKind::WheelDown),
            _ => panic!(),
        }
    }

    #[test]
    fn sgr_mouse_motion_flag_yields_move() {
        // Bit 32 set = motion. Hover depends on this being a Move, not a Down.
        match &dec().decode(b"\x1b[<35;10;10M")[0] {
            Event::Mouse { kind, .. } => assert_eq!(*kind, MouseKind::Move),
            _ => panic!(),
        }
    }

    #[test]
    fn sgr_mouse_modifier_bits() {
        // 4=shift, 8=alt, 16=ctrl -> 0+4+16 = 20
        match &dec().decode(b"\x1b[<20;1;1M")[0] {
            Event::Mouse { mods, .. } => {
                assert!(mods.shift && mods.ctrl && !mods.alt);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn split_escape_sequence_across_reads_is_reassembled() {
        // The whole reason the decoder buffers: a read() can split a sequence anywhere.
        let mut d = dec();
        assert!(d.decode(b"\x1b[<0;10").is_empty(), "partial sequence must not emit");
        let ev = d.decode(b";20M");
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            Event::Mouse { x, y, .. } => {
                assert_eq!((*x, *y), (10, 20));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn split_utf8_across_reads_is_reassembled() {
        let mut d = dec();
        let euro = "€".as_bytes(); // 3 bytes
        assert!(d.decode(&euro[..2]).is_empty(), "partial UTF-8 must wait");
        let ev = d.decode(&euro[2..]);
        match &ev[0] {
            Event::Key { code, .. } => assert_eq!(*code, KeyCode::Char('€')),
            _ => panic!(),
        }
    }

    #[test]
    fn lone_escape_is_ambiguous_until_flushed() {
        let mut d = dec();
        assert!(d.decode(b"\x1b").is_empty(), "lone ESC must not resolve immediately");
        assert_eq!(d.pending(), 1);
        let ev = d.flush_pending_escape().unwrap();
        match ev {
            Event::Key { code, .. } => assert_eq!(code, KeyCode::Escape),
            _ => panic!(),
        }
    }

    #[test]
    fn arrow_keys_legacy_and_ss3() {
        match &dec().decode(b"\x1b[A")[0] {
            Event::Key { code, .. } => assert_eq!(*code, KeyCode::Up),
            _ => panic!(),
        }
        match &dec().decode(b"\x1bOB")[0] {
            Event::Key { code, .. } => assert_eq!(*code, KeyCode::Down),
            _ => panic!(),
        }
    }

    #[test]
    fn legacy_modified_arrow() {
        // CSI 1;5A = Ctrl+Up  (modifier param is bitmask+1, so 5 => 4 => ctrl)
        match &dec().decode(b"\x1b[1;5A")[0] {
            Event::Key { code, mods, .. } => {
                assert_eq!(*code, KeyCode::Up);
                assert!(mods.ctrl);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn function_and_nav_keys() {
        let cases: &[(&[u8], KeyCode)] = &[
            (b"\x1b[3~", KeyCode::Delete),
            (b"\x1b[5~", KeyCode::PageUp),
            (b"\x1b[6~", KeyCode::PageDown),
            (b"\x1b[2~", KeyCode::Insert),
            (b"\x1b[15~", KeyCode::F(5)),
        ];
        for (bytes, expect) in cases {
            match &dec().decode(bytes)[0] {
                Event::Key { code, .. } => assert_eq!(code, expect, "for {bytes:?}"),
                _ => panic!("expected key for {bytes:?}"),
            }
        }
    }

    #[test]
    fn kitty_key_with_modifiers_and_release() {
        // CSI 97 ; 5 : 3 u  => 'a', ctrl (5-1=4), release (3)
        match &dec().decode(b"\x1b[97;5:3u")[0] {
            Event::Key { code, mods, kind, .. } => {
                assert_eq!(*code, KeyCode::Char('a'));
                assert!(mods.ctrl);
                assert_eq!(*kind, KeyEventKind::Release);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn kitty_key_reports_associated_text() {
        // Third section carries the text as codepoints -- this is how we get correct
        // characters for layouts and dead keys instead of guessing from keycodes.
        match &dec().decode(b"\x1b[97;1;97u")[0] {
            Event::Key { text, .. } => assert_eq!(text.as_deref(), Some("a")),
            _ => panic!(),
        }
    }

    #[test]
    fn modifier_param_is_bitmask_plus_one() {
        assert_eq!(Modifiers::from_kitty_param(1), Modifiers::default());
        let m = Modifiers::from_kitty_param(2);
        assert!(m.shift && !m.alt);
        let m = Modifiers::from_kitty_param(8); // 7 = shift|alt|ctrl
        assert!(m.shift && m.alt && m.ctrl);
    }

    #[test]
    fn focus_events() {
        assert_eq!(dec().decode(b"\x1b[I"), vec![Event::FocusGained]);
        assert_eq!(dec().decode(b"\x1b[O"), vec![Event::FocusLost]);
    }

    #[test]
    fn bracketed_paste_captures_body() {
        let ev = dec().decode(b"\x1b[200~hello world\x1b[201~");
        assert_eq!(ev, vec![Event::Paste("hello world".into())]);
    }

    #[test]
    fn bracketed_paste_split_across_reads() {
        let mut d = dec();
        assert!(d.decode(b"\x1b[200~part").is_empty());
        assert!(d.decode(b"ial ").is_empty());
        let ev = d.decode(b"text\x1b[201~");
        assert_eq!(ev, vec![Event::Paste("partial text".into())]);
    }

    #[test]
    fn paste_containing_escape_bytes_is_not_reinterpreted() {
        // A paste carrying escape-looking bytes must be treated as literal text, never
        // executed as terminal input -- this is an injection guard, not a nicety.
        let ev = dec().decode(b"\x1b[200~evil\x1b[<0;1;1M\x1b[201~");
        assert_eq!(ev, vec![Event::Paste("evil\x1b[<0;1;1M".into())]);
    }

    #[test]
    fn multiple_events_in_one_read() {
        let ev = dec().decode(b"ab\x1b[<0;1;1M");
        assert_eq!(ev.len(), 3);
    }

    #[test]
    fn unknown_sequence_is_surfaced_not_swallowed() {
        let ev = dec().decode(b"\x1b[999Z");
        assert!(matches!(ev[0], Event::Unknown(_)));
    }

    #[test]
    fn garbage_bytes_do_not_panic() {
        // Fuzz-lite: the decoder must never panic on hostile input.
        let mut d = dec();
        let mut x: u32 = 7;
        for _ in 0..2000 {
            let mut chunk = Vec::new();
            for _ in 0..16 {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                chunk.push((x >> 16) as u8);
            }
            let _ = d.decode(&chunk);
        }
    }
}
