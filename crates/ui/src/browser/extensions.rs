//! Chrome extensions in the browser tabs: added in Settings → Browser, from the Chrome Web Store (its CRX downloaded
//! and unpacked into a folder beside the settings) or from a folder the user picks, and loaded into every browser tab
//! by WebKit's own WebExtensions support (`WKWebExtensionController`, macOS 15.4 and later).
//! The settings keep each one's folder and switch (`UiSettings::browser_extensions`); the rest is read from its
//! manifest. Ported from Shepherd's `browser_extensions.rs`.
//!
//! The browser wires it in: [`attach`] on each page's `WKWebViewConfiguration` before its web view is made,
//! [`opened_tab`] once the web view exists (keep the [`TabMark`] as long as the page), [`activate_tab`] when a tab is
//! shown, and [`on_new_tab`] once, to open the tabs extensions ask for (`tabs.create`).
use crate::settings::BrowserExtension;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

/// The Chrome the Web Store is told it serves: some extensions ask for a recent one, and the store refuses older ones.
const CHROME_VERSION: &str = "140.0.0.0";

/// Why extensions can't load on this computer, or `None` when they can.
pub fn unavailable() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    return (!webkit::supported()).then_some("Browser extensions need macOS 15.4 or later, where WebKit runs them.");
    #[cfg(not(target_os = "macos"))]
    Some("Browser extensions run on macOS only, for now.")
}

/// Run in browser tabs: on a Web Store page of an extension, the store's own button (Add to Chrome, which it greys
/// out off Chrome) says Add to Commutator and goes to `commutator://install-extension/<id>`, which installs it as
/// Settings does. The store is a single-page app: the button is looked for again on each change of the page.
// ponytail: the button is the one whose short label names Chrome, as the store writes it in every language; a store
// redesign that drops the word needs a new match
pub const STORE_SCRIPT: &str = r#"(() => {
  if (location.host !== 'chromewebstore.google.com') return;
  const id = () => location.pathname.match(/\/detail\/(?:[^/]+\/)?([a-p]{32})/)?.[1];
  const dress = () => {
    if (!id()) return;
    for (const b of document.querySelectorAll('button:not([data-commutator])')) {
      const label = [...b.querySelectorAll('span')].find((s) => !s.children.length && /\bChrome\b/.test(s.textContent));
      if (!label || b.textContent.trim().length > 40) continue;
      b.dataset.commutator = '';
      b.disabled = false;
      b.removeAttribute('aria-disabled');
      label.textContent = label.textContent.replace('Chrome', 'Commutator');
    }
  };
  addEventListener('click', (e) => {
    if (!e.target.closest?.('button[data-commutator]') || !id()) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    location.href = 'commutator://install-extension/' + id();
  }, true);
  new MutationObserver(dress).observe(document, { childList: true, subtree: true });
})();"#;

/// The id a `commutator://install-extension/<id>` link asks for (see [`STORE_SCRIPT`]).
pub fn install_link(url: &str) -> Option<String> {
    store_id(url.strip_prefix("commutator://install-extension/")?)
}

/// The settings' data dir, told by `settings::init`.
static DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

pub(crate) fn set_data_dir(dir: &Path) {
    if let Ok(mut d) = DATA_DIR.write() {
        *d = Some(dir.to_path_buf());
    }
}

/// Where extensions from the Web Store are unpacked: beside `ui-settings.json`, so a second Commutator with a data
/// dir of its own keeps its own.
pub fn dir() -> PathBuf {
    let data = DATA_DIR.read().ok().and_then(|d| d.clone());
    data.unwrap_or_else(std::env::temp_dir).join("browser-extensions")
}

/// An extension's id in a Web Store link (`chromewebstore.google.com/detail/<slug>/<id>`, or the old
/// `chrome.google.com/webstore/detail/…`), or typed as is: 32 letters from a to p.
pub fn store_id(text: &str) -> Option<String> {
    let is_id = |s: &str| s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b));
    let path = text.trim().split(['?', '#']).next().unwrap_or_default();
    path.split('/').rev().find(|s| is_id(s)).map(str::to_string)
}

/// A CRX's ZIP: what follows its header, CRX3's (`Cr24`, version, header size, a protobuf of signatures) or CRX2's
/// (`Cr24`, version, key and signature sizes, then both).
// ponytail: the signatures aren't checked; the download is HTTPS from Google, and the user chose the extension
pub fn strip_crx(crx: &[u8]) -> Result<&[u8], String> {
    let word = |at: usize| crx.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    if !crx.starts_with(b"Cr24") {
        return Err("not a Chrome extension (CRX) file".into());
    }
    let start = match word(4) {
        Some(3) => word(8).map(|header| 12 + header),
        Some(2) => word(8).zip(word(12)).map(|(key, sig)| 16 + key + sig),
        _ => None,
    };
    start
        .and_then(|at| crx.get(at..))
        .filter(|zip| zip.starts_with(b"PK"))
        .ok_or_else(|| "a CRX of a version or shape Commutator doesn't read".into())
}

/// Fetches `url` (redirects followed), on a runtime of its own: [`install_from_store`] blocks a background thread.
fn download(url: &str, id: &str) -> Result<Vec<u8>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client.get(url).send().await.map_err(|e| format!("the Chrome Web Store didn't answer: {e}"))?;
        match response.status().as_u16() {
            204 | 404 => return Err(format!("the Chrome Web Store has no extension {id}")),
            status if !response.status().is_success() => return Err(format!("the Chrome Web Store answered {status}")),
            _ => {}
        }
        let body = response.bytes().await.map_err(|e| format!("the download broke off: {e}"))?;
        if body.len() > 256 << 20 {
            return Err("the extension is over 256 MB".into());
        }
        Ok(body.to_vec())
    })
}

/// Downloads Web Store extension `id`, unpacks it into `dir()/<id>` (over an older copy) and returns that folder.
/// Blocks: run it on a background thread, outside any Tokio runtime.
pub fn install_from_store(id: &str) -> Result<PathBuf, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={CHROME_VERSION}&acceptformat=crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
    );
    let crx = download(&url, id)?;
    // the store answers an unknown id with an empty body
    if crx.is_empty() {
        return Err(format!("the Chrome Web Store has no extension {id}"));
    }
    let zip = strip_crx(&crx)?;
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let (archive, unpacked, folder) = (dir().join(format!(".{id}.zip")), dir().join(format!(".{id}.new")), dir().join(id));
    std::fs::write(&archive, zip).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&unpacked);
    // ponytail: macOS's ditto unpacks it, as extensions load only there; a zip crate if they come to another system
    let out = std::process::Command::new("ditto").arg("-x").arg("-k").arg(&archive).arg(&unpacked).output();
    let _ = std::fs::remove_file(&archive);
    let out = out.map_err(|e| format!("ditto didn't run: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&unpacked);
        return Err(format!("the extension didn't unpack: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::rename(&unpacked, &folder).map_err(|e| e.to_string())?;
    info(&folder)?;
    Ok(folder)
}

/// Removes an extension's folder when Commutator unpacked it; a folder the user picked stays where it is.
pub fn remove_files(path: &Path) {
    if path.starts_with(dir()) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// What a Chrome extension's scripts find before their own code: its `chrome` and `browser` pinned, and, in its
/// background, the Chrome APIs WebKit lacks as do-nothing stand-ins.
const SHIM: &str = include_str!("extension_shim.js");
const SHIM_FILE: &str = "commutator_shim.js";
const WRAPPER_FILE: &str = "commutator_background.js";

/// Puts [`SHIM`] before the extension's own scripts in `folder`: writes the shim at its root, puts it first in each
/// content script (but those of the page's own world, which have no `chrome`) and, beside the background service
/// worker's script (so the script's own relative imports resolve as before), writes a wrapper that loads the shim and
/// then that script; `manifest.json` then names the wrapper. All of it holds on a folder done before, where only the
/// shim is written again (a newer Commutator's).
// ponytail: only a service worker is wrapped, the one background Chrome takes since MV3; an MV2 page or scripts run as
// is. A content script's world gets the shim with the manifest's scripts, not with scripting.executeScript alone.
pub fn prepare(folder: &Path) -> std::io::Result<()> {
    std::fs::write(folder.join(SHIM_FILE), SHIM)?;
    let path = folder.join("manifest.json");
    let mut manifest: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&path)?.trim_start_matches('\u{feff}'))?;
    let before = manifest.clone();
    for script in manifest.get_mut("content_scripts").and_then(|s| s.as_array_mut()).into_iter().flatten() {
        let isolated = script["world"].as_str().is_none_or(|w| w.eq_ignore_ascii_case("isolated"));
        if let Some(js) = script.get_mut("js").and_then(|js| js.as_array_mut()).filter(|js| isolated && !js.is_empty() && js[0] != SHIM_FILE) {
            js.insert(0, SHIM_FILE.into());
        }
    }
    if let Some(script) = manifest["background"]["service_worker"].as_str().map(|s| s.trim_start_matches('/').to_string()) {
        let (at, file) = script.rsplit_once('/').map_or((None, script.as_str()), |(at, file)| (Some(at), file));
        if file != WRAPPER_FILE {
            // the file's name as a JS string, quoted and escaped
            let name = serde_json::to_string(&format!("./{file}"))?;
            let wrapper = match manifest["background"]["type"].as_str() {
                Some("module") => format!("import '/{SHIM_FILE}';\nimport {name};\n"),
                _ => format!("importScripts('/{SHIM_FILE}', {name});\n"),
            };
            std::fs::write(folder.join(at.unwrap_or_default()).join(WRAPPER_FILE), wrapper)?;
            manifest["background"]["service_worker"] = at.map_or(WRAPPER_FILE.into(), |at| format!("{at}/{WRAPPER_FILE}")).into();
        }
    }
    if manifest == before {
        return Ok(());
    }
    std::fs::write(&path, serde_json::to_string_pretty(&manifest)?)
}

/// What Settings shows of an extension, from its manifest.
#[derive(Clone)]
pub struct Info {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Its largest PNG icon.
    pub icon: Option<Vec<u8>>,
}

/// Reads `folder`'s `manifest.json`, its name and description in its default language when they are `__MSG_…__`.
pub fn info(folder: &Path) -> Result<Info, String> {
    let read = |name: &str| {
        std::fs::read_to_string(folder.join(name)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(s.trim_start_matches('\u{feff}')).ok())
    };
    let manifest = read("manifest.json").ok_or_else(|| format!("no readable manifest.json in {}", folder.display()))?;
    let messages = manifest["default_locale"].as_str().and_then(|locale| read(&format!("_locales/{locale}/messages.json")));
    let text = |key: &str| {
        let value = manifest[key].as_str().unwrap_or_default();
        let message = value.strip_prefix("__MSG_").and_then(|v| v.strip_suffix("__")).and_then(|name| {
            // message names are matched without regard to case
            let messages = messages.as_ref()?.as_object()?;
            messages.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|(_, m)| m["message"].as_str())
        });
        message.unwrap_or(value).to_string()
    };
    let icons = manifest["icons"].as_object().or_else(|| manifest["action"]["default_icon"].as_object());
    let largest = icons.and_then(|icons| icons.iter().filter_map(|(size, path)| Some((size.parse::<u32>().ok()?, path.as_str()?))).max_by_key(|(size, _)| *size));
    let icon = largest.filter(|(_, path)| path.ends_with(".png")).and_then(|(_, path)| std::fs::read(folder.join(path.trim_start_matches('/'))).ok());
    Ok(Info { name: text("name"), version: manifest["version"].as_str().unwrap_or_default().to_string(), description: text("description"), icon })
}

/// Loads the extensions switched on and unloads the rest, in the tabs made from now on (and, for WebKit, in the
/// tabs already open as their pages next load). Nothing until a page's configuration is [`attach`]ed.
pub fn sync(list: &[BrowserExtension]) {
    #[cfg(target_os = "macos")]
    webkit::sync(list);
    #[cfg(not(target_os = "macos"))]
    let _ = list;
}

/// Why extension `path` didn't load, when it didn't.
pub fn error(path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    return webkit::error(path);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

thread_local! {
    static NEW_TAB: std::cell::RefCell<Option<std::rc::Rc<dyn Fn(String, &mut gpui::App)>>> = const { std::cell::RefCell::new(None) };
}

/// Opens the tabs extensions ask for (`tabs.create`): `f` gets each one's URL on the main thread, once the browser
/// has an [`attach`]ed page. A later call replaces it.
pub fn on_new_tab(f: impl Fn(String, &mut gpui::App) + 'static) {
    NEW_TAB.with_borrow_mut(|slot| *slot = Some(std::rc::Rc::new(f)));
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn new_tab(url: String, cx: &mut gpui::App) {
    if let Some(f) = NEW_TAB.with_borrow(|f| f.clone()) {
        f(url, cx)
    }
}

#[cfg(target_os = "macos")]
pub use webkit::{activate_tab, actions, attach, opened_tab, perform_action};

/// A browser tab as extensions know it, from [`opened_tab`] on; dropped with its page, it tells them it closed.
pub struct TabMark(#[cfg_attr(not(target_os = "macos"), allow(dead_code))] String);

#[cfg(not(target_os = "macos"))]
pub fn activate_tab(_: &str) {}

#[cfg(not(target_os = "macos"))]
pub fn actions() -> Vec<(PathBuf, String, Option<std::sync::Arc<gpui::Image>>)> {
    Vec::new()
}

#[cfg(target_os = "macos")]
impl Drop for TabMark {
    fn drop(&mut self) {
        webkit::closed_tab(&self.0);
    }
}

/// WebKit's side: one `WKWebExtensionController` for every browser tab, an extension context in it per extension.
#[cfg(target_os = "macos")]
mod webkit {
    use crate::settings::BrowserExtension;
    use block2::RcBlock;
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_foundation::{NSArray, NSError, NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL};
    use objc2_web_kit::{
        WKWebExtension, WKWebExtensionAction, WKWebExtensionContext, WKWebExtensionContextPermissionStatus, WKWebExtensionController,
        WKWebExtensionControllerConfiguration, WKWebExtensionControllerDelegate, WKWebExtensionTab, WKWebExtensionTabConfiguration,
        WKWebExtensionWindow, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    };
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[derive(Default)]
    struct State {
        controller: Option<Retained<WKWebExtensionController>>,
        /// The extensions switched on, as the settings last said.
        wanted: Vec<PathBuf>,
        loaded: HashMap<PathBuf, Retained<WKWebExtensionContext>>,
        /// Being read from their folders.
        reading: HashSet<PathBuf>,
        errors: HashMap<PathBuf, String>,
        /// Each loaded extension's action icon, once drawn.
        icons: HashMap<PathBuf, Option<Arc<gpui::Image>>>,
        /// Told when an extension has loaded or failed to: WebKit reads them after the tab is drawn, so the browser
        /// bar's buttons and Settings' "Didn't load" would otherwise wait for some other redraw.
        settled: Option<futures::channel::mpsc::UnboundedSender<()>>,
        /// The browser tabs, in the order they opened, by their page id.
        tabs: Vec<(String, Retained<Tab>)>,
        /// The one shown or last shown, the active tab extensions act on.
        active: Option<String>,
        /// Every browser tab is in this one window, as extensions see them; it and the delegate live as long as the
        /// controller (WebKit holds its delegate weakly).
        window: Option<Retained<ExtWindow>>,
        delegate: Option<Retained<Delegate>>,
        /// URLs extensions asked a new tab for (`tabs.create`), for [`super::on_new_tab`]'s callback.
        new_tabs: Option<futures::channel::mpsc::UnboundedSender<String>>,
        /// The data stores of the pages attached (one per workspace), for tracking prevention to go off in each.
        stores: Vec<Retained<WKWebsiteDataStore>>,
    }

    struct TabIvars {
        view: Weak<WKWebView>,
    }

    // What WebKit asks of a tab beyond these (its URL, title, loading) it reads from the web view itself.
    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = TabIvars]
        struct Tab;

        unsafe impl NSObjectProtocol for Tab {}

        unsafe impl WKWebExtensionTab for Tab {
            #[unsafe(method_id(windowForWebExtensionContext:))]
            fn window_for(&self, _: &WKWebExtensionContext) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
                window()
            }

            #[unsafe(method_id(webViewForWebExtensionContext:))]
            fn web_view_for(&self, _: &WKWebExtensionContext) -> Option<Retained<WKWebView>> {
                self.ivars().view.load()
            }

            #[unsafe(method(isSelectedForWebExtensionContext:))]
            fn is_selected(&self, _: &WKWebExtensionContext) -> bool {
                active_tab().is_some_and(|t| std::ptr::eq(&*t, self))
            }
        }
    );

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        struct ExtWindow;

        unsafe impl NSObjectProtocol for ExtWindow {}

        unsafe impl WKWebExtensionWindow for ExtWindow {
            #[unsafe(method_id(tabsForWebExtensionContext:))]
            fn tabs_for(&self, _: &WKWebExtensionContext) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionTab>>> {
                let tabs: Vec<_> = STATE.with_borrow(|s| s.tabs.iter().map(|(_, t)| ProtocolObject::from_retained(t.clone())).collect());
                NSArray::from_retained_slice(&tabs)
            }

            #[unsafe(method_id(activeTabForWebExtensionContext:))]
            fn active_tab_for(&self, _: &WKWebExtensionContext) -> Option<Retained<ProtocolObject<dyn WKWebExtensionTab>>> {
                active_tab().map(ProtocolObject::from_retained)
            }
        }
    );

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl WKWebExtensionControllerDelegate for Delegate {
            #[unsafe(method_id(webExtensionController:openWindowsForExtensionContext:))]
            fn open_windows(&self, _: &WKWebExtensionController, _: &WKWebExtensionContext) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionWindow>>> {
                NSArray::from_retained_slice(&window().into_iter().collect::<Vec<_>>())
            }

            #[unsafe(method_id(webExtensionController:focusedWindowForExtensionContext:))]
            fn focused_window(&self, _: &WKWebExtensionController, _: &WKWebExtensionContext) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
                window()
            }

            // ponytail: the tab opens a moment later; the extension is answered with none, so what it does with the
            // new tab right away (tabs.update on it) is lost
            #[unsafe(method(webExtensionController:openNewTabUsingConfiguration:forExtensionContext:completionHandler:))]
            fn open_new_tab(
                &self,
                _: &WKWebExtensionController,
                config: &WKWebExtensionTabConfiguration,
                _: &WKWebExtensionContext,
                done: &block2::DynBlock<dyn Fn(*mut ProtocolObject<dyn WKWebExtensionTab>, *mut NSError)>,
            ) {
                let url = unsafe { config.url() }.and_then(|u| u.absoluteString()).map(|s| s.to_string());
                if let Some(url) = url {
                    STATE.with_borrow(|s| s.new_tabs.as_ref().map(|tx| tx.unbounded_send(url)));
                }
                done.call((std::ptr::null_mut(), std::ptr::null_mut()));
            }
        }
    );

    fn window() -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
        STATE.with_borrow(|s| s.window.clone()).map(ProtocolObject::from_retained)
    }

    fn active_tab() -> Option<Retained<Tab>> {
        STATE.with_borrow(|s| s.active.as_ref().and_then(|id| s.tabs.iter().find(|(i, _)| i == id)).map(|(_, t)| t.clone()))
    }

    /// Tells the extensions about browser tab `id`, its page in `view`. WebKit asks the tab back while it is told, so
    /// the state is never borrowed across a call to the controller.
    pub fn opened_tab(id: &str, view: &WKWebView) -> super::TabMark {
        let mark = super::TabMark(id.to_string());
        let Some(mtm) = MainThreadMarker::new() else { return mark };
        let Some(controller) = STATE.with_borrow(|s| s.controller.clone()) else { return mark };
        let this = Tab::alloc(mtm).set_ivars(TabIvars { view: Weak::new(view) });
        let tab: Retained<Tab> = unsafe { msg_send![super(this), init] };
        STATE.with_borrow_mut(|s| s.tabs.push((id.to_string(), tab.clone())));
        unsafe { controller.didOpenTab(ProtocolObject::from_ref(&*tab)) };
        mark
    }

    pub(super) fn closed_tab(id: &str) {
        let (controller, tab) = STATE.with_borrow_mut(|s| {
            let at = s.tabs.iter().position(|(i, _)| i == id);
            if s.active.as_deref() == Some(id) {
                s.active = None;
            }
            (s.controller.clone(), at.map(|at| s.tabs.remove(at).1))
        });
        if let (Some(controller), Some(tab)) = (controller, tab) {
            unsafe { controller.didCloseTab_windowIsClosing(ProtocolObject::from_ref(&*tab), false) };
        }
    }

    /// Browser tab `id` is the active one now.
    pub fn activate_tab(id: &str) {
        if STATE.with_borrow(|s| s.active.as_deref() == Some(id)) {
            return;
        }
        let previous = active_tab();
        STATE.with_borrow_mut(|s| s.active = Some(id.to_string()));
        let (Some(controller), Some(now)) = (STATE.with_borrow(|s| s.controller.clone()), active_tab()) else { return };
        unsafe { controller.didActivateTab_previousActiveTab(ProtocolObject::from_ref(&*now), previous.as_deref().map(ProtocolObject::from_ref)) };
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::default();
    }

    pub(super) fn supported() -> bool {
        let at_least = NSOperatingSystemVersion { majorVersion: 15, minorVersion: 4, patchVersion: 0 };
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(at_least)
    }

    /// Puts the extensions' controller on a browser page's `config`, before its web view is made; nothing where
    /// extensions don't run. The page keeps its own data store. The extensions in the settings load with the first
    /// page.
    /// Returns whether extensions run here (and so whether [`super::STORE_SCRIPT`] belongs in the page).
    pub fn attach(config: &WKWebViewConfiguration, cx: &mut gpui::App) -> bool {
        use futures::StreamExt;
        let Some(mtm) = MainThreadMarker::new() else { return false };
        if !supported() {
            return false;
        }
        let store = unsafe { config.websiteDataStore() };
        let (controller, settled) = STATE.with_borrow_mut(|state| {
            if !state.stores.iter().any(|s| std::ptr::eq(&**s, &*store)) {
                state.stores.push(store.clone());
            }
            let first = state.controller.is_none();
            let controller = state
                .controller
                .get_or_insert_with(|| unsafe {
                    let config = WKWebExtensionControllerConfiguration::defaultConfiguration(mtm);
                    WKWebExtensionController::initWithConfiguration(WKWebExtensionController::alloc(mtm), &config)
                })
                .clone();
            let settled = first.then(|| {
                let (tx, rx) = futures::channel::mpsc::unbounded();
                state.settled = Some(tx);
                let (tabs_tx, tabs_rx) = futures::channel::mpsc::unbounded();
                state.new_tabs = Some(tabs_tx);
                let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(mtm), init] };
                unsafe { controller.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
                state.delegate = Some(delegate);
                state.window = Some(unsafe { msg_send![ExtWindow::alloc(mtm), init] });
                (rx, tabs_rx)
            });
            (controller, settled)
        });
        if let Some((mut settled, mut new_tabs)) = settled {
            cx.spawn(async move |cx| {
                while settled.next().await.is_some() {
                    cx.update(|cx| cx.refresh_windows());
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                while let Some(url) = new_tabs.next().await {
                    cx.update(|cx| super::new_tab(url, cx));
                }
            })
            .detach();
        }
        unsafe {
            config.setWebExtensionController(Some(&controller));
            idle_callbacks(config);
        }
        sync(&crate::settings::current(cx).browser_extensions);
        true
    }

    /// Turns on `requestIdleCallback` in `config`'s pages, which WebKit builds but keeps behind a flag. Chrome has it,
    /// and an extension's content scripts call it bare: Proton Pass finds a page's login fields in one, and without it
    /// puts no autofill icon in them.
    /// ponytail: private WebKit (`_features`, `_setEnabled:forFeature:`); the flag goes once Safari ships it
    unsafe fn idle_callbacks(config: &WKWebViewConfiguration) {
        use objc2::runtime::{AnyClass, AnyObject};
        let prefs: Retained<AnyObject> = unsafe { msg_send![config, preferences] };
        let Some(class) = AnyClass::get(c"WKPreferences") else { return };
        if !class.metaclass().responds_to(objc2::sel!(_features)) || !class.responds_to(objc2::sel!(_setEnabled:forFeature:)) {
            return;
        }
        let features: Retained<NSArray<AnyObject>> = unsafe { msg_send![class, _features] };
        for feature in features.iter() {
            let key: Retained<NSString> = unsafe { msg_send![&*feature, key] };
            if key.to_string() == "RequestIdleCallbackEnabled" {
                let _: () = unsafe { msg_send![&*prefs, _setEnabled: true, forFeature: &*feature] };
            }
        }
    }

    pub(super) fn error(path: &Path) -> Option<String> {
        STATE.with_borrow(|state| state.errors.get(path).cloned())
    }

    pub(super) fn sync(list: &[BrowserExtension]) {
        let wanted: Vec<PathBuf> = list.iter().filter(|e| e.on).map(|e| e.path.clone()).collect();
        let Some((controller, stores)) = STATE.with_borrow_mut(|state| {
            state.wanted = wanted.clone();
            state.controller.clone().map(|c| (c, state.stores.clone()))
        }) else {
            return;
        };
        // WebKit's tracking prevention (ITP) clears, a few seconds after launch, the data of every site nobody
        // interacted with, an extension's own origin among them, and with it the registration of its background
        // service worker: the worker stops, and the extension's context, unaware, never wakes it again, so each
        // message from then on gets `undefined`. Extensions' pages share the tabs' data store (and so their cookies, as
        // in Chrome), so tracking prevention goes for the tabs too while any extension is on.
        // ponytail: private WebKit; it stays off until Commutator quits once switched off
        if !wanted.is_empty() {
            for store in &stores {
                if store.respondsToSelector(objc2::sel!(_setResourceLoadStatisticsEnabled:)) {
                    let _: () = unsafe { msg_send![&**store, _setResourceLoadStatisticsEnabled: false] };
                }
            }
        }
        let gone: Vec<_> = STATE.with_borrow_mut(|state| {
            let paths: Vec<PathBuf> = state.loaded.keys().filter(|p| !wanted.contains(p)).cloned().collect();
            paths.iter().for_each(|p| drop(state.icons.remove(p)));
            paths.into_iter().filter_map(|p| state.loaded.remove(&p)).collect()
        });
        for context in gone {
            let _ = unsafe { controller.unloadExtensionContext_error(&context) };
        }
        for path in wanted {
            let start = STATE.with_borrow_mut(|state| !state.loaded.contains_key(&path) && state.reading.insert(path.clone()));
            if start {
                read(path, controller.clone());
            }
        }
    }

    /// Reads the extension in `path` (WebKit does it off the main thread), then loads it if it is still wanted.
    fn read(path: PathBuf, controller: Retained<WKWebExtensionController>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        // a folder the user picked is theirs (an extension they work on), so only the Web Store's get the shim
        if path.starts_with(super::dir())
            && let Err(e) = super::prepare(&path)
        {
            tracing::warn!("couldn't put the shim into browser extension {}: {e}", path.display());
        }
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), true);
        let at = path.clone();
        let done = RcBlock::new(move |extension: *mut WKWebExtension, error: *mut NSError| {
            let extension = unsafe { Retained::retain(extension) };
            let outcome = match extension {
                Some(extension) => load(&at, &extension, &controller),
                None => Err(unsafe { error.as_ref() }.map_or_else(|| "WebKit couldn't read it".into(), |e| e.localizedDescription().to_string())),
            };
            STATE.with_borrow_mut(|state| {
                state.reading.remove(&at);
                match outcome {
                    Ok(Some(context)) => {
                        state.errors.remove(&at);
                        state.loaded.insert(at.clone(), context);
                    }
                    Ok(None) => {}
                    Err(why) => {
                        tracing::warn!("browser extension {} didn't load: {why}", at.display());
                        state.errors.insert(at.clone(), why);
                    }
                }
                if let Some(settled) = &state.settled {
                    let _ = settled.unbounded_send(());
                }
            });
        });
        unsafe { WKWebExtension::extensionWithResourceBaseURL_completionHandler(&url, &done, mtm) };
    }

    /// Loads `extension` with all it asks for granted (the user added it on purpose), unless it was switched off or
    /// removed while it was read.
    fn load(path: &Path, extension: &WKWebExtension, controller: &WKWebExtensionController) -> Result<Option<Retained<WKWebExtensionContext>>, String> {
        if !STATE.with_borrow(|state| state.wanted.iter().any(|p| p == path)) {
            return Ok(None);
        }
        unsafe {
            let context = WKWebExtensionContext::contextForExtension(extension);
            // the same id each launch, so what it stored (chrome.storage) is there again
            context.setUniqueIdentifier(&NSString::from_str(&unique_id(path)));
            // and the same origin, so what its pages keep (IndexedDB, local storage) is there again too; WebKit makes
            // up a new one each launch otherwise
            if let Some(base) = NSURL::URLWithString(&NSString::from_str(&format!("webkit-extension://{}/", unique_id(path)))) {
                context.setBaseURL(&base);
            }
            // Safari's Develop menu reaches its background page and popups
            context.setInspectable(true);
            let granted = WKWebExtensionContextPermissionStatus::GrantedExplicitly;
            for permission in extension.requestedPermissions().iter() {
                context.setPermissionStatus_forPermission(granted, &permission);
            }
            for pattern in extension.allRequestedMatchPatterns().iter() {
                context.setPermissionStatus_forMatchPattern(granted, &pattern);
            }
            controller.loadExtensionContext_error(&context).map_err(|e| e.localizedDescription().to_string())?;
            // the tabs opened before it loaded: WebKit asks the window for them, yet counts none open until told
            let tabs: Vec<_> = STATE.with_borrow(|s| s.tabs.iter().map(|(_, t)| t.clone()).collect());
            for tab in &tabs {
                context.didOpenTab(ProtocolObject::from_ref(&**tab));
            }
            if let Some(tab) = active_tab() {
                context.didActivateTab_previousActiveTab(ProtocolObject::from_ref(&*tab), None);
            }
            Ok(Some(context))
        }
    }

    /// A stable id for the extension in `path`: its folder's name when that is a Web Store id, else a hash of the path.
    fn unique_id(path: &Path) -> String {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if super::store_id(&name).is_some() {
            return name;
        }
        use sha2::Digest;
        sha2::Sha256::digest(path.to_string_lossy().as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
    }

    /// The extensions loaded that have an action (a toolbar button), as (folder, label, icon).
    pub fn actions() -> Vec<(PathBuf, String, Option<Arc<gpui::Image>>)> {
        STATE.with_borrow_mut(|state| {
            let State { loaded, icons, .. } = state;
            let mut list: Vec<_> = loaded
                .iter()
                .filter_map(|(path, context)| unsafe {
                    let action = context.actionForTab(None)?;
                    let label = action.label().to_string();
                    let label = if label.is_empty() { context.webExtension().displayName().map(|n| n.to_string()).unwrap_or_default() } else { label };
                    let icon = icons.entry(path.clone()).or_insert_with(|| icon(&action)).clone();
                    Some((path.clone(), label, icon))
                })
                .collect();
            list.sort_by(|a, b| a.1.cmp(&b.1));
            list
        })
    }

    /// An action's icon, 16 points square at 2x, as GPUI draws it: a PNG.
    // ponytail: read once a load; an icon the extension changes later (chrome.action.setIcon) isn't followed
    fn icon(action: &WKWebExtensionAction) -> Option<Arc<gpui::Image>> {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
        use objc2_foundation::{NSDictionary, NSSize};
        unsafe {
            let image = action.iconForSize(NSSize::new(32., 32.))?;
            let tiff = image.TIFFRepresentation()?;
            let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
            let png = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
            Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png.to_vec())))
        }
    }

    /// What Chrome tells an action's popup asking for its tab: `tabs.getCurrent()` gives it none. WebKit gives it the
    /// tab it acts on, and a popup that checks (Proton Pass) takes itself for a page open in a tab, sizes itself to
    /// 100% of the popover, which sizes itself to the popup: an empty bubble. WebKit makes the popup's web view anew
    /// for each opening and has begun loading it, so it loads again with the script in.
    fn popup_not_a_tab(action: &WKWebExtensionAction) {
        use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime};
        const SCRIPT: &str = "for (const api of [globalThis.browser, globalThis.chrome]) try { if (api && api.tabs) Object.defineProperty(api.tabs, 'getCurrent', { value: (done) => typeof done === 'function' ? void done(undefined) : Promise.resolve(undefined) }) } catch {}";
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(web_view) = (unsafe { action.popupWebView() }) else { return };
        unsafe {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(WKUserScript::alloc(mtm), &NSString::from_str(SCRIPT), WKUserScriptInjectionTime::AtDocumentStart, true);
            web_view.configuration().userContentController().addUserScript(&script);
            web_view.reload();
        }
    }

    /// Clicks extension `path`'s toolbar button at `(x, y)` in `view`, the window's content view (points from its top
    /// left, as GPUI places them): WebKit runs its action, and its popup, when it has one, opens under that point.
    pub fn perform_action(path: &Path, view: &objc2_app_kit::NSView, x: f64, y: f64) {
        use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize};
        let Some(context) = STATE.with_borrow(|state| state.loaded.get(path).cloned()) else { return };
        let tab = active_tab();
        let tab = tab.as_deref().map(ProtocolObject::from_ref);
        unsafe {
            let Some(action) = context.actionForTab(tab) else { return };
            match action.popupPopover().filter(|_| action.presentsPopup()) {
                Some(popover) => {
                    if !popover.isShown() {
                        popup_not_a_tab(&action);
                    }
                    let y = if view.isFlipped() { y } else { view.frame().size.height - y };
                    let at = NSRect::new(NSPoint::new(x - 1., y - 1.), NSSize::new(2., 2.));
                    popover.showRelativeToRect_ofView_preferredEdge(at, view, if view.isFlipped() { NSRectEdge::MaxY } else { NSRectEdge::MinY });
                }
                None => context.performActionForTab(tab),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_install_links() {
        let id = "dbepggeogbaibhgnhhndojpepiihcmeb";
        assert_eq!(install_link(&format!("commutator://install-extension/{id}")).as_deref(), Some(id));
        assert_eq!(install_link(&format!("https://x/{id}")), None);
    }

    #[test]
    fn finds_store_ids() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(store_id(id).as_deref(), Some(id));
        assert_eq!(store_id(&format!("https://chromewebstore.google.com/detail/ublock-origin/{id}?hl=en")).as_deref(), Some(id));
        assert_eq!(store_id(&format!("https://chrome.google.com/webstore/detail/ublock-origin/{id}")).as_deref(), Some(id));
        assert_eq!(store_id("https://example.com/detail/zzzz"), None);
    }

    #[test]
    fn strips_crx_headers() {
        let zip = b"PK\x03\x04rest";
        let mut crx3 = b"Cr24\x03\0\0\0\x05\0\0\0hello".to_vec();
        crx3.extend_from_slice(zip);
        assert_eq!(strip_crx(&crx3).unwrap(), zip);
        let mut crx2 = b"Cr24\x02\0\0\0\x02\0\0\0\x03\0\0\0kkSSS".to_vec();
        crx2.extend_from_slice(zip);
        assert_eq!(strip_crx(&crx2).unwrap(), zip);
        assert!(strip_crx(b"PK\x03\x04").is_err());
        assert!(strip_crx(b"Cr24\x03\0\0\0\xff\0\0\0").is_err());
    }

    #[test]
    fn wraps_backgrounds_once() {
        let dir = tempfile::tempdir().unwrap();
        let (module, classic) = (dir.path().join("module"), dir.path().join("classic"));
        std::fs::create_dir_all(module.join("bg")).unwrap();
        std::fs::create_dir_all(&classic).unwrap();
        let manifest = |folder: &Path| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(folder.join("manifest.json")).unwrap()).unwrap();
        std::fs::write(module.join("manifest.json"), r#"{"name":"m","background":{"service_worker":"bg/main.js","type":"module"}}"#).unwrap();
        std::fs::write(classic.join("manifest.json"), "\u{feff}{\"name\":\"c\",\"background\":{\"service_worker\":\"/background.js\"}}").unwrap();
        for _ in 0..2 {
            prepare(&module).unwrap();
            prepare(&classic).unwrap();
            assert_eq!(manifest(&module)["background"]["service_worker"], "bg/commutator_background.js");
            assert_eq!(std::fs::read_to_string(module.join("bg/commutator_background.js")).unwrap(), "import '/commutator_shim.js';\nimport \"./main.js\";\n");
            assert_eq!(manifest(&classic)["background"]["service_worker"], "commutator_background.js");
            assert_eq!(std::fs::read_to_string(classic.join("commutator_background.js")).unwrap(), "importScripts('/commutator_shim.js', \"./background.js\");\n");
            assert_eq!(std::fs::read_to_string(classic.join("commutator_shim.js")).unwrap(), SHIM);
        }
        // no background, nothing but the shim; content scripts get it first, but those of the page's world
        std::fs::write(classic.join("manifest.json"), r#"{"name":"c"}"#).unwrap();
        prepare(&classic).unwrap();
        assert_eq!(manifest(&classic), serde_json::json!({"name": "c"}));
        let scripts = serde_json::json!([{"js": ["a.js"]}, {"js": ["b.js"], "world": "MAIN"}, {"css": ["c.css"]}]);
        std::fs::write(classic.join("manifest.json"), serde_json::json!({"content_scripts": scripts}).to_string()).unwrap();
        for _ in 0..2 {
            prepare(&classic).unwrap();
            let shimmed = serde_json::json!([{"js": ["commutator_shim.js", "a.js"]}, {"js": ["b.js"], "world": "MAIN"}, {"css": ["c.css"]}]);
            assert_eq!(manifest(&classic)["content_scripts"], shimmed);
        }
    }

    #[test]
    fn reads_manifest_info() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("_locales/en")).unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"name":"__MSG_appName__","version":"1.2","default_locale":"en","icons":{"16":"a.png","128":"b.png"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("_locales/en/messages.json"), r#"{"APPNAME":{"message":"Dark Reader"}}"#).unwrap();
        std::fs::write(dir.path().join("b.png"), b"png").unwrap();
        let info = info(dir.path()).unwrap();
        assert_eq!((info.name.as_str(), info.version.as_str()), ("Dark Reader", "1.2"));
        assert_eq!(info.icon.as_deref(), Some(&b"png"[..]));
        assert!(super::info(&dir.path().join("missing")).is_err());
    }

    /// Live: downloads Dark Reader from the Chrome Web Store into a temporary `dir()`:
    /// `cargo test -p commutator-ui extensions -- --ignored`.
    #[test]
    #[ignore]
    fn installs_from_store() {
        let data = tempfile::tempdir().unwrap();
        set_data_dir(data.path());
        let folder = install_from_store("eimadpbcbfnmbkopoojfekhnkhdbieeh").unwrap();
        let info = info(&folder).unwrap();
        assert_eq!(info.name, "Dark Reader");
        assert!(info.icon.is_some());
    }
}
