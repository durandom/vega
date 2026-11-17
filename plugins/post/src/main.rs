//! Agent mail: shared protocol, Unix daemon, and native Windows client.
#[cfg(all(unix, not(feature = "client-only")))]
include!("unix.rs");
#[cfg(any(windows, feature = "client-only"))]
include!("windows.rs");

mod mail_body;
mod mail_publish;
