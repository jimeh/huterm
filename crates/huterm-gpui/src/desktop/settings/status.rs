//! The file status in the Settings sidebar footer, without GPUI.

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
    /// A write replaced the file, but making that durable failed.
    SavedUnsure(String),
    /// A reload failed for a reason outside the file, such as a font that
    /// is not installed.
    NotApplied(String),
    /// A reload applied the file, but something it asks for is unavailable,
    /// such as a global shortcut another application holds.
    AppliedWithWarning(String),
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileStatus {
    watching: bool,
    latest: Option<Event>,
    /// The next reload is the one a save asked for, so it keeps "Saved".
    own_reload: bool,
}

impl FileStatus {
    pub(crate) fn new(watching: bool) -> Self {
        Self {
            watching,
            latest: None,
            own_reload: false,
        }
    }

    pub(crate) fn watching(&mut self, watching: bool) {
        self.watching = watching;
    }

    pub(crate) fn saving(&mut self) {
        self.latest = Some(Event::Saving);
    }

    pub(crate) fn saved(&mut self) {
        self.latest = Some(Event::Saved);
        self.own_reload = true;
    }

    /// A reload applied the file. The reload a save asked for keeps
    /// reporting the save, as does one while a save is running.
    pub(crate) fn reloaded(&mut self) {
        let own = std::mem::take(&mut self.own_reload);
        let saving = matches!(self.latest, Some(Event::Saving));
        let saved =
            matches!(self.latest, Some(Event::Saved | Event::SavedUnsure(_)));
        if !(saving || own && saved) {
            self.latest = Some(Event::Reloaded);
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
        self.latest = Some(Event::SavedUnsure(message.into()));
        self.own_reload = true;
    }

    pub(crate) fn not_applied(&mut self, message: impl Into<String>) {
        self.latest = Some(Event::NotApplied(message.into()));
    }

    /// A reload applied the file with a warning. It replaces even a save's
    /// report, since the save took effect only in part.
    pub(crate) fn applied_with_warning(&mut self, message: impl Into<String>) {
        self.latest = Some(Event::AppliedWithWarning(message.into()));
    }

    pub(crate) fn changed_on_disk(&mut self) {
        if !self.invalid() {
            self.latest = Some(Event::ChangedOnDisk);
        }
    }

    pub(crate) fn invalid_file(&mut self, message: impl Into<String>) {
        self.latest = Some(Event::Invalid(message.into()));
    }

    /// Creating or opening the file failed. An invalid file keeps its
    /// report, which also keeps the controls read-only.
    pub(crate) fn file_error(&mut self, message: impl Into<String>) {
        if !self.invalid() {
            self.latest = Some(Event::FileError(message.into()));
        }
    }

    pub(crate) fn failed(&mut self, message: impl Into<String>) {
        self.latest = Some(Event::Failed(message.into()));
    }

    /// Drops a failed write's report once the user settles it by keeping
    /// the file, so the footer shows the watch status again.
    pub(crate) fn settled(&mut self) {
        if matches!(self.latest, Some(Event::Failed(_))) {
            self.latest = None;
        }
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
            Some(Event::Failed(message)) => Line {
                tone: Tone::Error,
                text: format!("Not saved: {message}"),
                action: None,
            },
            Some(Event::FileError(message)) => Line {
                tone: Tone::Error,
                text: message.clone(),
                action: None,
            },
            Some(Event::SavedUnsure(message)) => Line {
                tone: Tone::Warning,
                text: format!("Saved, but {message}"),
                action: None,
            },
            Some(Event::NotApplied(message)) => Line {
                tone: Tone::Error,
                text: format!("Not applied: {message}"),
                action: Some(Action::Reload),
            },
            // A reload tries a refused shortcut again.
            Some(Event::AppliedWithWarning(message)) => Line {
                tone: Tone::Warning,
                text: format!("Applied, but {message}"),
                action: Some(Action::Reload),
            },
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
            (
                Tone::Warning,
                "Saved, but the disk did not confirm it".into(),
                None
            )
        );
        status.not_applied("font \"Nope\" is not installed");
        assert_eq!(
            text(&status),
            (
                Tone::Error,
                "Not applied: font \"Nope\" is not installed".into(),
                Some(Action::Reload)
            )
        );
        status.failed("permission denied");
        assert_eq!(
            text(&status),
            (Tone::Error, "Not saved: permission denied".into(), None)
        );
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
        status.applied_with_warning(
            "cannot register cmd-shift-space: already in use by another \
             application",
        );
        assert_eq!(
            text(&status),
            (
                Tone::Warning,
                "Applied, but cannot register cmd-shift-space: already in use \
                 by another application"
                    .into(),
                Some(Action::Reload)
            )
        );
        // The other application released the shortcut, and the watcher's
        // reload of the same file registers it.
        status.reloaded_unchanged();
        assert_eq!(text(&status), (Tone::Ok, "Reloaded".into(), None));
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
