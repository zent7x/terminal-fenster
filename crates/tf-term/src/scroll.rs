//! Wheel delta shaping for terminal mouse protocols (D03).
//!
//! Terminals report wheel as direction-only buttons; Ghostty may emit several events per
//! physical notch. Burst grouping collapses those to one logical notch, then a rate model
//! maps inter-notch timing to CSS-pixel deltas Chromium expects.

use std::time::{Duration, Instant};

const D_MAX: f64 = 120.0;
const D_MIN: f64 = 16.0;
const D_FLOOR: f64 = 4.0;
const T_FULL: f64 = 160.0;
const GAMMA: f64 = 0.5;
const V_MAX: f64 = 3000.0;
const T_MOMENTUM: f64 = 100.0;
const RECOVER: f64 = 1.15;
const IDLE_RESET: f64 = 400.0;
const ALPHA: f64 = 0.35;
const BURST_MS: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Default)]
struct ScrollAxis {
    dt_ema: f64,
    last_ts: Option<Instant>,
    last_dir: i8,
    d_prev: f64,
}

impl ScrollAxis {
    fn new() -> Self {
        Self {
            dt_ema: T_FULL,
            last_dir: 0,
            d_prev: D_MAX,
            ..Default::default()
        }
    }

    fn on_notch(&mut self, dir: i8, now: Instant) -> f64 {
        let dt_raw = self
            .last_ts
            .map(|t| ms(now.saturating_duration_since(t)))
            .unwrap_or(IDLE_RESET);

        if dir != self.last_dir || dt_raw > IDLE_RESET {
            self.dt_ema = T_FULL;
            self.d_prev = D_MAX;
        } else {
            self.dt_ema = ALPHA * dt_raw + (1.0 - ALPHA) * self.dt_ema;
        }

        let shape = D_MIN + (D_MAX - D_MIN) * (self.dt_ema / T_FULL).min(1.0).powf(GAMMA);
        let cap = (V_MAX * self.dt_ema / 1000.0).max(D_FLOOR);
        let mut d = shape.min(cap);

        if self.dt_ema < T_MOMENTUM {
            d = d.min(self.d_prev * RECOVER);
        }

        self.d_prev = d;
        self.last_ts = Some(now);
        self.last_dir = dir;
        d
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn dir_sign(d: WheelDir) -> i8 {
    match d {
        WheelDir::Up | WheelDir::Left => 1,
        WheelDir::Down | WheelDir::Right => -1,
    }
}

fn is_vertical(d: WheelDir) -> bool {
    matches!(d, WheelDir::Up | WheelDir::Down)
}

/// Stateful wheel delta calculator.
#[derive(Debug, Default)]
pub struct ScrollController {
    vertical: ScrollAxis,
    horizontal: ScrollAxis,
}

impl ScrollController {
    pub fn new() -> Self {
        Self {
            vertical: ScrollAxis::new(),
            horizontal: ScrollAxis::new(),
        }
    }

    /// Collapse a batch of raw wheel edges from one terminal read into pixel deltas.
    pub fn consume_batch(&mut self, edges: &[WheelEdge], now: Instant) -> (i32, i32) {
        let notches = group_notches(edges, now);
        let mut dx = 0.0f64;
        let mut dy = 0.0f64;
        for (dir, ts) in notches {
            let sign = dir_sign(dir) as f64;
            let d = if is_vertical(dir) {
                let px = self.vertical.on_notch(dir_sign(dir), ts);
                dy += sign * px;
                px
            } else {
                let px = self.horizontal.on_notch(dir_sign(dir), ts);
                dx += sign * px;
                px
            };
            let _ = d; // keep for clarity in port of the spec
        }
        (dx.round() as i32, dy.round() as i32)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WheelEdge {
    pub dir: WheelDir,
    pub at: Instant,
}

fn group_notches(edges: &[WheelEdge], now: Instant) -> Vec<(WheelDir, Instant)> {
    if edges.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < edges.len() {
        let dir = edges[i].dir;
        let mut j = i + 1;
        while j < edges.len() {
            let gap = ms(edges[j].at.saturating_duration_since(edges[j - 1].at));
            if edges[j].dir != dir || gap > BURST_MS {
                break;
            }
            j += 1;
        }
        out.push((dir, edges[j - 1].at));
        i = j;
    }
    if out.is_empty() && !edges.is_empty() {
        out.push((edges[edges.len() - 1].dir, now));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_groups_terminal_multiplier_into_one_notch() {
        let t0 = Instant::now();
        let edges = vec![
            WheelEdge {
                dir: WheelDir::Down,
                at: t0,
            },
            WheelEdge {
                dir: WheelDir::Down,
                at: t0 + Duration::from_micros(100),
            },
            WheelEdge {
                dir: WheelDir::Down,
                at: t0 + Duration::from_micros(200),
            },
        ];
        let notches = group_notches(&edges, t0);
        assert_eq!(notches.len(), 1);
    }

    #[test]
    fn isolated_notch_stays_at_d_max() {
        let mut s = ScrollController::new();
        let t0 = Instant::now();
        let (_, dy) = s.consume_batch(
            &[WheelEdge {
                dir: WheelDir::Down,
                at: t0,
            }],
            t0,
        );
        assert_eq!(dy, -120);
    }

    #[test]
    fn triple_burst_emits_one_notch_not_three() {
        let mut s = ScrollController::new();
        let t0 = Instant::now();
        let edges = vec![
            WheelEdge {
                dir: WheelDir::Down,
                at: t0,
            },
            WheelEdge {
                dir: WheelDir::Down,
                at: t0 + Duration::from_micros(100),
            },
            WheelEdge {
                dir: WheelDir::Down,
                at: t0 + Duration::from_micros(200),
            },
        ];
        let (_, dy) = s.consume_batch(&edges, t0);
        assert_eq!(dy, -120);
    }

    #[test]
    fn fast_trackpad_spacing_yields_smaller_steps() {
        let mut s = ScrollController::new();
        let t0 = Instant::now();
        let mut last_dy = 0;
        for i in 0..5 {
            let at = t0 + Duration::from_millis(i * 12);
            let (_, dy) = s.consume_batch(
                &[WheelEdge {
                    dir: WheelDir::Down,
                    at,
                }],
                at,
            );
            last_dy = dy;
        }
        assert!(last_dy.abs() < 120);
        assert!(last_dy.abs() > 16);
    }
}
