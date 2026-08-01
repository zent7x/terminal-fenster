//! Wire protocol between the terminal core and the Chromium engine host.
//!
//! Framing is `[u8 type][u32 BE length][payload]` in both directions. Length-prefixing
//! (rather than a delimiter) matters because frame payloads are raw BGRA that can contain
//! any byte sequence including newlines.
//!
//! A deliberate asymmetry: commands and events are JSON because they are low-rate and
//! benefit from being readable in logs, while frames are binary because a 5 MB BGRA buffer
//! through JSON would be indefensible.

pub const T_FRAME: u8 = 1;
pub const T_EVENT: u8 = 2;
pub const T_COMMAND: u8 = 10;

/// Bytes of the fixed frame header that precede the pixel payload.
pub const FRAME_HEADER_LEN: usize = 32;

/// Hard ceiling on a single message payload.
///
/// The length prefix is attacker-influenced in the sense that a buggy or compromised engine
/// can send any u32. Reserving on it unchecked lets a 4 GiB claim OOM the terminal core, so
/// the reader refuses oversized frames instead of trusting the header. 64 MiB comfortably
/// exceeds a 4K BGRA frame (3840*2160*4 = 33.2 MiB).
pub const MAX_MESSAGE_LEN: usize = 64 * 1024 * 1024;

/// A decoded frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u32,
    pub width: u32,
    pub height: u32,
    pub dirty_x: u32,
    pub dirty_y: u32,
    pub dirty_w: u32,
    pub dirty_h: u32,
    /// 0 = BGRA8888
    pub format: u32,
}

impl FrameHeader {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < FRAME_HEADER_LEN {
            return None;
        }
        let g = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Some(Self {
            seq: g(0),
            width: g(4),
            height: g(8),
            dirty_x: g(12),
            dirty_y: g(16),
            dirty_w: g(20),
            dirty_h: g(24),
            format: g(28),
        })
    }

    /// Expected pixel-payload length for this geometry, or `None` if the declared geometry
    /// cannot describe a real frame.
    ///
    /// `width * height * 4` with two attacker-influenced u32s overflows: u32::MAX squared
    /// times four is ~6.8e19, well past u64's 1.8e19. An overflowed product wraps to a small
    /// number, which would make a truncated-frame check pass and hand the renderer a buffer
    /// far shorter than the geometry claims.
    pub fn checked_payload(&self) -> Option<usize> {
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(4)
    }

    /// Convenience wrapper returning 0 for impossible geometry, so callers that compare
    /// `payload.len() < expected` reject rather than accept a bogus frame.
    pub fn expected_payload(&self) -> usize {
        self.checked_payload().unwrap_or(usize::MAX)
    }

    /// Byte length of the pixel payload when the engine sends only the dirty rectangle
    /// (damage tracking, proven by the B02 spike). `dirty_w * dirty_h * 4`, overflow-checked.
    ///
    /// A full-frame update is simply the case where the dirty rect equals the whole frame,
    /// so this subsumes [`checked_payload`](Self::checked_payload) — the renderer only ever
    /// needs the dirty length once it composites into a persistent framebuffer.
    pub fn checked_dirty_payload(&self) -> Option<usize> {
        (self.dirty_w as usize)
            .checked_mul(self.dirty_h as usize)?
            .checked_mul(4)
    }

    /// Saturating wrapper, so `payload.len() < dirty_payload()` rejects impossible geometry.
    pub fn dirty_payload(&self) -> usize {
        self.checked_dirty_payload().unwrap_or(usize::MAX)
    }

    /// Byte length of the full-frame packed-RGB framebuffer this header describes:
    /// `width * height * 3`, overflow-checked. `None` for geometry that cannot exist.
    ///
    /// This matters specifically because, with partial frames, the payload length no longer
    /// bounds `width * height`: a bogus header could claim a 4-gigapixel frame behind a
    /// 16-byte dirty rect. Callers size (and cap) their framebuffer allocation off this.
    pub fn checked_rgb_len(&self) -> Option<usize> {
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(3)
    }

    /// True iff the dirty rectangle lies wholly inside the declared frame and the pixel
    /// format is one we understand (0 = BGRA8888). This is the guard that makes a blit into
    /// the framebuffer safe: without it a malformed `(x,y,w,h)` would index out of bounds.
    pub fn dirty_within_frame(&self) -> bool {
        if self.format != 0 {
            return false;
        }
        match (
            self.dirty_x.checked_add(self.dirty_w),
            self.dirty_y.checked_add(self.dirty_h),
        ) {
            (Some(x1), Some(y1)) => x1 <= self.width && y1 <= self.height,
            _ => false,
        }
    }
}

/// Incremental reader for length-prefixed messages.
#[derive(Default)]
pub struct MessageReader {
    buf: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub type_id: u8,
    pub payload: Vec<u8>,
}

impl MessageReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Pop one complete message, if the buffer holds one.
    ///
    /// Returns `Err` if the declared length exceeds [`MAX_MESSAGE_LEN`]; the stream is then
    /// unrecoverable (we cannot know where the next message starts) and the caller must
    /// tear the session down rather than resynchronise on garbage.
    pub fn next_message(&mut self) -> Option<Message> {
        self.try_next_message().ok().flatten()
    }

    pub fn try_next_message(&mut self) -> Result<Option<Message>, ProtocolError> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let type_id = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_MESSAGE_LEN {
            return Err(ProtocolError::MessageTooLarge(len));
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Ok(Some(Message { type_id, payload }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Declared payload length exceeds [`MAX_MESSAGE_LEN`].
    MessageTooLarge(usize),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::MessageTooLarge(n) => write!(
                f,
                "engine declared a {n}-byte message, above the {MAX_MESSAGE_LEN}-byte limit"
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Frame a message for transmission.
pub fn frame_message(type_id: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(type_id);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Escape a string as a JSON string body (without surrounding quotes).
pub fn json_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

/// Extract a top-level string field from a flat JSON object.
///
/// Deliberately minimal: both sides of this protocol are ours, the messages are flat, and a
/// full JSON parser is a dependency and an attack surface we do not need here. It handles
/// escapes correctly, which is the part that actually matters.
pub fn json_get_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let colon = rest.find(':')? + 1;
    let rest = rest[colon..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let body = &rest[1..];
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let v = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Extract a top-level boolean field.
pub fn json_get_bool(json: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_roundtrip() {
        let mut b = Vec::new();
        for v in [7u32, 1440, 900, 10, 20, 30, 40, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let h = FrameHeader::parse(&b).unwrap();
        assert_eq!(h.seq, 7);
        assert_eq!(h.width, 1440);
        assert_eq!(h.height, 900);
        assert_eq!((h.dirty_x, h.dirty_y, h.dirty_w, h.dirty_h), (10, 20, 30, 40));
        assert_eq!(h.expected_payload(), 1440 * 900 * 4);
    }

    #[test]
    fn absurd_geometry_does_not_overflow_into_a_small_length() {
        // A malicious header claiming u32::MAX x u32::MAX must not wrap to something a
        // short payload can satisfy.
        let mut b = Vec::new();
        for v in [0u32, u32::MAX, u32::MAX, 0, 0, 0, 0, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let h = FrameHeader::parse(&b).unwrap();
        assert_eq!(h.checked_payload(), None);
        // expected_payload saturates high so `len < expected` rejects.
        assert_eq!(h.expected_payload(), usize::MAX);
    }

    #[test]
    fn frame_header_rejects_short_input() {
        assert!(FrameHeader::parse(&[0u8; 31]).is_none());
    }

    fn header(vals: [u32; 8]) -> FrameHeader {
        let mut b = Vec::new();
        for v in vals {
            b.extend_from_slice(&v.to_be_bytes());
        }
        FrameHeader::parse(&b).unwrap()
    }

    #[test]
    fn dirty_payload_is_the_rect_not_the_frame() {
        // A 40x40 damage rect on a 1440x900 frame: the payload is the rect, not the frame.
        let h = header([1, 1440, 900, 600, 400, 40, 40, 0]);
        assert_eq!(h.checked_dirty_payload(), Some(40 * 40 * 4));
        assert_eq!(h.checked_rgb_len(), Some(1440 * 900 * 3));
        // A full-frame update is just dirty == whole frame, and dirty_payload subsumes it.
        let full = header([1, 4, 4, 0, 0, 4, 4, 0]);
        assert_eq!(full.checked_dirty_payload(), full.checked_payload());
    }

    #[test]
    fn dirty_within_frame_accepts_a_real_rect_and_rejects_overflow() {
        assert!(header([0, 1440, 900, 600, 400, 40, 40, 0]).dirty_within_frame());
        assert!(header([0, 100, 100, 0, 0, 100, 100, 0]).dirty_within_frame()); // full-frame
        // Rect spilling past the right edge must be rejected before it indexes a blit OOB.
        assert!(!header([0, 100, 100, 80, 0, 40, 10, 0]).dirty_within_frame());
        // Rect spilling past the bottom edge.
        assert!(!header([0, 100, 100, 0, 95, 10, 10, 0]).dirty_within_frame());
        // x + w that overflows u32 must not wrap to something that looks in-bounds.
        assert!(!header([0, 100, 100, u32::MAX, 0, 10, 10, 0]).dirty_within_frame());
        // Unknown pixel format is refused.
        assert!(!header([0, 100, 100, 0, 0, 100, 100, 7]).dirty_within_frame());
    }

    #[test]
    fn absurd_frame_dims_do_not_overflow_rgb_len() {
        // Partial frames mean a tiny payload can carry a giant claimed frame size; the RGB
        // length must saturate to None rather than wrap to a small allocation.
        let h = header([0, u32::MAX, u32::MAX, 0, 0, 1, 1, 0]);
        assert_eq!(h.checked_rgb_len(), None);
    }

    #[test]
    fn reader_reassembles_split_messages() {
        // The whole point of length prefixing: a socket read can split anywhere.
        let msg = frame_message(T_EVENT, b"{\"t\":\"ready\"}");
        let mut r = MessageReader::new();
        r.feed(&msg[..3]);
        assert!(r.next_message().is_none());
        r.feed(&msg[3..8]);
        assert!(r.next_message().is_none());
        r.feed(&msg[8..]);
        let m = r.next_message().unwrap();
        assert_eq!(m.type_id, T_EVENT);
        assert_eq!(m.payload, b"{\"t\":\"ready\"}");
    }

    #[test]
    fn reader_handles_multiple_messages_in_one_chunk() {
        let mut buf = frame_message(T_EVENT, b"a");
        buf.extend(frame_message(T_EVENT, b"bb"));
        buf.extend(frame_message(T_FRAME, b"ccc"));
        let mut r = MessageReader::new();
        r.feed(&buf);
        assert_eq!(r.next_message().unwrap().payload, b"a");
        assert_eq!(r.next_message().unwrap().payload, b"bb");
        assert_eq!(r.next_message().unwrap().payload, b"ccc");
        assert!(r.next_message().is_none());
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn binary_payload_with_newlines_survives() {
        // A delimiter-based protocol would corrupt this; length prefixing must not.
        let payload: Vec<u8> = vec![0, 10, 13, 0x1b, 255, b'\n', b'\r'];
        let msg = frame_message(T_FRAME, &payload);
        let mut r = MessageReader::new();
        r.feed(&msg);
        assert_eq!(r.next_message().unwrap().payload, payload);
    }

    #[test]
    fn oversized_length_prefix_is_rejected_not_reserved() {
        // A 4 GiB claim must not become a 4 GiB allocation.
        let mut r = MessageReader::new();
        let mut hdr = vec![T_FRAME];
        hdr.extend_from_slice(&u32::MAX.to_be_bytes());
        r.feed(&hdr);
        assert!(matches!(
            r.try_next_message(),
            Err(ProtocolError::MessageTooLarge(n)) if n == u32::MAX as usize
        ));
    }

    #[test]
    fn a_4k_frame_is_still_under_the_cap() {
        // The cap must not reject legitimate large frames.
        assert!(3840 * 2160 * 4 + FRAME_HEADER_LEN < MAX_MESSAGE_LEN);
    }

    #[test]
    fn json_escape_handles_specials() {
        let mut s = String::new();
        json_escape("a\"b\\c\nd\te", &mut s);
        assert_eq!(s, "a\\\"b\\\\c\\nd\\te");
    }

    #[test]
    fn json_escape_encodes_control_bytes() {
        let mut s = String::new();
        json_escape("x\u{0001}y", &mut s);
        assert_eq!(s, "x\\u0001y");
    }

    #[test]
    fn json_get_str_reads_fields() {
        let j = r#"{"t":"title","v":"Hello World"}"#;
        assert_eq!(json_get_str(j, "t").as_deref(), Some("title"));
        assert_eq!(json_get_str(j, "v").as_deref(), Some("Hello World"));
        assert_eq!(json_get_str(j, "missing"), None);
    }

    #[test]
    fn json_get_str_handles_escapes() {
        let j = r#"{"v":"a\"b\\c\nd"}"#;
        assert_eq!(json_get_str(j, "v").as_deref(), Some("a\"b\\c\nd"));
    }

    #[test]
    fn json_get_str_handles_unicode_escape() {
        let j = r#"{"v":"été"}"#;
        assert_eq!(json_get_str(j, "v").as_deref(), Some("été"));
    }

    #[test]
    fn json_get_bool_reads_values() {
        assert_eq!(json_get_bool(r#"{"v":true}"#, "v"), Some(true));
        assert_eq!(json_get_bool(r#"{"v":false}"#, "v"), Some(false));
        assert_eq!(json_get_bool(r#"{"v":"x"}"#, "v"), None);
    }

    #[test]
    fn roundtrip_escape_then_parse() {
        // A page title containing quotes and newlines must survive the round trip.
        let title = "He said \"hi\"\nthen left\\";
        let mut body = String::from("{\"v\":\"");
        json_escape(title, &mut body);
        body.push_str("\"}");
        assert_eq!(json_get_str(&body, "v").as_deref(), Some(title));
    }
}
