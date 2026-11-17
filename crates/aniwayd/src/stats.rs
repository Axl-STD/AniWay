//! Cheap render counters, active only with `RUST_LOG=aniwayd=debug`; a summary is logged every 5 s.
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Call {
    MakeCurrent,
    Render,
    Swap,
}

#[derive(Clone, Copy)]
pub enum Counter {
    Wakeups,
    NewFrames,
    Renders,
}

#[derive(Default)]
struct Stats {
    times: [Vec<u32>; 3],
    counts: [u64; 3],
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::default();
}

pub fn enabled() -> bool {
    log::log_enabled!(log::Level::Debug)
}

/// Runs `f`, recording its duration under `call` when statistics are on.
pub fn time<T>(call: Call, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let out = f();
    let micros = start.elapsed().as_micros() as u32;
    STATS.with(|s| s.borrow_mut().times[call as usize].push(micros));
    out
}

pub fn count(counter: Counter) {
    if enabled() {
        STATS.with(|s| s.borrow_mut().counts[counter as usize] += 1);
    }
}

/// Logs and resets the counters; `overlays` normalises rates to one overlay.
pub fn report(overlays: usize, period: Duration) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        let secs = period.as_secs_f64();
        let per_overlay = |n: u64| n as f64 / secs / overlays.max(1) as f64;
        let stat = |v: &mut Vec<u32>| {
            v.sort_unstable();
            let avg = v.iter().map(|&x| u64::from(x)).sum::<u64>() as f64 / v.len().max(1) as f64;
            (avg, v.get(v.len() * 95 / 100).copied().unwrap_or(0))
        };
        let [make_current, render, swap] = &mut s.times;
        let (mc, rd, sw) = (stat(make_current), stat(render), stat(swap));
        log::debug!(
            "per overlay/s: wakeups {:.1} new frames {:.1} renders {:.1} | us avg/p95: make_current {:.0}/{} render {:.0}/{} swap {:.0}/{}",
            per_overlay(s.counts[0]),
            per_overlay(s.counts[1]),
            per_overlay(s.counts[2]),
            mc.0,
            mc.1,
            rd.0,
            rd.1,
            sw.0,
            sw.1
        );
        *s = Stats::default();
    });
}
