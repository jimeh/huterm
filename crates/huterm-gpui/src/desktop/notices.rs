//! Dismissible notices that replace the window and terminal status strings.
//!
//! [`NoticeStack`] is the per-window model. It takes every `Instant` from
//! its caller so expiry is testable, and it reports its frames and earliest
//! deadline through [`NoticeStack::schedule`] for the window's frame clock;
//! it schedules nothing itself. [`NoticeColumn`] draws the visible toasts
//! under the `notices` key context, fitted to the terminal area by
//! [`StackFit`]. The owner routes the `notice_*` catalog commands, hover
//! pause, and action invocations through [`ToastHandlers`].
//!
//! Each toast is an accessibility `Alert` named by
//! [`NoticeContent::accessible_text`]. [`NoticeStack::begin_frame`] decides
//! which toasts announce in the frame being drawn: each notice once, through
//! a live leaf added when it announces, since AccessKit announces a live
//! node when it is added and its descendants inherit its live state. The
//! leaf stays for [`ANNOUNCEMENT_HOLD`], because AT-SPI announces from the
//! leaf's own object and a screen reader may read it after the event.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::accesskit::{Live, Node};
use gpui::{
    AnyElement, App, Div, Edges, ElementId, FocusHandle, MouseButton,
    MouseDownEvent, Pixels, Rems, Role, SharedString, Size, Window, div,
    prelude::*, px, relative, svg,
};
use huterm_protocol::{CommandInvocation, TabId};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::typography::ui;

use super::overlay::{
    Swatch, accent_bar, mono_font_family, raised_panel, severity_mark,
};
use crate::assets::Icon;
use crate::ui::animation::AnimationSchedule;

/// How long a transient notice stays before expiring.
pub(crate) const NOTICE_LIFETIME: Duration = Duration::from_secs(6);
/// Toasts shown at once; the rest are counted in the overflow chip.
pub(crate) const VISIBLE_NOTICES: usize = 3;
/// How long a toast keeps the live leaf that announced it. AT-SPI sends
/// the announcement from the leaf, and a removed leaf's object is defunct,
/// so it outlasts the frames that follow, such as scrolling to the toast.
pub(crate) const ANNOUNCEMENT_HOLD: Duration = Duration::from_secs(1);

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

    /// The word assistive technology reads before a notice's text.
    fn spoken(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
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
    /// Session restore and saving.
    Sessions,
    Terminal {
        tab: TabId,
        title: String,
    },
    /// A program's notification (OSC 9 or OSC 777). Each tab keeps only its
    /// newest, so a program that keeps notifying cannot grow the stack, and
    /// a terminal failure in the tab does not replace it.
    Program {
        tab: TabId,
        title: String,
    },
}

impl NoticeSource {
    /// Whether two notices belong to the same replaceable source. Terminal
    /// sources compare by tab, so a renamed tab still replaces its notice.
    fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Config, Self::Config)
            | (Self::Keymap, Self::Keymap)
            | (Self::Command, Self::Command)
            | (Self::Sessions, Self::Sessions) => true,
            (Self::Terminal { tab, .. }, Self::Terminal { tab: other, .. })
            | (Self::Program { tab, .. }, Self::Program { tab: other, .. }) => {
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

    /// What assistive technology reads for the notice: its severity, then
    /// its title, message, and location as sentences, leaving out empty
    /// parts.
    pub(crate) fn accessible_text(&self) -> String {
        let mut text = self.severity.spoken().to_owned();
        let mut separator = ": ";
        for part in [
            self.title.as_str(),
            self.message.as_str(),
            self.location.as_deref().unwrap_or_default(),
        ] {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            text.push_str(separator);
            text.push_str(part);
            separator = if part.ends_with(['.', '!', '?', ':']) {
                " "
            } else {
                ". "
            };
        }
        text
    }

    /// How urgently the notice is announced. A failure of something the
    /// user or a terminal just did interrupts speech, as an alert does;
    /// diagnostics, restore and save problems, program notifications, and
    /// every warning and info wait for speech to finish, as a status does,
    /// so they do not cut off what is being read, such as terminal output.
    pub(crate) fn politeness(&self) -> Live {
        match (self.severity, &self.source) {
            (
                Severity::Error,
                NoticeSource::Command | NoticeSource::Terminal { .. },
            ) => Live::Assertive,
            _ => Live::Polite,
        }
    }

    /// One smoke state line: `severity|source|message`, newlines replaced by
    /// spaces. Terminal sources read `terminal:<tab title>`; consumers split
    /// on the first two `|` so the message may contain more.
    pub(crate) fn smoke_line(&self) -> String {
        let source = match &self.source {
            NoticeSource::Config => "config".to_owned(),
            NoticeSource::Keymap => "keymap".to_owned(),
            NoticeSource::Command => "command".to_owned(),
            NoticeSource::Sessions => "sessions".to_owned(),
            NoticeSource::Terminal { title, .. } => {
                format!("terminal:{}", title.replace(['|', '\n'], " "))
            }
            NoticeSource::Program { title, .. } => {
                format!("program:{}", title.replace(['|', '\n'], " "))
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
    /// When its announcement started and the mount it was drawn in; `None`
    /// until it announces.
    announced: Option<(Instant, u64)>,
    /// How many times its toast was drawn after a frame without it. Each
    /// return is a new accessibility node: AT-SPI reports a removed node's
    /// object defunct, and Orca then ignores it for good.
    mounts: u64,
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
    /// The toasts the last frame drew, for [`Notice::mounts`].
    drawn: Vec<NoticeId>,
    /// Whether the last frame drew the overflow chip.
    overflow_drawn: bool,
    /// The overflow chip's returns, as [`Notice::mounts`] counts a toast's.
    overflow_mounts: u64,
    /// When the earliest live leaf the last frame drew is due to go.
    holding_until: Option<Instant>,
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
                announced: None,
                mounts: 0,
            },
        );
        id
    }

    /// Records the toasts the frame being drawn at `now` shows, and returns
    /// those that carry the live leaf announcing them. A notice announces
    /// when it is shown, has not announced before, and `announce` holds,
    /// which the owner passes while its window is in front, can show
    /// notices, and builds an accessibility tree; a notice shown before
    /// that announces in the first frame that has it. Its leaf stays for
    /// [`ANNOUNCEMENT_HOLD`] while the toast stays drawn: a toast that
    /// returns, as a new node, does not bring it back.
    pub(crate) fn begin_frame(
        &mut self,
        announce: bool,
        now: Instant,
    ) -> HashSet<NoticeId> {
        let shown = self.notices.len().min(VISIBLE_NOTICES);
        let mut announcing = HashSet::new();
        self.holding_until = None;
        for notice in &mut self.notices[..shown] {
            if !self.drawn.contains(&notice.id) {
                notice.mounts += 1;
            }
            if announce && notice.announced.is_none() {
                notice.announced = Some((now, notice.mounts));
            }
            if let Some((at, mount)) = notice.announced {
                let until = at + ANNOUNCEMENT_HOLD;
                if mount == notice.mounts && now < until {
                    announcing.insert(notice.id);
                    self.holding_until = Some(
                        self.holding_until
                            .map_or(until, |held| held.min(until)),
                    );
                }
            }
        }
        self.drawn = self.notices[..shown].iter().map(|n| n.id).collect();
        let overflow = self.overflow() > 0;
        if overflow && !self.overflow_drawn {
            self.overflow_mounts += 1;
        }
        self.overflow_drawn = overflow;
        announcing
    }

    /// Whether a live leaf the last frame drew is due to go by `now`, so
    /// the owner redraws to remove it.
    pub(crate) fn release_announcements(&mut self, now: Instant) -> bool {
        if self.holding_until.is_some_and(|until| now >= until) {
            self.holding_until = None;
            return true;
        }
        false
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

    /// Whether a visible toast's progress line is shrinking: an expiring
    /// notice is shown and the stack is not paused.
    pub(crate) fn animating(&self) -> bool {
        self.paused_since.is_none()
            && self
                .visible()
                .iter()
                .any(|notice| notice.deadline.is_some())
    }

    /// The window's wake: frames while [`Self::animating`], so the progress
    /// line moves without other repaints, the earliest expiry as a
    /// deadline, so a window that draws no frames, such as a covered one,
    /// still expires on time, and the earliest live leaf's removal. Idle
    /// while paused, when a held leaf waits for the resume, or when every
    /// notice is persistent and no leaf is held.
    pub(crate) fn schedule(&self) -> AnimationSchedule {
        if self.paused_since.is_some() {
            return AnimationSchedule::IDLE;
        }
        let deadline = self
            .next_deadline()
            .map_or(AnimationSchedule::IDLE, AnimationSchedule::at)
            .merge(
                self.holding_until
                    .map_or(AnimationSchedule::IDLE, AnimationSchedule::at),
            );
        if self.animating() {
            deadline.merge(AnimationSchedule::FRAME)
        } else {
            deadline
        }
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

    /// The scroll region child that shows `id`: after the overflow chip,
    /// visible toasts run oldest first, so the newest is last.
    pub(crate) fn slot(&self, id: NoticeId) -> Option<usize> {
        let visible = self.visible();
        let index = visible.iter().position(|notice| notice.id == id)?;
        Some(usize::from(self.overflow() > 0) + visible.len() - 1 - index)
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
/// pause and resume. Entering needs a pointer move over the toast: GPUI
/// re-tests hover at paint against the last pointer position it saw, which
/// can be stale, so a toast raised under it would otherwise pause until the
/// pointer moved. Entering repeats on every move. A toast dismissed under
/// the pointer never reports leaving, so owners key hover state by notice
/// and drop gone ids.
pub(crate) type HoverHandler =
    Rc<dyn Fn(NoticeId, bool, &mut Window, &mut App)>;

/// Callbacks the toast stack reports through.
#[derive(Clone)]
pub(crate) struct ToastHandlers {
    pub(crate) dismiss: DismissHandler,
    pub(crate) run_action: ActionHandler,
    pub(crate) hover: HoverHandler,
}

/// Width of each toast when the terminal area leaves room for it.
const TOAST_WIDTH: f32 = 340.0;
/// Corner radius of each toast.
const TOAST_RADIUS: f32 = 9.0;
/// The space between a toast and the terminal area's left and right edges.
const EDGE: f32 = 12.0;
/// Space around each toast in its scroll slot for the focus ring, so a
/// toast scrolled to the column's edge keeps its ring in view. Two of these
/// make the gap between toasts.
const RING_ROOM: f32 = 4.0;
/// How far the scroll region reaches past the toasts' slots on the left,
/// top, and bottom, so [`toast_shadow`] fades before the region clips it.
const SHADOW_ROOM: f32 = 12.0;
/// The scroll region's distance from the terminal area's right edge, where
/// its scrollbar draws.
const SCROLLBAR_INSET: f32 = 2.0;
/// Below this terminal-area height, every toast but the newest collapses to
/// one line unless it is focused or hovered. It is about what three toasts
/// with a two-line message and an action need at the design size, and it
/// scales with the interface font, as the toasts do.
pub(crate) const COMPACT_BELOW: Rems = ui(320.0);

/// How the toast column fits the terminal area it is drawn over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StackFit {
    /// Each toast's width: [`TOAST_WIDTH`], or the area's width less its
    /// margins when that is narrower.
    toast_width: Pixels,
    /// The scroll region's height limit: the area's height above
    /// `region_bottom`. Taller content scrolls.
    max_height: Pixels,
    /// The newest toast's bottom, above the area's bottom edge.
    bottom: Pixels,
    /// The scroll region's bottom, above the area's bottom edge.
    region_bottom: Pixels,
    /// Whether older toasts collapse, because the area is short.
    compact: bool,
}

impl StackFit {
    /// The fit over a terminal area of `area`, with the newest toast's
    /// bottom `bottom` above the area's bottom edge, at `rem_size`.
    pub(crate) fn new(
        area: Size<Pixels>,
        bottom: Pixels,
        rem_size: Pixels,
    ) -> Self {
        let region_bottom =
            (bottom - px(RING_ROOM + SHADOW_ROOM)).max(Pixels::ZERO);
        Self {
            toast_width: px(TOAST_WIDTH)
                .min(area.width - px(EDGE) * 2.0)
                .max(Pixels::ZERO),
            max_height: (area.height - region_bottom).max(Pixels::ZERO),
            bottom,
            region_bottom,
            compact: area.height < COMPACT_BELOW.to_pixels(rem_size),
        }
    }

    /// Space inside the scroll region around its slots: room for shadows
    /// on the left, top, and bottom, and for the scrollbar between the
    /// toasts and the area's right edge.
    pub(crate) fn region_padding(&self) -> Edges<Pixels> {
        Edges {
            top: px(SHADOW_ROOM),
            right: px(EDGE - SCROLLBAR_INSET - RING_ROOM),
            bottom: self.bottom - px(RING_ROOM) - self.region_bottom,
            left: px(SHADOW_ROOM),
        }
    }

    fn region_width(&self) -> Pixels {
        let padding = self.region_padding();
        self.toast_width + px(RING_ROOM) * 2.0 + padding.left + padding.right
    }
}

/// The toast column: the visible toasts, newest nearest the bottom-right
/// corner beneath the overflow chip, in a scroll region the owner builds
/// from [`Self::slots`] and passes to [`Self::frame`].
pub(crate) struct NoticeColumn<'a> {
    pub(crate) stack: &'a NoticeStack,
    pub(crate) now: Instant,
    /// The toast that carries the focus ring and stays expanded.
    pub(crate) focused: Option<NoticeId>,
    /// The focused toast's current button, by index among its actions and
    /// then Dismiss.
    pub(crate) button: Option<usize>,
    /// Toasts under the pointer, which stay expanded.
    pub(crate) hovered: &'a HashSet<NoticeId>,
    /// Toasts that announce in this frame, from
    /// [`NoticeStack::begin_frame`].
    pub(crate) announcing: &'a HashSet<NoticeId>,
    pub(crate) fit: StackFit,
    pub(crate) swatch: Swatch,
    pub(crate) handlers: &'a ToastHandlers,
}

impl NoticeColumn<'_> {
    /// The scroll region's children in [`NoticeStack::slot`] order: the
    /// overflow chip, then each visible toast, oldest first.
    pub(crate) fn slots(&self) -> Vec<AnyElement> {
        let mut slots = Vec::new();
        let overflow = self.stack.overflow();
        if overflow > 0 {
            slots.push(
                div()
                    .flex()
                    .justify_end()
                    .p(px(RING_ROOM))
                    .child(
                        div()
                            .id(ElementId::NamedInteger(
                                "notice-overflow".into(),
                                self.stack.overflow_mounts,
                            ))
                            .role(Role::Label)
                            .aria_value(if overflow == 1 {
                                "1 more notice".to_owned()
                            } else {
                                format!("{overflow} more notices")
                            })
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(10.0))
                            .bg(self.swatch.surface)
                            .border_1()
                            .border_color(self.swatch.line)
                            .text_size(ui(11.5))
                            .text_color(self.swatch.muted)
                            .child(format!("+{overflow} more")),
                    )
                    .into_any_element(),
            );
        }
        let visible = self.stack.visible();
        for (index, notice) in visible.iter().enumerate().rev() {
            let id = notice.id;
            let expanded = index == 0
                || !self.fit.compact
                || self.focused == Some(id)
                || self.hovered.contains(&id);
            slots.push(
                div()
                    .flex_none()
                    .p(px(RING_ROOM))
                    .child(render_toast(
                        notice,
                        self.stack.progress(id, self.now),
                        match (self.focused == Some(id), self.button) {
                            (false, _) => ToastFocus::None,
                            (true, None) => ToastFocus::Toast,
                            (true, Some(index)) => ToastFocus::Button(index),
                        },
                        expanded,
                        self.announcing.contains(&id),
                        self.swatch,
                        self.handlers,
                    ))
                    .into_any_element(),
            );
        }
        slots
    }

    /// The column in the bottom-right corner of the terminal area, holding
    /// `region`, under the `notices` key context that `focus` tracks.
    pub(crate) fn frame(
        &self,
        focus: &FocusHandle,
        region: impl IntoElement,
    ) -> Div {
        div()
            .absolute()
            .right(px(SCROLLBAR_INSET))
            .bottom(self.fit.region_bottom)
            .w(self.fit.region_width())
            .max_h(self.fit.max_height)
            .flex()
            .flex_col()
            .key_context("notices")
            .track_focus(focus)
            .text_color(self.swatch.fg)
            .child(region)
    }
}

/// The raised-surface shadow, held within [`SHADOW_ROOM`] so the scroll
/// region's rectangular clip does not show around the rounded toast.
fn toast_shadow(swatch: Swatch) -> Vec<gpui::BoxShadow> {
    swatch
        .shadow()
        .into_iter()
        .map(|mut shadow| {
            shadow.blur_radius = shadow.blur_radius.min(px(SHADOW_ROOM - 4.0));
            shadow.offset.y = shadow.offset.y.min(px(4.0));
            shadow
        })
        .collect()
}

/// A collapsed toast's one line: its title, or its message when it has
/// none, cut short with an ellipsis.
fn collapsed_body(content: &NoticeContent) -> Div {
    let line = if content.title.is_empty() {
        content.message.lines().next().unwrap_or_default()
    } else {
        content.title.as_str()
    };
    div()
        .flex_basis(px(0.0))
        .flex_grow_1()
        .min_w_0()
        .text_size(ui(12.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .truncate()
        .child(line.to_owned())
}

/// A toast's Dismiss button, `current` for the keyboard.
fn dismiss_button(
    id: NoticeId,
    content: &NoticeContent,
    current: bool,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> Button {
    let dismiss = Rc::clone(&handlers.dismiss);
    // The name and tooltip say which of the stacked toasts it dismisses.
    let name = if content.title.is_empty() {
        "Dismiss".to_owned()
    } else {
        format!("Dismiss {}", content.title)
    };
    let selector = format!("toast-{name}");
    Button::new(("toast-dismiss", id.0), name, swatch)
        .variant(Variant::Ghost)
        .icon(
            svg()
                .path(Icon::X.asset_path())
                .w(px(12.0))
                .h(px(12.0))
                .text_color(swatch.muted),
        )
        .icon_only()
        .size(px(22.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .current(current)
        .debug_selector(|| selector)
        .on_click(move |window, cx| dismiss(id, window, cx))
}

/// A full toast's text and action links; `button` is the current one.
fn toast_body(
    id: NoticeId,
    content: &NoticeContent,
    button: Option<usize>,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> Div {
    let mut body = div()
        .flex_basis(px(0.0))
        .flex_grow_1()
        .min_w_0()
        .text_size(ui(12.5))
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
                .text_size(ui(11.0))
                .text_color(swatch.muted)
                .child(location.clone()),
        );
    }
    if content.actions.is_empty() {
        return body;
    }
    // Actions wrap rather than overflow a toast narrowed to the window.
    let mut actions = div()
        .flex()
        .flex_wrap()
        .gap_x(px(14.0))
        .gap_y(px(4.0))
        .mt(px(6.0));
    for (index, action) in content.actions.iter().enumerate() {
        let run = Rc::clone(&handlers.run_action);
        let command = action.command.clone();
        let selector = format!("toast-{}-{}", content.title, action.label);
        actions = actions.child(
            Button::new(
                SharedString::from(format!("toast-action-{}-{index}", id.0)),
                action.label.clone(),
                swatch,
            )
            .variant(Variant::Link)
            // It runs a command rather than opening a page.
            .button_role()
            .h_auto()
            .text_size(ui(12.5))
            .font_weight(gpui::FontWeight::MEDIUM)
            .cursor_pointer()
            .current(button == Some(index))
            .debug_selector(|| selector)
            .on_click(move |window, cx| {
                run(id, command.clone(), window, cx);
            }),
        );
    }
    body.child(actions)
}

/// How a toast shows keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastFocus {
    None,
    /// The toast has focus and no current button.
    Toast,
    /// The toast has focus, and this button, by index among its actions
    /// and then Dismiss, is current.
    Button(usize),
}

impl ToastFocus {
    fn button(self) -> Option<usize> {
        match self {
            Self::Button(index) => Some(index),
            Self::None | Self::Toast => None,
        }
    }
}

/// One toast: in full when `expanded`, otherwise collapsed to its mark,
/// title, and Dismiss button on one line. Its accessibility node reads the
/// whole notice either way, and `announce` adds the live leaf that
/// announces it.
fn render_toast(
    notice: &Notice,
    progress: Option<f32>,
    focus: ToastFocus,
    expanded: bool,
    announce: bool,
    swatch: Swatch,
    handlers: &ToastHandlers,
) -> gpui::Stateful<Div> {
    let id = notice.id;
    let content = &notice.content;
    let stripe = content.severity.color(swatch);
    let mut shadow = toast_shadow(swatch);
    if focus != ToastFocus::None {
        shadow.extend(swatch.focus_ring());
    }
    let button = focus.button();
    let hover = Rc::clone(&handlers.hover);
    // Presses stay on the toast: a click must not start a terminal selection
    // or application mouse input beneath it. Releases pass through, so a
    // terminal gesture that ends over the toast still finishes; the terminal
    // ignores releases it does not own.
    let stop = |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
        cx.stop_propagation();
    };
    let progress_selector = content.title.clone();
    let toast_selector = content.title.clone();
    let text = content.accessible_text();
    let mut toast = raised_panel(swatch, TOAST_RADIUS)
        .id(ElementId::NamedInteger(
            SharedString::from(format!("toast-{}", id.0)),
            notice.mounts,
        ))
        .role(Role::Alert)
        .aria_label(text.clone())
        .debug_selector(|| format!("notice-{toast_selector}"))
        // Over a terminal, whose text cursor would otherwise show.
        .cursor(gpui::CursorStyle::Arrow)
        .w_full()
        .relative()
        .flex()
        .flex_none()
        .gap(px(10.0))
        .pl(px(16.0))
        .pr(px(8.0))
        .map(|toast| {
            if expanded {
                toast.items_start().py(px(10.0))
            } else {
                toast.items_center().py(px(6.0))
            }
        })
        .shadow(shadow)
        .on_mouse_down(MouseButton::Left, stop)
        .on_mouse_down(MouseButton::Right, stop)
        .on_mouse_down(MouseButton::Middle, stop)
        .on_mouse_move({
            let hover = Rc::clone(&hover);
            move |_, window, cx| hover(id, true, window, cx)
        })
        .on_hover(move |hovering: &bool, window, cx| {
            if !*hovering {
                hover(id, false, window, cx);
            }
        })
        .child(accent_bar(
            stripe,
            px(6.0),
            px(if expanded { 10.0 } else { 6.0 }),
        ))
        .child(severity_mark(content.severity.glyph(), stripe, swatch))
        .child(if expanded {
            toast_body(id, content, button, swatch, handlers)
        } else {
            collapsed_body(content)
        })
        .child(dismiss_button(
            id,
            content,
            button == Some(content.actions.len()),
            swatch,
            handlers,
        ));
    if announce {
        // A leaf added when the toast announces: AccessKit announces a live
        // node when it is added, on macOS from its value and on AT-SPI from
        // its name, which a label takes from its value. Live on the toast
        // itself would pass to its buttons, which would announce too. The
        // leaf keeps its id while it is held, so it announces only once.
        let live = content.politeness();
        toast = toast.a11y_synthetic_children(move |builder| {
            let id = builder.synthetic_node_id("announcement");
            let mut node = Node::new(Role::Label);
            node.set_value(text.clone());
            node.set_live(live);
            builder.push_child(id, node);
        });
    }
    if let Some(progress) = progress {
        // Inset by the corner radius and lifted off the border, so the line
        // stays inside the rounded panel; the content mask is rectangular.
        toast = toast.child(
            div()
                .absolute()
                .left(px(TOAST_RADIUS))
                .right(px(TOAST_RADIUS))
                .bottom(px(3.0))
                .h(px(2.0))
                .child(
                    div()
                        .h_full()
                        .w(relative(progress))
                        .rounded_full()
                        .bg(swatch.accent)
                        .debug_selector(|| {
                            format!("toast-progress-{progress_selector}")
                        }),
                ),
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
    fn the_accessible_text_reads_severity_then_each_part_as_a_sentence() {
        let notice = |severity, title: &str, message: &str| NoticeContent {
            severity,
            source: NoticeSource::Command,
            title: title.to_owned(),
            message: message.to_owned(),
            location: None,
            actions: Vec::new(),
            lifetime: Lifetime::Expiring,
        };
        assert_eq!(
            notice(Severity::Error, "Copy failed", "No selection")
                .accessible_text(),
            "Error: Copy failed. No selection"
        );
        assert_eq!(
            notice(Severity::Warning, "Restore skipped.", "  State is newer ")
                .location("~/.local/state/huterm/sessions.json")
                .accessible_text(),
            "Warning: Restore skipped. State is newer. \
             ~/.local/state/huterm/sessions.json",
            "existing punctuation is kept, parts are trimmed"
        );
        assert_eq!(
            notice(Severity::Info, "", "build done").accessible_text(),
            "Info: build done",
            "an empty title is left out"
        );
        assert_eq!(notice(Severity::Error, "", " ").accessible_text(), "Error");
    }

    #[test]
    fn only_failures_of_commands_and_terminals_interrupt() {
        let politeness = |severity, source| {
            NoticeContent {
                severity,
                ..content(source, "t", Lifetime::Expiring)
            }
            .politeness()
        };
        assert_eq!(
            politeness(Severity::Error, NoticeSource::Command),
            Live::Assertive
        );
        assert_eq!(politeness(Severity::Error, terminal(1)), Live::Assertive);
        for source in [
            NoticeSource::Config,
            NoticeSource::Keymap,
            NoticeSource::Sessions,
            NoticeSource::Program {
                tab: TabId::new(1),
                title: "tab".to_owned(),
            },
        ] {
            assert_eq!(
                politeness(Severity::Error, source.clone()),
                Live::Polite,
                "{source:?}"
            );
        }
        assert_eq!(
            politeness(Severity::Warning, NoticeSource::Command),
            Live::Polite
        );
        assert_eq!(politeness(Severity::Info, terminal(1)), Live::Polite);
    }

    #[test]
    fn each_notice_announces_once_when_first_shown_in_front() {
        let now = Instant::now();
        let at = |ms: u64| now + Duration::from_millis(ms);
        let mut stack = NoticeStack::default();
        let push = |stack: &mut NoticeStack, tab: u64| {
            stack.push(content(terminal(tab), "n", Lifetime::Persistent), now)
        };
        let set = |ids: &[NoticeId]| ids.iter().copied().collect();
        let mounts = |stack: &NoticeStack, id| stack.get(id).unwrap().mounts;
        let first = push(&mut stack, 0);
        assert!(
            stack.begin_frame(false, at(0)).is_empty(),
            "nothing announces while the window is behind"
        );
        assert_eq!(stack.begin_frame(true, at(100)), set(&[first]));
        assert_eq!(
            stack.begin_frame(true, at(600)),
            set(&[first]),
            "the leaf holds across frames"
        );
        assert_eq!(
            stack.schedule(),
            AnimationSchedule::at(at(1100)),
            "a wake removes the leaf"
        );
        assert!(!stack.release_announcements(at(1099)));
        assert!(stack.release_announcements(at(1100)));
        assert!(
            stack.begin_frame(true, at(1100)).is_empty(),
            "announced once"
        );
        assert_eq!(stack.schedule(), AnimationSchedule::IDLE);
        assert_eq!(mounts(&stack, first), 1, "drawn ever since");

        // Three newer notices push the first into the overflow, and its
        // return after a dismissal is a new node that stays quiet.
        let newer: Vec<NoticeId> =
            (1..4).map(|tab| push(&mut stack, tab)).collect();
        assert_eq!(stack.begin_frame(true, at(2000)), set(&newer));
        assert_eq!(stack.overflow_mounts, 1, "the chip appeared");
        assert!(stack.dismiss(newer[2]));
        assert_eq!(
            stack.begin_frame(true, at(2100)),
            set(&newer[..2]),
            "held leaves stay, and the returning toast is quiet"
        );
        assert_eq!(mounts(&stack, first), 2, "a new node on return");
        assert_eq!(stack.overflow_mounts, 1);

        // A toast that leaves and returns while its leaf is held comes back
        // without it, since its new node would announce again.
        let x = push(&mut stack, 8);
        let y = push(&mut stack, 9);
        stack.begin_frame(true, at(2200));
        assert!(stack.dismiss(x) && stack.dismiss(y));
        assert_eq!(
            stack.begin_frame(true, at(2300)),
            set(&[newer[1]]),
            "newer[0] returned as a new node"
        );
        assert!(stack.begin_frame(true, at(3300)).is_empty());

        // Notices that overflowed before any frame showed them announce
        // when they are first shown.
        let hidden: Vec<NoticeId> =
            (4..8).map(|tab| push(&mut stack, tab)).collect();
        assert_eq!(stack.begin_frame(true, at(4000)), set(&hidden[1..]));
        assert!(stack.dismiss(hidden[3]));
        assert_eq!(
            stack.begin_frame(true, at(5000)),
            set(&[hidden[0]]),
            "first shown now"
        );
        assert!(stack.dismiss_all());
        assert!(stack.begin_frame(true, at(5100)).is_empty());
        assert!(!stack.overflow_drawn);
    }

    #[test]
    fn program_notices_keep_one_per_tab_and_outlive_its_failures() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        let program = |tab: u64| NoticeSource::Program {
            tab: TabId::new(tab),
            title: format!("tab {tab}"),
        };
        for title in ["first", "second", "third"] {
            stack.replace_source(
                &program(1),
                vec![content(program(1), title, Lifetime::Expiring)],
                now,
            );
        }
        stack.replace_source(
            &program(2),
            vec![content(program(2), "other tab", Lifetime::Expiring)],
            now,
        );
        assert_eq!(titles(&stack), ["other tab", "third"]);
        // A failure replaces the tab's terminal notices, not its program's.
        stack.replace_source(
            &terminal(1),
            vec![content(terminal(1), "failed", Lifetime::Expiring)],
            now,
        );
        assert_eq!(titles(&stack), ["failed", "other tab", "third"]);
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
    fn frames_run_only_while_a_visible_expiring_toast_is_unpaused() {
        let start = Instant::now();
        let mut stack = NoticeStack::default();
        assert_eq!(stack.schedule(), AnimationSchedule::IDLE);
        stack.push(
            content(NoticeSource::Config, "config", Lifetime::Persistent),
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
        assert_eq!(
            stack.schedule(),
            AnimationSchedule {
                frame: true,
                deadline: Some(start + NOTICE_LIFETIME),
            },
            "the progress lines move every frame, and the earliest expiry \
             still wakes a window that draws none"
        );
        stack.pause(start + Duration::from_secs(1));
        assert_eq!(stack.schedule(), AnimationSchedule::IDLE, "paused");
        stack.resume(start + Duration::from_secs(2));
        assert_eq!(
            stack.schedule(),
            AnimationSchedule {
                frame: true,
                deadline: Some(
                    start + NOTICE_LIFETIME + Duration::from_secs(1)
                ),
            }
        );

        // Three newer persistent notices push the expiring ones into the
        // overflow, which draws no progress line.
        for tab in 2..5 {
            stack.push(
                content(terminal(tab), "persistent", Lifetime::Persistent),
                start,
            );
        }
        assert_eq!(
            stack.schedule(),
            AnimationSchedule::at(
                start + NOTICE_LIFETIME + Duration::from_secs(1)
            ),
            "hidden expiring notices wake only to expire"
        );
    }

    #[test]
    fn the_fit_narrows_to_the_area_and_collapses_below_a_scaled_height() {
        let design = px(16.0);
        let fit =
            StackFit::new(Size::new(px(800.0), px(600.0)), px(12.0), design);
        assert_eq!(fit.toast_width, px(TOAST_WIDTH));
        assert_eq!(fit.max_height, px(600.0), "the area's whole height");
        assert_eq!(
            fit.region_padding().bottom,
            px(8.0),
            "the region reaches the area's edge below the newest toast"
        );
        assert!(!fit.compact);
        let narrow =
            StackFit::new(Size::new(px(300.0), px(600.0)), px(12.0), design);
        assert_eq!(narrow.toast_width, px(276.0), "12px either side");
        // The threshold is 320 design points: 320px at the design size and
        // 1.25 times that at a 1.25 interface scale.
        let at = |height: f32, rem: Pixels| {
            StackFit::new(Size::new(px(800.0), px(height)), px(12.0), rem)
                .compact
        };
        assert!(!at(320.0, design));
        assert!(at(319.0, design));
        assert!(at(399.0, px(20.0)), "larger interface text");
        assert!(!at(400.0, px(20.0)));
    }

    #[test]
    fn slots_put_the_overflow_chip_first_and_the_newest_toast_last() {
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        let ids: Vec<NoticeId> = (0..4)
            .map(|index| {
                stack.push(
                    content(terminal(index), "n", Lifetime::Persistent),
                    now,
                )
            })
            .collect();
        // Visible, newest first: ids 3, 2, 1, under a chip for id 0.
        assert_eq!(stack.slot(ids[3]), Some(3), "the newest is last");
        assert_eq!(stack.slot(ids[1]), Some(1), "the oldest after the chip");
        assert_eq!(stack.slot(ids[0]), None, "hidden in the overflow");
        assert!(stack.dismiss(ids[0]));
        assert_eq!(stack.slot(ids[1]), Some(0), "no chip");
        assert_eq!(stack.slot(ids[3]), Some(2));
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
