//! Process descriptor limit handling, following Ghostty's `fixMaxFiles`.

#[cfg(unix)]
use nix::sys::resource::{
    RLIM_INFINITY, Resource, getrlimit, rlim_t, setrlimit,
};
#[cfg(unix)]
use std::sync::OnceLock;

/// Highest soft limit searched when the hard limit is unlimited.
#[cfg(unix)]
const SEARCH_CEILING: rlim_t = 1 << 20;

/// The `RLIMIT_NOFILE` soft and hard limits before the first raise. `None`
/// inside means the query failed, so there is nothing to restore.
#[cfg(unix)]
static ORIGINAL: OnceLock<Option<(rlim_t, rlim_t)>> = OnceLock::new();

/// Raises this process's soft open-file limit as far as the system allows.
///
/// Only the first call changes the limit. Terminal children start with the
/// original limit, because `select(2)` users can fail with descriptors at or
/// above `FD_SETSIZE`. Call this at startup in every binary that hosts
/// terminals.
pub fn raise_open_file_limit() {
    #[cfg(unix)]
    ORIGINAL.get_or_init(|| {
        raise_with(getrlimit(Resource::RLIMIT_NOFILE).ok(), |soft, hard| {
            setrlimit(Resource::RLIMIT_NOFILE, soft, hard).is_ok()
        })
    });
}

/// Returns the limit recorded by [`raise_open_file_limit`], if any.
#[cfg(unix)]
pub(crate) fn original_open_file_limit() -> Option<(rlim_t, rlim_t)> {
    ORIGINAL.get().copied().flatten()
}

/// Sets the soft limit to a finite hard limit. When the hard limit is
/// unlimited, or the system rejects it as a soft limit, binary-searches the
/// highest accepted soft limit below it or [`SEARCH_CEILING`]. `set` applies
/// a soft and hard limit and reports success. Returns the queried original,
/// unchanged.
#[cfg(unix)]
fn raise_with(
    original: Option<(rlim_t, rlim_t)>,
    mut set: impl FnMut(rlim_t, rlim_t) -> bool,
) -> Option<(rlim_t, rlim_t)> {
    let (soft, hard) = original?;
    if soft >= hard {
        return original;
    }
    let (mut low, mut high) = if hard == RLIM_INFINITY {
        (soft, SEARCH_CEILING)
    } else if set(hard, hard) {
        return original;
    } else {
        (soft, hard)
    };
    while low + 1 < high {
        let candidate = low + (high - low) / 2;
        if set(candidate, hard) {
            low = candidate;
        } else {
            high = candidate;
        }
    }
    original
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Applies accepted limits the way the kernel would, recording calls.
    struct FakeLimit {
        soft: rlim_t,
        ceiling: rlim_t,
        calls: Vec<(rlim_t, rlim_t)>,
    }

    impl FakeLimit {
        fn new(soft: rlim_t, ceiling: rlim_t) -> Self {
            Self {
                soft,
                ceiling,
                calls: Vec::new(),
            }
        }

        fn set(&mut self, soft: rlim_t, hard: rlim_t) -> bool {
            self.calls.push((soft, hard));
            let accepted = soft <= self.ceiling;
            if accepted {
                self.soft = soft;
            }
            accepted
        }
    }

    #[test]
    fn limit_already_at_its_maximum_is_recorded_unchanged() {
        let mut limit = FakeLimit::new(4096, rlim_t::MAX);
        let original =
            raise_with(Some((4096, 4096)), |soft, hard| limit.set(soft, hard));
        assert_eq!(original, Some((4096, 4096)));
        assert!(limit.calls.is_empty());
    }

    #[test]
    fn finite_hard_limit_becomes_the_soft_limit_without_searching() {
        let mut limit = FakeLimit::new(256, rlim_t::MAX);
        let original =
            raise_with(Some((256, 10_240)), |soft, hard| limit.set(soft, hard));
        assert_eq!(original, Some((256, 10_240)));
        assert_eq!(limit.calls, [(10_240, 10_240)]);
        assert_eq!(limit.soft, 10_240);
    }

    #[test]
    fn rejected_finite_hard_limit_falls_back_to_searching_below_it() {
        let mut limit = FakeLimit::new(256, 5_000);
        let original =
            raise_with(Some((256, 10_240)), |soft, hard| limit.set(soft, hard));
        assert_eq!(original, Some((256, 10_240)));
        assert_eq!(limit.calls[0], (10_240, 10_240));
        assert_eq!(limit.soft, 5_000);
        assert!(limit.calls.iter().all(|(_, hard)| *hard == 10_240));
    }

    #[test]
    fn unlimited_hard_limit_searches_for_the_highest_accepted_soft_limit() {
        let mut limit = FakeLimit::new(256, 245_760);
        let original = raise_with(Some((256, RLIM_INFINITY)), |soft, hard| {
            limit.set(soft, hard)
        });
        assert_eq!(original, Some((256, RLIM_INFINITY)));
        assert_eq!(limit.soft, 245_760);
        assert!(limit.calls.iter().all(|(soft, hard)| {
            *soft < SEARCH_CEILING && *hard == RLIM_INFINITY
        }));
        assert!(limit.calls.len() <= 21, "{} calls", limit.calls.len());
    }

    #[test]
    fn failed_query_records_nothing_and_changes_nothing() {
        let mut limit = FakeLimit::new(256, rlim_t::MAX);
        assert_eq!(raise_with(None, |soft, hard| limit.set(soft, hard)), None);
        assert!(limit.calls.is_empty());
    }
}
