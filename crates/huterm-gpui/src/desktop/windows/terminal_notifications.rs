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

use gpui::{SharedString, SystemNotification};
use huterm_core::{
    NotificationLimiter, PendingHostEffect, notification_heading,
};
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
/// System notification tags name the pane they reveal when clicked, then a
/// serial number, so a pane's newer notification does not replace an
/// earlier one in the notification center.
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
    /// The serial number the next system notification's tag carries.
    serial: u64,
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
            serial: 0,
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
/// Huterm's windows, the Settings window included, is active; `in_front`
/// whether the terminal is drawn where the notice would appear: in the
/// active window while Huterm is in front, and otherwise in its own
/// window, where a fallback notice goes.
///
/// A system notification may not show: an undecided macOS permission
/// asks the user and loses the notification if they decline, and until
/// the first probe answers nothing is known. Both post and raise the
/// fallback notice too.
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
        Availability::Available => (true, false),
        Availability::Undecided | Availability::Unknown => (true, true),
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

/// The tab or pane label, bounded and cleaned like a program's title: a
/// program can set it through its terminal title.
fn clean_label(label: &str) -> String {
    notification_heading(label)
}

/// The heading a notice shows: the program's title, or the label when it
/// sent none, as OSC 9 never does.
fn heading(notification: &TerminalNotification, label: &str) -> String {
    if notification.title().is_empty() {
        label.to_owned()
    } else {
        notification.title().to_owned()
    }
}

/// The title a system notification shows. A program's own title carries
/// the label too, so a banner always names the terminal it came from.
fn system_title(notification: &TerminalNotification, label: &str) -> String {
    if notification.title().is_empty() {
        label.to_owned()
    } else {
        format!("{} \u{2014} {label}", notification.title())
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

fn tag(pane: PaneId, serial: u64) -> String {
    format!("{TAG_PREFIX}{}-{serial}", pane.get())
}

fn tagged_pane(tag: &str) -> Option<u64> {
    let (pane, serial) = tag.strip_prefix(TAG_PREFIX)?.split_once('-')?;
    serial.parse::<u64>().ok()?;
    pane.parse().ok()
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
        source: NoticeSource::Program {
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

/// Brings the window holding pane number `pane` forward, summoning a
/// hidden Quake window, selects its tab, clears a zoom that hides the
/// pane, and focuses it. A window that shows the pane's workspace goes
/// first; only then does one that holds it hidden switch to it. Returns
/// whether a window held it.
pub(super) fn reveal_pane(pane: u64, cx: &mut App) -> bool {
    for hidden in [false, true] {
        for handle in cx.windows() {
            let revealed = handle
                .update(cx, |root, window, cx| {
                    let Ok(view) = root.downcast::<WorkspaceView>() else {
                        return false;
                    };
                    view.update(cx, |view, cx| {
                        view.reveal(pane, hidden, window, cx)
                    })
                })
                .unwrap_or(false);
            if revealed {
                return true;
            }
        }
    }
    false
}

/// The handle of the active Huterm window, if any, checking `window`, the
/// one being updated, directly. Platform notions of a main window can
/// outlive activation, so each window is asked whether it is active.
fn active_window(window: &Window, cx: &mut App) -> Option<AnyWindowHandle> {
    let current = window.window_handle();
    if window.is_window_active() {
        return Some(current);
    }
    cx.windows().into_iter().find(|handle| {
        *handle != current
            && handle
                .update(cx, |_, window, _| window.is_window_active())
                .unwrap_or(false)
    })
}

impl WorkspaceView {
    /// Reveals pane number `pane` if this window shows its workspace, or,
    /// with `hidden`, holds it in a hidden workspace, which it then shows.
    fn reveal(
        &mut self,
        pane: u64,
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some((tab_id, pane_id)) = self.all_tab_views().find_map(|tab| {
            tab.panes
                .iter()
                .find(|shown| shown.pane.get() == pane)
                .map(|shown| (tab.id, shown.pane))
        }) else {
            return false;
        };
        // A pane in a hidden workspace shows its workspace first, which
        // only a window free to switch does.
        if let Some(workspace) = self.hidden_workspace_of(tab_id)
            && (!hidden
                || self.switch_workspace(workspace, window, cx).is_err())
        {
            return false;
        }
        let hidden_quake = self
            .quake
            .as_ref()
            .filter(|_| !self.quake_visible())
            .map(|state| state.name.clone());
        if let Some(name) = hidden_quake {
            let reporter = Some(cx.entity().downgrade());
            quake_windows::summon_later(cx, name, ids::SHOW_QUAKE, reporter);
        } else {
            window.activate_window();
            cx.activate(true);
        }
        self.select_tab(tab_id, true, window, cx);
        let hidden = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| self.shown_zoom(tab, cx))
            .is_some_and(|zoomed| zoomed != pane_id);
        if hidden {
            self.set_zoom(tab_id, None, window, cx);
        }
        self.focus_pane(tab_id, pane_id, window, cx);
        true
    }

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
            .all_tab_views()
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
        let label = clean_label(label);
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
            let active = active_window(window, cx);
            let in_this_window = active == Some(window.window_handle());
            let in_front = visible && (in_this_window || active.is_none());
            let shown = presentation(active.is_some(), in_front, availability);
            if shown.system {
                let notifier = &mut cx.global_mut::<Desktop>().notifier;
                notifier.serial += 1;
                let tag = tag(pane, notifier.serial);
                cx.show_system_notification(SystemNotification {
                    tag: SharedString::from(tag),
                    title: system_title(&notification, &label).into(),
                    body: plain_body(notification.body()).into(),
                    actions: Vec::new(),
                });
                cx.global_mut::<Desktop>().notifier.posted += 1;
                native_notifications::refresh();
            }
            if let Some(shape) = shown.notice {
                let content =
                    notice(tab, pane, &label, &notification, shape.reveal);
                self.raise_notification_notice(content, active, window, cx);
                if shape.attention {
                    self.request_attention(window);
                }
            }
        }
    }

    /// Raises a notification's notice in `active`, the active window, when
    /// it is another terminal window that can show it, and in this window
    /// otherwise, as when the Settings window is active. It replaces the
    /// tab's earlier notification notice.
    fn raise_notification_notice(
        &mut self,
        content: NoticeContent,
        active: Option<AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.global_mut::<Desktop>().notifier.notices += 1;
        let elsewhere = active
            .filter(|active| *active != window.window_handle())
            .and_then(|active| active.downcast::<WorkspaceView>());
        if let Some(active) = elsewhere {
            let raised = active
                .update(cx, |view, window, cx| {
                    if !view.can_show_notices(cx) {
                        return false;
                    }
                    view.replace_program_notice(content.clone(), window, cx);
                    true
                })
                .unwrap_or(false);
            if raised {
                return;
            }
        }
        self.replace_program_notice(content, window, cx);
    }

    fn replace_program_notice(
        &mut self,
        content: NoticeContent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let source = content.source.clone();
        self.notices
            .replace_source(&source, vec![content], Instant::now());
        self.reconcile_notices(window, cx);
        cx.notify();
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
        // Until the probe answers, a notice backs the post up.
        assert_eq!(
            presentation(false, false, Availability::Unknown),
            presentation(false, false, Availability::Undecided)
        );
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
        assert!(matches!(content.source, NoticeSource::Program { .. }));
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
    fn tags_name_their_pane_and_differ_per_notification() {
        let pane = PaneId::new(42);
        assert_ne!(tag(pane, 1), tag(pane, 2));
        assert_eq!(tagged_pane(&tag(pane, 7)), Some(42));
        assert_eq!(tagged_pane("gpui-system-notification-1"), None);
        assert_eq!(tagged_pane("huterm-pane-x-1"), None);
        assert_eq!(tagged_pane("huterm-pane-42"), None);
    }

    #[test]
    fn labels_are_cleaned_and_banners_name_their_terminal() {
        let long = format!("{}\u{202E}x", "t".repeat(200));
        let label = clean_label(&long);
        assert!(!label.contains('\u{202E}'));
        assert!(label.chars().count() <= 129);
        let titled = TerminalNotification::new("CI".into(), "done".into());
        assert_eq!(system_title(&titled, "cargo"), "CI \u{2014} cargo");
        let untitled = TerminalNotification::new(String::new(), "done".into());
        assert_eq!(system_title(&untitled, "cargo"), "cargo");
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
