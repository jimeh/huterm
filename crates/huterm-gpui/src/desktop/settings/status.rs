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
}

impl FileStatus {
    pub(crate) fn new(watching: bool) -> Self {
        Self {
            watching,
            latest: None,
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
    }

    /// A reload applied the file. One that follows this window's own save
    /// keeps reporting the save.
    pub(crate) fn reloaded(&mut self) {
        if !matches!(self.latest, Some(Event::Saving | Event::Saved)) {
            self.latest = Some(Event::Reloaded);
        }
    }

    pub(crate) fn changed_on_disk(&mut self) {
        if !self.invalid() {
            self.latest = Some(Event::ChangedOnDisk);
        }
    }

    pub(crate) fn invalid_file(&mut self, message: impl Into<String>) {
        self.latest = Some(Event::Invalid(message.into()));
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
            Some(Event::Invalid(_)) => line(
                Tone::Error,
                "config.toml has an error",
                Some(Action::Open),
            ),
            Some(Event::Failed(message)) => Line {
                tone: Tone::Error,
                text: format!("Not saved: {message}"),
                action: None,
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
    fn an_invalid_file_holds_until_a_reload_applies_it() {
        let mut status = FileStatus::new(true);
        status.invalid_file("expected `=` at line 14");
        status.changed_on_disk();
        assert!(status.invalid());
        assert_eq!(status.invalid_message(), Some("expected `=` at line 14"));
        assert_eq!(status.line().action, Some(Action::Open));
        status.reloaded();
        assert!(!status.invalid());
        assert_eq!(status.invalid_message(), None);
    }
}
