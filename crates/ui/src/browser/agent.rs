//! What an agent's browser tools run in a page (Shepherd's `shepherd browser`,
//! MIT): the instrument every page starts with (its requests and console
//! lines), the snapshot and actions (`agent_tools.js`), async eval with a poll,
//! and the screenshot. The calls come from `shell/browser_agent.rs`.
use super::BrowserSurface;
use futures::channel::oneshot;

/// The page's snapshot and actions, as `window.__commutatorTools`.
const TOOLS: &str = include_str!("agent_tools.js");

/// Every page's first script: keeps the page's last requests (fetch and
/// XMLHttpRequest, with their status and a text body) and console lines in
/// `window.__commutatorAgent`.
// ponytail: wraps the page's own fetch, XHR and console, so a page that saved
// them before (or a worker, or a frame) isn't seen; WebKit's inspector protocol
// would see everything, but no public API reaches it
pub(super) const INSTRUMENT: &str = r#"(() => {
  if (window.__commutatorAgent) return;
  const A = { net: [], log: [], seq: 0 };
  Object.defineProperty(window, '__commutatorAgent', { value: A });
  const MAX = 300, BODY = 100000;
  const keep = (list, e) => { list.push(e); if (list.length > MAX) list.shift(); return e; };
  const abs = u => { try { return new URL(u, location.href).href; } catch { return String(u); } };
  const textish = t => /json|text|xml|javascript|html|urlencoded|graphql/.test(t || '') && !/event-stream/.test(t);
  const show = v => {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return v.stack ? `${v}\n${v.stack}` : String(v);
    try { return JSON.stringify(v) ?? String(v); } catch { return String(v); }
  };
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const was = console[level];
    console[level] = function (...args) {
      keep(A.log, { t: Date.now(), level, text: args.map(show).join(' ') });
      return was.apply(this, args);
    };
  }
  addEventListener('error', e => keep(A.log, { t: Date.now(), level: 'error', text: `Uncaught ${e.message} (${e.filename}:${e.lineno})` }));
  addEventListener('unhandledrejection', e => keep(A.log, { t: Date.now(), level: 'error', text: `Unhandled rejection: ${show(e.reason)}` }));
  const done = (e, status, type) => { e.status = status; e.ms = Date.now() - e.t; e.contentType = type || ''; };
  const F = window.fetch;
  if (F) window.fetch = function (input, init) {
    const e = keep(A.net, { id: ++A.seq, kind: 'fetch', method: String(init?.method || input?.method || 'GET').toUpperCase(), url: abs(input?.url ?? input), t: Date.now() });
    return F.apply(this, arguments).then(r => {
      done(e, r.status, r.headers.get('content-type'));
      if (textish(e.contentType) && !(+r.headers.get('content-length') > 1e6)) r.clone().text().then(t => { e.body = t.slice(0, BODY); }, () => {});
      return r;
    }, err => { e.error = String(err); e.ms = Date.now() - e.t; throw err; });
  };
  const X = XMLHttpRequest.prototype, open = X.open, send = X.send;
  X.open = function (method, url) { this.__commutator = { method: String(method).toUpperCase(), url: abs(url) }; return open.apply(this, arguments); };
  X.send = function () {
    if (this.__commutator) {
      const e = keep(A.net, { id: ++A.seq, kind: 'xhr', ...this.__commutator, t: Date.now() });
      this.addEventListener('loadend', () => {
        done(e, this.status, this.getResponseHeader('content-type'));
        if (!this.status) e.error = 'failed';
        else if (textish(e.contentType) && (this.responseType === '' || this.responseType === 'text')) e.body = this.responseText.slice(0, BODY);
      });
    }
    return send.apply(this, arguments);
  };
  try { performance.setResourceTimingBufferSize(1000); } catch {}
})();"#;

/// A string as a JS literal.
pub(crate) fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// `call` (on `T`, the tools) as a script that answers `{"ok", "text"}`, an
/// error's message as the text.
pub(crate) fn tools_call(call: &str) -> String {
    format!(
        "{TOOLS}\n;(() => {{ const T = window.__commutatorTools; try {{ return JSON.stringify({{ ok: true, text: String({call}) }}); }} \
         catch (e) {{ return JSON.stringify({{ ok: false, text: String(e && e.message || e) }}); }} }})()"
    )
}

/// The requests: the recorded ones (`#n`) and Resource Timing's others, by
/// when they started; `entry`, that one recorded with its body.
pub(crate) fn network_script(entry: Option<u64>, clear: bool) -> String {
    if clear {
        return "(() => { window.__commutatorAgent && (window.__commutatorAgent.net.length = 0); performance.clearResourceTimings(); return 'cleared'; })()".into();
    }
    if let Some(n) = entry {
        return format!(
            "(() => {{ const e = window.__commutatorAgent?.net.find(e => e.id === {n}); return e ? JSON.stringify(e, null, 2) : 'no request #{n}: browser_network lists them'; }})()"
        );
    }
    r#"(() => {
  const A = window.__commutatorAgent;
  const rows = (A?.net || []).map(e => ({ t: e.t, line: `#${e.id}  ${e.method} ${e.error ? 'failed' : e.status ?? 'pending'}  ${e.kind}  ${e.ms ?? '-'}ms  ${e.url}${e.body !== undefined ? '  (body)' : ''}` }));
  const nav = performance.getEntriesByType('navigation').concat(performance.getEntriesByType('resource'));
  for (const r of nav) {
    if (r.initiatorType === 'fetch' || r.initiatorType === 'xmlhttprequest') continue;
    rows.push({ t: performance.timeOrigin + r.startTime, line: `-  GET ${r.responseStatus || ''}  ${r.entryType === 'navigation' ? 'document' : r.initiatorType}  ${Math.round(r.duration)}ms  ${r.name}` });
  }
  rows.sort((a, b) => a.t - b.t);
  return (A ? '' : '(this page loaded before its requests were recorded: reload it to see fetch and XHR)\n') + rows.map(r => r.line).join('\n');
})()"#
        .into()
}

pub(crate) fn console_script(clear: bool) -> String {
    if clear {
        return "(() => { window.__commutatorAgent && (window.__commutatorAgent.log.length = 0); return 'cleared'; })()".into();
    }
    r#"(() => {
  const A = window.__commutatorAgent;
  if (!A) return '(this page loaded before its console was recorded: reload it)';
  const at = t => { const d = new Date(t); return `${d.toTimeString().slice(0, 8)}.${String(d.getMilliseconds()).padStart(3, '0')}`; };
  return A.log.map(l => `${at(l.t)} [${l.level}] ${l.text}`).join('\n');
})()"#
        .into()
}

/// Starts `code` in the page as run `id`, its outcome kept in
/// `window.__commutatorRuns[id]` for [`poll_script`]. As an expression, or
/// as a function's body when `body`; both async, so `await` works. The code
/// goes in as source, not to `eval`, which a page's CSP may forbid.
pub(crate) fn eval_script(id: u64, code: &str, body: bool) -> String {
    let code = code.trim();
    let wrapped = match body {
        false => format!("(\n{}\n)", code.trim_end_matches(';')),
        true => format!("{{\n{code}\n}}"),
    };
    format!(
        r#"(() => {{
  const R = window.__commutatorRuns || Object.defineProperty(window, '__commutatorRuns', {{ value: {{}} }}).__commutatorRuns;
  R[{id}] = null;
  const show = v => {{
    if (v === undefined) return 'undefined';
    if (typeof v === 'string') return v;
    if (typeof Element !== 'undefined' && v instanceof Element) return v.outerHTML;
    try {{ return JSON.stringify(v, null, 2) ?? String(v); }} catch {{ return String(v); }}
  }};
  (async () => {wrapped})().then(
    v => {{ R[{id}] = {{ ok: true, text: show(v) }}; }},
    e => {{ R[{id}] = {{ ok: false, text: e && e.stack ? `${{e}}\n${{e.stack}}` : String(e) }}; }});
  return 'started';
}})()"#
    )
}

/// Run `id`'s outcome: `null` while it runs, `{"ok", "text"}` once done,
/// `{"gone": true}` when the page navigated.
pub(crate) fn poll_script(id: u64) -> String {
    format!(
        "(() => {{ const R = window.__commutatorRuns; if (!R || !({id} in R)) return JSON.stringify({{ gone: true }}); const r = R[{id}]; if (r) delete R[{id}]; return JSON.stringify(r); }})()"
    )
}

/// A script's result as WebKit hands it (JSON) when it's a string.
pub(crate) fn js_string(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(raw).ok()
}

impl BrowserSurface {
    /// Runs `script` in the page; the receiver gets its result as JSON, or
    /// nothing when there is no page.
    pub(crate) fn agent_js(&self, script: &str) -> Option<oneshot::Receiver<String>> {
        #[cfg(target_os = "macos")]
        {
            let native = self.native.as_ref()?;
            let (tx, rx) = oneshot::channel();
            // WebKit's callback is an `Fn`: it hands the one answer over once.
            let tx = std::sync::Mutex::new(Some(tx));
            native.handle().borrow().eval_with_callback(script, move |result| {
                if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
                    let _ = tx.send(result);
                }
            });
            Some(rx)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = script;
            None
        }
    }

    /// The page as its viewport shows it, as a PNG.
    pub(crate) fn screenshot(&self) -> Option<oneshot::Receiver<Result<Vec<u8>, String>>> {
        #[cfg(target_os = "macos")]
        {
            let native = self.native.as_ref()?;
            let (tx, rx) = oneshot::channel();
            native.handle().borrow().snapshot(move |png| {
                let _ = tx.send(png);
            });
            Some(rx)
        }
        #[cfg(not(target_os = "macos"))]
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_wraps_code_both_ways() {
        let e = eval_script(7, "document.title;", false);
        assert!(e.contains("(async () => (\ndocument.title\n))()"), "{e}");
        let s = eval_script(7, "const a = 1; return a", true);
        assert!(s.contains("(async () => {\nconst a = 1; return a\n})()"), "{s}");
        assert_eq!(js_string("\"started\"").as_deref(), Some("started"));
        assert_eq!(js_string(""), None);
        assert!(tools_call("T.click(\"e1\")").contains("window.__commutatorTools"));
    }
}
