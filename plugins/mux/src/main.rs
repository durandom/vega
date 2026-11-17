//! Unix coordinator and native Windows remote target subset.
#[cfg(all(unix, not(feature = "target-only")))]
include!("unix.rs");
#[cfg(any(windows, feature = "target-only"))]
include!("windows.rs");
