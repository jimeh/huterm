//! Paint policy and the single native-alpha owner shared with Quake fades.
use std::{cell::Cell, rc::Rc};

use gpui::{Context, Hsla, Window, WindowBackgroundAppearance};
use huterm_config::WindowConfig;

use crate::keymap::Platform;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Support {
    #[default]
    Available,
    #[cfg_attr(
        all(not(target_os = "linux"), not(test)),
        expect(dead_code, reason = "Linux fallback policy")
    )]
    NoCompositor,
    #[cfg_attr(
        all(not(target_os = "linux"), not(test)),
        expect(dead_code, reason = "Linux fallback policy")
    )]
    NoArgbVisual,
    #[cfg_attr(
        all(not(target_os = "linux"), not(test)),
        expect(dead_code, reason = "Linux fallback policy")
    )]
    Unavailable,
}
impl Support {
    pub(crate) fn note(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::NoCompositor => Some(
                "No compositing manager is running, so windows stay opaque.",
            ),
            Self::NoArgbVisual => Some(
                "No 32-bit TrueColor visual is available, so windows stay opaque.",
            ),
            Self::Unavailable => Some(
                "X11 transparency support could not be checked, so windows stay opaque.",
            ),
        }
    }
    pub(crate) fn paint(self, config: WindowConfig) -> (f32, f32) {
        if self == Self::Available {
            (config.background_opacity, config.foreground_opacity)
        } else {
            (1.0, 1.0)
        }
    }
}

pub(crate) fn support() -> Support {
    #[cfg(target_os = "linux")]
    return crate::quake::native::transparency_support()
        .unwrap_or(Support::Unavailable);
    #[cfg(not(target_os = "linux"))]
    Support::Available
}

/// Overall opacity never changes the surface's appearance.
pub(crate) fn appearance(
    platform: Platform,
    decorated: bool,
    support: Support,
    background: f32,
    blur: bool,
) -> WindowBackgroundAppearance {
    if support != Support::Available || background >= 1.0 {
        if decorated {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        }
    } else if platform == Platform::MacOs && background < 1.0 && blur {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// Tints colors without changing their alpha. The opaque path uses the
/// original overlays; translucent panes use these tints instead.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tint {
    pub(crate) dim: f32,
    pub(crate) darken: f32,
}
impl Tint {
    pub(crate) fn apply(self, color: Hsla, background: Hsla) -> Hsla {
        if self == Self::default() {
            return color;
        }
        let mut rgb: gpui::Rgba = color.into();
        let bg: gpui::Rgba = background.into();
        rgb.r = (rgb.r + (bg.r - rgb.r) * self.dim) * (1.0 - self.darken);
        rgb.g = (rgb.g + (bg.g - rgb.g) * self.dim) * (1.0 - self.darken);
        rgb.b = (rgb.b + (bg.b - rgb.b) * self.dim) * (1.0 - self.darken);
        rgb.into()
    }
}

#[derive(Clone, Default)]
pub(crate) struct Alpha(Rc<Cell<Factors>>);
#[derive(Clone, Copy, PartialEq)]
struct Factors {
    configured: f64,
    supported: bool,
    fade: f64,
    applied: f64,
}
impl Default for Factors {
    fn default() -> Self {
        Self {
            configured: 1.0,
            supported: true,
            fade: 1.0,
            applied: 1.0,
        }
    }
}
impl Factors {
    fn effective(self) -> f64 {
        if self.supported {
            self.configured * self.fade
        } else {
            1.0
        }
    }
}
impl Alpha {
    pub(crate) fn set_supported(&self, supported: bool) {
        self.0.set(Factors {
            supported,
            ..self.0.get()
        });
    }

    pub(crate) fn configure(&self, configured: f64) {
        self.0.set(Factors {
            configured,
            ..self.0.get()
        });
    }
    pub(crate) fn fade(
        &self,
        fade: f64,
        write: impl FnMut(f64) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.0.set(Factors {
            fade,
            ..self.0.get()
        });
        self.apply(write)
    }
    fn apply(
        &self,
        mut write: impl FnMut(f64) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        loop {
            let factors = self.0.get();
            let effective = factors.effective();
            if effective.to_bits() == factors.applied.to_bits() {
                return Ok(());
            }
            write(effective)?;
            // Native setters can reenter GPUI. Keep any reload or fade
            // received during the write, then reconcile it before returning.
            self.0.set(Factors {
                applied: effective,
                ..self.0.get()
            });
        }
    }
    pub(crate) fn sync<V: 'static>(
        &self,
        window: &Window,
        cx: &mut Context<'_, V>,
    ) {
        let factors = self.0.get();
        if factors.effective().to_bits() == factors.applied.to_bits() {
            return;
        }
        #[cfg(target_os = "macos")]
        let native = crate::native_appearance::macos::NativeWindow::of(window);
        #[cfg(target_os = "linux")]
        let native = crate::quake::native::Platform::new(cx)
            .and_then(|platform| platform.window(window));
        match native {
            Ok(native) => {
                let owner = self.clone();
                cx.spawn(async move |_, _| {
                    if let Err(error) =
                        owner.apply(|value| native.opacity(value))
                    {
                        eprintln!("Window opacity: {error}");
                    }
                })
                .detach();
            }
            Err(error) => eprintln!("Window opacity: {error}"),
        }
    }
}

pub(crate) fn native_alpha(
    window: &Window,
    cx: &gpui::App,
) -> anyhow::Result<f64> {
    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        Ok(crate::native_appearance::macos::NativeWindow::of(window)?
            .opacity_value())
    }
    #[cfg(target_os = "linux")]
    {
        crate::quake::native::Platform::new(cx)?
            .window(window)?
            .opacity_value()
    }
}

/// Disjoint pieces around a rectangular hole. A strip tab replaces the bar
/// fill beneath it rather than blending a second translucent background.
pub(crate) fn background_regions(
    bounds: gpui::Bounds<gpui::Pixels>,
    hole: Option<gpui::Bounds<gpui::Pixels>>,
) -> Vec<gpui::Bounds<gpui::Pixels>> {
    use gpui::{Bounds, point, size};
    let Some(hole) = hole else {
        return vec![bounds];
    };
    let hole = hole.intersect(&bounds);
    if hole.size.width <= gpui::px(0.0) || hole.size.height <= gpui::px(0.0) {
        return vec![bounds];
    }
    [
        Bounds::new(
            bounds.origin,
            size(bounds.size.width, hole.top() - bounds.top()),
        ),
        Bounds::new(
            point(bounds.left(), hole.bottom()),
            size(bounds.size.width, bounds.bottom() - hole.bottom()),
        ),
        Bounds::new(
            point(bounds.left(), hole.top()),
            size(hole.left() - bounds.left(), hole.size.height),
        ),
        Bounds::new(
            point(hole.right(), hole.top()),
            size(bounds.right() - hole.right(), hole.size.height),
        ),
    ]
    .into_iter()
    .filter(|part| {
        part.size.width > gpui::px(0.0) && part.size.height > gpui::px(0.0)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_alpha_has_one_owner_through_reload_fade_and_recovery() {
        let owner = Alpha::default();
        let clone = owner.clone();
        owner.configure(0.7);
        let written = Cell::new(1.0);
        clone
            .fade(0.4, |value| {
                written.set(value);
                Ok(())
            })
            .unwrap();
        assert!((written.get() - 0.28).abs() < 1e-9);
        owner.configure(0.5);
        owner
            .apply(|value| {
                written.set(value);
                Ok(())
            })
            .unwrap();
        assert_eq!(owner.0.get().fade, 0.4);
        assert_eq!(written.get(), 0.2);
        clone
            .fade(1.0, |value| {
                written.set(value);
                Ok(())
            })
            .unwrap();
        assert_eq!(written.get(), 0.5);
        owner.configure(1.0);
        owner
            .apply(|value| {
                written.set(value);
                Ok(())
            })
            .unwrap();
        assert_eq!(written.get(), 1.0);
    }

    #[test]
    fn defaults_never_write_native_alpha() {
        Alpha::default()
            .apply(|_| panic!("default alpha must be untouched"))
            .unwrap();
    }

    #[test]
    fn native_alpha_restores_default_after_reload_reenters_a_write() {
        let owner = Alpha::default();
        let written = Cell::new(1.0);
        owner.configure(0.5);
        owner
            .apply(|value| {
                written.set(value);
                // Native setters can reenter GPUI while this write is running.
                owner.configure(1.0);
                Ok(())
            })
            .unwrap();
        assert_eq!(written.get(), 1.0, "reload must restore native alpha");
    }

    #[test]
    fn queued_native_alpha_sync_uses_latest_reload_and_fade_in_either_order() {
        for sync_first in [false, true] {
            let owner = Alpha::default();
            let written = Cell::new(1.0);
            owner.configure(0.5);
            owner
                .apply(|value| {
                    written.set(value);
                    Ok(())
                })
                .unwrap();
            owner.configure(0.7);
            // A sync queued by this reload must read live factors when run.
            let sync = || {
                owner.apply(|value| {
                    written.set(value);
                    Ok(())
                })
            };
            owner.configure(1.0);
            let fade = || {
                owner.fade(0.4, |value| {
                    written.set(value);
                    Ok(())
                })
            };
            if sync_first {
                sync().unwrap();
                fade().unwrap();
            } else {
                fade().unwrap();
                sync().unwrap();
            }
            assert_eq!(written.get(), 0.4);
            owner
                .fade(1.0, |value| {
                    written.set(value);
                    Ok(())
                })
                .unwrap();
            sync().unwrap();
            assert_eq!(written.get(), 1.0);
        }
    }

    #[test]
    fn appearance_and_effective_alpha_follow_platform_support() {
        use WindowBackgroundAppearance::{Blurred, Opaque, Transparent};
        assert!(Support::Unavailable.note().is_some());
        for platform in [Platform::MacOs, Platform::Linux] {
            assert_eq!(
                appearance(platform, false, Support::Available, 1.0, true),
                Opaque
            );
            assert_eq!(
                appearance(platform, false, Support::Available, 0.6, false),
                Transparent
            );
            assert_eq!(
                appearance(platform, true, Support::NoCompositor, 0.6, true),
                Transparent
            );
            assert_eq!(
                appearance(platform, false, Support::NoArgbVisual, 0.6, false),
                Opaque
            );
        }
        assert_eq!(
            appearance(Platform::MacOs, false, Support::Available, 0.6, true),
            Blurred
        );
        assert_eq!(
            appearance(Platform::Linux, false, Support::Available, 0.6, true),
            Transparent
        );
        let config = WindowConfig {
            opacity: 0.7,
            background_opacity: 0.6,
            foreground_opacity: 0.8,
            ..WindowConfig::default()
        };
        let (bg, fg) = Support::Available.paint(config);
        assert!((config.opacity * bg - 0.42).abs() < 1e-6);
        assert!((config.opacity * fg - 0.56).abs() < 1e-6);
        assert_eq!(Support::NoCompositor.paint(config), (1.0, 1.0));
    }

    #[test]
    fn dimming_preserves_composed_alpha() {
        let source = gpui::white().opacity(0.6);
        let result = Tint {
            dim: 0.2,
            darken: 0.3,
        }
        .apply(source, gpui::black());
        assert_eq!(result.a, source.a);
        let rgb: gpui::Rgba = result.into();
        assert!((rgb.r - 0.56).abs() < 1e-6);
        assert_eq!(Tint::default().apply(source, gpui::black()), source);
    }
    #[test]
    fn strip_tab_and_merged_title_regions_have_exactly_one_background_painter()
    {
        use gpui::{Bounds, point, px, size};
        let row =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(32.0)));
        let bar =
            Bounds::new(point(px(10.0), px(0.0)), size(px(80.0), px(32.0)));
        let active =
            Bounds::new(point(px(20.0), px(0.0)), size(px(30.0), px(32.0)));
        let mut fills = background_regions(row, Some(bar));
        fills.extend(background_regions(bar, Some(active)));
        fills.push(active);
        for x in 0_u16..100 {
            let sample = point(px(f32::from(x) + 0.5), px(15.0));
            assert_eq!(
                fills
                    .iter()
                    .filter(|region| region.contains(&sample))
                    .count(),
                1,
                "one painter at {x}"
            );
        }
        assert_eq!(background_regions(row, None), [row]);
    }
}
