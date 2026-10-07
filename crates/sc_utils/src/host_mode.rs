//! Runtime switch for hosted mode (server embedded in a GUI/mobile app).
//!
//! A standalone server (sc_bootstrap binary) leaves the hosted flag unset and exits via
//! `std::process::exit(0)`; a host (e.g. Tauri app sc_app) calls [`set_hosted`] before
//! startup so SCExit teardown (stop region threads + flush logs) only sets the stopped
//! flag, letting the host runner poll it and return normally while the process stays alive
//! for repeated start/stop inside the app.

use std::sync::atomic::{AtomicBool, Ordering};

static HOSTED: AtomicBool = AtomicBool::new(false);
static SERVER_STOPPED: AtomicBool = AtomicBool::new(false);

/// Marks the process as hosted (call before assembling the server app).
pub fn set_hosted() {
    HOSTED.store(true, Ordering::Relaxed);
}

/// Whether hosted mode is active.
pub fn is_hosted() -> bool {
    HOSTED.load(Ordering::Relaxed)
}

/// Graceful server shutdown finished: notifies the host runner to leave its main loop.
pub fn notify_server_stopped() {
    SERVER_STOPPED.store(true, Ordering::Relaxed);
}

/// Host runner poll: takes the stopped flag (single consumption).
pub fn take_server_stopped() -> bool {
    SERVER_STOPPED.swap(false, Ordering::Relaxed)
}
