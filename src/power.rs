//! What the Android app holds for rettui while it runs (`RettuiService`
//! asks through `Native.wants()`): a wake lock, which keeps the phone's
//! processor from sleeping, and a multicast lock, without which Wi-Fi drops
//! the multicast AutoInterface finds its peers by. Both cost battery, so
//! each only when needed. Elsewhere nothing reads them.

use std::sync::atomic::{AtomicBool, Ordering};

static WAKE: AtomicBool = AtomicBool::new(false);
static MULTICAST: AtomicBool = AtomicBool::new(false);

/// The wake lock: the Keep the phone awake setting.
pub fn set_wake(on: bool) {
    WAKE.store(on, Ordering::Relaxed);
}

/// The multicast lock: an AutoInterface is on in the config Reticulum
/// starts with.
pub fn set_multicast(on: bool) {
    MULTICAST.store(on, Ordering::Relaxed);
}

/// Bit 1 the wake lock, bit 2 the multicast lock.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn wants() -> u8 {
    u8::from(WAKE.load(Ordering::Relaxed)) | (u8::from(MULTICAST.load(Ordering::Relaxed)) << 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wants_both_locks_as_bits() {
        super::set_wake(true);
        super::set_multicast(false);
        assert_eq!(super::wants(), 1);
        super::set_multicast(true);
        assert_eq!(super::wants(), 3);
        super::set_wake(false);
        assert_eq!(super::wants(), 2);
        super::set_multicast(false);
        assert_eq!(super::wants(), 0);
    }
}
