//! Sparkle integration for the packaged macOS application.

// Objective-C runtime integration is confined to this module. The rest of the
// desktop keeps the workspace's unsafe-code prohibition.
#![allow(unsafe_code)]
// objc 0.2's message macros refer to an obsolete cargo-clippy feature.
#![allow(unexpected_cfgs)]

use std::cell::Cell;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;

use anyhow::{Context as _, ensure};
use huterm_config::UpdateConfig;
use objc::runtime::{BOOL, Class, NO, Object, YES};
use objc::{msg_send, sel, sel_impl};

use crate::APP_ID;

/// Retained application-lifetime controller for Sparkle's standard interface.
pub(crate) struct Updater {
    state: State,
    manual_checks: Cell<u32>,
    _main_thread_only: PhantomData<Rc<()>>,
}

enum State {
    Ready {
        controller: NonNull<Object>,
        updater: NonNull<Object>,
    },
    Unavailable(String),
}

impl Updater {
    /// Creates and starts Sparkle only for the packaged production application.
    pub(crate) fn initialize(config: UpdateConfig) -> Self {
        let state = initialize(config)
            .unwrap_or_else(|error| State::Unavailable(format!("{error:#}")));
        Self {
            state,
            manual_checks: Cell::new(0),
            _main_thread_only: PhantomData,
        }
    }

    /// Opens or focuses Sparkle's standard update interface.
    pub(crate) fn check_for_updates(&self) -> Result<(), String> {
        let State::Ready { controller, .. } = &self.state else {
            let State::Unavailable(reason) = &self.state else {
                unreachable!();
            };
            return Err(reason.clone());
        };
        ensure_main_thread().map_err(|error| format!("{error:#}"))?;
        // SAFETY: The retained standard controller's action starts a new check
        // or brings its current update session forward. Sparkle requires this
        // selector on AppKit's main thread, enforced above.
        unsafe {
            let nil = std::ptr::null_mut::<Object>();
            let _: () = msg_send![controller.as_ptr(), checkForUpdates: nil];
        }
        self.manual_checks
            .set(self.manual_checks.get().saturating_add(1));
        Ok(())
    }

    pub(crate) fn can_check_for_updates(&self) -> Result<bool, String> {
        let State::Ready { updater, .. } = &self.state else {
            let State::Unavailable(reason) = &self.state else {
                unreachable!();
            };
            return Err(reason.clone());
        };
        ensure_main_thread().map_err(|error| format!("{error:#}"))?;
        // SAFETY: Initialization retains this SPUUpdater and this method runs
        // on AppKit's main thread as Sparkle requires.
        let can_check: BOOL =
            unsafe { msg_send![updater.as_ptr(), canCheckForUpdates] };
        Ok(can_check == YES)
    }

    pub(crate) fn startup_error(&self) -> Option<&str> {
        match &self.state {
            State::Ready { .. } => None,
            State::Unavailable(reason) => Some(reason),
        }
    }

    pub(crate) fn framework_bundle_path(&self) -> Result<String, String> {
        let State::Ready { .. } = &self.state else {
            let State::Unavailable(reason) = &self.state else {
                unreachable!();
            };
            return Err(reason.clone());
        };
        framework_bundle_path().map_err(|error| format!("{error:#}"))
    }

    pub(crate) fn automatic_checks_preference(
        &self,
    ) -> Result<Option<bool>, String> {
        let State::Ready { .. } = &self.state else {
            let State::Unavailable(reason) = &self.state else {
                unreachable!();
            };
            return Err(reason.clone());
        };
        ensure_main_thread().map_err(|error| format!("{error:#}"))?;
        user_default_bool(c"SUEnableAutomaticChecks")
            .map_err(|error| format!("{error:#}"))
    }

    pub(crate) fn manual_check_count(&self) -> u32 {
        self.manual_checks.get()
    }

    /// Applies only explicit config overrides, leaving absent fields untouched.
    pub(crate) fn apply_config(
        &self,
        config: UpdateConfig,
    ) -> Result<(), String> {
        let State::Ready { updater, .. } = &self.state else {
            return Ok(());
        };
        ensure_main_thread().map_err(|error| format!("{error:#}"))?;
        apply_config(updater.as_ptr(), config);
        Ok(())
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        let State::Ready { controller, .. } = &self.state else {
            return;
        };
        // SAFETY: Balances the controller's alloc/init retain on the same
        // AppKit thread. The controller owns the updater for its lifetime.
        unsafe {
            let _: () = msg_send![controller.as_ptr(), release];
        }
    }
}

fn initialize(config: UpdateConfig) -> anyhow::Result<State> {
    ensure_main_thread()?;
    ensure!(
        is_packaged_application()?,
        "self-updates are available only in the packaged Huterm application"
    );
    let controller_class = Class::get("SPUStandardUpdaterController")
        .context("the packaged Sparkle framework is unavailable")?;
    let nil = std::ptr::null_mut::<Object>();
    // SAFETY: The class and initializer are part of the pinned Sparkle API.
    // Both delegates are nullable and the controller remains retained until
    // Updater is dropped on the main thread.
    let controller: *mut Object = unsafe {
        let allocated: *mut Object = msg_send![controller_class, alloc];
        msg_send![allocated,
            initWithStartingUpdater: NO
            updaterDelegate: nil
            userDriverDelegate: nil]
    };
    let Some(controller) = NonNull::new(controller) else {
        anyhow::bail!("Sparkle returned a null updater controller");
    };
    // SAFETY: The initialized controller owns and returns a non-null SPUUpdater.
    let updater: *mut Object =
        unsafe { msg_send![controller.as_ptr(), updater] };
    let Some(updater) = NonNull::new(updater) else {
        unsafe {
            let _: () = msg_send![controller.as_ptr(), release];
        }
        anyhow::bail!("Sparkle returned a null updater");
    };
    let mut error = std::ptr::null_mut::<Object>();
    // SAFETY: startUpdater: is the pinned main-thread API. It writes a borrowed
    // NSError pointer only when returning NO.
    let started: BOOL =
        unsafe { msg_send![updater.as_ptr(), startUpdater: &mut error] };
    if started == NO {
        let description = native_error(error)
            .unwrap_or_else(|_| "unknown Sparkle startup error".to_owned());
        unsafe {
            let _: () = msg_send![controller.as_ptr(), release];
        }
        anyhow::bail!("Sparkle failed to start: {description}");
    }
    // Sparkle schedules its first cycle on the next run-loop iteration, so
    // explicit settings take effect first. Omitted settings never call setters.
    apply_config(updater.as_ptr(), config);
    Ok(State::Ready {
        controller,
        updater,
    })
}

fn apply_config(updater: *mut Object, config: UpdateConfig) {
    let overrides = overrides(config);
    // SAFETY: Callers retain updater and enforce Sparkle's main-thread rule.
    unsafe {
        if let Some(enabled) = overrides.automatic_checks {
            let desired = if enabled { YES } else { NO };
            let _: () =
                msg_send![updater, setAutomaticallyChecksForUpdates: desired];
        }
        if let Some(desired) = overrides.check_interval_seconds {
            let current: f64 = msg_send![updater, updateCheckInterval];
            if (current - desired).abs() >= 0.5 {
                let _: () = msg_send![updater, setUpdateCheckInterval: desired];
            }
        }
    }
}

#[derive(Debug, PartialEq)]
struct UpdateOverrides {
    automatic_checks: Option<bool>,
    check_interval_seconds: Option<f64>,
}

fn overrides(config: UpdateConfig) -> UpdateOverrides {
    UpdateOverrides {
        automatic_checks: config.automatic_checks,
        check_interval_seconds: config
            .check_interval_hours
            .map(|hours| f64::from(hours) * 60.0 * 60.0),
    }
}

fn is_packaged_application() -> anyhow::Result<bool> {
    let class = Class::get("NSBundle").context("Foundation is unavailable")?;
    // SAFETY: NSBundle owns its process-wide main bundle and returned strings.
    unsafe {
        let bundle: *mut Object = msg_send![class, mainBundle];
        ensure!(!bundle.is_null(), "NSBundle returned a null main bundle");
        let identifier: *mut Object = msg_send![bundle, bundleIdentifier];
        let bundle_path: *mut Object = msg_send![bundle, bundlePath];
        Ok(native_string(identifier).is_ok_and(|value| value == APP_ID)
            && native_string(bundle_path).is_ok_and(|path| {
                Path::new(&path).extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("app")
                })
            }))
    }
}

fn framework_bundle_path() -> anyhow::Result<String> {
    ensure_main_thread()?;
    let sparkle = Class::get("SPUStandardUpdaterController")
        .context("the packaged Sparkle framework is unavailable")?;
    let bundle_class =
        Class::get("NSBundle").context("Foundation is unavailable")?;
    // SAFETY: bundleForClass: and bundlePath return autoreleased objects whose
    // lifetime covers conversion to an owned Rust string on this thread.
    unsafe {
        let bundle: *mut Object =
            msg_send![bundle_class, bundleForClass: sparkle];
        ensure!(!bundle.is_null(), "Sparkle has no owning bundle");
        let bundle_path: *mut Object = msg_send![bundle, bundlePath];
        native_string(bundle_path)
    }
}

fn user_default_bool(key: &CStr) -> anyhow::Result<Option<bool>> {
    let defaults_class =
        Class::get("NSUserDefaults").context("Foundation is unavailable")?;
    let string_class =
        Class::get("NSString").context("Foundation is unavailable")?;
    // SAFETY: These Foundation objects are process-owned or autoreleased and
    // remain valid while the value is read synchronously on the main thread.
    unsafe {
        let defaults: *mut Object =
            msg_send![defaults_class, standardUserDefaults];
        ensure!(
            !defaults.is_null(),
            "NSUserDefaults returned a null standard defaults object"
        );
        let key: *mut Object =
            msg_send![string_class, stringWithUTF8String: key.as_ptr()];
        ensure!(!key.is_null(), "NSString rejected a defaults key");
        let value: *mut Object = msg_send![defaults, objectForKey: key];
        if value.is_null() {
            Ok(None)
        } else {
            let enabled: BOOL = msg_send![value, boolValue];
            Ok(Some(enabled == YES))
        }
    }
}

fn ensure_main_thread() -> anyhow::Result<()> {
    let class = Class::get("NSThread").context("Foundation is unavailable")?;
    // SAFETY: isMainThread is a parameterless class method available from any
    // thread and returns Objective-C BOOL.
    let main: BOOL = unsafe { msg_send![class, isMainThread] };
    ensure!(
        main == YES,
        "Sparkle operations require the AppKit main thread"
    );
    Ok(())
}

fn native_error(error: *mut Object) -> anyhow::Result<String> {
    ensure!(!error.is_null(), "Sparkle returned no NSError");
    // SAFETY: NSError owns its localizedDescription for this call.
    let description: *mut Object =
        unsafe { msg_send![error, localizedDescription] };
    native_string(description)
}

fn native_string(value: *mut Object) -> anyhow::Result<String> {
    ensure!(!value.is_null(), "Objective-C returned a null string");
    // SAFETY: NSString returns a NUL-terminated pointer valid for its lifetime.
    let bytes: *const std::ffi::c_char =
        unsafe { msg_send![value, UTF8String] };
    ensure!(!bytes.is_null(), "NSString returned null UTF-8 bytes");
    Ok(unsafe { CStr::from_ptr(bytes) }
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_interval_converts_whole_hours_to_seconds() {
        assert_eq!(
            overrides(UpdateConfig {
                automatic_checks: None,
                check_interval_hours: Some(1),
            }),
            UpdateOverrides {
                automatic_checks: None,
                check_interval_seconds: Some(3_600.0),
            }
        );
        assert_eq!(
            overrides(UpdateConfig {
                automatic_checks: None,
                check_interval_hours: Some(24),
            })
            .check_interval_seconds,
            Some(86_400.0)
        );
    }

    #[test]
    fn absent_settings_produce_no_sparkle_setters() {
        assert_eq!(
            overrides(UpdateConfig::default()),
            UpdateOverrides {
                automatic_checks: None,
                check_interval_seconds: None,
            }
        );
    }

    #[test]
    fn interval_does_not_enable_automatic_checks() {
        assert_eq!(
            overrides(UpdateConfig {
                automatic_checks: None,
                check_interval_hours: Some(8),
            }),
            UpdateOverrides {
                automatic_checks: None,
                check_interval_seconds: Some(28_800.0),
            }
        );
    }

    #[test]
    fn explicit_automatic_check_values_remain_distinct() {
        for enabled in [false, true] {
            assert_eq!(
                overrides(UpdateConfig {
                    automatic_checks: Some(enabled),
                    check_interval_hours: None,
                })
                .automatic_checks,
                Some(enabled)
            );
        }
    }
}
