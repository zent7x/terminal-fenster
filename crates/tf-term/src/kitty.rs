//! Kitty graphics protocol encoder.
//!
//! Wire format is an APC escape:  `ESC _ G <control-data> ; <base64 payload> ESC \`
//!
//! Verified against Ghostty 1.3.1 on macOS 26.1: a query
//! `ESC _ G i=31,s=1,v=1,a=q,t=d,f=24; <b64 of 3 bytes> ESC \` returns
//! `ESC _ G i=31;OK ESC \`, confirming the terminal parses and accepts this grammar.
//!
//! # Why raw RGB + zlib rather than PNG
//!
//! Format `f=100` hands Kitty a PNG and lets it decode. That sounds cheaper but moves a full
//! PNG *encode* onto our critical path every frame. Chromium gives us BGRA; converting to
//! RGB and deflating is markedly cheaper than a PNG encode, and zlib is exactly what PNG
//! would use internally anyway. So we send `f=24,o=z`.
//!
//! Base64 inflates payloads by 4/3, which is why compression is not optional: an
//! uncompressed 2482x851 RGB frame is 6.3 MB, or 8.4 MB base64 -- unshippable at 60 fps.

use crate::b64;
use crate::Rect;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::ffi::CString;
use std::io::Write;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Kitty spec: at most 4096 bytes of base64 payload per escape sequence.
pub const MAX_CHUNK: usize = 4096;

/// Image id reserved for the monolithic page base. Dense repaints replace this one image;
/// sparse damage is layered over it with the tile ids below.
pub const PAGE_IMAGE_ID: u32 = 2000;

/// Base of the id block owned by the page mosaic (C08). Every tile at grid `(col, row)` gets
/// a *permanent* id for the lifetime of the layout — the kitty spec deletes an image and all
/// its placements when an id is re-transmitted, so an id bound to a moving damage rect would
/// erase whatever it last covered. This namespace deliberately does not overlap
/// [`PAGE_IMAGE_ID`], allowing sparse tiles to sit above a monolithic base image.
pub const PAGE_TILE_ID_BASE: u32 = 1000;
/// Upper bound of the mosaic id block; chrome/overlays namespace above this.
pub const PAGE_TILE_ID_MAX: u32 = 1999;
const _: () = assert!(PAGE_IMAGE_ID > PAGE_TILE_ID_MAX);
/// One placement id for every tile. Scoped per image, so a single value suffices, and it
/// makes each placement individually deletable via `a=d,d=i,i=<id>,p=1`.
pub const TILE_PLACEMENT_ID: u32 = 1;

/// Bound outstanding shared-memory objects if a terminal says it supports `t=s` but stops
/// consuming commands. Normal terminals unlink each object after reading it, so the queue is
/// normally empty again by the next frame; this is a defensive ceiling, not a pacing target.
pub const MAX_PENDING_SHM: usize = 8;

static SHM_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Permanent image id for the tile at grid `(col, row)`. Row-major within the id block.
#[inline]
pub fn tile_image_id(col: u32, row: u32, grid_cols: u32) -> u32 {
    PAGE_TILE_ID_BASE + row * grid_cols + col
}

/// Convert a BGRA buffer (Chromium's native offscreen format) to packed RGB.
///
/// Chromium's `NativeImage::toBitmap()` was measured to return BGRA, 4 bytes/pixel,
/// non-strided: a 1440x900 frame is exactly 5,184,000 bytes. A known pure-red page yields
/// `[0,0,255,255]`, which is what pins the channel order.
///
/// Alpha is dropped: the page is composited opaque, so transmitting it would cost 33% more
/// bytes for no visual difference.
pub fn bgra_to_rgb(bgra: &[u8], out: &mut Vec<u8>) {
    out.clear();
    let rgb_len = bgra.len() / 4 * 3;
    out.reserve(rgb_len);
    // Every output byte is initialized by swizzle_bgra_to_rgb before the new length becomes
    // observable. Avoiding three Vec::push capacity branches per pixel was measured as the
    // dominant conversion win in B05; this pointer loop is portable and vectorises well.
    swizzle_bgra_to_uninit(
        &bgra[..bgra.len() / 4 * 4],
        &mut out.spare_capacity_mut()[..rgb_len],
    );
    // SAFETY: swizzle_bgra_to_uninit initialized exactly `rgb_len` spare elements.
    unsafe {
        out.set_len(rgb_len);
    }
}

fn swizzle_bgra_to_uninit(src: &[u8], dst: &mut [MaybeUninit<u8>]) {
    assert_eq!(src.len() % 4, 0);
    assert_eq!(dst.len(), src.len() / 4 * 3);
    let mut s = src.as_ptr();
    let mut d = dst.as_mut_ptr();
    // SAFETY: the length relation above proves every read/write lies within its slice.
    unsafe {
        for _ in 0..src.len() / 4 {
            (*d).write(*s.add(2));
            (*d.add(1)).write(*s.add(1));
            (*d.add(2)).write(*s);
            s = s.add(4);
            d = d.add(3);
        }
    }
}

/// Convert equal-pixel-count BGRA and RGB slices. Length assertions establish all pointer
/// bounds up front, letting the hot loop avoid per-channel bounds/capacity checks.
fn swizzle_bgra_to_rgb(src: &[u8], dst: &mut [u8]) {
    assert_eq!(src.len() % 4, 0);
    assert_eq!(dst.len(), src.len() / 4 * 3);
    let pixels = src.len() / 4;
    let mut s = src.as_ptr();
    let mut d = dst.as_mut_ptr();
    // SAFETY: `s` advances exactly `pixels*4 == src.len()` bytes and `d` advances exactly
    // `pixels*3 == dst.len()` bytes. The slices are disjoint at every call site.
    unsafe {
        for _ in 0..pixels {
            *d = *s.add(2);
            *d.add(1) = *s.add(1);
            *d.add(2) = *s;
            s = s.add(4);
            d = d.add(3);
        }
    }
}

/// Copy a sub-rectangle out of a packed-RGB framebuffer into a contiguous buffer.
///
/// Used by the Kitty tile mosaic: each dirty tile is encoded as its own image, and the
/// source of truth is the persistent RGB canvas the damage path composites into.
/// `img_w` is the **source stride in pixels** (full page width), not the tile width.
pub fn copy_rgb_rect(rgb: &[u8], img_w: u32, rect: Rect, out: &mut Vec<u8>) {
    out.clear();
    out.reserve((rect.area() * 3) as usize);
    let stride = img_w as usize * 3;
    let tw = rect.w as usize * 3;
    for row in 0..rect.h as usize {
        let start = (rect.y as usize + row) * stride + rect.x as usize * 3;
        out.extend_from_slice(&rgb[start..start + tw]);
    }
}

/// Extract a sub-rectangle of a BGRA image as packed RGB.
///
/// Used for damage updates: transmitting only the changed region is the single biggest
/// bandwidth win available, especially over SSH.
pub fn bgra_rect_to_rgb(bgra: &[u8], img_w: u32, rect: Rect, out: &mut Vec<u8>) {
    out.clear();
    let rgb_len = (rect.w as usize)
        .checked_mul(rect.h as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .expect("RGB rectangle length overflow");
    out.reserve(rgb_len);
    let stride = img_w as usize * 4;
    let dst_stride = rect.w as usize * 3;
    {
        let spare = &mut out.spare_capacity_mut()[..rgb_len];
        for row in 0..rect.h as usize {
            let y = rect.y as usize + row;
            let start = y * stride + rect.x as usize * 4;
            let line = &bgra[start..start + rect.w as usize * 4];
            swizzle_bgra_to_uninit(line, &mut spare[row * dst_stride..(row + 1) * dst_stride]);
        }
    }
    // SAFETY: every row of the `rgb_len` spare region was initialized above.
    unsafe {
        out.set_len(rgb_len);
    }
}

/// Composite a dirty-rectangle BGRA update into a persistent packed-RGB framebuffer.
///
/// This is the *consume* side of damage tracking (proven possible by the B02 spike). The
/// engine now sends only the changed region — `src` is `w*h*4` BGRA bytes — and the terminal
/// core keeps the whole page as RGB in `dst` (`frame_w * frame_h * 3`), rewriting only the
/// pixels that changed. An idle page therefore costs nothing and a caret costs ~one glyph,
/// where before every frame rewrote the entire viewport.
///
/// The rectangle must lie within the frame and `src`/`dst` must be correctly sized; callers
/// validate that with `FrameHeader::dirty_within_frame` and the dirty/RGB length helpers in
/// `tf-proto` before calling. Given those invariants this indexes only in bounds.
pub fn blit_bgra_into_rgb(
    src: &[u8],
    dst: &mut [u8],
    frame_w: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) {
    let fw = frame_w as usize;
    let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
    let src_stride = w * 4;
    let dst_stride = fw * 3;
    for row in 0..h {
        let s0 = row * src_stride;
        let d0 = (y + row) * dst_stride + x * 3;
        let src_row = &src[s0..s0 + src_stride];
        let dst_row = &mut dst[d0..d0 + w * 3];
        swizzle_bgra_to_rgb(src_row, dst_row);
    }
}

fn deflate(data: &[u8], level: u32) -> std::io::Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 4), Compression::new(level));
    enc.write_all(data)?;
    enc.finish()
}

/// One POSIX shared-memory object awaiting consumption by the terminal.
///
/// Kitty requires the terminal to unlink the name after reading it. We retain the name so a
/// normal process exit can still remove it if the terminal disappears between capability
/// detection and a frame command. The file descriptor itself is closed immediately after the
/// bytes are written; keeping it open would retain megabytes after the terminal unlinks it.
pub struct PendingShm {
    name: CString,
}

impl std::fmt::Debug for PendingShm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingShm")
            .field("name", &self.name.to_string_lossy())
            .finish()
    }
}

impl PendingShm {
    fn create(data: &[u8]) -> std::io::Result<Self> {
        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shared-memory image payload cannot be empty",
            ));
        }
        let len: libc::off_t = data.len().try_into().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shared-memory image payload is too large",
            )
        })?;

        // PID + nanoseconds + a process-local counter makes collisions negligible, while
        // O_EXCL still turns any collision into a retry instead of opening somebody else's
        // object. POSIX shm names contain exactly one leading slash.
        for _ in 0..8 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u32;
            let seq = SHM_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
            // Darwin limits POSIX shm names to 31 bytes including the leading slash. Keep
            // this at <=30 even for an eight-hex-digit PID.
            let name = CString::new(format!(
                "/bg-{:x}-{stamp:08x}-{seq:08x}",
                std::process::id()
            ))
            .expect("generated shm name has no NUL");
            let fd = unsafe {
                libc::shm_open(
                    name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                    0o600,
                )
            };
            if fd < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    continue;
                }
                return Err(err);
            }

            let write_result = (|| {
                if unsafe { libc::ftruncate(fd, len) } != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Darwin's POSIX shm descriptors cannot be written with write(2) (ENXIO);
                // shared memory is accessed through mmap on both Darwin and Linux.
                let mapped = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        data.len(),
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                };
                if mapped == libc::MAP_FAILED {
                    return Err(std::io::Error::last_os_error());
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), mapped.cast::<u8>(), data.len());
                }
                if unsafe { libc::munmap(mapped, data.len()) } != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })();
            unsafe {
                libc::close(fd);
            }
            if let Err(err) = write_result {
                unsafe {
                    libc::shm_unlink(name.as_ptr());
                }
                return Err(err);
            }
            return Ok(Self { name });
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique shared-memory image name",
        ))
    }

    /// True while the POSIX name still exists. A conforming terminal unlinks it after read.
    pub fn is_linked(&self) -> bool {
        let fd = unsafe { libc::shm_open(self.name.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return false;
        }
        unsafe {
            libc::close(fd);
        }
        true
    }

    fn append_encoded_name(&self, out: &mut Vec<u8>) {
        b64::encode_into(self.name.as_bytes(), out);
    }
}

impl Drop for PendingShm {
    fn drop(&mut self) {
        // ENOENT is the healthy path: the terminal already consumed and unlinked it.
        unsafe {
            libc::shm_unlink(self.name.as_ptr());
        }
    }
}

/// How an image should be placed on screen.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// Image id to (re)use.
    pub image_id: u32,
    /// Columns/rows the image should occupy. `None` lets Kitty derive from pixel size.
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    /// Z-index. Page content sits below chrome overlays.
    pub z: i32,
    /// If true, the cursor does not move after placement (`C=1`), which is what we want:
    /// a moving cursor would scroll the screen and fight our own layout.
    pub no_cursor_move: bool,
    /// Absolute pixel placement (`x`/`y` keys). When set, `cols`/`rows` are omitted.
    pub pixel_x: Option<u32>,
    pub pixel_y: Option<u32>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            image_id: PAGE_IMAGE_ID,
            cols: None,
            rows: None,
            z: 0,
            no_cursor_move: true,
            pixel_x: None,
            pixel_y: None,
        }
    }
}

/// Encode an RGB image as Kitty transmit-and-display escape sequences.
///
/// `rgb` must be exactly `w*h*3` bytes. Output is appended to `out`.
pub fn encode_rgb_frame(
    rgb: &[u8],
    w: u32,
    h: u32,
    place: Placement,
    compress_level: u32,
    out: &mut Vec<u8>,
) -> std::io::Result<EncodeStats> {
    assert_eq!(
        rgb.len(),
        (w as usize) * (h as usize) * 3,
        "rgb buffer size must match w*h*3"
    );

    let raw_len = rgb.len();
    let (payload, compressed) = if compress_level > 0 {
        (deflate(rgb, compress_level)?, true)
    } else {
        (rgb.to_vec(), false)
    };
    let deflated_len = payload.len();

    let mut b64buf = Vec::with_capacity(payload.len() * 4 / 3 + 4);
    b64::encode_into(&payload, &mut b64buf);

    let chunks: Vec<&[u8]> = b64buf.chunks(MAX_CHUNK).collect();
    let chunk_count = chunks.len().max(1);
    let start = out.len();

    for (idx, chunk) in chunks.iter().enumerate() {
        let more = if idx + 1 < chunk_count { 1 } else { 0 };
        out.extend_from_slice(b"\x1b_G");
        if idx == 0 {
            // Control keys only appear on the first chunk; continuations carry just m=.
            // q=1: suppress success ACKs but report errors so a rejected frame cannot
            // silently diverge the screen (C09 §2.5 / C01 D1). Markers still supply ACKs.
            out.extend_from_slice(b"a=T,f=24,t=d,q=1");
            if compressed {
                out.extend_from_slice(b",o=z");
            }
            write_image_meta(out, w, h, place);
            write_kv(out, b",m=", more);
        } else {
            write_kv(out, b"m=", more);
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }

    Ok(EncodeStats {
        raw_bytes: raw_len,
        deflated_bytes: deflated_len,
        wire_bytes: out.len() - start,
        chunks: chunk_count,
    })
}

/// Result of encoding a Kitty command whose pixels live in POSIX shared memory.
#[derive(Debug)]
pub struct ShmFrame {
    pub stats: EncodeStats,
    /// Must remain alive until the terminal unlinks it. Dropping it is the cleanup fallback.
    pub pending: PendingShm,
}

/// Encode an RGB image using Kitty's local shared-memory medium (`t=s`).
///
/// Unlike the direct path this deliberately sends raw RGB, not zlib. With shared memory the
/// terminal command carries only the object name, so compression saves no PTY bandwidth and
/// becomes pure CPU/decode overhead precisely on full-motion, high-entropy pages. The object
/// is mode 0600, the terminal unlinks it after reading, and the returned guard cleans it up if
/// the command is never consumed.
pub fn encode_rgb_frame_shm(
    rgb: &[u8],
    w: u32,
    h: u32,
    place: Placement,
    out: &mut Vec<u8>,
) -> std::io::Result<ShmFrame> {
    assert_eq!(
        rgb.len(),
        (w as usize) * (h as usize) * 3,
        "rgb buffer size must match w*h*3"
    );
    let pending = PendingShm::create(rgb)?;
    let start = out.len();
    out.extend_from_slice(b"\x1b_Ga=T,f=24,t=s,q=1");
    write_kv(out, b",S=", rgb.len() as i64);
    write_image_meta(out, w, h, place);
    out.push(b';');
    pending.append_encoded_name(out);
    out.extend_from_slice(b"\x1b\\");
    Ok(ShmFrame {
        stats: EncodeStats {
            raw_bytes: rgb.len(),
            deflated_bytes: rgb.len(),
            wire_bytes: out.len() - start,
            chunks: 1,
        },
        pending,
    })
}

/// Build a real `a=q,t=s` capability probe. The caller must keep the returned object alive
/// until it has read the terminal's response.
pub fn shared_memory_support_query(id: u32) -> std::io::Result<(Vec<u8>, PendingShm)> {
    let pending = PendingShm::create(&[0, 0, 0])?;
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_Ga=q,f=24,t=s");
    write_kv(&mut out, b",i=", id as i64);
    out.extend_from_slice(b",s=1,v=1,S=3;");
    pending.append_encoded_name(&mut out);
    out.extend_from_slice(b"\x1b\\");
    Ok((out, pending))
}

fn write_image_meta(out: &mut Vec<u8>, w: u32, h: u32, place: Placement) {
    write_kv(out, b",s=", w as i64);
    write_kv(out, b",v=", h as i64);
    write_kv(out, b",i=", place.image_id as i64);
    if let Some(c) = place.cols {
        write_kv(out, b",c=", c as i64);
    }
    if let Some(r) = place.rows {
        write_kv(out, b",r=", r as i64);
    }
    if place.z != 0 {
        write_kv(out, b",z=", place.z as i64);
    }
    if let Some(x) = place.pixel_x {
        write_kv(out, b",x=", x as i64);
    }
    if let Some(y) = place.pixel_y {
        write_kv(out, b",y=", y as i64);
    }
    if place.no_cursor_move {
        out.extend_from_slice(b",C=1");
    }
}

fn write_kv(out: &mut Vec<u8>, key: &[u8], val: i64) {
    out.extend_from_slice(key);
    let mut buf = itoa(val);
    out.append(&mut buf);
}

fn itoa(mut v: i64) -> Vec<u8> {
    if v == 0 {
        return vec![b'0'];
    }
    let neg = v < 0;
    if neg {
        v = -v;
    }
    let mut d = Vec::with_capacity(12);
    while v > 0 {
        d.push(b'0' + (v % 10) as u8);
        v /= 10;
    }
    if neg {
        d.push(b'-');
    }
    d.reverse();
    d
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeStats {
    pub raw_bytes: usize,
    pub deflated_bytes: usize,
    /// Total bytes actually written to the terminal, including escapes and base64 overhead.
    pub wire_bytes: usize,
    pub chunks: usize,
}

impl EncodeStats {
    pub fn compression_ratio(&self) -> f64 {
        if self.deflated_bytes == 0 {
            return 0.0;
        }
        self.raw_bytes as f64 / self.deflated_bytes as f64
    }
}

/// Re-place a previously transmitted image (`a=p`). `p=1` replaces the prior placement for
/// this id so cursor moves do not stack unbounded overlays.
pub fn place_image(w: u32, h: u32, place: Placement, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_Ga=p,p=1");
    write_image_meta(out, w, h, place);
    out.extend_from_slice(b"\x1b\\");
}

/// Delete a specific image and free its data.
pub fn delete_image(id: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_Ga=d,d=I,i=");
    let mut b = itoa(id as i64);
    out.append(&mut b);
    out.extend_from_slice(b"\x1b\\");
}

/// Delete every image. Used on teardown so nothing lingers over the user's shell.
pub fn delete_all(out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_Ga=d,d=A\x1b\\");
}

/// Image id used for Kitty graphics capability probing (`caps::detect`).
pub const GRAPHICS_PROBE_ID: u32 = 31;

/// User-visible summary of the graphics probe (matches terminal-browser style).
pub fn graphics_probe_line(id: u32) -> String {
    format!("Gi={id},a=q,t=d,f=24,s=1,v=1;AAAA")
}

/// Build the support-detection query. A conforming terminal answers
/// `ESC _ G i=<id>;OK ESC \`.
pub fn support_query(id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_Gi=");
    let mut b = itoa(id as i64);
    out.append(&mut b);
    out.extend_from_slice(b",s=1,v=1,a=q,t=d,f=24;");
    b64::encode_into(&[0, 0, 0], &mut out);
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Marker for marker-bracketed drain timing (C09 §2.2). Same wire shape as
/// [`support_query`]; named for the frame hot path so capability probing is not implied.
pub fn marker(id: u32) -> Vec<u8> {
    support_query(id)
}

/// High bit range reserved for MBDT markers: `0x7000_0000 | (epoch<<16) | seq`.
pub const MARKER_ID_BASE: u32 = 0x7000_0000;

/// Encode a 12-bit reconnect epoch and 16-bit sequence into a marker image id.
pub fn marker_id(epoch: u16, seq: u16) -> u32 {
    MARKER_ID_BASE | ((u32::from(epoch) & 0x0FFF) << 16) | u32::from(seq)
}

/// Split a marker id back into `(epoch, seq)`, or `None` if outside the marker range.
pub fn parse_marker_id(id: u32) -> Option<(u16, u16)> {
    if id & 0xF000_0000 != MARKER_ID_BASE {
        return None;
    }
    let epoch = ((id >> 16) & 0x0FFF) as u16;
    let seq = (id & 0xFFFF) as u16;
    Some((epoch, seq))
}

/// Resync after a truncated transfer or reconnect (C09 §7.4 steps 1–3).
pub fn resync_preamble(out: &mut Vec<u8>) {
    // Terminate any partial APC/DCS the far parser may still be consuming.
    out.extend_from_slice(b"\x1b\\");
    // Close a dangling chunked transfer left mid-stream.
    out.extend_from_slice(b"\x1b_Gq=1,m=0;\x1b\\");
    delete_all(out);
}

/// Wrap a byte sequence for tmux passthrough.
///
/// tmux requires `ESC P tmux; <payload with every ESC doubled> ESC \`, and the user must
/// have `allow-passthrough on` set. Without the doubling tmux terminates the DCS early and
/// the terminal sees a truncated graphics command.
pub fn wrap_tmux(payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in payload {
        if b == 0x1b {
            out.push(0x1b);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgb_swaps_channels_and_drops_alpha() {
        // One pure-red BGRA pixel as Chromium delivers it.
        let bgra = [0u8, 0, 255, 255];
        let mut rgb = Vec::new();
        bgra_to_rgb(&bgra, &mut rgb);
        assert_eq!(rgb, vec![255, 0, 0]);
    }

    #[test]
    fn bgra_to_rgb_length_is_three_quarters() {
        let bgra = vec![7u8; 1440 * 900 * 4];
        let mut rgb = Vec::new();
        bgra_to_rgb(&bgra, &mut rgb);
        assert_eq!(rgb.len(), 1440 * 900 * 3);
    }

    #[test]
    fn optimized_swizzle_matches_scalar_reference_across_lengths() {
        for pixels in 0..257usize {
            for trailing in 0..4usize {
                let bgra: Vec<u8> = (0..pixels * 4 + trailing)
                    .map(|i| ((i * 73 + 19) % 251) as u8)
                    .collect();
                let expected: Vec<u8> = bgra
                    .chunks_exact(4)
                    .flat_map(|px| [px[2], px[1], px[0]])
                    .collect();
                let mut actual = Vec::new();
                bgra_to_rgb(&bgra, &mut actual);
                assert_eq!(actual, expected, "pixels={pixels} trailing={trailing}");
            }
        }
    }

    #[test]
    fn tile_image_id_is_stable_and_position_bound() {
        assert_eq!(tile_image_id(0, 0, 10), PAGE_TILE_ID_BASE);
        assert_eq!(tile_image_id(3, 2, 10), PAGE_TILE_ID_BASE + 23);
        assert!(tile_image_id(9, 21, 10) <= PAGE_TILE_ID_MAX);
    }

    #[test]
    fn copy_rgb_rect_uses_full_frame_stride() {
        // 4x2 RGB; each pixel is (x, y, 0) so we can see if we walked the wrong stride.
        let mut rgb = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                rgb.extend_from_slice(&[x, y, 0]);
            }
        }
        let mut out = Vec::new();
        copy_rgb_rect(&rgb, 4, Rect::new(1, 0, 2, 2), &mut out);
        assert_eq!(out, vec![1, 0, 0, 2, 0, 0, 1, 1, 0, 2, 1, 0]);
    }

    #[test]
    fn rect_extraction_picks_correct_pixels() {
        // 3x2 image; each pixel's blue channel encodes its index so we can assert placement.
        let mut bgra = Vec::new();
        for i in 0..6u8 {
            bgra.extend_from_slice(&[i, 0, 0, 255]); // B=i
        }
        let mut rgb = Vec::new();
        // Take the right-hand 2x1 of the top row: indices 1 and 2.
        bgra_rect_to_rgb(&bgra, 3, Rect::new(1, 0, 2, 1), &mut rgb);
        // B channel lands in RGB position 2.
        assert_eq!(rgb, vec![0, 0, 1, 0, 0, 2]);
    }

    #[test]
    fn blit_writes_only_the_dirty_rect_and_swaps_channels() {
        // 4x4 RGB framebuffer, pre-filled mid-grey so we can see exactly what changes.
        let mut fb = vec![128u8; 4 * 4 * 3];
        // A 2x2 BGRA patch of pure red, to land at (1,1).
        let mut patch = Vec::new();
        for _ in 0..4 {
            patch.extend_from_slice(&[0u8, 0, 255, 255]); // B,G,R,A -> red
        }
        blit_bgra_into_rgb(&patch, &mut fb, 4, 1, 1, 2, 2);

        let px = |x: usize, y: usize| {
            let i = (y * 4 + x) * 3;
            (fb[i], fb[i + 1], fb[i + 2])
        };
        // Inside the rect: red.
        assert_eq!(px(1, 1), (255, 0, 0));
        assert_eq!(px(2, 2), (255, 0, 0));
        // Outside the rect: untouched grey. Corners and the row/col just outside the patch.
        assert_eq!(px(0, 0), (128, 128, 128));
        assert_eq!(px(3, 3), (128, 128, 128));
        assert_eq!(px(0, 1), (128, 128, 128));
        assert_eq!(px(3, 1), (128, 128, 128));
    }

    #[test]
    fn blit_full_frame_equals_bgra_to_rgb() {
        // The full-frame case (dirty == whole frame) must produce exactly what the
        // whole-buffer converter would, so switching every frame to the blit path is a
        // no-op for full repaints.
        let mut bgra = Vec::new();
        for i in 0..(3 * 2) as u8 {
            bgra.extend_from_slice(&[i, i.wrapping_add(10), i.wrapping_add(20), 255]);
        }
        let mut expected = Vec::new();
        bgra_to_rgb(&bgra, &mut expected);

        let mut fb = vec![0u8; 3 * 2 * 3];
        blit_bgra_into_rgb(&bgra, &mut fb, 3, 0, 0, 3, 2);
        assert_eq!(fb, expected);
    }

    #[test]
    fn encoded_frame_has_valid_apc_envelope() {
        let rgb = vec![0u8; 4 * 4 * 3];
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 4, 4, Placement::default(), 6, &mut out).unwrap();
        assert!(
            out.starts_with(b"\x1b_G"),
            "must open with the APC introducer"
        );
        assert!(out.ends_with(b"\x1b\\"), "must close with ST");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a=T"), "transmit-and-display");
        assert!(s.contains("f=24"), "raw RGB format");
        assert!(s.contains("o=z"), "zlib compressed");
        assert!(s.contains("s=4"), "width");
        assert!(s.contains("v=4"), "height");
        assert!(s.contains("C=1"), "cursor must not move");
        assert_eq!(stats.raw_bytes, 48);
    }

    #[test]
    fn place_image_is_a_put_without_payload() {
        let mut out = Vec::new();
        place_image(
            14,
            20,
            Placement {
                image_id: 2100,
                pixel_x: Some(10),
                pixel_y: Some(20),
                z: 100,
                no_cursor_move: true,
                ..Default::default()
            },
            &mut out,
        );
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a=p"), "placement action");
        assert!(s.contains("p=1"), "replace prior placement");
        assert!(s.contains("i=2100"));
        assert!(s.contains("x=10"));
        assert!(s.contains("y=20"));
        assert!(!s.contains(';'), "put must not carry pixel payload");
    }

    #[test]
    fn shared_memory_frame_contains_raw_pixels_and_a_short_wire_command() {
        let rgb: Vec<u8> = (0..12 * 9 * 3).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        let frame = encode_rgb_frame_shm(&rgb, 12, 9, Placement::default(), &mut out).unwrap();
        assert!(frame.pending.is_linked());
        assert_eq!(frame.stats.raw_bytes, rgb.len());
        assert_eq!(frame.stats.deflated_bytes, rgb.len());
        assert_eq!(frame.stats.chunks, 1);
        assert!(out.len() < rgb.len(), "wire carries a name, not the pixels");

        let encoded = String::from_utf8_lossy(&out);
        assert!(encoded.starts_with("\x1b_Ga=T,f=24,t=s,q=1,S=324"));
        assert!(encoded.contains(",s=12,v=9,i=2000,C=1;"));
        assert!(!encoded.contains("o=z"), "shared memory stays raw");

        let fd = unsafe { libc::shm_open(frame.pending.name.as_ptr(), libc::O_RDONLY, 0) };
        assert!(fd >= 0);
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                rgb.len(),
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(mapped, libc::MAP_FAILED);
        let actual = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), rgb.len()) };
        assert_eq!(actual, rgb);
        unsafe {
            libc::munmap(mapped, rgb.len());
            libc::close(fd);
        }
    }

    #[test]
    fn shared_memory_guard_unlinks_an_unconsumed_object() {
        let mut out = Vec::new();
        let frame = encode_rgb_frame_shm(&[1, 2, 3], 1, 1, Placement::default(), &mut out).unwrap();
        let name = frame.pending.name.clone();
        drop(frame);
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        assert_eq!(fd, -1, "drop must remove an object the terminal never read");
    }

    #[test]
    fn shared_memory_probe_uses_query_action_and_real_object() {
        let (query, pending) = shared_memory_support_query(32).unwrap();
        let encoded = String::from_utf8_lossy(&query);
        assert!(encoded.starts_with("\x1b_Ga=q,f=24,t=s,i=32,s=1,v=1,S=3;"));
        assert!(encoded.ends_with("\x1b\\"));
        assert!(pending.is_linked());
    }

    #[test]
    fn shared_memory_wire_size_is_constant_for_a_measured_full_viewport() {
        let rgb = vec![0x7fu8; 2108 * 1406 * 3];
        let mut out = Vec::new();
        let frame = encode_rgb_frame_shm(&rgb, 2108, 1406, Placement::default(), &mut out).unwrap();
        assert_eq!(frame.stats.raw_bytes, 8_891_544);
        assert!(
            frame.stats.wire_bytes < 160,
            "the PTY should carry metadata and a name, got {} bytes",
            frame.stats.wire_bytes
        );
    }

    #[test]
    fn chunking_respects_the_4096_byte_limit() {
        // Random-ish data so zlib cannot collapse it to nothing and we really get chunks.
        let mut rgb = Vec::with_capacity(200 * 200 * 3);
        let mut x: u32 = 12345;
        for _ in 0..200 * 200 * 3 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            rgb.push((x >> 16) as u8);
        }
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 200, 200, Placement::default(), 6, &mut out).unwrap();
        assert!(
            stats.chunks > 1,
            "incompressible data should need multiple chunks"
        );

        // Every payload segment must be <= MAX_CHUNK bytes.
        let text = out.clone();
        let mut idx = 0;
        let mut seen = 0;
        while let Some(p) = find(&text[idx..], b"\x1b_G") {
            let start = idx + p;
            let semi = find(&text[start..], b";").map(|v| start + v).unwrap();
            let end = find(&text[semi..], b"\x1b\\").map(|v| semi + v).unwrap();
            let payload_len = end - semi - 1;
            assert!(
                payload_len <= MAX_CHUNK,
                "chunk of {payload_len} exceeds {MAX_CHUNK}"
            );
            seen += 1;
            idx = end + 2;
        }
        assert_eq!(
            seen, stats.chunks,
            "chunk count must match what we reported"
        );
    }

    #[test]
    fn only_the_last_chunk_is_marked_final() {
        let mut rgb = Vec::with_capacity(200 * 200 * 3);
        let mut x: u32 = 999;
        for _ in 0..200 * 200 * 3 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            rgb.push((x >> 16) as u8);
        }
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 200, 200, Placement::default(), 6, &mut out).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert_eq!(s.matches("m=0").count(), 1, "exactly one terminal chunk");
        assert_eq!(s.matches("m=1").count(), stats.chunks - 1);
    }

    #[test]
    fn compression_actually_helps_on_realistic_content() {
        // A flat-colour region stands in for the large uniform areas real pages have.
        let rgb = vec![0xEEu8; 800 * 600 * 3];
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 800, 600, Placement::default(), 6, &mut out).unwrap();
        assert!(
            stats.compression_ratio() > 50.0,
            "flat content should compress hugely, got {:.1}x",
            stats.compression_ratio()
        );
    }

    #[test]
    fn graphics_probe_line_matches_support_query() {
        assert_eq!(
            graphics_probe_line(GRAPHICS_PROBE_ID),
            "Gi=31,a=q,t=d,f=24,s=1,v=1;AAAA"
        );
        let q = support_query(GRAPHICS_PROBE_ID);
        let s = String::from_utf8_lossy(&q);
        assert!(s.contains("Gi=31"));
        assert!(s.contains(";AAAA"));
    }

    #[test]
    fn support_query_matches_the_sequence_ghostty_answered() {
        let q = support_query(31);
        let s = String::from_utf8_lossy(&q);
        // This exact shape was confirmed to return `ESC _ G i=31;OK ESC \` from Ghostty 1.3.1.
        assert!(s.starts_with("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;"));
        assert!(s.ends_with("\x1b\\"));
    }

    #[test]
    fn frames_use_q1_so_errors_are_reported() {
        let rgb = vec![0u8; 2 * 2 * 3];
        let mut out = Vec::new();
        encode_rgb_frame(&rgb, 2, 2, Placement::default(), 1, &mut out).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("q=1"), "error channel must stay open");
        assert!(!s.contains("q=2"), "q=2 would silence rejected frames");
        assert_eq!(marker(42), support_query(42));
    }

    #[test]
    fn marker_ids_encode_epoch_and_seq() {
        let id = marker_id(0x0ABC, 0x1234);
        assert_eq!(id, 0x7000_0000 | (0x0ABC << 16) | 0x1234);
        assert_eq!(id, 0x7ABC_1234);
        assert_eq!(parse_marker_id(id), Some((0x0ABC, 0x1234)));
        assert_eq!(parse_marker_id(PAGE_IMAGE_ID), None);
    }

    #[test]
    fn resync_preamble_clears_partial_transfer_then_images() {
        let mut out = Vec::new();
        resync_preamble(&mut out);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b\\"));
        assert!(s.contains("\x1b_Gq=1,m=0;\x1b\\"));
        assert!(s.ends_with("\x1b_Ga=d,d=A\x1b\\"));
    }

    #[test]
    fn tmux_wrapper_doubles_escapes() {
        let mut out = Vec::new();
        wrap_tmux(b"\x1b_Gx\x1b\\", &mut out);
        assert!(out.starts_with(b"\x1bPtmux;"));
        // The inner ESCs must appear doubled or tmux truncates the DCS.
        assert!(find(&out[7..], b"\x1b\x1b_Gx").is_some());
    }

    #[test]
    fn delete_all_emits_the_teardown_sequence() {
        let mut out = Vec::new();
        delete_all(&mut out);
        assert_eq!(out, b"\x1b_Ga=d,d=A\x1b\\");
    }

    #[test]
    fn itoa_handles_edges() {
        assert_eq!(itoa(0), b"0".to_vec());
        assert_eq!(itoa(1000), b"1000".to_vec());
        assert_eq!(itoa(-42), b"-42".to_vec());
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }
}
