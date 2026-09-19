//! Opt-in display selection and frame diagnostics for native benchmarks.

use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui::{App, DisplayId, Window};

pub(crate) fn selected(cx: &App) -> anyhow::Result<Option<DisplayId>> {
    let value = match std::env::var("HUTERM_BENCH_DISPLAY_ID") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let requested = value
        .parse::<u32>()
        .context("HUTERM_BENCH_DISPLAY_ID must be a numeric display ID")?;
    select(requested, cx.displays().iter().map(|display| display.id()))
        .map(Some)
}

fn select(
    requested: u32,
    displays: impl IntoIterator<Item = DisplayId>,
) -> anyhow::Result<DisplayId> {
    for id in displays {
        if u32::from(id) == requested {
            return Ok(id);
        }
    }
    bail!("benchmark display {requested} is not connected")
}

pub(crate) fn verify(window: &Window, cx: &App, expected: DisplayId) {
    let actual = window.display(cx).map(|display| display.id());
    if actual != Some(expected) {
        eprintln!(
            "HUTERM_BENCH wrong_display expected={expected:?} actual={actual:?}"
        );
        std::process::exit(2);
    }
}

/// Observes delivered callbacks without requesting additional paints.
pub(crate) fn observe(window: &Window, cx: &App, expected: DisplayId) {
    verify(window, cx, expected);
    eprintln!(
        "HUTERM_BENCH display_id={} scale={}",
        u32::from(expected),
        window.scale_factor(),
    );
    next_frame(window, expected, FrameIntervals::default());
}

fn next_frame(
    window: &Window,
    expected: DisplayId,
    mut frames: FrameIntervals,
) {
    window.on_next_frame(move |window, cx| {
        verify(window, cx, expected);
        frames.tick();
        if frames.elapsed >= Duration::from_secs(1) {
            frames.report("callbacks", expected);
            frames = FrameIntervals::default();
        }
        next_frame(window, expected, frames);
    });
}

#[derive(Default)]
pub(crate) struct FrameIntervals {
    last: Option<Instant>,
    elapsed: Duration,
    intervals: Vec<Duration>,
}

impl FrameIntervals {
    pub(crate) fn tick(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last {
            let interval = now.duration_since(last);
            self.elapsed += interval;
            self.intervals.push(interval);
        }
        self.last = Some(now);
    }

    pub(crate) fn report(&self, source: &str, display: DisplayId) {
        let mut sorted = self.intervals.clone();
        sorted.sort_unstable();
        let median = sorted.get(sorted.len() / 2).copied().unwrap_or_default();
        eprintln!(
            "HUTERM_BENCH source={source} display_id={} intervals={} elapsed_us={} median_interval_us={}",
            u32::from(display),
            sorted.len(),
            self.elapsed.as_micros(),
            median.as_micros(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_display_does_not_fall_back() {
        assert!(
            select(42, [])
                .unwrap_err()
                .to_string()
                .contains("not connected")
        );
    }
}
