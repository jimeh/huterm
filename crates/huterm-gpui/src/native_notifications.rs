//! Whether the platform can show Huterm's system notifications right now.
//!
//! GPUI posts notifications, but drops one silently when the platform
//! cannot show it: macOS without permission or outside an app bundle, and
//! Linux without a notification daemon. This module asks the platform in
//! the background and caches the answer, so a terminal notification that
//! would be lost is presented inside Huterm instead.

use std::sync::atomic::{AtomicU8, Ordering};

/// What the platform last said about showing notifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Availability {
    /// Not asked yet, or the answer has not arrived.
    Unknown,
    /// Notifications will show.
    Available,
    /// macOS has not asked the user yet; the first notification asks.
    Undecided,
    /// Notifications will not show: permission was denied, Huterm runs
    /// outside an app bundle, or no notification daemon answered.
    Unavailable,
}

impl Availability {
    const fn to_bits(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Available => 1,
            Self::Undecided => 2,
            Self::Unavailable => 3,
        }
    }

    const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Available,
            2 => Self::Undecided,
            3 => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}

static AVAILABILITY: AtomicU8 = AtomicU8::new(0);

/// The cached answer.
pub(crate) fn availability() -> Availability {
    Availability::from_bits(AVAILABILITY.load(Ordering::Acquire))
}

fn store(availability: Availability) {
    AVAILABILITY.store(availability.to_bits(), Ordering::Release);
}

/// Asks the platform again without blocking; the answer replaces the cache
/// when it arrives. Call it at startup, when Huterm becomes active (the user
/// may have changed permission in the meantime), and after posting.
pub(crate) fn refresh() {
    platform::refresh();
}

#[cfg(target_os = "macos")]
mod platform {
    #![expect(
        unsafe_code,
        reason = "the completion handler receives a raw settings pointer"
    )]

    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_foundation::NSBundle;
    use objc2_user_notifications::{
        UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
    };

    use super::{Availability, store};

    pub(super) fn refresh() {
        // `UNUserNotificationCenter` aborts the process outside an app
        // bundle, as development builds run; GPUI uses the same guard.
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            store(Availability::Unavailable);
            return;
        }
        let completion =
            RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
                // SAFETY: UNUserNotificationCenter.h passes a non-null
                // settings object that is valid for the handler's call.
                let settings = unsafe { settings.as_ref() };
                store(match settings.authorizationStatus() {
                    UNAuthorizationStatus::NotDetermined => {
                        Availability::Undecided
                    }
                    UNAuthorizationStatus::Denied => Availability::Unavailable,
                    _ => Availability::Available,
                });
            });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&completion);
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Availability, store};

    pub(super) fn refresh() {
        // The D-Bus call blocks for up to its timeout.
        let spawned = std::thread::Builder::new()
            .name("notification-probe".into())
            .spawn(|| {
                store(match notify_rust::get_server_information() {
                    Ok(_) => Availability::Available,
                    Err(_) => Availability::Unavailable,
                });
            });
        if spawned.is_err() {
            store(Availability::Unknown);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub(super) fn refresh() {
        super::store(super::Availability::Unavailable);
    }
}
