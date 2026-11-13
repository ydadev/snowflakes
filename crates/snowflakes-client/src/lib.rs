//! Platform-independent client behavior, shared by the Windows event loop and tests.
pub mod config;
pub mod control;
pub mod display_identity;
pub mod geometry;
#[cfg(windows)]
pub mod platform;
pub mod quality;
pub mod scene_registry;
pub mod session;
pub mod snow;
pub mod storage;
pub mod timing;
pub mod ui;
