//! GPUI's frame-rate caps for terminal windows.

use gpui::WindowOptions;

/// GPUI's default window options with a terminal window's minimum frame
/// intervals. GPUI captures the intervals when it creates the window, so a
/// change to `power_frame_limits` applies to windows opened afterwards.
pub(super) fn terminal_window_options(
    power_frame_limits: bool,
) -> WindowOptions {
    let defaults = WindowOptions::default();
    let (low_power_frame_interval, thermal_frame_interval) =
        if power_frame_limits {
            (
                defaults.low_power_frame_interval,
                defaults.thermal_frame_interval,
            )
        } else {
            (None, None)
        };
    WindowOptions {
        // GPUI caps unfocused windows at 30 frames per second by default.
        // A terminal left unfocused still shows live output, and Huterm's own
        // frame admission already stops idle redraws.
        inactive_frame_interval: None,
        // GPUI's Low Power Mode or power-saver cap and its serious or critical
        // thermal cap, unless `[window] power_frame_limits` turns them off.
        low_power_frame_interval,
        thermal_frame_interval,
        ..defaults
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_window_options;
    use gpui::WindowOptions;

    #[test]
    fn power_frame_limits_keep_or_clear_gpuis_power_caps() {
        let defaults = WindowOptions::default();
        assert!(defaults.inactive_frame_interval.is_some());
        assert!(defaults.low_power_frame_interval.is_some());
        assert!(defaults.thermal_frame_interval.is_some());

        let on = terminal_window_options(true);
        assert_eq!(on.inactive_frame_interval, None);
        assert_eq!(
            on.low_power_frame_interval,
            defaults.low_power_frame_interval
        );
        assert_eq!(on.thermal_frame_interval, defaults.thermal_frame_interval);

        let off = terminal_window_options(false);
        assert_eq!(
            (
                off.inactive_frame_interval,
                off.low_power_frame_interval,
                off.thermal_frame_interval
            ),
            (None, None, None)
        );
    }
}
