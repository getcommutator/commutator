//! AppKit boundary for the browser. Wry owns the native child and its UI
//! delegate; our navigation delegate supplies browser policy and state. All
//! callbacks enqueue events, never re-enter GPUI. No page-to-engine IPC.
use super::model::{PageState, Presentation, allowed_navigation};
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSObject,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKWebView,
};
use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::Rc,
};
use wry::{WebView, WebViewBuilderExtMacos, WebViewExtMacOS};

#[derive(Default)]
struct BrowserStore {
    profile: Option<super::profile::BrowserProfile>,
    store: Option<Retained<objc2_web_kit::WKWebsiteDataStore>>,
    preview_hosts: std::collections::BTreeSet<String>,
}
#[derive(Clone, Default)]
pub(super) struct BrowserData(Rc<RefCell<BrowserStore>>);
impl BrowserData {
    pub fn for_profile(profile: super::profile::BrowserProfile) -> Self {
        thread_local! {
            // Keep named stores alive through process termination, including
            // after the last window closes or the active identity changes.
            // macOS runs Rust TLS destructors during AppKit's exit(). Releasing
            // a WKWebsiteDataStore there removes its network session before
            // WebKit's UI-process-disconnect handler can flush its cookies.
            // In particular, deleted cookies can reappear on the next launch.
            // Intentionally skip this registry's destructor: WebKit must still
            // own the sessions when our process disconnects, and the OS reclaims
            // these process-lifetime references on exit.
            static PROFILES: ManuallyDrop<RefCell<std::collections::HashMap<std::path::PathBuf, Rc<RefCell<BrowserStore>>>>> =
                ManuallyDrop::new(RefCell::new(std::collections::HashMap::new()));
        }
        PROFILES.with(|profiles| {
            let mut profiles = profiles.borrow_mut();
            if let Some(store) = profiles.get(&profile.root) {
                return Self(store.clone());
            }
            let root = profile.root.clone();
            let store = Rc::new(RefCell::new(BrowserStore {
                profile: Some(profile),
                ..Default::default()
            }));
            profiles.insert(root, store.clone());
            Self(store)
        })
    }
    fn configuration(
        &self,
        mtm: MainThreadMarker,
    ) -> Retained<objc2_web_kit::WKWebViewConfiguration> {
        let mut data = self.0.borrow_mut();
        if data.store.is_none() {
            data.store = Some(unsafe {
                match &data.profile {
                    Some(profile) if persistent_profiles_supported() => {
                        let identifier =
                            objc2_foundation::NSUUID::from_bytes(profile.data_store_identifier());
                        objc2_web_kit::WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm)
                    }
                    _ => objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(mtm),
                }
            });
            if let Err(error) =
                configure_preview_proxy(data.store.as_ref().unwrap(), &data.preview_hosts)
            {
                tracing::warn!(%error, "preview hostname proxy unavailable");
            }
        }
        let configuration = unsafe { objc2_web_kit::WKWebViewConfiguration::new(mtm) };
        unsafe {
            configuration.setWebsiteDataStore(data.store.as_ref().unwrap());
        }
        configuration
    }
    pub(super) fn register_preview(&self, address: &str) {
        let Ok(url) = url::Url::parse(address) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        if url.scheme() != "http"
            || url.port() != Some(commutator_proto::PREVIEW_PROXY_PORT)
            || !host.ends_with(".localhost")
        {
            return;
        }
        let mut data = self.0.borrow_mut();
        if !data.preview_hosts.insert(host.into()) {
            return;
        }
        if let Some(store) = &data.store {
            if let Err(error) = configure_preview_proxy(store, &data.preview_hosts) {
                tracing::warn!(%error, "preview hostname proxy unavailable");
            }
        }
    }
}

/// macOS 12/13 have only a single default persistent store. Do not silently
/// share it across accounts; retain isolated temporary stores on those systems.
pub(super) fn persistent_profiles_supported() -> bool {
    unsafe {
        msg_send![class!(WKWebsiteDataStore), respondsToSelector: sel!(dataStoreForIdentifier:)]
    }
}

impl BrowserData {
    pub(super) fn persistence_notice(&self) -> Option<&'static str> {
        (self.0.borrow().profile.is_some() && !persistent_profiles_supported())
            .then_some("Logins are temporary on this macOS version. Use macOS 14 or later, or open this page in your default browser, to keep them.")
    }
}

/// Network.framework objects are Objective-C OS objects. Resolve the newer API
/// dynamically so older systems can still open ordinary browser tabs. Match
/// only discovered/opened preview hostnames; other websites retain normal routing.
fn configure_preview_proxy(
    store: &objc2_web_kit::WKWebsiteDataStore,
    hosts: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if hosts.is_empty() {
        return Ok(());
    }
    use objc2_foundation::{NSArray, NSObject};
    use std::ffi::{CStr, CString, c_char};
    unsafe {
        let supported: bool = msg_send![store, respondsToSelector: sel!(setProxyConfigurations:)];
        if !supported {
            return Err("Automatic preview hostnames require macOS 14 or later".into());
        }
        static NETWORK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let handle = *NETWORK.get_or_init(|| {
            libc::dlopen(
                c"/System/Library/Frameworks/Network.framework/Network".as_ptr(),
                libc::RTLD_LAZY,
            ) as usize
        });
        let symbol = |name: &CStr| -> Result<*mut std::ffi::c_void, String> {
            let value = libc::dlsym(handle as *mut _, name.as_ptr());
            if handle == 0 || value.is_null() {
                Err("Preview proxy API unavailable".into())
            } else {
                Ok(value)
            }
        };
        let endpoint: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut NSObject =
            std::mem::transmute(symbol(c"nw_endpoint_create_host")?);
        let proxy: unsafe extern "C" fn(*mut NSObject, *mut NSObject) -> *mut NSObject =
            std::mem::transmute(symbol(c"nw_proxy_config_create_http_connect")?);
        let match_domain: unsafe extern "C" fn(*mut NSObject, *const c_char) =
            std::mem::transmute(symbol(c"nw_proxy_config_add_match_domain")?);
        let endpoint = Retained::from_raw(endpoint(c"127.0.0.1".as_ptr(), c"7331".as_ptr()))
            .ok_or("Could not create preview endpoint")?;
        let config = Retained::from_raw(proxy(
            Retained::as_ptr(&endpoint) as *mut _,
            std::ptr::null_mut(),
        ))
        .ok_or("Could not create preview proxy")?;
        for host in hosts {
            let host = CString::new(host.as_str()).map_err(|_| "Invalid preview hostname")?;
            match_domain(Retained::as_ptr(&config) as *mut _, host.as_ptr());
        }
        let proxies = NSArray::arrayWithObject(&*config);
        let _: () = msg_send![store, setProxyConfigurations: &*proxies];
    }
    Ok(())
}

type Sender = tokio::sync::mpsc::Sender<NativeEvent>;
const OBSERVED: [&str; 6] = [
    "URL",
    "title",
    "loading",
    "canGoBack",
    "canGoForward",
    "underPageBackgroundColor",
];

pub(super) enum NativeEvent {
    Changed,
    Finished,
    NewTab(String),
    Key(gpui::Keystroke),
    Favicon { page: String, url: String },
    /// A click landed in the page: GPUI's focus follows it there.
    Clicked,
    /// The Web Store's Add button asked for extension `id`.
    InstallExtension(String),
}

struct ObserverState {
    tx: Sender,
    pending: Cell<bool>,
    error: RefCell<Option<String>>,
    requested_url: RefCell<Option<String>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CommutatorBrowserObserver"]
    #[ivars = ObserverState]
    struct Observer;
    unsafe impl NSObjectProtocol for Observer {}
    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(&self, _key: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut std::ffi::c_void) {
            self.changed();
        }
    }
    unsafe impl WKNavigationDelegate for Observer {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn policy(&self, _view: &WKWebView, action: &WKNavigationAction, decision: &block2::Block<dyn Fn(WKNavigationActionPolicy)>) {
            let url = unsafe { action.request().URL() }.and_then(|u| u.absoluteString()).map(|u| u.to_string()).unwrap_or_default();
            if let Some(id) = super::extensions::install_link(&url) {
                let _ = self.ivars().tx.try_send(NativeEvent::InstallExtension(id));
                return decision.call((WKNavigationActionPolicy::Cancel,));
            }
            if allowed_navigation(&url) && unsafe { action.targetFrame() }.is_some_and(|frame| unsafe { frame.isMainFrame() }) {
                *self.ivars().requested_url.borrow_mut() = Some(url.clone());
            }
            decision.call((if allowed_navigation(&url) { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }
        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn response(&self, _view: &WKWebView, response: &WKNavigationResponse, decision: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>) {
            let displayable = unsafe { response.canShowMIMEType() };
            if !displayable { self.fail("This file can’t be previewed here. Open it in your default browser."); }
            decision.call((if displayable { WKNavigationResponsePolicy::Allow } else { WKNavigationResponsePolicy::Cancel },));
        }
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn start(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().error.borrow_mut().take(); self.changed();
        }
        #[unsafe(method(webView:didCommitNavigation:))]
        fn commit(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().requested_url.borrow_mut().take(); self.changed();
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finish(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.changed(); let _ = self.ivars().tx.try_send(NativeEvent::Finished);
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn provisional_error(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if error.code() != -999 {
                tracing::warn!(domain = %error.domain(), code = error.code(), "browser provisional navigation failed");
                self.fail("Check the address and make sure your server is running, then try again.");
            }
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn navigation_error(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if error.code() != -999 {
                tracing::warn!(domain = %error.domain(), code = error.code(), "browser navigation failed");
                self.fail("The connection was interrupted. Try loading this page again.");
            }
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, _view: &WKWebView) {
            self.fail("The page stopped responding. Reload to continue.");
        }
    }
);

impl Observer {
    fn new(tx: Sender, mtm: MainThreadMarker) -> Retained<Self> {
        let object = mtm.alloc().set_ivars(ObserverState {
            tx,
            pending: Cell::new(false),
            error: RefCell::new(None),
            requested_url: RefCell::new(None),
        });
        unsafe { msg_send![super(object), init] }
    }
    fn changed(&self) {
        if !self.ivars().pending.replace(true) {
            if self.ivars().tx.try_send(NativeEvent::Changed).is_err() {
                self.ivars().pending.set(false);
            }
        }
    }
    fn fail(&self, message: &str) {
        *self.ivars().error.borrow_mut() = Some(message.into());
        self.changed();
    }
}

struct ClipState {
    dragging: Cell<bool>,
    region: Cell<objc2_foundation::NSRect>,
    resize_inset: Cell<f64>,
}

// Clips native content to GPUI's current paint mask. During app drags the
// entire native subtree passes pointer events back to GPUI, without hiding it.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CommutatorBrowserClipView"]
    #[ivars = ClipState]
    struct BrowserClipView;
    impl BrowserClipView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: objc2_foundation::NSPoint) -> *mut NSView {
            let region = self.ivars().region.get();
            if self.ivars().dragging.get()
                || point.x < region.origin.x + self.ivars().resize_inset.get() || point.x >= region.origin.x + region.size.width
                || point.y < region.origin.y || point.y >= region.origin.y + region.size.height
            { std::ptr::null_mut() }
            else { unsafe { msg_send![super(self), hitTest: point] } }
        }
    }
);

pub(super) struct NativePage(Rc<RefCell<Host>>);

pub(super) struct Host {
    web: WebView,
    data: BrowserData,
    view: Retained<WKWebView>,
    observer: Retained<Observer>,
    clip: Retained<BrowserClipView>,
    /// The web view's own superview inside the clip, at the page's bounds: the
    /// web view fills it, and WebKit docks its inspector in it beside the page.
    frame: Retained<NSView>,
    /// GPUI's focus is on the page: it takes AppKit's keyboard while shown.
    keyboard: bool,
    tab: String,
    /// Extensions know the page as a tab while it lives.
    _extension_tab: Option<super::extensions::TabMark>,
    clip_mask: Retained<AnyObject>,
    parent: Retained<NSView>,
    visible_bounds: Option<Bounds<Pixels>>,
    background_color: Option<Retained<NSColor>>,
    monitor: Option<Retained<AnyObject>>,
    shortcuts: Rc<RefCell<Vec<String>>>,
    bounds: Option<Bounds<Pixels>>,
    presentation: Presentation,
    tx: Sender,
    #[cfg(feature = "browser-fixture")]
    visibility_changes: Cell<u64>,
}

impl NativePage {
    pub fn new(
        window: &Window,
        data: &BrowserData,
        tx: Sender,
        cx: &mut gpui::App,
    ) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Browser must be created on the main thread")?;
        let new_tab = tx.clone();
        let configuration = data.configuration(mtm);
        let extended = super::extensions::attach(&configuration, cx);
        let builder = wry::WebViewBuilder::new()
            .with_webview_configuration(configuration)
            .with_devtools(true)
            // Each page's requests and console lines, for agents' tools.
            .with_initialization_script(super::agent::INSTRUMENT)
            // A two-finger swipe goes back and forward, as in Safari.
            .with_back_forward_navigation_gestures(true);
        // The Web Store's own button installs into Commutator.
        let builder = match extended {
            true => builder.with_initialization_script(super::extensions::STORE_SCRIPT),
            false => builder,
        };
        let web = builder
            .with_visible(false)
            .with_focused(false)
            .with_new_window_req_handler(move |url, _| {
                if allowed_navigation(&url) {
                    let _ = new_tab.try_send(NativeEvent::NewTab(url));
                }
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .build_as_child(window)
            .map_err(|e| e.to_string())?;
        let view = Retained::into_super(web.webview());
        window
            .enable_scene_overlay()
            .map_err(|error| error.to_string())?;
        let parent = unsafe { view.superview() }.ok_or("Browser parent is missing")?;
        let clip: Retained<BrowserClipView> = unsafe {
            let object = mtm.alloc().set_ivars(ClipState {
                dragging: Cell::new(false),
                region: Cell::new(objc2_foundation::NSRect::ZERO),
                resize_inset: Cell::new(0.0),
            });
            msg_send![super(object), initWithFrame: objc2_foundation::NSRect::ZERO]
        };
        clip.setWantsLayer(true);
        clip.setAutoresizesSubviews(false);
        let clip_mask: Retained<AnyObject> = unsafe { msg_send![class!(CALayer), new] };
        unsafe {
            let color = NSColor::blackColor().CGColor();
            let _: () = msg_send![&*clip_mask, setBackgroundColor: &*color];
            let layer: *mut AnyObject = msg_send![&*clip, layer];
            let _: () = msg_send![layer, setMasksToBounds: true];
            let _: () = msg_send![layer, setMask: &*clip_mask];
        }
        clip.setHidden(true);
        parent.addSubview(&clip);
        // Keep every native tab beneath the shared GPUI overlay plane.
        for sibling in parent.subviews() {
            if sibling.class().name() == c"GPUIOverlayView" {
                parent.addSubview_positioned_relativeTo(
                    &clip,
                    NSWindowOrderingMode::Below,
                    Some(&sibling),
                );
                break;
            }
        }
        let frame = NSView::initWithFrame(NSView::alloc(mtm), objc2_foundation::NSRect::ZERO);
        frame.setAutoresizesSubviews(true);
        clip.addSubview(&frame);
        view.setFrame(frame.bounds());
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        frame.addSubview(&view);
        // A trackpad's pinch zooms the page, as in Safari.
        let _: () = unsafe { msg_send![&*view, setAllowsMagnification: true] };

        let observer = Observer::new(tx.clone(), mtm);
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*observer)));
            for key in OBSERVED {
                view.addObserver_forKeyPath_options_context(
                    &observer,
                    &NSString::from_str(key),
                    NSKeyValueObservingOptions::empty(),
                    std::ptr::null_mut(),
                );
            }
        }
        let shortcuts = Rc::new(RefCell::new(Vec::<String>::new()));
        let monitor_view = frame.clone();
        let monitor_clip = clip.clone();
        let monitor_tx = tx.clone();
        let monitor_shortcuts = shortcuts.clone();
        let callback = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
            let e = unsafe { event.as_ref() };
            if monitor_clip.isHidden() || e.window(mtm) != monitor_view.window() {
                return event.as_ptr();
            }
            if e.r#type() != objc2_app_kit::NSEventType::KeyDown {
                // What AppKit will deliver the click to: the page (or its
                // inspector) tells GPUI, whose focus follows it there.
                let hit = monitor_view
                    .window()
                    .and_then(|w| w.contentView())
                    .and_then(|content| content.hitTest(e.locationInWindow()));
                if hit.is_some_and(|v| v.isDescendantOf(&monitor_view)) {
                    let _ = monitor_tx.try_send(NativeEvent::Clicked);
                }
                return event.as_ptr();
            }
            let responder = monitor_view
                .window()
                .and_then(|w| w.firstResponder())
                .and_then(|r| r.downcast::<NSView>().ok())
                .filter(|v| v.isDescendantOf(&monitor_view));
            let Some(responder) = responder else {
                return event.as_ptr();
            };
            let mods = e.modifierFlags();
            // The key as ⌘ types it: Latin in a layout like Russian, whose own
            // letters ⌘ doesn't use (⌥ left out, as charactersIgnoringModifiers).
            let as_cmd = NSEventModifierFlags::Command | (mods & NSEventModifierFlags::Shift);
            let key = e
                .charactersByApplyingModifiers(as_cmd)
                .map(|s| s.to_string().to_lowercase())
                .unwrap_or_default();
            let mut combo = String::new();
            if mods.contains(NSEventModifierFlags::Command) {
                combo.push_str("cmd-");
            }
            if mods.contains(NSEventModifierFlags::Control) {
                combo.push_str("ctrl-");
            }
            if mods.contains(NSEventModifierFlags::Option) {
                combo.push_str("alt-");
            }
            if mods.contains(NSEventModifierFlags::Shift) {
                combo.push_str("shift-");
            }
            combo.push_str(super::model::native_shortcut_key(&key));
            let Ok(keystroke) = gpui::Keystroke::parse(&combo) else {
                return event.as_ptr();
            };
            let browser_key = matches!(
                combo.as_str(),
                "cmd-l" | "cmd-t" | "cmd-w" | "cmd-[" | "cmd-]" | "cmd-shift-r" | "cmd-k" | "cmd-,"
                    | "cmd-alt-i"
            );
            let app_key = monitor_shortcuts
                .borrow()
                .iter()
                .any(|s| gpui::Keystroke::parse(s).is_ok_and(|s| s == keystroke));
            if (browser_key || app_key) && monitor_tx.try_send(NativeEvent::Key(keystroke)).is_ok() {
                return std::ptr::null_mut();
            }
            // ⌘C, ⌘V and the rest go to the page's first responder, as an Edit
            // menu's items would send them.
            if let Some(action) = edit_action(mods, &key) {
                let sent = unsafe {
                    objc2_app_kit::NSApplication::sharedApplication(mtm).sendAction_to_from(
                        objc2::runtime::Sel::register(action),
                        None,
                        None,
                    )
                };
                if sent {
                    return std::ptr::null_mut();
                }
            }
            // An arrow, ⌃ or ⌥ chord would reach GPUI's view as a key
            // equivalent first: the page takes it straight. Other ⌘ chords stay
            // on AppKit's way, to WebKit's key equivalents and the menu.
            if mods.contains(NSEventModifierFlags::Command)
                && !mods.contains(NSEventModifierFlags::Function)
            {
                return event.as_ptr();
            }
            responder.keyDown(e);
            std::ptr::null_mut()
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
                &callback,
            )
        };
        let tab = tab_id();
        let _extension_tab = extended.then(|| super::extensions::opened_tab(&tab, &view));
        Ok(Self(Rc::new(RefCell::new(Host {
            data: data.clone(),
            web,
            view,
            observer,
            clip,
            frame,
            keyboard: false,
            tab,
            _extension_tab,
            clip_mask,
            parent,
            visible_bounds: None,
            background_color: None,
            monitor,
            shortcuts,
            bounds: None,
            presentation: Presentation::Hidden,
            #[cfg(feature = "browser-fixture")]
            visibility_changes: Cell::new(0),
            tx,
        }))))
    }
    pub fn handle(&self) -> Rc<RefCell<Host>> {
        self.0.clone()
    }
    pub fn focus_chrome(&self) {
        let _ = self.0.borrow().web.focus_parent();
    }
    pub fn set_shortcuts(&self, shortcuts: Vec<String>) {
        *self.0.borrow().shortcuts.borrow_mut() = shortcuts;
    }
    pub fn present(&mut self, presentation: Presentation) {
        self.0.borrow_mut().present(presentation);
    }
    pub fn load(&self, url: &str) -> Result<(), String> {
        let host = self.0.borrow();
        host.data.register_preview(url);
        host.observer.ivars().error.borrow_mut().take();
        *host.observer.ivars().requested_url.borrow_mut() = Some(url.into());
        host.web.load_url(url).map_err(|e| e.to_string())
    }
    pub fn reload(&self) {
        let host = self.0.borrow();
        host.observer.ivars().error.borrow_mut().take();
        // Retrying the requested URL also works after a failed provisional load.
        unsafe {
            host.view.reload();
        }
        host.observer.changed();
    }
    pub fn history(&self, forward: bool) {
        let host = self.0.borrow();
        unsafe {
            if forward {
                host.view.goForward();
            } else {
                host.view.goBack();
            }
        }
    }
    pub fn state(&self) -> PageState {
        let host = self.0.borrow();
        host.observer.ivars().pending.set(false);
        unsafe {
            PageState {
                url: host
                    .observer
                    .ivars()
                    .requested_url
                    .borrow()
                    .clone()
                    .or_else(|| {
                        host.view
                            .URL()
                            .and_then(|u| u.absoluteString())
                            .map(|u| u.to_string())
                    }),
                title: host.view.title().map(|s| s.to_string()).unwrap_or_default(),
                loading: host.view.isLoading(),
                can_back: host.view.canGoBack(),
                can_forward: host.view.canGoForward(),
                error: host.observer.ivars().error.borrow().clone(),
            }
        }
    }
    pub fn discover_favicon(&self, page: String) {
        let host = self.0.borrow();
        let tx = host.tx.clone();
        // Our navigation delegate owns readiness, so Wry's private pending-
        // script queue is intentionally bypassed. This runs only after finish.
        let completion = block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            if !error.is_null() {
                return;
            }
            if let Some(url) =
                unsafe { value.as_ref() }.and_then(|value| value.downcast_ref::<NSString>())
            {
                let _ = tx.try_send(NativeEvent::Favicon {
                    page: page.clone(),
                    url: url.to_string(),
                });
            }
        });
        unsafe {
            host.view.evaluateJavaScript_completionHandler(
                &NSString::from_str("(() => { const link = document.querySelector('link[rel~=icon]'); return link ? link.href : new URL('/favicon.ico', location.href).href; })()"),
                Some(&completion),
            );
        }
    }
}

/// Each page's id as extensions know it.
fn tab_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("browser-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// The Edit menu's action for a ⌘ chord typed in the page.
fn edit_action(flags: NSEventModifierFlags, key: &str) -> Option<&'static std::ffi::CStr> {
    let has = |f| flags.contains(f);
    if !has(NSEventModifierFlags::Command) || has(NSEventModifierFlags::Control) {
        return None;
    }
    let (alt, shift) = (has(NSEventModifierFlags::Option), has(NSEventModifierFlags::Shift));
    Some(match (key, alt, shift) {
        ("c", false, false) => c"copy:",
        ("x", false, false) => c"cut:",
        ("v", false, false) => c"paste:",
        ("v", true, true) => c"pasteAsPlainText:",
        ("a", false, false) => c"selectAll:",
        ("z", false, false) => c"undo:",
        ("z", false, true) => c"redo:",
        _ => return None,
    })
}

fn has_focus(view: &NSView) -> bool {
    view.window()
        .and_then(|w| w.firstResponder())
        .and_then(|r| r.downcast::<NSView>().ok())
        .is_some_and(|v| v.isDescendantOf(view))
}

impl Host {
    pub fn sync(
        &mut self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        dragging: bool,
        resize_inset: Pixels,
    ) {
        // WebKit may fill newly exposed tiles a frame after a viewport change.
        // Match its page background beneath those tiles instead of exposing
        // the application's dark window background at the resize edge.
        unsafe {
            let color = self.view.underPageBackgroundColor();
            let unchanged = self
                .background_color
                .as_ref()
                .is_some_and(|previous| msg_send![&**previous, isEqual: &*color]);
            if !unchanged {
                // WebKit's native backing otherwise uses controlBackgroundColor,
                // independently of the CSS page background. Exposed resize tiles
                // must use the page color while the remote content catches up.
                let _: () = msg_send![&*self.view, _setBackgroundColor: &*color];
                let cg_color = color.CGColor();
                let layer: *mut AnyObject = msg_send![&*self.clip, layer];
                let _: () = msg_send![layer, setBackgroundColor: &*cg_color];
                self.background_color = Some(color);
            }
        }
        let visible = bounds.intersect(&mask);
        self.clip.ivars().dragging.set(dragging);
        self.clip
            .ivars()
            .resize_inset
            .set(f64::from(f32::from(resize_inset)));
        if self.bounds != Some(bounds) || self.visible_bounds != Some(visible) {
            self.bounds = Some(bounds);
            self.visible_bounds = Some(visible);
            let x = f64::from(f32::from(visible.origin.x));
            let y = f64::from(f32::from(visible.origin.y));
            let width = f64::from(f32::from(visible.size.width)).max(0.);
            let height = f64::from(f32::from(visible.size.height)).max(0.);
            let y = if self.parent.isFlipped() {
                y
            } else {
                self.parent.bounds().size.height - y - height
            };
            let actions_disabled: bool = unsafe {
                let previous = msg_send![class!(CATransaction), disableActions];
                let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                previous
            };
            // Keep the host stationary. Moving it while WebKit updates its
            // remote layer tree can combine the old origin with the new size.
            self.clip.setFrame(self.parent.bounds());
            let region = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(x, y),
                objc2_foundation::NSSize::new(width, height),
            );
            self.clip.ivars().region.set(region);
            unsafe {
                let _: () = msg_send![&*self.clip_mask, setFrame: region];
            }
            // Wry's set_bounds rounds logical origins and sizes to whole points. GPUI
            // uses fractional points, so that leaves uncovered background
            // strips between the WKWebView and its clip after a resize.
            let web_height = f64::from(f32::from(bounds.size.height)).max(0.);
            let web_y = f64::from(f32::from(bounds.origin.y));
            let web_y = if self.parent.isFlipped() {
                web_y
            } else {
                self.parent.bounds().size.height - web_y - web_height
            };
            self.frame.setFrame(objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(f64::from(f32::from(bounds.origin.x)), web_y),
                objc2_foundation::NSSize::new(
                    f64::from(f32::from(bounds.size.width)).max(0.),
                    web_height,
                ),
            ));
            unsafe {
                // Leave the implicit transaction open for the matching Metal
                // presentation. Committing here would expose native geometry early.
                let _: () = msg_send![class!(CATransaction), setDisableActions: actions_disabled];
            }
            #[cfg(feature = "browser-fixture")]
            unsafe {
                let layer: *mut AnyObject = msg_send![&*self.clip, layer];
                let frame: objc2_foundation::NSRect = msg_send![layer, frame];
                let presentation: *mut AnyObject = msg_send![layer, presentationLayer];
                let presented: objc2_foundation::NSRect = if presentation.is_null() {
                    frame
                } else {
                    msg_send![presentation, frame]
                };
                eprintln!(
                    "Browser geometry: bounds={bounds:?} mask={mask:?} clip={:?} layer={frame:?} presented={presented:?} web={:?} background={:?}",
                    self.clip.frame(),
                    self.view.frame(),
                    self.view.underPageBackgroundColor()
                );
            }
        }
        self.update_visibility();
    }
    fn update_visibility(&self) {
        let visible = self.presentation != Presentation::Hidden
            && self.observer.ivars().error.borrow().is_none()
            && self
                .visible_bounds
                .is_some_and(|b| f32::from(b.size.width) > 0. && f32::from(b.size.height) > 0.);
        if self.view.isHidden() == visible {
            #[cfg(feature = "browser-fixture")]
            self.visibility_changes
                .set(self.visibility_changes.get() + 1);
            let _ = self.web.set_visible(visible);
        }
        self.clip.setHidden(!visible);
        // AppKit's keyboard follows GPUI's focus: the page has it only while
        // GPUI's focus is on it and it is shown; a menu or dialog that takes
        // GPUI's focus takes the keys too.
        let wants = visible && self.keyboard && self.presentation == Presentation::Live;
        if wants && !has_focus(&self.frame) {
            let _ = self.web.focus();
        } else if !wants && has_focus(&self.frame) {
            let _ = self.web.focus_parent();
        }
    }
    pub fn set_keyboard(&mut self, keyboard: bool) {
        if self.keyboard != keyboard {
            self.keyboard = keyboard;
            self.update_visibility();
        }
    }
    pub fn devtools_open(&self) -> bool {
        self.web.is_devtools_open()
    }
    /// WebKit's inspector, docked beside the page inside the panel.
    pub fn toggle_devtools(&self) {
        match self.web.is_devtools_open() {
            true => self.web.close_devtools(),
            false => self.web.open_devtools(),
        }
    }
    pub fn set_zoom(&self, zoom: f64) {
        let _ = self.web.zoom(zoom);
    }
    pub fn hard_reload(&self) {
        self.observer.ivars().error.borrow_mut().take();
        let _: Option<Retained<AnyObject>> = unsafe { msg_send![&*self.view, reloadFromOrigin] };
        self.observer.changed();
    }
    /// The page as its viewport shows it, at one pixel a point (small enough
    /// for an agent), as a PNG.
    pub fn snapshot(&self, done: impl FnOnce(Result<Vec<u8>, String>) + 'static) {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
        let Some(mtm) = MainThreadMarker::new() else {
            return done(Err("not on the main thread".into()));
        };
        let done = std::cell::Cell::new(Some(done));
        let finish = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(done) = done.take() else { return };
            let png = unsafe { image.as_ref() }
                .and_then(|image| {
                    let tiff = image.TIFFRepresentation()?;
                    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
                    let png = unsafe {
                        rep.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &objc2_foundation::NSDictionary::new(),
                        )
                    }?;
                    Some(png.to_vec())
                })
                .ok_or_else(|| {
                    unsafe { error.as_ref() }
                        .map_or("WebKit took no picture".into(), |e| e.localizedDescription().to_string())
                });
            done(png);
        });
        unsafe {
            let config = objc2_web_kit::WKSnapshotConfiguration::new(mtm);
            let width = self.view.bounds().size.width;
            let number = objc2_foundation::NSNumber::new_f64(width);
            config.setSnapshotWidth(Some(&number));
            self.view
                .takeSnapshotWithConfiguration_completionHandler(Some(&config), &finish);
        }
    }
    /// Runs `script` in the page; `done` gets its result as JSON.
    pub fn eval_with_callback(&self, script: &str, done: impl Fn(String) + Send + 'static) {
        let _ = self.web.evaluate_script_with_callback(script, done);
    }
    /// Removes the data of `kinds` WebKit keeps for `host`'s site in this
    /// profile's store: the records named `host` or a domain it is under.
    pub fn clear_site_data(&self, kinds: &[&str], host: String) {
        use objc2_foundation::{NSArray, NSSet};
        let kinds: Vec<Retained<NSString>> = kinds.iter().map(|k| NSString::from_str(k)).collect();
        let kinds = NSSet::from_retained_slice(&kinds);
        let store: Retained<AnyObject> = unsafe {
            let configuration: Retained<AnyObject> = msg_send![&*self.view, configuration];
            msg_send![&configuration, websiteDataStore]
        };
        let (from, removed) = (store.clone(), kinds.clone());
        let found = block2::RcBlock::new(move |records: std::ptr::NonNull<NSArray<AnyObject>>| {
            let records = unsafe { records.as_ref() };
            let ours: Vec<Retained<AnyObject>> = records
                .iter()
                .filter(|r| {
                    let name: Retained<NSString> = unsafe { msg_send![&**r, displayName] };
                    let name = name.to_string();
                    host == name || host.ends_with(&format!(".{name}"))
                })
                .collect();
            let ours = NSArray::from_retained_slice(&ours);
            let done = block2::RcBlock::new(|| {});
            let () = unsafe {
                msg_send![&from, removeDataOfTypes: &*removed, forDataRecords: &*ours, completionHandler: &*done]
            };
        });
        let () = unsafe { msg_send![&store, fetchDataRecordsOfTypes: &*kinds, completionHandler: &*found] };
    }
    fn present(&mut self, presentation: Presentation) {
        self.presentation = presentation;
        // The tab shown is the one extensions act on, as a Chrome window's.
        if presentation == Presentation::Live && self._extension_tab.is_some() {
            super::extensions::activate_tab(&self.tab);
        }
        self.clip
            .ivars()
            .dragging
            .set(presentation == Presentation::Passthrough);
        self.update_visibility();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if has_focus(&self.frame) {
            let _ = self.web.focus_parent();
        }
        unsafe {
            if let Some(monitor) = self.monitor.take() {
                NSEvent::removeMonitor(&monitor);
            }
            for key in OBSERVED {
                self.view
                    .removeObserver_forKeyPath(&self.observer, &NSString::from_str(key));
            }
            self.view.setNavigationDelegate(None);
            self.view.stopLoading();
        }
        if self.web.is_devtools_open() {
            self.web.close_devtools();
        }
        self.view.removeFromSuperview();
        self.frame.removeFromSuperview();
        self.clip.removeFromSuperview();
    }
}

#[cfg(feature = "browser-fixture")]
impl NativePage {
    pub fn fixture_move_cursor(&self, x: f64, y: f64) {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGWarpMouseCursorPosition(point: objc2_foundation::NSPoint) -> i32;
        }
        let host = self.0.borrow();
        let window = host.view.window().unwrap();
        let height = host.parent.clone().bounds().size.height;
        let p = window.convertPointToScreen(objc2_foundation::NSPoint::new(x, height - y));
        let screen = objc2_app_kit::NSScreen::mainScreen(host.view.mtm()).unwrap();
        unsafe {
            CGWarpMouseCursorPosition(objc2_foundation::NSPoint::new(
                p.x,
                screen.frame().size.height - p.y,
            ));
        }
    }
    pub fn fixture_backdrop_layers(&self) -> String {
        unsafe fn describe(layer: *mut AnyObject, depth: usize, out: &mut String) {
            if layer.is_null() || depth > 8 {
                return;
            }
            unsafe {
                let desc: Retained<NSString> = msg_send![layer, description];
                out.push_str(&format!("{}{}\n", " ".repeat(depth), desc));
                let layers: *mut AnyObject = msg_send![layer, sublayers];
                if !layers.is_null() {
                    let count: usize = msg_send![layers, count];
                    for i in 0..count.min(30) {
                        let child: *mut AnyObject = msg_send![layers, objectAtIndex:i];
                        describe(child, depth + 1, out);
                    }
                }
            }
        }
        let host = self.0.borrow();
        let mut out = String::new();
        for view in host.parent.subviews() {
            if view.class().name() == c"GPUIBackdropView" {
                unsafe {
                    let layer: *mut AnyObject = msg_send![&*view, layer];
                    describe(layer, 0, &mut out);
                }
            }
        }
        out
    }
    pub fn fixture_page_hit(&self, x: f64, y: f64) -> bool {
        let host = self.0.borrow();
        let point = objc2_foundation::NSPoint::new(x, host.parent.bounds().size.height - y);
        host.parent
            .hitTest(point)
            .is_some_and(|hit| hit.isDescendantOf(&host.clip))
    }
    pub fn fixture_backdrops(&self) -> Vec<(f64, f64, f64, f64)> {
        let host = self.0.borrow();
        host.parent
            .subviews()
            .iter()
            .filter(|view| view.class().name() == c"GPUIBackdropView" && !view.isHidden())
            .map(|view| {
                let r = view.frame();
                (
                    r.origin.x,
                    host.parent.bounds().size.height - r.origin.y - r.size.height,
                    r.size.width,
                    r.size.height,
                )
            })
            .collect()
    }
    pub fn fixture_geometry(&self) -> (f32, f32, f32) {
        let host = self.0.borrow();
        (
            host.bounds.unwrap().size.width.into(),
            host.view.frame().size.width as f32,
            host.visible_bounds.unwrap().size.width.into(),
        )
    }
    pub fn fixture_origin(&self) -> (f32, f32) {
        let b = self.0.borrow().bounds.unwrap();
        (b.origin.x.into(), b.origin.y.into())
    }
    pub fn fixture_overlay_visible(&self) -> bool {
        let host = self.0.borrow();
        host.parent
            .clone()
            .subviews()
            .iter()
            .any(|v| v.class().name() == c"GPUIOverlayView" && !v.isHidden())
    }
    pub fn fixture_visibility_changes(&self) -> u64 {
        let host = self.0.borrow();
        host.visibility_changes.get()
    }
    pub fn fixture_focus(&self) {
        let _ = self.0.borrow().web.focus();
    }
    pub fn fixture_focused(&self) -> bool {
        has_focus(&self.0.borrow().view)
    }
    pub fn fixture_overlay_at(&self, x: f64, y: f64) -> bool {
        let host = self.0.borrow();
        let parent = host.parent.clone();
        let point = objc2_foundation::NSPoint::new(
            x,
            if parent.isFlipped() {
                y
            } else {
                parent.bounds().size.height - y
            },
        );
        parent
            .hitTest(point)
            .is_some_and(|hit| hit.class().name() == c"GPUIOverlayView")
    }
    pub fn fixture_click(&self, x: f64, y: f64) {
        self.fixture_move_cursor(x, y);
        let host = self.0.borrow();
        let parent = host.parent.clone();
        let window = host.view.window().unwrap();
        let point = objc2_foundation::NSPoint::new(x, parent.bounds().size.height - y);
        let app = objc2_app_kit::NSApplication::sharedApplication(host.view.mtm());
        for kind in [
            objc2_app_kit::NSEventType::LeftMouseDown,
            objc2_app_kit::NSEventType::LeftMouseUp,
        ] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(kind, point, NSEventModifierFlags::empty(), 0., window.windowNumber(), None, 0, 1, 1.).unwrap();
            app.postEvent_atStart(&event, false);
        }
    }
    pub fn fixture_visible(&self) -> bool {
        !self.0.borrow().view.isHidden()
    }
    pub fn fixture_cookies(&self, completion: impl FnOnce(Vec<String>) + 'static) {
        use objc2_foundation::{NSArray, NSHTTPCookie};
        let completion = RefCell::new(Some(completion));
        let block = block2::RcBlock::new(move |cookies: std::ptr::NonNull<NSArray<NSHTTPCookie>>| {
            let cookies = unsafe { cookies.as_ref() };
            let values = cookies.iter().map(|cookie| format!(
                "{}={}; domain={}; path={}; httpOnly={}; sessionOnly={}",
                cookie.name(), cookie.value(), cookie.domain(), cookie.path(),
                cookie.isHTTPOnly(), cookie.isSessionOnly()
            )).collect();
            if let Some(completion) = completion.borrow_mut().take() {
                completion(values);
            }
        });
        unsafe {
            self.0.borrow().view.configuration().websiteDataStore().httpCookieStore().getAllCookies(&block);
        }
    }

    pub fn fixture_eval(&self, script: &str) {
        unsafe {
            self.0
                .borrow()
                .view
                .evaluateJavaScript_completionHandler(&NSString::from_str(script), None);
        }
    }
}
