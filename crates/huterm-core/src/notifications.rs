//! Policy for terminal-requested desktop notifications (OSC 9 and OSC 777):
//! turning untrusted bytes into bounded plain text, and limiting how often
//! one terminal may notify.

use std::time::{Duration, Instant};

use huterm_protocol::TerminalNotification;

/// Characters a notification title keeps; longer titles end in an ellipsis.
pub(crate) const TITLE_CHAR_LIMIT: usize = 128;
/// Characters a notification body keeps; longer bodies end in an ellipsis.
pub(crate) const BODY_CHAR_LIMIT: usize = 512;
/// Notifications one terminal may send at once before the limit applies.
pub(crate) const NOTIFICATION_BURST: u32 = 4;
/// How long one terminal waits for each notification after its burst.
pub(crate) const NOTIFICATION_INTERVAL: Duration = Duration::from_secs(3);

/// Builds a notification from the bytes a program sent, or `None` when
/// nothing presentable remains.
///
/// Invalid UTF-8 becomes U+FFFD. Control characters, which could move a
/// presenter's cursor or forge line structure, become spaces; Unicode
/// bidirectional formatting characters, which can make text read in a
/// different order than it was written, are removed. Whitespace runs
/// collapse to one space. Each field is truncated to its limit.
///
/// OSC 9 also carries `ConEmu` commands such as `9;4` progress. Ghostty
/// reports a `ConEmu` command it does not recognize, such as `9;4;9`, as a
/// notification whose text starts with digits and a semicolon; that text is
/// a malformed command, not a message, so it is dropped.
pub(crate) fn normalize(
    title: &[u8],
    body: &[u8],
) -> Option<TerminalNotification> {
    if title.is_empty() && looks_like_conemu_command(body) {
        return None;
    }
    let title = clean(title, TITLE_CHAR_LIMIT);
    let body = clean(body, BODY_CHAR_LIMIT);
    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(TerminalNotification::new(title, body))
}

fn looks_like_conemu_command(body: &[u8]) -> bool {
    let digits = body.iter().take_while(|byte| byte.is_ascii_digit()).count();
    digits > 0 && matches!(body.get(digits), None | Some(b';'))
}

fn clean(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len().min(limit * 4));
    let mut kept = 0;
    let mut pending_space = false;
    for character in text.chars() {
        if is_bidi_control(character) {
            continue;
        }
        if character.is_control() || character.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        let needed = usize::from(pending_space) + 1;
        if kept + needed > limit {
            out.push('…');
            return out;
        }
        if pending_space {
            out.push(' ');
            kept += 1;
            pending_space = false;
        }
        out.push(character);
        kept += 1;
    }
    out
}

/// Unicode's explicit bidirectional formatting characters.
const fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// A token bucket: `burst` notifications at once, then one every
/// `interval`. Notifications over the limit are dropped, never queued for
/// later. Each terminal's runtime keeps one at [`NOTIFICATION_BURST`] and
/// [`NOTIFICATION_INTERVAL`]; a client may keep its own across terminals.
#[derive(Debug)]
pub struct NotificationLimiter {
    burst: u32,
    interval: Duration,
    available: u32,
    /// When the next token is earned; `None` while the bucket is full.
    next_token: Option<Instant>,
}

impl Default for NotificationLimiter {
    fn default() -> Self {
        Self::new(NOTIFICATION_BURST, NOTIFICATION_INTERVAL)
    }
}

impl NotificationLimiter {
    /// A full bucket of `burst` tokens that refills one every `interval`.
    #[must_use]
    pub const fn new(burst: u32, interval: Duration) -> Self {
        Self {
            burst,
            interval,
            available: burst,
            next_token: None,
        }
    }

    /// Takes a token at `now`, or returns false when none is left.
    pub fn admit(&mut self, now: Instant) -> bool {
        while let Some(due) = self.next_token
            && now >= due
        {
            self.available += 1;
            self.next_token =
                (self.available < self.burst).then(|| due + self.interval);
        }
        if self.available == 0 {
            return false;
        }
        self.available -= 1;
        if self.next_token.is_none() {
            self.next_token = Some(now + self.interval);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(title: &str, body: &str) -> Option<(String, String)> {
        normalize(title.as_bytes(), body.as_bytes())
            .map(|note| (note.title().to_owned(), note.body().to_owned()))
    }

    #[test]
    fn controls_become_spaces_and_bidi_controls_are_removed() {
        assert_eq!(
            text("Build\tdone", " line one\r\nline\x1b[31mtwo\u{85} "),
            Some(("Build done".into(), "line one line [31mtwo".into()))
        );
        // An override that would show "exe.txt" as "txt.exe" is dropped.
        assert_eq!(
            text("", "invoice\u{202E}txt.exe\u{2066}x\u{2069}"),
            Some((String::new(), "invoicetxt.exex".into()))
        );
        assert_eq!(
            normalize(b"", b"bad \xff byte").unwrap().body(),
            "bad \u{FFFD} byte"
        );
    }

    #[test]
    fn fields_are_truncated_at_their_character_limits() {
        let exact = "é".repeat(BODY_CHAR_LIMIT);
        assert_eq!(text("", &exact).unwrap().1, exact);
        let long =
            text(&"t".repeat(TITLE_CHAR_LIMIT + 1), &format!("{exact}é"))
                .unwrap();
        assert_eq!(long.0.chars().count(), TITLE_CHAR_LIMIT + 1);
        assert!(long.0.ends_with("t…"));
        assert_eq!(long.1, format!("{exact}…"));
    }

    #[test]
    fn empty_text_and_malformed_conemu_commands_are_dropped() {
        assert_eq!(text("", ""), None);
        assert_eq!(text(" \x07 ", "\u{202E}\t"), None);
        assert_eq!(text("", "4;9"), None);
        assert_eq!(text("", "10"), None);
        // A title means OSC 777, which carries no ConEmu commands.
        assert_eq!(text("T", "4;9"), Some(("T".into(), "4;9".into())));
        assert_eq!(
            text("", "4 tests passed"),
            Some((String::new(), "4 tests passed".into()))
        );
        assert_eq!(text("Done", ""), Some(("Done".into(), String::new())));
    }

    #[test]
    fn the_limiter_allows_a_burst_then_one_per_interval() {
        let start = Instant::now();
        let mut limiter = NotificationLimiter::default();
        for _ in 0..NOTIFICATION_BURST {
            assert!(limiter.admit(start));
        }
        assert!(!limiter.admit(start), "the burst is spent");
        let almost = start
            + NOTIFICATION_INTERVAL.saturating_sub(Duration::from_millis(1));
        assert!(!limiter.admit(almost));
        assert!(limiter.admit(start + NOTIFICATION_INTERVAL));
        assert!(!limiter.admit(start + NOTIFICATION_INTERVAL));
        // A long silence refills the burst, and no more.
        let later = start + NOTIFICATION_INTERVAL * 100;
        for _ in 0..NOTIFICATION_BURST {
            assert!(limiter.admit(later));
        }
        assert!(!limiter.admit(later));
    }
}
