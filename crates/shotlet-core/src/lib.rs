//! Procedures for capture, history, scrolling, and recording.
//! The desktop shell calls these modules, then opens or closes windows.
//! The `pub use` lines only keep the window layer on its existing paths.
//! This crate does not open a window.

pub mod capture_session;
pub mod editor;
pub mod library;
pub mod record_session;
pub mod scroll_session;
pub mod shortcut;
pub mod state;

pub use shotlet_capture as capture;
pub use shotlet_media::ocr;
pub use shotlet_media::record;
pub use shotlet_media::scroll;
pub use shotlet_platform as util;
pub use shotlet_store::history;
pub use shotlet_store::settings;
