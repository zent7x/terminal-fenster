//! Marker-bracketed drain timing (MBDT) and byte-credit pacing (C09).
//!
//! On direct/zlib Kitty paths the PTY is the bottleneck. Frame-count pacing (`max_in_flight
//! = 1`) collapses throughput on fast, high-latency links; pacing on outstanding *bytes*
//! recovers that headroom while still degenerating to one frame on slow links.

use crate::kitty;
use std::collections::VecDeque;
use std::time::Instant;

/// Default interactivity budget: worst-case queued pixel staleness before send (C09 §4.3).
pub const DEFAULT_LAG_MS: u64 = 100;

/// Frames below this wire size use a trailing marker for liveness only (C09 §2.4).
pub const MBDT_SAMPLE_MIN: usize = 8192;

/// Conservative cold-start drain estimate (~100 Mbit/s) until MBDT samples arrive.
const INITIAL_DRAIN_RATE: f64 = 12_500_000.0;

/// Wikipedia-class on-wire size used as the ladder design point (C09 §3.2 / §5.4).
pub const LADDER_REF_WIRE: usize = 637_141;

const DRAIN_ALPHA: f64 = 0.20;
const RTT_ALPHA: f64 = 0.25;
const MIN_DRAIN_NS: u64 = 1_000_000; // 1 ms — clock granularity floor
const MAX_DRAIN_MULT: f64 = 20.0;
const BUDGET_MARGIN: f64 = 0.70;
const SCALES: &[f64] = &[1.0, 0.75, 0.50, 0.33];
const FPS_STEPS: &[u32] = &[120, 100, 90, 60, 30, 15, 8, 4, 2, 1];
const UP_MIN_FRAMES: u64 = 20;
const UP_MIN_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentGate {
    /// Safe to write `wire_bytes` to the terminal now.
    Send,
    /// Credit exhausted; keep dirty state and retry on the next loop turn.
    Deferred,
}

#[derive(Debug, Clone)]
pub struct TransportStats {
    pub drain_rate_ewma: f64,
    pub rtt_ewma_ms: f64,
    pub bytes_outstanding: usize,
    pub credit_bytes: usize,
    pub deferred: u64,
    pub samples: u64,
    pub rung: LadderRung,
}

/// Animation capability selected from drain rate + RTT (C09 §5).
///
/// Scale is held at 1.0 while fps can absorb the budget; only then does scale drop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderRung {
    pub scale: f64,
    pub fps: u32,
    /// True when continuous animation should be suppressed (satellite / sub-1 Mbit).
    pub static_only: bool,
}

impl Default for LadderRung {
    fn default() -> Self {
        Self {
            scale: 1.0,
            fps: 60,
            static_only: false,
        }
    }
}

impl LadderRung {
    fn rank(self) -> i32 {
        let scale_i = (self.scale * 100.0).round() as i32;
        scale_i * 100 + self.fps as i32
    }
}

/// Pick the highest fps at the largest scale that fits `drain_bps` (C09 §5.4).
pub fn select_rung(drain_bps: f64, rtt_ms: f64, ref_wire: usize, fps_cap: u32) -> LadderRung {
    let drain_bps = drain_bps.max(1.0);
    let ref_wire = ref_wire.max(1) as f64;
    let fps_cap = fps_cap.clamp(1, 240);

    if rtt_ms > 250.0 || drain_bps < 125_000.0 {
        return LadderRung {
            scale: 0.33,
            fps: 1,
            static_only: true,
        };
    }

    let max_fps = if rtt_ms > 100.0 {
        fps_cap.min(60)
    } else {
        fps_cap
    };

    for &scale in SCALES {
        let wire = ref_wire * scale * scale;
        for &fps in FPS_STEPS {
            if fps > max_fps {
                continue;
            }
            let budget = drain_bps / f64::from(fps) * BUDGET_MARGIN;
            if wire <= budget {
                return LadderRung {
                    scale,
                    fps,
                    static_only: false,
                };
            }
        }
    }

    LadderRung {
        scale: 0.33,
        fps: 1,
        static_only: true,
    }
}

#[derive(Debug)]
struct PendingFlight {
    marker_a: u32,
    marker_b: u32,
    wire_bytes: usize,
    sent_at: Instant,
    anchor_at: Option<Instant>,
}

/// Byte-credit transport controller for direct Kitty presentation.
#[derive(Debug)]
pub struct AdaptiveTransport {
    epoch: u16,
    seq: u16,
    bytes_outstanding: usize,
    drain_rate_ewma: f64,
    rtt_ewma_ns: u64,
    lag_ms: u64,
    flights: VecDeque<PendingFlight>,
    deferred: u64,
    samples: u64,
    rung: LadderRung,
    ref_wire: usize,
    fps_cap: u32,
    ok_since_change: u64,
    last_rung_change: Option<Instant>,
}

impl AdaptiveTransport {
    pub fn new(lag_ms: u64) -> Self {
        Self::with_fps_cap(lag_ms, 120)
    }

    pub fn with_fps_cap(lag_ms: u64, fps_cap: u32) -> Self {
        let fps_cap = fps_cap.clamp(1, 240);
        Self {
            epoch: 0,
            seq: 0,
            bytes_outstanding: 0,
            drain_rate_ewma: INITIAL_DRAIN_RATE,
            rtt_ewma_ns: 0,
            lag_ms: lag_ms.max(1),
            flights: VecDeque::new(),
            deferred: 0,
            samples: 0,
            rung: LadderRung {
                scale: 1.0,
                fps: fps_cap,
                static_only: false,
            },
            ref_wire: LADDER_REF_WIRE,
            fps_cap,
            ok_since_change: 0,
            last_rung_change: None,
        }
    }

    pub fn from_env_with_fps_cap(fps_cap: u32) -> Self {
        let lag_ms = std::env::var("TERMINAL_FENSTER_LAG_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_LAG_MS);
        Self::with_fps_cap(lag_ms, fps_cap)
    }

    pub fn from_env() -> Self {
        Self::from_env_with_fps_cap(120)
    }

    pub fn stats(&self) -> TransportStats {
        TransportStats {
            drain_rate_ewma: self.drain_rate_ewma,
            rtt_ewma_ms: self.rtt_ewma_ns as f64 / 1_000_000.0,
            bytes_outstanding: self.bytes_outstanding,
            credit_bytes: self.credit_bytes(0),
            deferred: self.deferred,
            samples: self.samples,
            rung: self.rung,
        }
    }

    pub fn rung(&self) -> LadderRung {
        self.rung
    }

    /// Observe a presented frame's wire size so the ladder tracks real content.
    pub fn observe_wire(&mut self, wire_bytes: usize) {
        if wire_bytes > self.ref_wire {
            // Cap at Wikipedia-class so a one-off huge frame cannot pin the ladder forever.
            self.ref_wire = wire_bytes.min(LADDER_REF_WIRE.saturating_mul(2));
        }
        self.ok_since_change = self.ok_since_change.saturating_add(1);
    }

    /// Recompute the ladder. Returns `Some` when the applied rung changed (caller notifies engine).
    pub fn update_rung(&mut self, now: Instant) -> Option<LadderRung> {
        // Cold-start drain is a placeholder (~100 Mbit). Applying it before MBDT samples
        // would pin local sessions at the Wikipedia design point (8 fps) and starve paints.
        if self.samples == 0 {
            return None;
        }
        let rtt_ms = self.rtt_ewma_ns as f64 / 1_000_000.0;
        let desired = select_rung(self.drain_rate_ewma, rtt_ms, self.ref_wire, self.fps_cap);
        if desired == self.rung {
            return None;
        }

        let going_down = desired.rank() < self.rung.rank();
        if going_down {
            self.apply_rung(desired, now);
            return Some(self.rung);
        }

        // Slow up: ≥20 ok frames and ≥2 s since last change (C09 §5.5).
        let elapsed_ok = self
            .last_rung_change
            .map(|t| now.saturating_duration_since(t).as_millis() as u64)
            .unwrap_or(UP_MIN_MS);
        if self.ok_since_change >= UP_MIN_FRAMES && elapsed_ok >= UP_MIN_MS {
            // Climb at most one fps/scale step toward desired.
            let stepped = step_up(self.rung, desired);
            if stepped != self.rung {
                self.apply_rung(stepped, now);
                return Some(self.rung);
            }
        }
        None
    }

    fn apply_rung(&mut self, rung: LadderRung, now: Instant) {
        self.rung = rung;
        self.ok_since_change = 0;
        self.last_rung_change = Some(now);
    }

    fn next_marker_id(&mut self) -> u32 {
        let id = kitty::marker_id(self.epoch, self.seq);
        self.seq = self.seq.wrapping_add(1);
        id
    }

    fn credit_bytes(&self, one_frame: usize) -> usize {
        let lag_bytes = (self.drain_rate_ewma * self.lag_ms as f64 / 1000.0) as usize;
        one_frame.max(lag_bytes.max(1))
    }

    /// Whether another `wire_bytes` presentation may be written.
    pub fn may_send(&self, wire_bytes: usize) -> PresentGate {
        if wire_bytes == 0 {
            return PresentGate::Send;
        }
        let credit = self.credit_bytes(wire_bytes);
        if self.bytes_outstanding.saturating_add(wire_bytes) <= credit {
            PresentGate::Send
        } else {
            PresentGate::Deferred
        }
    }

    /// Prepend/append MBDT markers around a page payload about to be written.
    ///
    /// Returns the marker ids when bracketing was applied. The caller must pass replies for
    /// those ids into [`Self::on_kitty_reply`].
    pub fn bracket_presentation(&mut self, out: &mut Vec<u8>, page_start: usize) -> BracketInfo {
        let wire_bytes = out.len().saturating_sub(page_start);
        if wire_bytes == 0 {
            return BracketInfo::default();
        }

        if wire_bytes >= MBDT_SAMPLE_MIN {
            let marker_a = self.next_marker_id();
            let marker_b = self.next_marker_id();
            let mut bracketed = Vec::new();
            bracketed.extend(kitty::marker(marker_a));
            bracketed.extend_from_slice(&out[page_start..]);
            bracketed.extend(kitty::marker(marker_b));
            out.truncate(page_start);
            out.extend_from_slice(&bracketed);
            self.flights.push_back(PendingFlight {
                marker_a,
                marker_b,
                wire_bytes,
                sent_at: Instant::now(),
                anchor_at: None,
            });
            self.bytes_outstanding = self.bytes_outstanding.saturating_add(wire_bytes);
            BracketInfo {
                bracketed: true,
                marker_a: Some(marker_a),
                marker_b: Some(marker_b),
                wire_bytes,
            }
        } else {
            let marker = self.next_marker_id();
            out.extend(kitty::marker(marker));
            BracketInfo {
                bracketed: false,
                marker_a: None,
                marker_b: Some(marker),
                wire_bytes,
            }
        }
    }

    /// Record a Kitty graphics reply (`ESC _ G i=<id>;…`).
    ///
    /// Non-`OK` statuses cut the drain estimate (C09 §5.5 down-trigger) so the next credit
    /// window shrinks without waiting for a full MBDT sample.
    pub fn on_kitty_reply(&mut self, id: u32, status: &str, now: Instant) {
        let ok = status == "OK";
        if !ok {
            self.note_terminal_error();
        }

        let Some((epoch, _seq)) = kitty::parse_marker_id(id) else {
            return;
        };
        if epoch != self.epoch {
            return;
        }

        if let Some(idx) = self
            .flights
            .iter()
            .position(|f| f.marker_a == id || f.marker_b == id)
        {
            if self.flights[idx].marker_a == id {
                let sent_at = self.flights[idx].sent_at;
                self.flights[idx].anchor_at = Some(now);
                if ok {
                    let rtt_ns = now.saturating_duration_since(sent_at).as_nanos() as u64;
                    self.update_rtt(rtt_ns);
                }
                return;
            }
            if self.flights[idx].marker_b == id {
                let wire_bytes = self.flights[idx].wire_bytes;
                if ok {
                    let anchor = self.flights[idx]
                        .anchor_at
                        .unwrap_or(self.flights[idx].sent_at);
                    let drain_ns = now.saturating_duration_since(anchor).as_nanos() as u64;
                    self.record_drain_sample(wire_bytes, drain_ns);
                }
                self.bytes_outstanding = self.bytes_outstanding.saturating_sub(wire_bytes);
                self.flights.remove(idx);
            }
        }
    }

    /// Terminal rejected a frame or marker — collapse credit quickly and drop the ladder.
    pub fn note_terminal_error(&mut self) {
        self.drain_rate_ewma = (self.drain_rate_ewma * 0.25).max(125_000.0);
        self.deferred += 1;
        let now = Instant::now();
        let rtt_ms = self.rtt_ewma_ns as f64 / 1_000_000.0;
        let desired = select_rung(self.drain_rate_ewma, rtt_ms, self.ref_wire, self.fps_cap);
        self.apply_rung(desired, now);
    }

    fn update_rtt(&mut self, sample_ns: u64) {
        if self.rtt_ewma_ns == 0 {
            self.rtt_ewma_ns = sample_ns;
        } else {
            let s = sample_ns as f64;
            let e = self.rtt_ewma_ns as f64;
            self.rtt_ewma_ns = (RTT_ALPHA * s + (1.0 - RTT_ALPHA) * e) as u64;
        }
    }

    fn record_drain_sample(&mut self, wire_bytes: usize, drain_ns: u64) {
        if drain_ns < MIN_DRAIN_NS {
            return;
        }
        let ewma_ns = if self.drain_rate_ewma > 0.0 {
            (wire_bytes as f64 / self.drain_rate_ewma * 1_000_000_000.0) as u64
        } else {
            drain_ns
        };
        if drain_ns > ewma_ns.saturating_mul(MAX_DRAIN_MULT as u64) {
            return;
        }
        let rate = wire_bytes as f64 / (drain_ns as f64 / 1_000_000_000.0);
        if self.samples == 0 {
            self.drain_rate_ewma = rate;
        } else {
            self.drain_rate_ewma = DRAIN_ALPHA * rate + (1.0 - DRAIN_ALPHA) * self.drain_rate_ewma;
        }
        self.samples += 1;
    }

    pub fn note_deferred(&mut self) {
        self.deferred += 1;
    }

    pub fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1) & 0x0FFF;
        self.seq = 0;
        self.bytes_outstanding = 0;
        self.flights.clear();
        self.ok_since_change = 0;
        self.last_rung_change = None;
    }
}

fn step_up(current: LadderRung, desired: LadderRung) -> LadderRung {
    if current.scale < desired.scale - f64::EPSILON {
        // Prefer raising scale only after fps is already high at the current scale.
        if current.fps < 15 {
            let next_fps = FPS_STEPS
                .iter()
                .rev()
                .copied()
                .find(|f| *f > current.fps)
                .unwrap_or(current.fps);
            return LadderRung {
                scale: current.scale,
                fps: next_fps.min(desired.fps),
                static_only: false,
            };
        }
        let next_scale = SCALES
            .iter()
            .copied()
            .find(|s| *s > current.scale + f64::EPSILON)
            .unwrap_or(current.scale)
            .min(desired.scale);
        return LadderRung {
            scale: next_scale,
            fps: 1,
            static_only: false,
        };
    }
    let next_fps = FPS_STEPS
        .iter()
        .rev()
        .copied()
        .find(|f| *f > current.fps)
        .unwrap_or(current.fps)
        .min(desired.fps);
    LadderRung {
        scale: current.scale,
        fps: next_fps,
        static_only: desired.static_only && next_fps <= 1,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BracketInfo {
    pub bracketed: bool,
    pub marker_a: Option<u32>,
    pub marker_b: Option<u32>,
    pub wire_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn credit_recovers_fast_link_throughput() {
        let mut t = AdaptiveTransport::new(100);
        t.drain_rate_ewma = 12_500_000.0; // 100 Mbit/s → 1.25 MB credit window
        let frame = 54_000;
        // Credit window holds ~23 frames; far more than the 1-frame cap on slow links.
        let mut sent = 0usize;
        while sent < 23 {
            assert_eq!(t.may_send(frame), PresentGate::Send);
            t.bytes_outstanding += frame;
            sent += 1;
        }
        assert_eq!(t.may_send(frame), PresentGate::Deferred);
        t.bytes_outstanding = 0;
        assert_eq!(t.may_send(frame), PresentGate::Send);
    }

    #[test]
    fn slow_link_degenerates_to_one_frame() {
        let mut t = AdaptiveTransport::new(100);
        t.drain_rate_ewma = 125_000.0; // 1 Mbit/s → 12.5 KB / 100 ms credit
        let frame = 54_000;
        assert_eq!(t.may_send(frame), PresentGate::Send);
        t.bytes_outstanding += frame;
        assert_eq!(t.may_send(frame), PresentGate::Deferred);
    }

    #[test]
    fn mbdt_cancels_rtt_in_drain_estimate() {
        let mut t = AdaptiveTransport::new(100);
        let start = Instant::now();
        let a = kitty::marker_id(0, 0);
        let b = kitty::marker_id(0, 1);
        t.flights.push_back(PendingFlight {
            marker_a: a,
            marker_b: b,
            wire_bytes: 100_000,
            sent_at: start,
            anchor_at: None,
        });
        t.bytes_outstanding = 100_000;
        t.on_kitty_reply(a, "OK", start + Duration::from_millis(100));
        t.on_kitty_reply(b, "OK", start + Duration::from_millis(110));
        assert!(t.drain_rate_ewma > 8_000_000.0);
        assert_eq!(t.bytes_outstanding, 0);
    }

    #[test]
    fn error_reply_shrinks_drain_estimate() {
        let mut t = AdaptiveTransport::new(100);
        t.drain_rate_ewma = 12_500_000.0;
        t.on_kitty_reply(kitty::PAGE_IMAGE_ID, "EBADPNG", Instant::now());
        assert!(t.drain_rate_ewma < 4_000_000.0);
    }

    #[test]
    fn stale_epoch_replies_are_ignored() {
        let mut t = AdaptiveTransport::new(100);
        t.epoch = 2;
        let stale = kitty::marker_id(1, 9);
        t.on_kitty_reply(stale, "OK", Instant::now());
        assert_eq!(t.samples, 0);
    }

    #[test]
    fn bracket_adds_markers_for_large_payloads() {
        let mut t = AdaptiveTransport::new(100);
        let mut out = vec![0u8; 9000];
        let info = t.bracket_presentation(&mut out, 0);
        assert!(info.bracketed);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b_Gi="));
        assert_eq!(t.bytes_outstanding, 9000);
    }

    #[test]
    fn small_payload_gets_trailing_marker_only() {
        let mut t = AdaptiveTransport::new(100);
        let mut out = b"hello".to_vec();
        let info = t.bracket_presentation(&mut out, 0);
        assert!(!info.bracketed);
        assert!(out.len() > 5);
        assert_eq!(t.bytes_outstanding, 0);
    }

    #[test]
    fn ladder_matches_c09_design_points() {
        let cap = 120;
        let r = select_rung(100_000_000.0, 10.0, LADDER_REF_WIRE, cap);
        assert_eq!(r.scale, 1.0);
        assert!(
            r.fps >= 100,
            "fast link should reach triple-digit fps, got {}",
            r.fps
        );

        let r = select_rung(12_500_000.0, 20.0, LADDER_REF_WIRE, cap);
        assert_eq!(r.scale, 1.0);
        assert_eq!(r.fps, 8);

        let r = select_rung(1_250_000.0, 20.0, LADDER_REF_WIRE, cap);
        assert_eq!((r.scale, r.fps), (1.0, 1));

        let r = select_rung(625_000.0, 20.0, LADDER_REF_WIRE, cap);
        assert!((r.scale - 0.75).abs() < 1e-9);
        assert_eq!(r.fps, 1);

        let r = select_rung(100_000.0, 20.0, LADDER_REF_WIRE, cap);
        assert!(r.static_only);

        let r = select_rung(12_500_000.0, 150.0, LADDER_REF_WIRE, cap);
        assert!(r.fps <= 60);
    }

    #[test]
    fn ladder_waits_for_mbdt_samples() {
        let mut t = AdaptiveTransport::new(100);
        t.drain_rate_ewma = 12_500_000.0;
        assert!(t.update_rung(Instant::now()).is_none());
    }

    #[test]
    fn ladder_drops_immediately_climbs_slowly() {
        let mut t = AdaptiveTransport::new(100);
        t.drain_rate_ewma = 12_500_000.0;
        t.samples = 1;
        let now = Instant::now();
        assert_eq!(t.update_rung(now).unwrap().fps, 8);

        t.drain_rate_ewma = 100_000_000.0;
        assert!(
            t.update_rung(now).is_none(),
            "must not climb without history"
        );
        t.ok_since_change = 20;
        t.last_rung_change = Some(now - Duration::from_secs(3));
        let up = t.update_rung(now).unwrap();
        assert!(up.fps > 8);
    }
}
