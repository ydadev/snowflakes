//! Platform-independent client behavior, shared by the Windows event loop and tests.
pub mod config;
pub mod control;
#[cfg(windows)]
pub mod platform;
pub mod session;
pub mod storage;
pub mod timing;
pub mod ui;
