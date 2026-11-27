//! Placeholder until the Chrome extensions port lands.
#![allow(dead_code)]
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const STORE_SCRIPT: &str = "";

pub fn install_link(_url: &str) -> Option<String> {
    None
}

pub fn install_from_store(_id: &str) -> Result<PathBuf, String> {
    Err("unavailable".into())
}

pub fn add_extension(_cx: &mut gpui::App, _path: PathBuf) {}

pub fn info_name(_path: &Path) -> String {
    String::new()
}

#[cfg(target_os = "macos")]
pub fn attach(_config: &objc2_web_kit::WKWebViewConfiguration, _cx: &mut gpui::App) -> bool {
    false
}

pub struct TabMark;

#[cfg(target_os = "macos")]
pub fn opened_tab(_id: &str, _view: &objc2_web_kit::WKWebView) -> TabMark {
    TabMark
}

pub fn activate_tab(_id: &str) {}

pub fn actions() -> Vec<(PathBuf, String, Option<Arc<gpui::Image>>)> {
    Vec::new()
}

#[cfg(target_os = "macos")]
pub fn perform_action(_path: &Path, _view: &objc2_app_kit::NSView, _x: f64, _y: f64) {}
