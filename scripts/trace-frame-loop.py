"""Temporary HUTERM_FRAME_TRACE_20260921 probe, used only on disposable runners."""
import json
import subprocess
import sys
from pathlib import Path

if len(sys.argv) == 2:
    root = Path(sys.argv[1])
else:
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"]))
    root = Path(next(p["manifest_path"] for p in metadata["packages"] if p["name"] == "gpui" and p["version"] == "0.2.2")).parent

def edit(rel, old, new):
    p = root / rel
    s = p.read_text()
    assert s.count(old) == 1, (rel, old, s.count(old))
    p.write_text(s.replace(old, new))

window = "src/window.rs"
edit(window, "            move |request_frame_options| {\n                let next_frame_callbacks", """            // HUTERM_FRAME_TRACE_20260921
            let mut trace_previous = std::time::Instant::now();
            move |request_frame_options| {
                let trace_start = std::time::Instant::now();
                let trace_interval = trace_start.duration_since(trace_previous);
                trace_previous = trace_start;
                let trace_enabled = std::env::var_os("HUTERM_FRAME_TRACE").is_some();
                let next_frame_callbacks""")
edit(window, "                // Keep presenting the current scene for 1 extra second", """                let trace_callbacks_done = std::time::Instant::now();
                // Keep presenting the current scene for 1 extra second""")
edit(window, """                                let arena_clear_needed = window.draw(cx);
                                window.present();""", """                                let trace_draw_start = std::time::Instant::now();
                                let arena_clear_needed = window.draw(cx);
                                let trace_present_start = std::time::Instant::now();
                                window.present();
                                if trace_enabled {
                                    eprintln!("HUTERM_FRAME_TRACE draw_us={} present_us={}", trace_present_start.duration_since(trace_draw_start).as_micros(), trace_present_start.elapsed().as_micros());
                                }""")
edit(window, """                    .log_err();
            }
        }));
        platform_window.on_resize""", """                    .log_err();
                if trace_enabled {
                    eprintln!("HUTERM_FRAME_TRACE interval_us={} callbacks_us={} total_us={}", trace_interval.as_micros(), trace_callbacks_done.duration_since(trace_start).as_micros(), trace_start.elapsed().as_micros());
                }
            }
        }));
        platform_window.on_resize""")
blade = "src/platform/blade/blade_renderer.rs"
edit(blade, """        let frame = {
            profiling::scope!("acquire frame");
            self.surface.acquire_frame()
        };""", """        // HUTERM_FRAME_TRACE_20260921
        let trace_acquire = std::time::Instant::now();
        let frame = {
            profiling::scope!("acquire frame");
            self.surface.acquire_frame()
        };
        let trace_acquire_us = trace_acquire.elapsed().as_micros();""")
edit(blade, """        self.wait_for_gpu();
        self.last_sync_point = Some(sync_point);""", """        let trace_wait = std::time::Instant::now();
        self.wait_for_gpu();
        if std::env::var_os("HUTERM_FRAME_TRACE").is_some() {
            eprintln!("HUTERM_FRAME_TRACE acquire_us={} gpu_wait_us={}", trace_acquire_us, trace_wait.elapsed().as_micros());
        }
        self.last_sync_point = Some(sync_point);""")
client = "src/platform/linux/x11/client.rs"
edit(client, """                    let now = Instant::now();
                    while instant < now {
                        instant += refresh_rate;
                    }
                    calloop::timer::TimeoutAction::ToInstant(instant)""", """                    // HUTERM_FRAME_TRACE_20260921
                    let now = Instant::now();
                    let mut trace_ticks = 0;
                    while instant < now {
                        instant += refresh_rate;
                        trace_ticks += 1;
                    }
                    if std::env::var_os("HUTERM_FRAME_TRACE").is_some() {
                        eprintln!("HUTERM_FRAME_TRACE period_us={} ticks_advanced={}", refresh_rate.as_micros(), trace_ticks);
                    }
                    calloop::timer::TimeoutAction::ToInstant(instant)""")
print("instrumented", root)
