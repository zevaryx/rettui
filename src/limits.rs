//! The process's open-file limit.
//!
//! Every connection a server interface accepts, every socket and every file
//! rettui opens takes a descriptor. The usual soft limit (1,024 on many
//! Linux desktops) is what peers' connections share with rettui's own web
//! server, its database and its files; a hub that raises the connection limits
//! (`RSRETICULUM_MAX_SERVER_CONNECTIONS`) needs more than that to use them.
//! The hard limit is usually far higher, and a process may raise its own soft
//! limit up to it, so rettui does at start.

/// The soft limit rettui asks for: room for the library's default connection
/// limits (256 for each server interface) many times over, and for a hub that
/// raises them to a few thousand. Not the whole hard limit (often a million):
/// programs rettui starts inherit it, and some of them close or scan every
/// descriptor up to it.
#[cfg(unix)]
pub const OPEN_FILES_WANTED: u64 = 16_384;

/// The soft limit to set, given the current soft and hard limits: `wanted`
/// within what the system allows, and never lower than now. `None`: nothing
/// to change.
#[cfg(unix)]
fn new_soft_limit(soft: u64, hard: u64, wanted: u64) -> Option<u64> {
    let target = wanted.min(hard);
    // macOS refuses a soft limit above this, whatever the hard limit says.
    #[cfg(target_os = "macos")]
    let target = target.min(10_240);
    (target > soft).then_some(target)
}

/// A limit as `u64`: `rlim_t` is `u32` on 32-bit Android and `u64` elsewhere, so the
/// cast is needed on one and a no-op on the other.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn wide(limit: libc::rlim_t) -> u64 {
    limit as u64
}

/// Raise the soft open-file limit towards [`OPEN_FILES_WANTED`]. Returns the
/// limit it set, or `None` if it was already enough or couldn't be raised
/// (it stays as it was).
#[cfg(unix)]
pub fn raise_open_files() -> Option<u64> {
    let mut current = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `current` is a valid, writable `rlimit` for the call to fill in.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut current) } != 0 {
        return None;
    }
    let target = new_soft_limit(wide(current.rlim_cur), wide(current.rlim_max), OPEN_FILES_WANTED)?;
    let raised = libc::rlimit { rlim_cur: libc::rlim_t::try_from(target).ok()?, rlim_max: current.rlim_max };
    // SAFETY: `raised` is a valid `rlimit`; the call only reads it.
    (unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0).then_some(target)
}

/// Windows has no such limit to raise.
#[cfg(not(unix))]
pub fn raise_open_files() -> Option<u64> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_goes_up_to_what_is_wanted_and_the_hard_limit() {
        // macOS refuses more than 10,240 whatever the hard limit says.
        let allowed = |limit: u64| if cfg!(target_os = "macos") { limit.min(10_240) } else { limit };
        assert_eq!(new_soft_limit(1024, 524_288, 16_384), Some(allowed(16_384)));
        assert_eq!(new_soft_limit(1024, 4096, 16_384), Some(4096));
        assert_eq!(new_soft_limit(1024, u64::MAX, 16_384), Some(allowed(16_384)));
    }

    #[test]
    fn a_limit_that_is_enough_or_cannot_go_higher_is_left_alone() {
        assert_eq!(new_soft_limit(16_384, 524_288, 16_384), None);
        assert_eq!(new_soft_limit(65_536, 65_536, 16_384), None);
        assert_eq!(new_soft_limit(4096, 4096, 16_384), None);
        assert_eq!(new_soft_limit(0, 0, 16_384), None);
    }

    #[test]
    fn raising_the_limit_never_lowers_it_and_stays_within_the_hard_limit() {
        let read = || {
            let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: `limit` is a valid, writable `rlimit`.
            assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
            (wide(limit.rlim_cur), wide(limit.rlim_max))
        };
        let (soft, hard) = read();
        let raised = raise_open_files();
        let (soft_after, hard_after) = read();
        assert_eq!(hard, hard_after);
        assert!(soft_after >= soft && soft_after <= hard);
        match raised {
            Some(limit) => assert_eq!(limit, soft_after),
            None => assert_eq!(soft, soft_after),
        }
        assert_eq!(raise_open_files(), None, "a second call has nothing to do");
    }
}
