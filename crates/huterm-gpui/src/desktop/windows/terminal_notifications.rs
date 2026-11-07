//! Presents notifications that programs request (OSC 9 and OSC 777).
//!
//! Core delivers each notification to one window, the most recently used
//! one that shows its terminal. That window decides how it appears: a
//! system notification while Huterm is in the background, and a notice
//! while Huterm is in front, which offers Show Tab when the terminal is not
//! the one in front. When system notifications cannot show, the notice
//! stands in for them and the window asks for attention. A program's text
//! is only ever shown as plain text; the one action is Huterm's own.

use std::time::Duration;

use gpui::{SharedString, SystemNotification, WindowHandle};
use huterm_core::{NotificationLimiter, PendingHostEffect};
use huterm_protocol::{
    CommandArgument, CommandId, CommandInvocation, CommandValue,
    TerminalNotification,
};

use super::*;
use crate::desktop::notices::NoticeAction;
use crate::native_notifications::{self, Availability};

/// Private notice action that reveals a pane, handled by the window rather
/// than the command catalog.
pub(super) const SHOW_PANE: CommandId = CommandId::new("notice_show_pane");
/// Notifications the whole app presents at once, across terminals, before
/// [`APP_INTERVAL`] applies; each terminal has its own limit in core too.
const APP_BURST: u32 = 8;
const APP_INTERVAL: Duration = Duration::from_secs(1);
/// System notification tags name the pane they reveal when clicked.
const TAG_PREFIX: &str = "huterm-pane-";

/// The app-wide notification state the desktop keeps.
#[derive(Debug)]
pub(super) struct Notifier {
    limiter: NotificationLimiter,
    /// Notices raised for programs' notifications, for smoke state.
    pub(super) notices: u64,
    /// System notifications posted, for smoke state.
    pub(super) posted: u64,
    /// Notifications the app-wide limit dropped.
    pub(super) dropped: u64,
    /// Stands in for the platform's answer, which tests cannot choose.
    #[cfg(test)]
    pub(super) availability: Option<Availability>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self {
            limiter: NotificationLimiter::new(APP_BURST, APP_INTERVAL),
            notices: 0,
            posted: 0,
            dropped: 0,
            #[cfg(test)]
            availability: None,
        }
    }
}

/// How one notification appears.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Presentation {
    /// Post a system notification.
    pub(super) system: bool,
    /// Raise a notice.
    pub(super) notice: Option<NoticeShape>,
}

/// The notice a notification raises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NoticeShape {
    /// Offer Show Tab.
    pub(super) reveal: bool,
    /// Ask for attention too: the notice stands in for a system
    /// notification the user may not see.
    pub(super) attention: bool,
}

/// Chooses how a notification appears. `app_active` is whether one of
/// Huterm's windows is active; `in_front` whether the terminal is drawn
/// where the notice would appear: in the active window while Huterm is in
/// front, and in its own window, where a fallback notice goes, otherwise. An undecided macOS permission still
/// posts, which asks the user, and shows the notice too, since the first
/// notification is lost if the user declines.
pub(super) fn presentation(
    app_active: bool,
    in_front: bool,
    availability: Availability,
) -> Presentation {
    if app_active {
        return Presentation {
            system: false,
            notice: Some(NoticeShape {
                reveal: !in_front,
                attention: false,
            }),
        };
    }
    let (system, fallback) = match availability {
        Availability::Available | Availability::Unknown => (true, false),
        Availability::Undecided => (true, true),
        Availability::Unavailable => (false, true),
    };
    Presentation {
        system,
        notice: fallback.then_some(NoticeShape {
            reveal: !in_front,
            attention: true,
        }),
    }
}

/// The heading a notification shows: the program's title, or the tab's
/// label when it sent none, as OSC 9 never does.
fn heading(notification: &TerminalNotification, label: &str) -> String {
    if notification.title().is_empty() {
        label.to_owned()
    } else {
        notification.title().to_owned()
    }
}

/// Escapes the markup that Linux notification daemons parse in a body, so
/// a program's text can neither format itself nor add a link.
fn plain_body(body: &str) -> String {
    if cfg!(target_os = "linux") {
        body.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    } else {
        body.to_owned()
    }
}

fn tag(pane: PaneId) -> String {
    format!("{TAG_PREFIX}{}", pane.get())
}

fn tagged_pane(tag: &str) -> Option<u64> {
    tag.strip_prefix(TAG_PREFIX)?.parse().ok()
}

/// The notice for `notification` from `pane`, labelled `label`.
fn notice(
    tab: TabId,
    pane: PaneId,
    label: &str,
    notification: &TerminalNotification,
    reveal: bool,
) -> NoticeContent {
    let title = heading(notification, label);
    // The label names the terminal when the program's title took its place.
    let location = (title != label).then(|| label.to_owned());
    let actions = if reveal {
        vec![NoticeAction {
            label: "Show Tab".to_owned(),
            command: CommandInvocation::new(
                SHOW_PANE,
                vec![CommandArgument::new("pane", CommandValue::Pane(pane))],
            ),
        }]
    } else {
        Vec::new()
    };
    NoticeContent {
        severity: Severity::Info,
        source: NoticeSource::Terminal {
            tab,
            title: label.to_owned(),
        },
        title,
        message: notification.body().to_owned(),
        location,
        actions,
        lifetime: Lifetime::Expiring,
    }
}

/// Starts following permission changes and system notification clicks.
pub(super) fn install(cx: &mut App) {
    cx.set_app_identity(crate::APP_ID, "Huterm");
    native_notifications::refresh();
    cx.on_system_notification_response(|response, cx| {
        if let Some(pane) = tagged_pane(&response.tag) {
            reveal_pane(pane, cx);
        }
    });
}

/// Brings the window holding pane number `pane` forward, selects its tab,
/// and focuses it. Returns whether a window held it. A hidden Quake
/// window stays hidden, as summoning it is its hotkey's job.
pub(super) fn reveal_pane(pane: u64, cx: &mut App) -> bool {
    for handle in cx.windows() {
        let revealed = handle
            .update(cx, |root, window, cx| {
                let Ok(view) = root.downcast::<WorkspaceView>() else {
                    return false;
                };
                view.update(cx, |view, cx| {
                    let Some(tab) = view.tabs.iter().find(|tab| {
                        tab.panes.iter().any(|shown| shown.pane.get() == pane)
                    }) else {
                        return false;
                    };
                    if !view.quake_visible() {
                        return false;
                    }
                    let tab_id = tab.id;
                    let pane_id = tab
                        .panes
                        .iter()
                        .find(|shown| shown.pane.get() == pane)
                        .map(|shown| shown.pane);
                    window.activate_window();
                    cx.activate(true);
                    view.select_tab(tab_id, true, window, cx);
                    if let Some(pane_id) = pane_id {
                        view.focus_pane(tab_id, pane_id, window, cx);
                    }
                    true
                })
            })
            .unwrap_or(false);
        if revealed {
            return true;
        }
    }
    false
}

/// Whether any of Huterm's windows is active.
fn app_active(cx: &App) -> bool {
    cx.active_window().is_some_and(|window| {
        cx.global::<Desktop>()
            .windows
            .record(window.window_id())
            .is_some()
    })
}

/// The window a notice raised while Huterm is in front goes to: the active
/// one, where the user is looking, when it can show notices.
fn active_workspace(cx: &App) -> Option<WindowHandle<WorkspaceView>> {
    let window = cx.active_window()?;
    window.downcast::<WorkspaceView>()
}

impl WorkspaceView {
    /// Presents the notifications `pane` of `tab` took. Each is rechecked
    /// against its recipient's current permission first.
    pub(super) fn present_notifications(
        &mut self,
        tab: TabId,
        pane: PaneId,
        label: &str,
        pending: &[PendingHostEffect],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(view) = self
            .tabs
            .iter()
            .find(|shown| shown.id == tab)
            .and_then(|shown| shown.pane(pane))
            .map(|shown| shown.view.clone())
        else {
            return;
        };
        let visible = view.read(cx).visible;
        let notifications: Vec<TerminalNotification> = {
            let terminal = view.read(cx);
            let Some(recipient) = terminal.viewer.host_effects() else {
                return;
            };
            pending
                .iter()
                .filter(|pending| recipient.is_current(pending))
                .filter_map(|pending| match pending.effect() {
                    HostEffect::Notification(notification) => {
                        Some(notification.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        for notification in notifications {
            let now = Instant::now();
            let notifier = &mut cx.global_mut::<Desktop>().notifier;
            if !notifier.limiter.admit(now) {
                notifier.dropped += 1;
                continue;
            }
            #[cfg(test)]
            let availability = cx
                .global::<Desktop>()
                .notifier
                .availability
                .unwrap_or_else(native_notifications::availability);
            #[cfg(not(test))]
            let availability = native_notifications::availability();
            let app_active = app_active(cx);
            let in_front =
                visible && (window.is_window_active() || !app_active);
            let shown = presentation(app_active, in_front, availability);
            if shown.system {
                cx.show_system_notification(SystemNotification {
                    tag: SharedString::from(tag(pane)),
                    title: heading(&notification, label).into(),
                    body: plain_body(notification.body()).into(),
                    actions: Vec::new(),
                });
                cx.global_mut::<Desktop>().notifier.posted += 1;
                native_notifications::refresh();
            }
            if let Some(shape) = shown.notice {
                let content =
                    notice(tab, pane, label, &notification, shape.reveal);
                self.raise_notification_notice(content, window, cx);
                if shape.attention {
                    self.request_attention(window);
                }
            }
        }
    }

    /// Raises a notification's notice in the active window when Huterm is
    /// in front and that window can show it, and in this window otherwise.
    fn raise_notification_notice(
        &mut self,
        content: NoticeContent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.global_mut::<Desktop>().notifier.notices += 1;
        let elsewhere = active_workspace(cx).filter(|active| {
            active.window_id() != window.window_handle().window_id()
        });
        if let Some(active) = elsewhere {
            let raised = active
                .update(cx, |view, _, cx| {
                    if !view.can_show_notices(cx) {
                        return false;
                    }
                    view.notify(content.clone(), cx);
                    true
                })
                .unwrap_or(false);
            if raised {
                return;
            }
        }
        self.notify(content, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huterm_in_front_raises_a_notice_and_never_a_system_notification() {
        for availability in [
            Availability::Unknown,
            Availability::Available,
            Availability::Undecided,
            Availability::Unavailable,
        ] {
            assert_eq!(
                presentation(true, true, availability),
                Presentation {
                    system: false,
                    notice: Some(NoticeShape {
                        reveal: false,
                        attention: false,
                    }),
                }
            );
            assert!(
                presentation(true, false, availability)
                    .notice
                    .is_some_and(|shape| shape.reveal)
            );
        }
    }

    #[test]
    fn huterm_in_the_background_posts_or_falls_back_with_attention() {
        let posted = presentation(false, false, Availability::Available);
        assert_eq!(
            posted,
            Presentation {
                system: true,
                notice: None,
            }
        );
        assert_eq!(presentation(false, false, Availability::Unknown), posted);
        let fallback = |reveal| {
            Some(NoticeShape {
                reveal,
                attention: true,
            })
        };
        let asked = presentation(false, false, Availability::Undecided);
        assert!(asked.system);
        assert_eq!(asked.notice, fallback(true));
        let denied = presentation(false, true, Availability::Unavailable);
        assert!(!denied.system);
        // The terminal is already the one its window shows.
        assert_eq!(denied.notice, fallback(false));
    }

    #[test]
    fn notices_keep_program_text_plain_and_offer_only_show_tab() {
        let tab = TabId::new(1);
        let pane = PaneId::new(7);
        let osc9 = TerminalNotification::new(String::new(), "done".into());
        let content = notice(tab, pane, "cargo build", &osc9, true);
        assert_eq!(content.title, "cargo build");
        assert_eq!(content.message, "done");
        assert_eq!(content.location, None);
        assert_eq!(content.severity, Severity::Info);
        assert_eq!(content.lifetime, Lifetime::Expiring);
        let [action] = content.actions.as_slice() else {
            panic!("one action expected");
        };
        assert_eq!(action.label, "Show Tab");
        assert_eq!(action.command.id, SHOW_PANE);
        assert_eq!(
            action.command.argument("pane"),
            Some(&CommandValue::Pane(pane))
        );

        let titled = TerminalNotification::new("CI".into(), "<b>x</b>".into());
        let content = notice(tab, pane, "cargo build", &titled, false);
        assert_eq!(content.title, "CI");
        assert_eq!(content.location.as_deref(), Some("cargo build"));
        assert_eq!(content.actions, []);
    }

    #[test]
    fn tags_name_their_pane_and_nothing_else() {
        let pane = PaneId::new(42);
        assert_eq!(tagged_pane(&tag(pane)), Some(42));
        assert_eq!(tagged_pane("gpui-system-notification-1"), None);
        assert_eq!(tagged_pane("huterm-pane-x"), None);
    }

    #[test]
    fn linux_bodies_escape_markup() {
        let body = plain_body("<a href=\"x\">a & b</a>");
        if cfg!(target_os = "linux") {
            assert_eq!(body, "&lt;a href=\"x\"&gt;a &amp; b&lt;/a&gt;");
        } else {
            assert_eq!(body, "<a href=\"x\">a & b</a>");
        }
    }
}
