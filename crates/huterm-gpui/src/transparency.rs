//! Paint policy, the layered transparency values each window resolves, the
//! single native-alpha owner shared with Quake fades, and the native blur
//! owner.
use std::{cell::Cell, cell::RefCell, rc::Rc};

use gpui::{Context, Hsla, Task, Window, WindowBackgroundAppearance};
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
}

pub(crate) fn support() -> Support {
    #[cfg(target_os = "linux")]
    return crate::quake::native::transparency_support()
        .unwrap_or(Support::Unavailable);
    #[cfg(not(target_os = "linux"))]
    Support::Available
}

/// One of the four values a window's transparency layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Opacity,
    Background,
    Foreground,
    Blur,
}
impl Key {
    pub(crate) const ALL: [Self; 4] = [
        Self::Opacity,
        Self::Background,
        Self::Foreground,
        Self::Blur,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Opacity => "Opacity",
            Self::Background => "Background opacity",
            Self::Foreground => "Text opacity",
            Self::Blur => "Blur",
        }
    }

    /// The `[window]` key, which a Quake profile's key shares.
    pub(crate) fn config_key(self) -> &'static str {
        match self {
            Self::Opacity => "opacity",
            Self::Background => "background_opacity",
            Self::Foreground => "foreground_opacity",
            Self::Blur => "blur",
        }
    }
}

/// The four transparency values one window uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Values {
    pub(crate) opacity: f32,
    pub(crate) background: f32,
    pub(crate) foreground: f32,
    pub(crate) blur: u8,
}
impl Values {
    pub(crate) fn of(config: &WindowConfig) -> Self {
        Self {
            opacity: config.opacity,
            background: config.background_opacity,
            foreground: config.foreground_opacity,
            blur: config.blur,
        }
    }

    pub(crate) fn get(self, key: Key) -> f64 {
        match key {
            Key::Opacity => f64::from(self.opacity),
            Key::Background => f64::from(self.background),
            Key::Foreground => f64::from(self.foreground),
            Key::Blur => f64::from(self.blur),
        }
    }
}

/// Values that override the layer beneath, each on its own: a Quake
/// profile's keys over the global defaults, and a window's overrides over
/// both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Layer {
    pub(crate) opacity: Option<f32>,
    pub(crate) background: Option<f32>,
    pub(crate) foreground: Option<f32>,
    pub(crate) blur: Option<u8>,
}
impl Layer {
    pub(crate) fn of_profile(profile: &huterm_config::quake::Profile) -> Self {
        Self {
            opacity: profile.opacity,
            background: profile.background_opacity,
            foreground: profile.foreground_opacity,
            blur: profile.blur,
        }
    }

    pub(crate) fn over(self, values: Values) -> Values {
        Values {
            opacity: self.opacity.unwrap_or(values.opacity),
            background: self.background.unwrap_or(values.background),
            foreground: self.foreground.unwrap_or(values.foreground),
            blur: self.blur.unwrap_or(values.blur),
        }
    }

    pub(crate) fn is_set(self, key: Key) -> bool {
        match key {
            Key::Opacity => self.opacity.is_some(),
            Key::Background => self.background.is_some(),
            Key::Foreground => self.foreground.is_some(),
            Key::Blur => self.blur.is_some(),
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Sets or, with `None`, clears one value. A value is clamped to its
    /// key's range, so an override always resolves.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each value is clamped to its key's small range first"
    )]
    pub(crate) fn set(&mut self, key: Key, value: Option<f64>) {
        let fraction =
            |min: f64| value.map(|value| value.clamp(min, 1.0) as f32);
        match key {
            Key::Opacity => self.opacity = fraction(0.1),
            Key::Background => self.background = fraction(0.0),
            Key::Foreground => self.foreground = fraction(0.1),
            Key::Blur => {
                self.blur = value.map(|value| {
                    value.round().clamp(0.0, f64::from(huterm_config::MAX_BLUR))
                        as u8
                });
            }
        }
    }
}

/// Where a window's effective value for one key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// `[window]` in the configuration.
    Default,
    /// The window's Quake profile.
    Profile,
    /// The window's own override.
    Window,
}
impl Source {
    pub(crate) fn of(key: Key, profile: Layer, window: Layer) -> Self {
        if window.is_set(key) {
            Self::Window
        } else if profile.is_set(key) {
            Self::Profile
        } else {
            Self::Default
        }
    }
}

/// What a window applies for its resolved values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Effective {
    /// The configured factor of the native window alpha, which a Quake fade
    /// multiplies.
    pub(crate) native_alpha: f64,
    /// The opacity GPUI applies to the window's root element, below 1 only
    /// while blurring.
    pub(crate) root_opacity: f32,
    /// The paint alpha of default backgrounds and bars.
    pub(crate) background: f32,
    pub(crate) foreground: f32,
    /// Whether anything shows through, so structural underlays must not
    /// stack translucent fills.
    pub(crate) translucent: bool,
    /// The window server's background blur radius, 0 for none.
    pub(crate) blur_radius: u8,
    pub(crate) appearance: WindowBackgroundAppearance,
}
impl Default for Effective {
    fn default() -> Self {
        resolve(
            Values::of(&WindowConfig::default()),
            Support::Available,
            Platform::current(),
            false,
        )
    }
}

/// Resolves a window's values against what the platform supports. Blur
/// mode, on macOS with a radius and any translucency, keeps native alpha at
/// 1 and fades the root element instead, because the window server's blur
/// fades with native alpha.
pub(crate) fn resolve(
    values: Values,
    support: Support,
    platform: Platform,
    decorated: bool,
) -> Effective {
    let opaque_surface = if decorated {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    };
    if support != Support::Available {
        return Effective {
            native_alpha: 1.0,
            root_opacity: 1.0,
            background: 1.0,
            foreground: 1.0,
            translucent: false,
            blur_radius: 0,
            appearance: opaque_surface,
        };
    }
    let blur = platform == Platform::MacOs
        && values.blur > 0
        && (values.opacity < 1.0 || values.background < 1.0);
    if blur {
        return Effective {
            native_alpha: 1.0,
            root_opacity: values.opacity,
            background: values.background,
            foreground: values.foreground,
            translucent: true,
            blur_radius: values.blur,
            appearance: WindowBackgroundAppearance::Transparent,
        };
    }
    Effective {
        native_alpha: f64::from(values.opacity),
        root_opacity: 1.0,
        background: values.background,
        foreground: values.foreground,
        translucent: values.background < 1.0,
        blur_radius: 0,
        // Overall opacity never changes the surface's appearance.
        appearance: if values.background >= 1.0 {
            opaque_surface
        } else {
            WindowBackgroundAppearance::Transparent
        },
    }
}

/// The opacity a window's root element paints. It follows the native alpha
/// writes, so a switch into or out of blur mode never shows both factors
/// (`opacity` squared) or a frame of opacity 1: entering, native alpha goes
/// to 1 first; leaving, the root opacity drops to 1 as native alpha takes
/// the opacity back. Without a pending native write both apply at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RootOpacity {
    resolved: f32,
    painted: f32,
}
impl Default for RootOpacity {
    fn default() -> Self {
        Self {
            resolved: 1.0,
            painted: 1.0,
        }
    }
}
impl RootOpacity {
    /// Takes the resolved root opacity. It paints at once unless a native
    /// alpha write is still pending. Returns whether the painted value
    /// changed.
    pub(crate) fn resolve(
        &mut self,
        resolved: f32,
        native_pending: bool,
    ) -> bool {
        self.resolved = resolved;
        if native_pending {
            return false;
        }
        self.landed()
    }

    /// The native alpha write landed. Returns whether the painted value
    /// changed.
    pub(crate) fn landed(&mut self) -> bool {
        let changed = self.painted.to_bits() != self.resolved.to_bits();
        self.painted = self.resolved;
        changed
    }

    pub(crate) fn painted(self) -> f32 {
        self.painted
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
    /// Without transparency support only the configured factor drops out:
    /// a Quake fade needs a compositor, not an ARGB visual.
    fn effective(self) -> f64 {
        if self.supported {
            self.configured * self.fade
        } else {
            self.fade
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
    /// Whether a native write is still owed.
    pub(crate) fn pending(&self) -> bool {
        let factors = self.0.get();
        factors.effective().to_bits() != factors.applied.to_bits()
    }

    /// Writes the native alpha on a later foreground turn, then runs
    /// `landed` on the view. Returns whether a write was scheduled; without
    /// one, as when the window has no native handle, nothing waits on it.
    pub(crate) fn sync<V: 'static>(
        &self,
        window: &Window,
        cx: &mut Context<'_, V>,
        landed: impl FnOnce(&mut V, &mut Context<'_, V>) + 'static,
    ) -> bool {
        if !self.pending() {
            return false;
        }
        #[cfg(target_os = "macos")]
        let native = crate::native_appearance::macos::NativeWindow::of(window);
        #[cfg(target_os = "linux")]
        let native = crate::quake::native::Platform::new(cx)
            .and_then(|platform| platform.window(window));
        match native {
            Ok(native) => {
                let owner = self.clone();
                cx.spawn(async move |view, cx| {
                    if let Err(error) =
                        owner.apply(|value| native.opacity(value))
                    {
                        eprintln!("Window opacity: {error}");
                    }
                    let _ = view.update(cx, landed);
                })
                .detach();
                true
            }
            Err(error) => {
                eprintln!("Window opacity: {error}");
                false
            }
        }
    }
}

/// The window server's background blur radius for one window. It writes on
/// a later foreground turn, outside GPUI update borrows, reading the live
/// desired radius when it runs. Only a successful write records the
/// applied radius. A failed write leaves it unknown and waits for the next
/// change, reload, or style change to retry once. It never writes 0 to a
/// window that never blurred or already has 0, and closing the window drops
/// pending work.
#[derive(Clone, Default)]
pub(crate) struct Blur(Rc<RefCell<BlurState>>);
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact about the window's blur"
)]
struct BlurState {
    desired: u8,
    /// `None` while unknown: before the first write, after a failure, and
    /// after a style change.
    applied: Option<u8>,
    blurred: bool,
    failed: bool,
    logged: bool,
    closed: bool,
    scheduled: bool,
    /// The latest scheduled write; dropping it cancels the write.
    task: Option<Task<()>>,
}
impl BlurState {
    fn wants_write(&self) -> bool {
        !self.failed
            && !self.closed
            && match self.applied {
                Some(applied) => applied != self.desired,
                None => self.desired > 0 || self.blurred,
            }
    }
}
impl Blur {
    pub(crate) fn set_desired(&self, radius: u8) {
        let mut state = self.0.borrow_mut();
        if state.desired != radius {
            state.desired = radius;
            state.failed = false;
        }
    }

    /// A reload lets a failed write try again once.
    pub(crate) fn retry(&self) {
        self.0.borrow_mut().failed = false;
    }

    /// A Quake style change, a conversion, or native fullscreen may have
    /// replaced the window server's state, so the next sync writes again.
    pub(crate) fn invalidate(&self) {
        let mut state = self.0.borrow_mut();
        state.applied = None;
        state.failed = false;
    }

    pub(crate) fn close(&self) {
        let mut state = self.0.borrow_mut();
        state.closed = true;
        state.scheduled = false;
        state.task = None;
    }

    /// Whether the next sync has a radius to write.
    pub(crate) fn wants_write(&self) -> bool {
        let state = self.0.borrow();
        !state.scheduled && state.wants_write()
    }

    pub(crate) fn applied(&self) -> Option<u8> {
        self.0.borrow().applied
    }

    pub(crate) fn sync<V: 'static>(
        &self,
        window: &Window,
        cx: &mut Context<'_, V>,
    ) {
        {
            let state = self.0.borrow();
            if state.scheduled || !state.wants_write() {
                return;
            }
        }
        match blur_writer(window) {
            Ok(write) => {
                let state = Rc::downgrade(&self.0);
                let task = cx.spawn(async move |_, _| {
                    if let Some(state) = state.upgrade() {
                        state.borrow_mut().scheduled = false;
                        Self(state).apply(write);
                    }
                });
                let mut state = self.0.borrow_mut();
                state.scheduled = true;
                state.task = Some(task);
            }
            Err(error) => self.failed(&error),
        }
    }

    fn failed(&self, error: &anyhow::Error) {
        let mut state = self.0.borrow_mut();
        state.applied = None;
        state.failed = true;
        if !std::mem::replace(&mut state.logged, true) {
            eprintln!("Window blur: {error:#}");
        }
    }

    /// Writes the desired radius until it holds, outside any borrow of the
    /// state, since a native setter may reenter.
    fn apply(&self, mut write: impl FnMut(u8) -> anyhow::Result<()>) {
        loop {
            let radius = {
                let state = self.0.borrow();
                if !state.wants_write() {
                    return;
                }
                state.desired
            };
            if let Err(error) = write(radius) {
                self.failed(&error);
                return;
            }
            let mut state = self.0.borrow_mut();
            state.applied = Some(radius);
            state.blurred |= radius > 0;
        }
    }
}

/// The writer of `window`'s window server blur radius.
#[cfg(target_os = "macos")]
fn blur_writer(
    window: &Window,
) -> anyhow::Result<impl FnMut(u8) -> anyhow::Result<()> + 'static> {
    let native = crate::native_appearance::macos::NativeWindow::of(window)?;
    Ok(move |radius| native.set_blur_radius(radius))
}

/// Linux has no window server blur, and `resolve` never asks it for a
/// radius, so no write reaches this.
#[cfg(not(target_os = "macos"))]
fn blur_writer(_: &Window) -> anyhow::Result<fn(u8) -> anyhow::Result<()>> {
    anyhow::bail!("this platform has no window server blur")
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
    fn unsupported_transparency_keeps_the_quake_fade() {
        let owner = Alpha::default();
        let written = Cell::new(1.0);
        owner.set_supported(false);
        owner.configure(0.7);
        for fade in [0.4, 1.0] {
            owner
                .fade(fade, |value| {
                    written.set(value);
                    Ok(())
                })
                .unwrap();
            assert_eq!(written.get(), fade, "configured opacity stays at 1");
        }
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

    fn values(opacity: f32, background: f32, blur: u8) -> Values {
        Values {
            opacity,
            background,
            foreground: 0.8,
            blur,
        }
    }

    #[test]
    fn resolve_switches_blur_mode_on_translucency_and_a_radius() {
        use WindowBackgroundAppearance::{Opaque, Transparent};
        assert!(Support::Unavailable.note().is_some());
        let mac = |values, decorated| {
            resolve(values, Support::Available, Platform::MacOs, decorated)
        };
        // Either translucency blurs with a radius, with native alpha at 1
        // and the opacity on the root element.
        for (opacity, background) in [(0.6, 1.0), (1.0, 0.6), (0.6, 0.6)] {
            let effective = mac(values(opacity, background, 20), false);
            assert_eq!(
                effective,
                Effective {
                    native_alpha: 1.0,
                    root_opacity: opacity,
                    background,
                    foreground: 0.8,
                    translucent: true,
                    blur_radius: 20,
                    appearance: Transparent,
                },
                "opacity {opacity}, background {background}"
            );
        }
        // An opaque window never blurs, whatever the radius.
        let opaque = mac(values(1.0, 1.0, 20), false);
        assert_eq!(
            (opaque.blur_radius, opaque.translucent, opaque.appearance),
            (0, false, Opaque)
        );
        // Without a radius, opacity stays native alpha, as before blur.
        let faded = mac(values(0.6, 1.0, 0), false);
        assert_eq!(
            (
                faded.native_alpha,
                faded.root_opacity,
                faded.translucent,
                faded.blur_radius,
                faded.appearance
            ),
            ((0.6_f32).into(), 1.0, false, 0, Opaque)
        );
        assert_eq!(mac(values(0.6, 1.0, 0), true).appearance, Transparent);
        let background = mac(values(1.0, 0.6, 0), false);
        assert_eq!(
            (background.translucent, background.appearance),
            (true, Transparent)
        );
        // Linux never blurs: opacity stays native alpha.
        let linux = resolve(
            values(0.6, 0.6, 20),
            Support::Available,
            Platform::Linux,
            false,
        );
        assert_eq!(
            (
                linux.native_alpha,
                linux.root_opacity,
                linux.blur_radius,
                linux.translucent
            ),
            ((0.6_f32).into(), 1.0, 0, true)
        );
        // Without support every window paints opaque and never blurs.
        for platform in [Platform::MacOs, Platform::Linux] {
            for (support, decorated, appearance) in [
                (Support::NoCompositor, true, Transparent),
                (Support::NoArgbVisual, false, Opaque),
            ] {
                assert_eq!(
                    resolve(values(0.6, 0.6, 20), support, platform, decorated),
                    Effective {
                        native_alpha: 1.0,
                        root_opacity: 1.0,
                        background: 1.0,
                        foreground: 1.0,
                        translucent: false,
                        blur_radius: 0,
                        appearance,
                    }
                );
            }
        }
    }

    #[test]
    fn layers_override_each_value_on_its_own_and_report_their_source() {
        let defaults = values(0.6, 0.6, 0);
        let profile = Layer {
            opacity: Some(0.8),
            blur: Some(10),
            ..Layer::default()
        };
        let mut window = Layer::default();
        window.set(Key::Opacity, Some(0.5));
        window.set(Key::Background, Some(0.3));
        let resolved = window.over(profile.over(defaults));
        assert_eq!(
            resolved,
            Values {
                opacity: 0.5,
                background: 0.3,
                foreground: 0.8,
                blur: 10,
            }
        );
        assert_eq!(
            Key::ALL.map(|key| Source::of(key, profile, window)),
            [
                Source::Window,
                Source::Window,
                Source::Default,
                Source::Profile
            ]
        );
        // A cleared override inherits again, and a later change to the
        // layer beneath reaches it.
        window.set(Key::Opacity, None);
        assert_eq!(Source::of(Key::Opacity, profile, window), Source::Profile);
        assert_eq!(window.over(profile.over(defaults)).opacity, 0.8);
        assert_eq!(
            window
                .over(Layer::default().over(values(0.4, 0.6, 0)))
                .opacity,
            0.4
        );
        // Values clamp to their key's range.
        window.set(Key::Opacity, Some(0.0));
        window.set(Key::Blur, Some(80.4));
        assert_eq!((window.opacity, window.blur), (Some(0.1), Some(64)));
        assert!(!window.is_empty());
        assert!(Layer::default().is_empty());
        let file: huterm_config::quake::Profile =
            toml::from_str("opacity = 0.8\nblur = 10").unwrap();
        assert_eq!(Layer::of_profile(&file), profile);
    }

    /// What the screen shows of a window: native alpha times the root
    /// opacity it paints.
    fn visible(alpha: &Alpha, root: RootOpacity) -> f64 {
        alpha.0.get().applied * f64::from(root.painted())
    }

    #[test]
    fn mode_switches_never_paint_opacity_squared_or_a_flash_of_one() {
        for fade in [1.0, 0.5] {
            let alpha = Alpha::default();
            let mut root = RootOpacity::default();
            let write = |alpha: &Alpha| {
                alpha.apply(|_| Ok(())).unwrap();
            };
            alpha.fade(fade, |_| Ok(())).unwrap();
            let expected = 0.6 * fade;
            // Blur off: opacity is native alpha.
            alpha.configure(0.6);
            root.resolve(1.0, alpha.pending());
            write(&alpha);
            root.landed();
            assert!((visible(&alpha, root) - expected).abs() < 1e-6);
            // Entering blur mode: native alpha goes to 1 first; the root
            // takes the opacity once that write lands.
            alpha.configure(1.0);
            root.resolve(0.6, alpha.pending());
            assert!(
                (visible(&alpha, root) - expected).abs() < 1e-6,
                "before the write, fade {fade}"
            );
            write(&alpha);
            root.landed();
            assert!(
                (visible(&alpha, root) - expected).abs() < 1e-6,
                "after entering, fade {fade}"
            );
            // Leaving: the root drops to 1 as native alpha takes it back.
            alpha.configure(0.6);
            root.resolve(1.0, alpha.pending());
            assert!(
                (visible(&alpha, root) - expected).abs() < 1e-6,
                "before leaving, fade {fade}"
            );
            write(&alpha);
            root.landed();
            assert!(
                (visible(&alpha, root) - expected).abs() < 1e-6,
                "after leaving, fade {fade}"
            );
            // A change within blur mode writes nothing native and paints
            // at once.
            alpha.configure(1.0);
            write(&alpha);
            root.resolve(0.6, alpha.pending());
            root.landed();
            assert!(!alpha.pending());
            assert!(root.resolve(0.5, false));
            assert_eq!(root.painted(), 0.5);
        }
    }

    #[test]
    fn blur_mode_fades_quake_from_a_configured_one_and_keeps_the_fade() {
        let alpha = Alpha::default();
        let written = Cell::new(1.0);
        let write = |value| {
            written.set(value);
            Ok(())
        };
        alpha.configure(0.8);
        alpha.fade(0.5, write).unwrap();
        assert!((written.get() - 0.4).abs() < 1e-9);
        // A switch to blur mode mid-fade keeps the fade's progress.
        alpha.configure(1.0);
        alpha.apply(write).unwrap();
        assert_eq!(written.get(), 0.5);
        alpha.fade(1.0, write).unwrap();
        assert_eq!(written.get(), 1.0);
        // And back: the fade ends at the effective opacity.
        alpha.fade(0.25, write).unwrap();
        alpha.configure(0.8);
        alpha.apply(write).unwrap();
        assert!((written.get() - 0.2).abs() < 1e-9);
        alpha.fade(1.0, write).unwrap();
        assert!((written.get() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn blur_owner_writes_changes_once_and_recovers_from_failure_on_request() {
        let writes = RefCell::new(Vec::new());
        let ok = |radius| {
            writes.borrow_mut().push(radius);
            Ok(())
        };
        let blur = Blur::default();
        // A window that never blurred is never written 0.
        blur.apply(ok);
        blur.set_desired(0);
        blur.apply(ok);
        blur.invalidate();
        blur.apply(ok);
        assert!(writes.borrow().is_empty());
        blur.set_desired(20);
        blur.apply(ok);
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20]);
        assert_eq!(blur.applied(), Some(20));
        // A style change makes the radius unknown, so it is written again.
        blur.invalidate();
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20, 20]);
        // Off writes 0 to a window that blurred, once.
        blur.set_desired(0);
        blur.apply(ok);
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20, 20, 0]);
        // A failure leaves the radius unknown and waits for a request.
        let failing = |_| Err(anyhow::anyhow!("refused"));
        blur.set_desired(30);
        blur.apply(failing);
        assert_eq!(blur.applied(), None);
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20, 20, 0], "no retry loop");
        blur.retry();
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20, 20, 0, 30]);
        // Closing drops pending work.
        blur.set_desired(40);
        blur.close();
        blur.apply(ok);
        assert_eq!(*writes.borrow(), [20, 20, 0, 30]);
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
