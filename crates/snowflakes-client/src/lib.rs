//! Platform-independent client behavior, shared by the Windows event loop and tests.
mod collapse;
pub mod config;
pub mod control;
pub mod diagnostics;
pub mod display_identity;
pub mod geometry;
#[cfg(windows)]
pub mod platform;
pub mod quality;
pub mod rain;
pub mod scene_registry;
pub mod session;
pub mod snow;
pub mod storage;
pub mod timing;
pub mod ui;
