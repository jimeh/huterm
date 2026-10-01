//! Dismissible notices that replace the window and terminal status strings.
//!
//! [`NoticeStack`] is the per-window model. It takes every `Instant` from
//! its caller so expiry is testable, and it reports one earliest deadline
//! through [`NoticeStack::next_deadline`] for the window's animation timer;
//! it schedules nothing itself. [`render_notice_stack`] draws the visible
//! toasts under the `notices` key context. The owner routes the `notice_*`
//! catalog commands, hover pause, and action invocations through
//! [`ToastHandlers`].

use std::fmt::Write as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, Div, FocusHandle, MouseButton, MouseDownEvent, Pixels,
    SharedString, Window, div, prelude::*, px, relative, svg,
};
use huterm_protocol::{CommandInvocation, TabId};

use super::overlay::{
    Swatch, accent_bar, mono_font_family, raised_panel, severity_mark,
};
use crate::assets::Icon;
use crate::ui::animation::AnimationSchedule;

/// How long a transient notice stays before expiring.
pub(crate) const NOTICE_LIFETIME: Duration = Duration::from_secs(6);
/// Toasts shown at once; the rest are counted in the overflow chip.
pub(crate) const VISIBLE_NOTICES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    /// The lowercase name used by smoke state dumps.
    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Error | Self::Warning => "!",
            Self::Info => "i",
        }
    }

    fn color(self, swatch: Swatch) -> gpui::Hsla {
        match self {
            Self::Error => swatch.danger,
            Self::Warning => swatch.warning,
            Self::Info => swatch.accent,
        }
    }
}

/// What raised a notice. Config, keymap, and each terminal tab are keyed
/// sources: a reload or a new failure replaces their earlier notices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoticeSource {
    Config,
    Keymap,
    Command,
    Terminal { tab: TabId, title: String },
}

impl NoticeSource {
    /// Whether two notices belong to the same replaceable source. Terminal
    /// sources compare by tab, so a renamed tab still replaces its notice.
    fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Config, Self::Config)
            | (Self::Keymap, Self::Keymap)
            | (Self::Command, Self::Command) => true,
            (Self::Terminal { tab, .. }, Self::Terminal { tab: other, .. }) => {
                tab == other
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lifetime {
    /// Stays until dismissed or replaced.
    Persistent,
    /// Expires [`NOTICE_LIFETIME`] after creation, paused while hovered or
    /// focused.
    Expiring,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NoticeAction {
    pub(crate) label: String,
    pub(crate) command: CommandInvocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NoticeContent {
    pub(crate) severity: Severity,
    pub(crate) source: NoticeSource,
    pub(crate) title: String,
    pub(crate) message: String,
    /// A file location or config path, shown in monospace.
    pub(crate) location: Option<String>,
    pub(crate) actions: Vec<NoticeAction>,
    pub(crate) lifetime: Lifetime,
}

impl NoticeContent {
    /// An expiring error from a window command; the common failure shape.
    pub(crate) fn command_failure(
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            source: NoticeSource::Command,
            title: title.into(),
            message: message.into(),
            location: None,
            actions: Vec::new(),
            lifetime: Lifetime::Expiring,
        }
    }

    /// A persistent configuration diagnostic from `source`, which must be
    /// [`NoticeSource::Config`] or [`NoticeSource::Keymap`] so a reload can
    /// replace it.
    pub(crate) fn diagnostic(
        severity: Severity,
        source: NoticeSource,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            source,
            title: title.into(),
            message: message.into(),
            location: None,
            actions: Vec::new(),
            lifetime: Lifetime::Persistent,
        }
    }

    pub(crate) fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub(crate) fn action(
        mut self,
        label: impl Into<String>,
        command: CommandInvocation,
    ) -> Self {
        self.actions.push(NoticeAction {
            label: label.into(),
            command,
        });
        self
    }

    /// One smoke state line: `severity|source|message`, newlines replaced by
    /// spaces. Terminal sources read `terminal:<tab title>`; consumers split
    /// on the first two `|` so the message may contain more.
    pub(crate) fn smoke_line(&self) -> String {
        let source = match &self.source {
            NoticeSource::Config => "config".to_owned(),
            NoticeSource::Keymap => "keymap".to_owned(),
            NoticeSource::Command => "command".to_owned(),
            NoticeSource::Terminal { title, .. } => {
                format!("terminal:{}", title.replace(['|', '\n'], " "))
            }
        };
        format!(
            "{}|{source}|{}",
            self.severity.name(),
            self.message.replace('\n', " ")
        )
    }
}

/// Smoke state lines for `notices`, newest first: `{prefix}notices=<n>` and
/// one `{prefix}notice<i>=` line per notice in [`NoticeContent::smoke_line`]
/// form.
pub(crate) fn smoke_lines<'a>(
    prefix: &str,
    notices: impl ExactSizeIterator<Item = &'a NoticeContent>,
) -> String {
    let mut output = format!("{prefix}notices={}\n", notices.len());
    for (index, notice) in notices.enumerate() {
        writeln!(output, "{prefix}notice{index}={}", notice.smoke_line())
            .expect("string formatting");
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NoticeId(u64);

#[derive(Clone, Debug)]
pub(crate) struct Notice {
    pub(crate) id: NoticeId,
    pub(crate) content: NoticeContent,
    created: Instant,
    deadline: Option<Instant>,
}

impl Notice {
    /// The fraction of an expiring notice's lifetime that remains at `now`,
    /// for its progress line; `None` for persistent notices.
    fn remaining(&self, now: Instant) -> Option<f32> {
        let deadline = self.deadline?;
        let left = deadline.saturating_duration_since(now).as_secs_f32();
        Some((left / NOTICE_LIFETIME.as_secs_f32()).clamp(0.0, 1.0))
    }
}

/// The window's notices, newest first.
#[derive(Debug, Default)]
pub(crate) struct NoticeStack {
    notices: Vec<Notice>,
    next_id: u64,
    paused_since: Option<Instant>,
}

impl NoticeStack {
    /// Adds a notice in front of the others and returns its identifier.
    pub(crate) fn push(
        &mut self,
        content: NoticeContent,
        now: Instant,
    ) -> NoticeId {
        self.next_id += 1;
        let id = NoticeId(self.next_id);
        let deadline = match content.lifetime {
            Lifetime::Persistent => None,
            Lifetime::Expiring => Some(now + NOTICE_LIFETIME),
        };
        self.notices.insert(
            0,
            Notice {
                id,
                content,
                created: now,
                deadline,
            },
        );
        id
    }

    /// Replaces every notice from `source` with `replacements`, so a reload
    /// that fixed a file clears its notices and one that still fails raises
    /// them again, even after dismissal. Returns whether anything changed.
    pub(crate) fn replace_source(
        &mut self,
        source: &NoticeSource,
        replacements: Vec<NoticeContent>,
        now: Instant,
    ) -> bool {
        let before = self.notices.len();
        self.notices
            .retain(|notice| !notice.content.source.same_source(source));
        let removed = self.notices.len() != before;
        let added = !replacements.is_empty();
        for content in replacements.into_iter().rev() {
            debug_assert!(content.source.same_source(source));
            self.push(content, now);
        }
        removed || added
    }

    /// Replaces the Config and Keymap notices with `diagnostics`, keeping
    /// their precedence order in front, so every reload re-raises what
    /// still fails and clears what was fixed. Returns whether anything
    /// changed.
    pub(crate) fn replace_diagnostics(
        &mut self,
        diagnostics: &[NoticeContent],
        now: Instant,
    ) -> bool {
        let before = self.notices.len();
        self.notices.retain(|notice| {
            !matches!(
                notice.content.source,
                NoticeSource::Config | NoticeSource::Keymap
            )
        });
        let removed = self.notices.len() != before;
        for content in diagnostics.iter().rev() {
            debug_assert!(matches!(
                content.source,
                NoticeSource::Config | NoticeSource::Keymap
            ));
            self.push(content.clone(), now);
        }
        removed || !diagnostics.is_empty()
    }

    pub(crate) fn dismiss(&mut self, id: NoticeId) -> bool {
        let before = self.notices.len();
        self.notices.retain(|notice| notice.id != id);
        self.notices.len() != before
    }

    /// Dismisses every notice whose content matches `matches`.
    pub(crate) fn dismiss_where(
        &mut self,
        matches: impl Fn(&NoticeContent) -> bool,
    ) -> bool {
        let before = self.notices.len();
        self.notices.retain(|notice| !matches(&notice.content));
        self.notices.len() != before
    }

    /// Dismisses the focused toast `id` and returns the toast that should
    /// take keyboard focus: the one that moves into its slot, the nearest
    /// remaining toast otherwise, or `None` when the stack is empty.
    pub(crate) fn dismiss_focused(&mut self, id: NoticeId) -> Option<NoticeId> {
        let slot = self.visible().iter().position(|notice| notice.id == id);
        self.dismiss(id);
        let visible = self.visible();
        let index = slot.unwrap_or(0).min(visible.len().checked_sub(1)?);
        visible.get(index).map(|notice| notice.id)
    }

    pub(crate) fn dismiss_all(&mut self) -> bool {
        let changed = !self.notices.is_empty();
        self.notices.clear();
        changed
    }

    /// Stops expiry while a toast is hovered or focused.
    pub(crate) fn pause(&mut self, now: Instant) {
        if self.paused_since.is_none() {
            self.paused_since = Some(now);
        }
    }

    /// Resumes expiry, giving every expiring notice back the time it spent
    /// paused.
    pub(crate) fn resume(&mut self, now: Instant) {
        let Some(paused_since) = self.paused_since.take() else {
            return;
        };
        for notice in &mut self.notices {
            if let Some(deadline) = &mut notice.deadline {
                let paused_from = paused_since.max(notice.created);
                *deadline += now.saturating_duration_since(paused_from);
            }
        }
    }

    /// Removes notices whose deadline has passed. Returns whether any did.
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        if self.paused_since.is_some() {
            return false;
        }
        let before = self.notices.len();
        self.notices
            .retain(|notice| notice.deadline.is_none_or(|at| at > now));
        self.notices.len() != before
    }

    /// The earliest expiry, for the window's single deadline timer. `None`
    /// while paused or when every notice is persistent.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        if self.paused_since.is_some() {
            return None;
        }
        self.notices
            .iter()
            .filter_map(|notice| notice.deadline)
            .min()
    }

    /// The remaining lifetime fraction of `id` at `now`, frozen while
    /// paused.
    pub(crate) fn progress(&self, id: NoticeId, now: Instant) -> Option<f32> {
        let now = self.paused_since.map_or(now, |paused| paused.min(now));
        self.get(id)?.remaining(now)
    }

    /// The window's wake for expiry: no deadline while paused or while
    /// every notice is persistent, the earliest expiry otherwise. Never a
    /// frame: the progress line advances only when something else repaints.
    pub(crate) fn schedule(&self) -> AnimationSchedule {
        self.next_deadline()
            .map_or(AnimationSchedule::IDLE, AnimationSchedule::at)
    }

    pub(crate) fn get(&self, id: NoticeId) -> Option<&Notice> {
        self.notices.iter().find(|notice| notice.id == id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }

    /// Every notice's content, newest first.
    pub(crate) fn contents(
        &self,
    ) -> impl ExactSizeIterator<Item = &NoticeContent> {
        self.notices.iter().map(|notice| &notice.content)
    }

    /// The toasts shown, newest first.
    pub(crate) fn visible(&self) -> &[Notice] {
        &self.notices[..self.notices.len().min(VISIBLE_NOTICES)]
    }

    /// Notices beyond the visible ones.
    pub(crate) fn overflow(&self) -> usize {
        self.notices.len().saturating_sub(VISIBLE_NOTICES)
    }

    /// The newest notice, where "Focus Notices" lands.
    pub(crate) fn newest(&self) -> Option<NoticeId> {
        self.notices.first().map(|notice| notice.id)
    }

    /// The visible neighbour of `id`: `forward` moves toward the corner
    /// (newer), otherwise away from it. Stops at the ends.
    pub(crate) fn neighbour(
        &self,
        id: NoticeId,
        forward: bool,
    ) -> Option<NoticeId> {
        let visible = self.visible();
        let index = visible.iter().position(|notice| notice.id == id)?;
        let next = if forward {
            index.checked_sub(1)?
        } else {
            index + 1
        };
        visible.get(next).map(|notice| notice.id)
    }
}

// ---- presentation ---------------------------------------------------------

/// Reports a dismiss click or `notice_dismiss`.
pub(crate) type DismissHandler = Rc<dyn Fn(NoticeId, &mut Window, &mut App)>;
/// Reports an action link click or `notice_run_action`.
pub(crate) type ActionHandler =
    Rc<dyn Fn(NoticeId, CommandInvocation, &mut Window, &mut App)>;
/// Reports the pointer entering (`true`) or leaving (`false`) a toast, for
/// pause and resume. A toast dismissed under the pointer never reports
/// leaving, so owners key hover state by notice and drop gone ids.
pub(crate) type HoverHandler =
    Rc<dyn Fn(NoticeId, bool, &mut Window, &mut App)>;

/// Callbacks the toast stack reports through.
#[derive(Clone)]
pub(crate) struct ToastHandlers {
    pub(crate) dismiss: DismissHandler,
    pub(crate) run_action: ActionHandler,
    pub(crate) hover: HoverHandler,
}

/// Width of the toast column.
const TOAST_WIDTH: f32 = 340.0;

/// The toast column in the bottom-right corner of its parent, `bottom`
/// points above the edge. Newest toasts sit nearest the corner beneath the
/// overflow chip. `focused` carries the focus ring; `focus` is the handle the
/// `notices` context tracks.
pub(crate) fn render_notice_stack(
    stack: &NoticeStack,
    now: Instant,
    focused: Option<NoticeId>,
    focus: &FocusHandle,
    bottom: Pixels,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> Div {
    let mut column = div()
        .absolute()
        .right(px(12.0))
        .bottom(bottom)
        .w(px(TOAST_WIDTH))
        .max_w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(8.0))
        .key_context("notices")
        .track_focus(focus)
        .text_color(swatch.fg);
    let overflow = stack.overflow();
    if overflow > 0 {
        column = column.child(
            div()
                .flex_none()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(10.0))
                .bg(swatch.surface)
                .border_1()
                .border_color(swatch.line)
                .text_size(px(11.5))
                .text_color(swatch.muted)
                .child(format!("+{overflow} more")),
        );
    }
    for notice in stack.visible().iter().rev() {
        let progress = stack.progress(notice.id, now);
        column = column.child(render_toast(
            notice,
            progress,
            focused == Some(notice.id),
            swatch,
            handlers,
        ));
    }
    column
}

fn toast_body(
    id: NoticeId,
    content: &NoticeContent,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> Div {
    let mut body = div()
        .flex_basis(px(0.0))
        .flex_grow_1()
        .min_w_0()
        .text_size(px(12.5))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .mb(px(1.0))
                .child(content.title.clone()),
        )
        .child(content.message.clone());
    if let Some(location) = &content.location {
        body = body.child(
            div()
                .font_family(mono_font_family())
                .text_size(px(11.0))
                .text_color(swatch.muted)
                .child(location.clone()),
        );
    }
    if content.actions.is_empty() {
        return body;
    }
    let mut actions = div().flex().gap(px(14.0)).mt(px(6.0));
    for (index, action) in content.actions.iter().enumerate() {
        let run = Rc::clone(&handlers.run_action);
        let command = action.command.clone();
        actions = actions.child(
            div()
                .id(SharedString::from(format!(
                    "toast-action-{}-{index}",
                    id.0
                )))
                .cursor_pointer()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(swatch.accent)
                .active(|action| action.opacity(0.6))
                .on_click(move |_: &ClickEvent, window, cx| {
                    run(id, command.clone(), window, cx);
                })
                .child(action.label.clone()),
        );
    }
    body.child(actions)
}

fn render_toast(
    notice: &Notice,
    progress: Option<f32>,
    focused: bool,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> gpui::Stateful<Div> {
    let id = notice.id;
    let content = &notice.content;
    let stripe = content.severity.color(swatch);
    let mut shadow = swatch.shadow();
    if focused {
        shadow.extend(swatch.focus_ring());
    }
    let dismiss = Rc::clone(&handlers.dismiss);
    let hover = Rc::clone(&handlers.hover);
    // Presses stay on the toast: a click must not start a terminal selection
    // or application mouse input beneath it. Releases pass through, so a
    // terminal gesture that ends over the toast still finishes; the terminal
    // ignores releases it does not own.
    let stop = |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
        cx.stop_propagation();
    };
    let mut toast = raised_panel(swatch, 9.0)
        .id(("toast", id.0))
        .w_full()
        .relative()
        .flex()
        .flex_none()
        .items_start()
        .gap(px(10.0))
        .pl(px(16.0))
        .pr(px(8.0))
        .py(px(10.0))
        .shadow(shadow)
        .on_mouse_down(MouseButton::Left, stop)
        .on_mouse_down(MouseButton::Right, stop)
        .on_mouse_down(MouseButton::Middle, stop)
        .on_hover(move |hovering: &bool, window, cx| {
            hover(id, *hovering, window, cx);
        })
        .child(accent_bar(stripe, px(6.0), px(10.0)))
        .child(severity_mark(content.severity.glyph(), stripe, swatch))
        .child(toast_body(id, content, swatch, handlers))
        .child(
            div()
                .id(("toast-dismiss", id.0))
                .flex_none()
                .w(px(22.0))
                .h(px(22.0))
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|button| button.bg(swatch.hover))
                .active(|button| button.bg(swatch.pressed()))
                .on_click(move |_: &ClickEvent, window, cx| {
                    dismiss(id, window, cx);
                })
                .child(
                    svg()
                        .path(Icon::X.asset_path())
                        .w(px(12.0))
                        .h(px(12.0))
                        .text_color(swatch.muted),
                ),
        );
    if let Some(progress) = progress {
        toast = toast.child(
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .h(px(2.0))
                .w(relative(progress))
                .bg(swatch.accent),
        );
    }
    toast
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::ids;

    fn content(
        source: NoticeSource,
        title: &str,
        lifetime: Lifetime,
    ) -> NoticeContent {
        NoticeContent {
            severity: Severity::Error,
            source,
            title: title.to_owned(),
            message: String::new(),
            location: None,
            actions: vec![NoticeAction {
                label: "Reload".to_owned(),
                command: CommandInvocation::new(ids::RELOAD_CONFIG, Vec::new()),
            }],
            lifetime,
        }
    }

    fn titles(stack: &NoticeStack) -> Vec<&str> {
        stack
            .notices
            .iter()
            .map(|notice| notice.content.title.as_str())
            .collect()
    }

    fn terminal(tab: u64) -> NoticeSource {
        NoticeSource::Terminal {
            tab: TabId::new(tab),
            title: format!("tab {tab}"),
        }
    }

    #[test]
    fn a_reload_replaces_its_source_and_re_raises_after_dismissal() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        let config = stack.push(
            content(NoticeSource::Config, "config v1", Lifetime::Persistent),
            now,
        );
        stack.push(
            content(NoticeSource::Keymap, "keymap", Lifetime::Persistent),
            now,
        );
        assert!(stack.dismiss(config));
        assert_eq!(titles(&stack), ["keymap"]);
        assert!(stack.replace_source(
            &NoticeSource::Config,
            vec![content(
                NoticeSource::Config,
                "config v2",
                Lifetime::Persistent
            )],
            now,
        ));
        assert_eq!(titles(&stack), ["config v2", "keymap"], "re-raised");
        assert!(stack.replace_source(
            &NoticeSource::Config,
            vec![
                content(NoticeSource::Config, "first", Lifetime::Persistent),
                content(NoticeSource::Config, "second", Lifetime::Persistent),
            ],
            now,
        ));
        assert_eq!(
            titles(&stack),
            ["first", "second", "keymap"],
            "replacements keep their order in front"
        );
        assert!(stack.replace_source(&NoticeSource::Config, Vec::new(), now));
        assert_eq!(titles(&stack), ["keymap"], "a fixed file clears them");
        assert!(
            !stack.replace_source(&NoticeSource::Config, Vec::new(), now),
            "nothing to clear"
        );
    }

    #[test]
    fn terminal_sources_are_keyed_by_tab() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        stack.push(content(terminal(1), "one", Lifetime::Expiring), now);
        stack.push(content(terminal(2), "two", Lifetime::Expiring), now);
        stack.replace_source(
            &terminal(1),
            vec![content(terminal(1), "one again", Lifetime::Expiring)],
            now,
        );
        assert_eq!(titles(&stack), ["one again", "two"]);
    }

    #[test]
    fn expiring_notices_expire_in_creation_order_with_one_deadline() {
        let start = Instant::now();
        let mut stack = NoticeStack::default();
        stack.push(
            content(NoticeSource::Config, "persistent", Lifetime::Persistent),
            start,
        );
        assert_eq!(stack.next_deadline(), None);
        stack.push(
            content(NoticeSource::Command, "first", Lifetime::Expiring),
            start,
        );
        let later = start + Duration::from_secs(2);
        stack.push(content(terminal(1), "second", Lifetime::Expiring), later);
        assert_eq!(stack.next_deadline(), Some(start + NOTICE_LIFETIME));
        assert!(!stack.expire(start + Duration::from_secs(5)));
        assert!(stack.expire(start + NOTICE_LIFETIME));
        assert_eq!(titles(&stack), ["second", "persistent"]);
        assert_eq!(stack.next_deadline(), Some(later + NOTICE_LIFETIME));
        assert!(stack.expire(later + NOTICE_LIFETIME));
        assert_eq!(titles(&stack), ["persistent"]);
        assert_eq!(stack.next_deadline(), None);
    }

    #[test]
    fn pausing_defers_expiry_and_resuming_restores_the_remaining_time() {
        let start = Instant::now();
        let mut stack = NoticeStack::default();
        let id = stack.push(
            content(NoticeSource::Command, "hovered", Lifetime::Expiring),
            start,
        );
        let hover = start + Duration::from_secs(4);
        stack.pause(hover);
        assert_eq!(stack.next_deadline(), None, "no timer while paused");
        let long_after = start + Duration::from_secs(60);
        assert!(!stack.expire(long_after));
        assert_eq!(
            stack.progress(id, long_after),
            Some(2.0 / 6.0),
            "the progress line freezes"
        );
        // A notice raised mid-pause only gains the pause time after it.
        let mid = start + Duration::from_secs(30);
        let newer =
            stack.push(content(terminal(1), "new", Lifetime::Expiring), mid);
        stack.resume(long_after);
        assert_eq!(
            stack.next_deadline(),
            Some(long_after + Duration::from_secs(2)),
            "two seconds remained when hovered"
        );
        assert!(!stack.expire(long_after + Duration::from_secs(1)));
        assert!(stack.expire(long_after + Duration::from_secs(2)));
        assert_eq!(titles(&stack), ["new"]);
        assert_eq!(
            stack.get(newer).unwrap().deadline,
            Some(mid + NOTICE_LIFETIME + Duration::from_secs(30))
        );
    }

    #[test]
    fn the_window_schedule_wakes_only_for_the_earliest_unpaused_expiry() {
        let start = Instant::now();
        let mut stack = NoticeStack::default();
        assert_eq!(stack.schedule(), AnimationSchedule::IDLE);
        stack.push(
            content(NoticeSource::Config, "config", Lifetime::Persistent),
            start,
        );
        stack.push(
            content(NoticeSource::Keymap, "keymap", Lifetime::Persistent),
            start,
        );
        assert_eq!(
            stack.schedule(),
            AnimationSchedule::IDLE,
            "persistent notices need no wakeups"
        );
        let later = start + Duration::from_secs(3);
        stack.push(content(terminal(1), "late", Lifetime::Expiring), later);
        stack.push(
            content(NoticeSource::Command, "early", Lifetime::Expiring),
            start,
        );
        let schedule = stack.schedule();
        assert!(!schedule.frame, "no per-frame work for the progress line");
        assert_eq!(schedule.deadline, Some(start + NOTICE_LIFETIME));
        stack.pause(start + Duration::from_secs(1));
        assert_eq!(stack.schedule(), AnimationSchedule::IDLE, "paused");
        stack.resume(start + Duration::from_secs(2));
        assert_eq!(
            stack.schedule().deadline,
            Some(start + NOTICE_LIFETIME + Duration::from_secs(1))
        );
    }

    #[test]
    fn dismissing_the_focused_toast_focuses_the_one_that_takes_its_slot() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        let ids: Vec<NoticeId> = (0..5)
            .map(|index| {
                stack.push(
                    content(
                        terminal(index),
                        &format!("n{index}"),
                        Lifetime::Persistent,
                    ),
                    now,
                )
            })
            .collect();
        // Visible, newest first: n4, n3, n2; n1 and n0 overflow.
        assert_eq!(
            stack.dismiss_focused(ids[3]),
            Some(ids[2]),
            "the older toast moves into the middle slot"
        );
        assert_eq!(
            stack
                .visible()
                .iter()
                .map(|notice| notice.id)
                .collect::<Vec<_>>(),
            [ids[4], ids[2], ids[1]],
            "an overflow toast becomes visible"
        );
        assert_eq!(
            stack.dismiss_focused(ids[1]),
            Some(ids[0]),
            "the promoted toast takes the last slot"
        );
        assert_eq!(
            stack.dismiss_focused(ids[0]),
            Some(ids[2]),
            "last slot: nearest remaining"
        );
        assert_eq!(stack.dismiss_focused(ids[4]), Some(ids[2]));
        assert_eq!(
            stack.dismiss_focused(ids[2]),
            None,
            "focus returns to the terminal"
        );
        assert!(stack.is_empty());
        assert_eq!(stack.dismiss_focused(ids[2]), None, "already gone");
    }

    #[test]
    fn the_newest_three_are_visible_and_the_rest_count_as_overflow() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        let ids: Vec<NoticeId> = (0..5)
            .map(|index| {
                stack.push(
                    content(
                        terminal(index),
                        &format!("n{index}"),
                        Lifetime::Persistent,
                    ),
                    now,
                )
            })
            .collect();
        let visible: Vec<&str> = stack
            .visible()
            .iter()
            .map(|notice| notice.content.title.as_str())
            .collect();
        assert_eq!(visible, ["n4", "n3", "n2"]);
        assert_eq!(stack.overflow(), 2);
        assert_eq!(stack.newest(), Some(ids[4]));
        assert_eq!(stack.neighbour(ids[4], true), None, "already nearest");
        assert_eq!(stack.neighbour(ids[4], false), Some(ids[3]));
        assert_eq!(stack.neighbour(ids[2], false), None, "overflow is hidden");
        assert_eq!(stack.neighbour(ids[0], false), None);
        assert!(stack.dismiss(ids[4]));
        assert!(!stack.dismiss(ids[4]));
        assert_eq!(stack.overflow(), 1);
        assert!(stack.dismiss_all());
        assert!(stack.is_empty());
        assert!(!stack.dismiss_all());
    }
}
