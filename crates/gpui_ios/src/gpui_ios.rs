#![cfg(target_os = "ios")]
//! iOS platform implementation for GPUI.
//!
//! This platform is a work in progress. It renders via `gpui_wgpu` (Metal) into
//! a `UIWindow` and runs GPUI's executors on top of Grand Central Dispatch. Touch
//! input, text input, and the mobile application lifecycle are not implemented yet.

mod dispatcher;
mod display;
mod keyboard;
mod platform;
mod window;

pub use platform::IosPlatform;
