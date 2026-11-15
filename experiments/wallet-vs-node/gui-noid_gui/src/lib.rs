// Harness-only shim: expose the GUI crate as a library so the audit
// harness can drive the real backend code. The module list mirrors
// src/main.rs verbatim; no other file is modified.
pub mod app;
pub mod app_icon;
pub mod assets;
pub mod backend;
pub mod contracts;
pub mod i18n;
pub mod market_data;
pub mod model;
pub mod secret;
pub mod theme;
pub mod view;
pub mod widgets;
