//! The file status in the Settings sidebar footer, and the notice that
//! spells out a warning or failure above the section, without GPUI.

/// How the footer colors its status dot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    Ok,
    Idle,
    Warning,
    Error,
}

/// What the footer's link next to the status does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Reload,
    Open,
}

/// The latest thing that happened to the file, which the footer reports
/// until the next one.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Saving,
    Saved,
    Reloaded,
    /// The file changed on disk while watching was off, so the running
    /// settings and the controls are older than the file.
    ChangedOnDisk,
    /// The file does not parse or does not validate.
    Invalid(String),
    /// A write failed and left the file as it was.
    Failed(String),
    /// Reading the file failed, so the controls show an older read.
    ReadFailed(String),
    /// A write replaced the file, but making that durable failed.
    SavedUnsure(String),
    /// A reload failed for a reason outside the file, such as a font that
    /// is not installed.
    NotApplied(String),
    /// A reload applied the file, but some of what it asks for is
    /// unavailable, such as global shortcuts other applications hold: one
    /// message each.
    AppliedWithWarning(Vec<String>),
    /// Creating or opening the file failed.
    FileError(String),
}

/// The footer's status line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Line {
    pub(crate) tone: Tone,
    pub(crate) text: String,
    pub(crate) action: Option<Action>,
}

/// A warning or failure spelled out above the section, since the footer
/// line has room for only its summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Notice {
    /// [`Tone::Warning`] or [`Tone::Error`].
    pub(crate) tone: Tone,
    /// The summary the footer line shows too.
    pub(crate) title: String,
    /// What went wrong, one message per problem.
    pub(crate) messages: Vec<String>,
    pub(crate) action: Option<Action>,
    /// Whether it shows its messages: a failure always does, a warning
    /// once the user unfolds it.
    pub(crate) open: bool,
}

impl Notice {
    /// Whether it starts with its messages hidden: a warning, which the
    /// user may deal with later, unlike a failure.
    pub(crate) fn collapsible(&self) -> bool {
        self.tone == Tone::Warning
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileStatus {
    watching: bool,
    latest: Option<Event>,
    /// The warning or failure the notice spells out. The saves and reloads
    /// that follow it leave it in place until their outcome settles or
    /// replaces it, so the notice never vanishes and returns while they
    /// run.
    problem: Option<Event>,
    /// The user unfolded the warning; it stays unfolded while the problem
    /// lasts, even as a reload updates its messages.
    notice_open: bool,
    /// The next reload is the one a save asked for, so it keeps "Saved".
    own_reload: bool,
}

impl FileStatus {
    pub(crate) fn new(watching: bool) -> Self {
        Self {
            watching,
            latest: None,
            problem: None,
            notice_open: false,
            own_reload: false,
        }
    }

    fn report(&mut self, event: Event) {
        self.latest = Some(event);
    }

    /// Reports a warning or failure, which the notice spells out.
    fn report_problem(&mut self, event: Event) {
        self.problem = Some(event.clone());
        self.report(event);
    }

    /// Drops the notice's problem when `settles` says this outcome settles
    /// it.
    fn settle_problem(&mut self, settles: impl Fn(&Event) -> bool) {
        if self.problem.as_ref().is_some_and(settles) {
            self.problem = None;
            self.notice_open = false;
        }
    }

    pub(crate) fn watching(&mut self, watching: bool) {
        self.watching = watching;
    }

    pub(crate) fn saving(&mut self) {
        self.report(Event::Saving);
    }

    /// A write succeeded, which settles a write's problem. A reload's
    /// problem waits for the reload the save asks for.
    pub(crate) fn saved(&mut self) {
        self.report(Event::Saved);
        self.settle_problem(|problem| {
            matches!(
                problem,
                Event::Failed(_)
                    | Event::ReadFailed(_)
                    | Event::SavedUnsure(_)
                    | Event::FileError(_)
            )
        });
        self.own_reload = true;
    }

    /// A reload applied the file. The reload a save asked for keeps
    /// reporting the save, as does one while a save is running.
    pub(crate) fn reloaded(&mut self) {
        // A failure to create or open the file, or to run a command, lasts
        // until the next reload, whose report replaced it before the
        // notice held its place.
        self.settle_problem(|problem| matches!(problem, Event::FileError(_)));
        let own = std::mem::take(&mut self.own_reload);
        let saving = matches!(self.latest, Some(Event::Saving));
        let saved =
            matches!(self.latest, Some(Event::Saved | Event::SavedUnsure(_)));
        if !(saving || own && saved) {
            self.report(Event::Reloaded);
        }
    }

    /// A reload read the file as this window last wrote it, such as the
    /// watcher catching up with a save after that save's own reload. It
    /// keeps a save's report; anything else, such as a refused reload that
    /// now applied, reports the reload.
    pub(crate) fn reloaded_unchanged(&mut self) {
        if !matches!(self.latest, Some(Event::Saved | Event::SavedUnsure(_))) {
            self.reloaded();
        }
    }

    /// A write replaced the file but could not make it durable. Like
    /// "Saved", the warning stays through the reload the save asks for.
    pub(crate) fn saved_with_warning(&mut self, message: impl Into<String>) {
        self.report_problem(Event::SavedUnsure(message.into()));
        self.own_reload = true;
    }

    pub(crate) fn not_applied(&mut self, message: impl Into<String>) {
        self.report_problem(Event::NotApplied(message.into()));
    }

    /// A reload applied the file with `warnings`. It replaces even a save's
    /// report, since the save took effect only in part.
    pub(crate) fn applied_with_warnings(&mut self, warnings: Vec<String>) {
        self.report_problem(Event::AppliedWithWarning(warnings));
    }

    /// A reload's reread found that it applied the file in full, which
    /// settles a reload's problem. The footer reports the reload through
    /// [`Self::reloaded`].
    pub(crate) fn applied(&mut self) {
        self.settle_problem(|problem| {
            matches!(
                problem,
                Event::NotApplied(_) | Event::AppliedWithWarning(_)
            )
        });
    }

    /// Reading the file failed.
    pub(crate) fn read_failed(&mut self, message: impl Into<String>) {
        self.report_problem(Event::ReadFailed(message.into()));
    }

    /// A read succeeded, which settles a failed one.
    pub(crate) fn read(&mut self) {
        if matches!(self.latest, Some(Event::ReadFailed(_))) {
            self.latest = None;
        }
        self.settle_problem(|problem| matches!(problem, Event::ReadFailed(_)));
    }

    pub(crate) fn changed_on_disk(&mut self) {
        if !self.invalid() {
            self.report(Event::ChangedOnDisk);
        }
    }

    /// The file does not parse or validate. Its banner replaces the
    /// notice.
    pub(crate) fn invalid_file(&mut self, message: impl Into<String>) {
        self.report(Event::Invalid(message.into()));
        self.settle_problem(|_| true);
    }

    /// Creating or opening the file failed. An invalid file keeps its
    /// report, which also keeps the controls read-only.
    pub(crate) fn file_error(&mut self, message: impl Into<String>) {
        if !self.invalid() {
            self.report_problem(Event::FileError(message.into()));
        }
    }

    pub(crate) fn failed(&mut self, message: impl Into<String>) {
        self.report_problem(Event::Failed(message.into()));
    }

    /// Drops a failed write's report once the user settles it by keeping
    /// the file, so the footer shows the watch status again.
    pub(crate) fn settled(&mut self) {
        if matches!(self.latest, Some(Event::Failed(_))) {
            self.latest = None;
        }
        self.settle_problem(|problem| matches!(problem, Event::Failed(_)));
    }

    /// Shows or hides a warning's messages.
    pub(crate) fn toggle_notice(&mut self) {
        self.notice_open = !self.notice_open;
    }

    /// Whether the file failed to parse or validate, which makes every
    /// control read-only.
    pub(crate) fn invalid(&self) -> bool {
        matches!(self.latest, Some(Event::Invalid(_)))
    }

    pub(crate) fn line(&self) -> Line {
        let line = |tone, text: &str, action| Line {
            tone,
            text: text.to_owned(),
            action,
        };
        match &self.latest {
            Some(Event::Saving) => line(Tone::Idle, "Saving…", None),
            Some(Event::Saved) => line(Tone::Ok, "Saved", None),
            Some(Event::Reloaded) => line(Tone::Ok, "Reloaded", None),
            Some(Event::ChangedOnDisk) => line(
                Tone::Warning,
                "config.toml changed on disk",
                Some(Action::Reload),
            ),
            // With watching off, a fixed file applies only on a reload.
            Some(Event::Invalid(_)) => line(
                Tone::Error,
                "config.toml has an error",
                Some(if self.watching {
                    Action::Open
                } else {
                    Action::Reload
                }),
            ),
            // The notice carries the action, which leaves the line room
            // for its summary.
            Some(
                event @ (Event::Failed(_)
                | Event::ReadFailed(_)
                | Event::FileError(_)
                | Event::SavedUnsure(_)
                | Event::NotApplied(_)
                | Event::AppliedWithWarning(_)),
            ) => {
                let notice = self.notice_of(event).expect("a problem");
                Line {
                    tone: notice.tone,
                    text: notice.title,
                    action: None,
                }
            }
            None if self.watching => {
                line(Tone::Ok, "Watching config.toml", None)
            }
            None => line(
                Tone::Idle,
                "Not watching config.toml",
                Some(Action::Reload),
            ),
        }
    }

    /// The standing warning or failure with its messages, for the notice
    /// above the section. An invalid file has its own banner instead.
    pub(crate) fn notice(&self) -> Option<Notice> {
        self.notice_of(self.problem.as_ref()?)
    }

    fn notice_of(&self, event: &Event) -> Option<Notice> {
        let notice = |tone, title: &str, messages: &[String], action| Notice {
            tone,
            title: title.to_owned(),
            messages: messages.to_vec(),
            action,
            open: tone != Tone::Warning || self.notice_open,
        };
        Some(match event {
            Event::Failed(message) => notice(
                Tone::Error,
                "Not saved",
                std::slice::from_ref(message),
                None,
            ),
            Event::FileError(message) => notice(
                Tone::Error,
                "Failed",
                std::slice::from_ref(message),
                None,
            ),
            Event::ReadFailed(message) => notice(
                Tone::Error,
                "Not read",
                std::slice::from_ref(message),
                Some(Action::Reload),
            ),
            Event::SavedUnsure(message) => notice(
                Tone::Warning,
                "Saved with a warning",
                std::slice::from_ref(message),
                None,
            ),
            Event::NotApplied(message) => notice(
                Tone::Error,
                "Not applied",
                std::slice::from_ref(message),
                Some(Action::Reload),
            ),
            // A reload tries a refused shortcut again.
            Event::AppliedWithWarning(warnings) => notice(
                Tone::Warning,
                &match warnings.len() {
                    1 => "Applied with a warning".to_owned(),
                    count => format!("Applied with {count} warnings"),
                },
                warnings,
                Some(Action::Reload),
            ),
            Event::Saving
            | Event::Saved
            | Event::Reloaded
            | Event::ChangedOnDisk
            | Event::Invalid(_) => return None,
        })
    }

    /// The parser's message while the file is invalid, for the banner.
    pub(crate) fn invalid_message(&self) -> Option<&str> {
        match &self.latest {
            Some(Event::Invalid(message)) => Some(message),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(status: &FileStatus) -> (Tone, String, Option<Action>) {
        let line = status.line();
        (line.tone, line.text, line.action)
    }

    #[test]
    fn the_footer_reports_the_latest_file_event() {
        let mut status = FileStatus::new(true);
        assert_eq!(
            text(&status),
            (Tone::Ok, "Watching config.toml".into(), None)
        );
        status.saving();
        status.reloaded();
        status.saved();
        status.reloaded();
        assert_eq!(text(&status), (Tone::Ok, "Saved".into(), None));
        // A later reload, such as from another editor, reports itself.
        status.reloaded();
        assert_eq!(text(&status), (Tone::Ok, "Reloaded".into(), None));
        status.saved_with_warning("the disk did not confirm it");
        status.reloaded();
        assert_eq!(
            text(&status),
            (Tone::Warning, "Saved with a warning".into(), None)
        );
        // The notice above the section carries Reload.
        status.not_applied("font \"Nope\" is not installed");
        assert_eq!(text(&status), (Tone::Error, "Not applied".into(), None));
        status.failed("permission denied");
        assert_eq!(text(&status), (Tone::Error, "Not saved".into(), None));
        status.reloaded();
        assert_eq!(text(&status), (Tone::Ok, "Reloaded".into(), None));
        status.failed("config.toml changed on disk");
        status.settled();
        assert_eq!(
            text(&status),
            (Tone::Ok, "Watching config.toml".into(), None)
        );

        let mut status = FileStatus::new(false);
        assert_eq!(
            text(&status),
            (
                Tone::Idle,
                "Not watching config.toml".into(),
                Some(Action::Reload)
            )
        );
        status.changed_on_disk();
        assert_eq!(
            text(&status),
            (
                Tone::Warning,
                "config.toml changed on disk".into(),
                Some(Action::Reload)
            )
        );
    }

    #[test]
    fn a_reload_of_the_file_as_saved_keeps_only_a_saves_report() {
        let mut status = FileStatus::new(true);
        status.saved();
        status.reloaded();
        status.reloaded_unchanged();
        assert_eq!(text(&status), (Tone::Ok, "Saved".into(), None));
        status.saved_with_warning("the disk did not confirm it");
        status.reloaded();
        status.reloaded_unchanged();
        assert_eq!(text(&status).0, Tone::Warning);
        // A theme fixed elsewhere, or a font installed, applies the same
        // file.
        status.not_applied("font \"Nope\" is not installed");
        status.reloaded_unchanged();
        assert_eq!(text(&status), (Tone::Ok, "Reloaded".into(), None));
        status.invalid_file("missing theme");
        status.reloaded_unchanged();
        assert!(!status.invalid());
    }

    #[test]
    fn a_refused_shortcut_replaces_a_saves_report_until_a_clean_reload() {
        let mut status = FileStatus::new(true);
        status.saved();
        status.reloaded();
        status.applied_with_warnings(vec![
            "cannot register cmd-shift-space: already in use by another \
             application"
                .to_owned(),
        ]);
        assert_eq!(
            text(&status),
            (Tone::Warning, "Applied with a warning".into(), None)
        );
        // The other application released the shortcut, and the watcher's
        // reload of the same file registers it.
        status.reloaded_unchanged();
        assert_eq!(text(&status), (Tone::Ok, "Reloaded".into(), None));
    }

    #[test]
    fn a_notice_spells_out_each_problem_and_folds_only_warnings() {
        let mut status = FileStatus::new(true);
        status.saved();
        assert_eq!(status.notice(), None);
        let refused = vec![
            "cannot register cmd-shift-space: already in use".to_owned(),
            "cannot register ctrl-alt-t: registration rejected".to_owned(),
        ];
        status.applied_with_warnings(refused.clone());
        let notice = status.notice().expect("a notice");
        assert_eq!(notice.title, "Applied with 2 warnings");
        assert_eq!(notice.messages, refused);
        assert_eq!(notice.action, Some(Action::Reload));
        assert!(notice.collapsible() && !notice.open);
        status.failed("permission denied");
        let notice = status.notice().expect("a notice");
        assert!(!notice.collapsible() && notice.open);
        assert_eq!(
            (notice.tone, notice.title.as_str(), notice.messages),
            (
                Tone::Error,
                "Not saved",
                vec!["permission denied".to_owned()]
            )
        );
        // An invalid file has its own banner.
        status.invalid_file("expected `=` at line 14");
        assert_eq!(status.notice(), None);
    }

    #[test]
    fn a_notice_holds_its_place_and_fold_until_an_outcome_settles_it() {
        let refused = |key: &str| vec![format!("cannot register {key}")];
        let mut status = FileStatus::new(true);
        status.applied_with_warnings(refused("ctrl-alt-t"));
        status.toggle_notice();
        // A change saves, and the save's reload finds the same problem.
        status.saving();
        assert_eq!(text(&status).1, "Saving…");
        status.saved();
        status.reloaded();
        let held = status.notice().expect("the notice holds");
        assert_eq!(held.messages, refused("ctrl-alt-t"));
        assert!(held.open);
        status.applied_with_warnings(refused("cmd-shift-space"));
        let updated = status.notice().expect("an updated notice");
        assert_eq!(updated.messages, refused("cmd-shift-space"));
        assert!(updated.open, "an update keeps the notice unfolded");
        // A reload that applies the file in full settles it, and the next
        // warning starts folded.
        status.applied();
        assert_eq!(status.notice(), None);
        status.applied_with_warnings(refused("ctrl-alt-t"));
        assert!(!status.notice().expect("a new notice").open);

        // A write's failure lasts until a write succeeds.
        status.failed("permission denied");
        status.saving();
        status.applied();
        assert_eq!(
            status.notice().map(|notice| notice.title),
            Some("Not saved".into())
        );
        status.saved();
        assert_eq!(status.notice(), None);
    }

    #[test]
    fn a_failed_read_holds_until_a_read_succeeds() {
        let mut status = FileStatus::new(true);
        status.read_failed("permission denied");
        assert_eq!(text(&status), (Tone::Error, "Not read".into(), None));
        // A reload applying the file leaves an unread file's notice.
        status.reloaded();
        status.applied();
        assert_eq!(
            status.notice().map(|notice| notice.messages),
            Some(vec!["permission denied".to_owned()])
        );
        status.read();
        assert_eq!(status.notice(), None);
        // A failure to create the file, or a command's, lasts until the
        // next reload.
        status.file_error("Cannot create config.toml");
        status.applied();
        assert!(status.notice().is_some());
        status.reloaded();
        assert_eq!(status.notice(), None);
    }

    #[test]
    fn an_invalid_file_holds_until_a_reload_applies_it() {
        let mut status = FileStatus::new(true);
        status.invalid_file("expected `=` at line 14");
        status.changed_on_disk();
        assert!(status.invalid());
        assert_eq!(status.invalid_message(), Some("expected `=` at line 14"));
        assert_eq!(status.line().action, Some(Action::Open));
        // With watching off, the fix applies only on a reload.
        status.watching(false);
        assert_eq!(status.line().action, Some(Action::Reload));
        status.reloaded();
        assert!(!status.invalid());
        assert_eq!(status.invalid_message(), None);
    }
}
