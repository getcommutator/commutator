//! The browser tabs beside this agent's chat in the Commutator app, as an
//! agent drives them: list, open and close them, read a page (its snapshot,
//! console and requests), act on it, run JavaScript and take a screenshot.
//! Each call goes to the engine (`BrowserCall`), which hands it to the app's
//! window holding the tabs; this side only names the operation.

use serde_json::{Value, json};

use super::{ToolDef, Tools};

pub(super) const BROWSER_TABS: &str = "browser_tabs";
pub(super) const BROWSER_OPEN: &str = "browser_open";
pub(super) const BROWSER_CLOSE: &str = "browser_close";
pub(super) const BROWSER_SNAPSHOT: &str = "browser_snapshot";
pub(super) const BROWSER_ACT: &str = "browser_act";
pub(super) const BROWSER_EVAL: &str = "browser_eval";
pub(super) const BROWSER_CONSOLE: &str = "browser_console";
pub(super) const BROWSER_NETWORK: &str = "browser_network";
pub(super) const BROWSER_SCREENSHOT: &str = "browser_screenshot";
pub(super) const NAMES: [&str; 9] = [
    BROWSER_TABS,
    BROWSER_OPEN,
    BROWSER_CLOSE,
    BROWSER_SNAPSHOT,
    BROWSER_ACT,
    BROWSER_EVAL,
    BROWSER_CONSOLE,
    BROWSER_NETWORK,
    BROWSER_SCREENSHOT,
];

fn tab() -> Value {
    json!({ "type": "string", "description": "The tab's id from browser_tabs. Omit for the tab the chat shows, else its newest." })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: BROWSER_TABS,
            description: "The browser tabs beside this chat in the Commutator app (the person sees them in the chat's right panel): each one's `id`, `url`, `title`, whether it's `loading`, and which one is `shown`. Works in the desktop app on macOS.",
            input_schema: object(json!({}), &[]),
        },
        ToolDef {
            name: BROWSER_OPEN,
            description: "Open `url` in a browser tab beside this chat (a dev server at localhost:3000 works as is), in a new tab or, with `tab`, in that one. Returns the tab once its page has loaded (or after 30s). The person sees the tab open.",
            input_schema: object(
                json!({ "url": { "type": "string" }, "tab": tab() }),
                &["url"],
            ),
        },
        ToolDef {
            name: BROWSER_CLOSE,
            description: "Close a browser tab beside this chat.",
            input_schema: object(json!({ "tab": tab() }), &["tab"]),
        },
        ToolDef {
            name: BROWSER_SNAPSHOT,
            description: "Read a tab's page as an accessibility-like outline: headings, text, links, buttons, fields and their values, each element you can act on with a ref (`e12`) for browser_act. `target` (a ref or CSS selector) reads only that part. Refs last as long as the page.",
            input_schema: object(json!({ "tab": tab(), "target": { "type": "string" } }), &[]),
        },
        ToolDef {
            name: BROWSER_ACT,
            description: "Act on a tab's page as a person would: `click`, `dblclick`, `hover`, `focus`, `fill` (replace a field's text), `type` (key by key), `press` (a key or chord, `Enter`, `Meta+a`), `select` (options of a <select>, by value or text), `check`/`uncheck`, `scroll` (`up`, `down`, `top`, `bottom`, or to `target`). `target` is a ref from browser_snapshot or a CSS selector. Events come from the page's own JavaScript, so a page that checks `isTrusted` may ignore them. Returns what happened; read the page again with browser_snapshot.",
            input_schema: object(
                json!({
                    "tab": tab(),
                    "action": { "type": "string", "enum": ["click", "dblclick", "hover", "focus", "fill", "type", "press", "select", "check", "uncheck", "scroll"] },
                    "target": { "type": "string", "description": "A ref (`e12`) or a CSS selector; for `press`, the element (default: the focused one); for `scroll`, where to (default `down`)." },
                    "text": { "type": "string", "description": "For fill and type: the text. For press: the key." },
                    "values": { "type": "array", "items": { "type": "string" }, "description": "For select." }
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: BROWSER_EVAL,
            description: "Run JavaScript in a tab's page and return its result (a string as is, anything else as JSON, an element as its HTML). `code` is an expression (`document.title`), or with `body: true` a function body that `return`s; both are async, so `await` works. Waits up to `timeout_secs` (default 30).",
            input_schema: object(
                json!({
                    "tab": tab(),
                    "code": { "type": "string" },
                    "body": { "type": "boolean", "default": false },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 55, "default": 30 }
                }),
                &["code"],
            ),
        },
        ToolDef {
            name: BROWSER_CONSOLE,
            description: "A tab's console lines since its page loaded (log, warn, error, uncaught errors and rejections, dialogs), the last 300. `clear: true` empties them.",
            input_schema: object(json!({ "tab": tab(), "clear": { "type": "boolean" } }), &[]),
        },
        ToolDef {
            name: BROWSER_NETWORK,
            description: "A tab's requests since its page loaded: fetch and XMLHttpRequest ones as `#n` with method, status, time and URL, and the page's other loads (scripts, images, the document). `request: n` shows request #n whole, with its response body when it is text. `clear: true` empties the list.",
            input_schema: object(
                json!({ "tab": tab(), "request": { "type": "integer" }, "clear": { "type": "boolean" } }),
                &[],
            ),
        },
        ToolDef {
            name: BROWSER_SCREENSHOT,
            description: "A screenshot of a tab's page as you see it in its viewport, returned to you as an image.",
            input_schema: object(json!({ "tab": tab() }), &[]),
        },
    ]
}

impl Tools {
    pub(super) async fn browser(&self, tool: &str, args: Value) -> Result<Value, String> {
        let chat_id = self
            .commutator
            .origin()
            .chat_id
            .clone()
            .ok_or("The browser tools only work from an agent Commutator launched in a chat.")?;
        let op = tool.strip_prefix("browser_").unwrap_or(tool);
        let mut request = match args {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        request["op"] = json!(op);
        self.commutator
            .call(
                commutator_rpc::methods::BROWSER_CALL,
                json!({ "chatId": chat_id, "request": request }),
            )
            .await
            .map_err(|e| format!("{e:#}"))
    }
}
