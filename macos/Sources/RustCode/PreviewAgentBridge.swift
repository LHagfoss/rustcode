import AppKit
import RustCodeKit
import WebKit

/// Runs the agent's `localhost_preview` actions in the visible preview
/// (#2138). Every request is checked against the user's grant for its chat
/// and the approved origin before it runs and again after any wait, runs
/// one at a time, and is bounded by `actionTimeout`. Page scripts run in
/// WebKit's isolated client world, so the page cannot see or tamper with
/// them, and results come back as return values: no script message handler
/// is installed, so the page has no channel to the app.
extension PreviewController {
    static let actionTimeout: Duration = .seconds(20)
    static let maxScreenshotWidth = 1280
    /// A panel narrower than this lays the page out like a phone; the
    /// agent's screenshot is taken at `desktopWidth` instead.
    static let narrowPanel: Double = 1024
    static let desktopWidth: Double = 1280
    static let desktopHeight: Double = 900

    /// Collects the page's uncaught errors, failed resource loads and
    /// unhandled rejections, newest last, in the isolated client world.
    static let pageErrorCollector = #"""
    (() => {
      const errors = window.__rustcodePageErrors = [];
      const push = (line) => { errors.push(String(line).slice(0, 400)); if (errors.length > 20) errors.shift(); };
      addEventListener('error', (e) => {
        if (e instanceof ErrorEvent) {
          const where = e.filename ? ` (${e.filename.replace(location.origin, '')}:${e.lineno}:${e.colno})` : '';
          push((e.message || String(e.error)) + where);
        } else if (e.target && e.target !== window) {
          const t = e.target;
          push(`Failed to load <${t.localName}> ${t.src || t.href || ''}`.trim());
        }
      }, true);
      addEventListener('unhandledrejection', (e) => {
        const r = e.reason;
        push('Unhandled promise rejection: ' + (r instanceof Error ? `${r.name}: ${r.message}` : String(r)));
      });
    })();
    """#

    /// Answer one `_rustcode/preview` request. Never throws: every failure
    /// is an `{"ok": false}` answer the engine reports as an error.
    func runAgentRequest(_ params: JSONValue) async -> JSONValue {
        let request: PreviewAgentRequest
        do { request = try PreviewAgentRequest(params: params) } catch { return Self.agentError(error).result }
        guard !isAgentActing else { return PreviewAgentError.busy.result }

        // The origin the request needs: the page's, or a navigate target's
        // when nothing is shown yet or the chat has no grant (the panel may
        // still show another dev server the user opened).
        let granted = agent.access.origin(for: request.sessionId)
        let origin: PreviewOrigin
        if case .navigate(let target) = request.operation, agentPageURL == nil || granted == nil {
            switch PreviewOrigin.navigationTarget(target, from: agentPageURL, approved: granted) {
            case .success(let url):
                guard let target = PreviewOrigin(url) else { return PreviewAgentError.noPage.result }
                origin = target
            case .failure(let error): return error.result
            }
        } else {
            guard let page = agentPageURL else { return PreviewAgentError.noPage.result }
            guard let pageOrigin = PreviewOrigin(page) else {
                return PreviewAgentError.outOfScope("The preview is not showing a local page.").result
            }
            origin = pageOrigin
        }
        if let error = await agent.authorize(session: request.sessionId, workspace: workspace, origin: origin) {
            return error.result
        }
        // The prompt may have waited: check the project, grant and page again.
        // A navigate checks its target against the grant when it runs.
        var isNavigate = false
        if case .navigate = request.operation { isNavigate = true }
        let page = agentPageURL
        if !isNavigate, page == nil { return PreviewAgentError.noPage.result }
        if let error = agent.access.check(session: request.sessionId, workspace: workspace,
                                          pageURL: isNavigate ? nil : page) {
            return error.result
        }
        guard !isAgentActing else { return PreviewAgentError.busy.result }

        isAgentActing = true
        agentNotes = []
        let activity = request.operation.activity
        agentActivity = activity
        defer {
            isAgentActing = false
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(1.5))
                if self?.agentActivity == activity { self?.agentActivity = nil }
            }
        }
        let operation = request.operation
        do {
            var result = try await withPreviewTimeout(Self.actionTimeout, action: operation.action) { @MainActor [self] in
                try await self.perform(operation, session: request.sessionId)
            }
            if case .object(var object) = result {
                if !agentNotes.isEmpty { object["note"] = .string(agentNotes.joined(separator: " ")) }
                if let errors = await pageErrors(), !errors.isEmpty { object["errors"] = .array(errors) }
                result = .object(object)
            }
            return result
        } catch {
            return Self.agentError(error).result
        }
    }

    /// What the shown page threw since it loaded, for every answer: nil when
    /// they cannot be read.
    private func pageErrors() async -> [JSONValue]? {
        guard let view = createdWebView else { return nil }
        let raw = try? await view.callAsyncJavaScript(
            "return JSON.stringify(window.__rustcodePageErrors || [])", arguments: [:], in: nil, contentWorld: .defaultClient)
        guard let text = raw as? String, let data = text.data(using: .utf8),
              case .array(let errors) = try? JSONDecoder().decode(JSONValue.self, from: data) else { return nil }
        return errors
    }

    /// The page the agent would act on, when it is a loaded local page.
    var agentPageURL: URL? {
        guard let url = createdWebView?.url ?? state.url, PreviewURL.isLocal(url) else { return nil }
        return url
    }

    private static func agentError(_ error: Error) -> PreviewAgentError {
        if let error = error as? PreviewAgentError { return error }
        if error is CancellationError { return .timedOut("The action was cancelled; its outcome is unknown.") }
        return .failed(error.localizedDescription)
    }

    private func perform(_ operation: PreviewAgentOperation, session: String) async throws -> JSONValue {
        let view = webView
        switch operation {
        case .inspect(let maxChars):
            var page = try await evaluate(view, Self.inspectScript, ["maxChars": maxChars])
            try ensureCurrent(view)
            if case .object(var object) = page {
                object["ok"] = true
                page = .object(object)
            }
            return page
        case .screenshot:
            guard view.window != nil, view.bounds.width >= 1, view.bounds.height >= 1 else {
                throw PreviewAgentError.failed("The preview panel is hidden, so there is nothing to capture. Ask the user to show it (⇧⌘P).")
            }
            // A narrow panel lays the page out like a phone, and the agent
            // judged a desktop app by it (762×1672 in session 01a16b37). Lay
            // it out at desktop width for the capture, zoomed to fit the
            // panel, and put the reader's zoom back after.
            let panel = view.bounds.width
            let zoom = view.pageZoom
            let desktop = panel < Self.narrowPanel
            if desktop {
                view.pageZoom = panel / Self.desktopWidth
                try await Task.sleep(for: .milliseconds(400))
            }
            let configuration = WKSnapshotConfiguration()
            if desktop {
                configuration.snapshotWidth = NSNumber(value: Self.desktopWidth)
                // A desktop window's share of the page, not the panel's tall one.
                let height = min(view.bounds.height, panel * Self.desktopHeight / Self.desktopWidth)
                configuration.rect = CGRect(x: 0, y: 0, width: panel, height: height)
            }
            let image: NSImage
            do {
                image = try await view.takeSnapshot(configuration: configuration)
            } catch {
                if desktop { view.pageZoom = zoom }
                throw error
            }
            if desktop { view.pageZoom = zoom }
            try ensureCurrent(view)
            let (data, mime, width, height) = try Self.encodeForAgent(image)
            var page = pageInfo(view)
            page["image"] = ["mimeType": .string(mime), "data": .string(data.base64EncodedString()),
                             "width": .number(Double(width)), "height": .number(Double(height))]
            page["viewport"] = ["width": .number(desktop ? Self.desktopWidth : panel),
                                "panelWidth": .number(panel), "desktop": .bool(desktop)]
            return .object(page)
        case .navigate(let target):
            let approved = agent.access.origin(for: session)
            let url = try PreviewOrigin.navigationTarget(target, from: agentPageURL, approved: approved).get()
            load(url)
            try await waitForLoad(view)
            if let failure = state.failure {
                throw PreviewAgentError.failed("\(failure.title(for: state.url)). \(failure.detail)")
            }
            return try settledPage(view, session: session)
        case .click(let selector):
            _ = try await evaluate(view, Self.prelude + Self.clickScript, ["selector": selector])
            return try await settle(view, session: session)
        case .fill(let selector, let value):
            _ = try await evaluate(view, Self.prelude + Self.fillScript, ["selector": selector, "value": value])
            return try await settle(view, session: session)
        case .select(let selector, let value):
            _ = try await evaluate(view, Self.prelude + Self.selectScript, ["selector": selector, "value": value])
            return try await settle(view, session: session)
        case .scroll(let selector, let dy):
            var arguments: [String: Any] = [:]
            if let selector { arguments["selector"] = selector }
            if let dy { arguments["dy"] = dy }
            let position = try await evaluate(view, Self.prelude + Self.scrollScript, arguments)
            var page = try settledPage(view, session: session)
            if case .object(var object) = page, let y = position["scrollY"] {
                object["scrollY"] = y
                page = .object(object)
            }
            return page
        }
    }

    /// The web view was replaced (a project switch) while the action ran.
    private func ensureCurrent(_ view: WKWebView) throws {
        guard view === createdWebView else { throw PreviewAgentError.revoked }
    }

    private func pageInfo(_ view: WKWebView) -> [String: JSONValue] {
        ["ok": true, "url": .string(view.url?.absoluteString ?? state.url?.absoluteString ?? ""),
         "title": .string(view.title ?? "")]
    }

    /// After a click or an edit: give the page a moment, and wait for a
    /// navigation it started.
    private func settle(_ view: WKWebView, session: String) async throws -> JSONValue {
        try await Task.sleep(for: .milliseconds(300))
        try await waitForLoad(view)
        return try settledPage(view, session: session)
    }

    private func settledPage(_ view: WKWebView, session: String) throws -> JSONValue {
        try ensureCurrent(view)
        if let url = view.url, let approved = agent.access.origin(for: session), PreviewOrigin(url) != approved {
            agentNotes.append("The page is now at \(url.absoluteString.prefix(200)), outside the approved origin \(approved); further actions are refused.")
        }
        return .object(pageInfo(view))
    }

    /// Poll the load state; the navigation delegate drives it.
    private func waitForLoad(_ view: WKWebView) async throws {
        while state.isLoading || view.isLoading {
            try await Task.sleep(for: .milliseconds(100))
            try ensureCurrent(view)
        }
    }

    /// Run a script in the isolated client world and decode its JSON answer.
    /// A script reports a problem as `{"error": code, "message": …}`.
    private func evaluate(_ view: WKWebView, _ body: String, _ arguments: [String: Any]) async throws -> JSONValue {
        let raw: Any?
        do {
            raw = try await view.callAsyncJavaScript(body, arguments: arguments, in: nil, contentWorld: .defaultClient)
        } catch {
            throw PreviewAgentError.failed("The page script failed: \(error.localizedDescription)")
        }
        try ensureCurrent(view)
        guard let text = raw as? String, let data = text.data(using: .utf8),
              let value = try? JSONDecoder().decode(JSONValue.self, from: data) else {
            throw PreviewAgentError.failed("The page script returned no result.")
        }
        if let code = value["error"]?.stringValue {
            let message = value["message"]?.stringValue ?? code
            switch code {
            case "not_found": throw PreviewAgentError.notFound(message)
            case "invalid": throw PreviewAgentError.invalid(message)
            default: throw PreviewAgentError.failed(message)
            }
        }
        return value
    }

    /// PNG (JPEG when the PNG is large), at most `maxScreenshotWidth` wide.
    static func encodeForAgent(_ image: NSImage) throws -> (Data, String, Int, Int) {
        guard let source = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            throw PreviewAgentError.failed("The screenshot could not be read.")
        }
        let scale = min(1, Double(maxScreenshotWidth) / Double(max(source.width, 1)))
        let width = max(1, Int((Double(source.width) * scale).rounded()))
        let height = max(1, Int((Double(source.height) * scale).rounded()))
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            throw PreviewAgentError.failed("The screenshot could not be scaled.")
        }
        context.interpolationQuality = .high
        context.draw(source, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let scaled = context.makeImage() else { throw PreviewAgentError.failed("The screenshot could not be scaled.") }
        let bitmap = NSBitmapImageRep(cgImage: scaled)
        if let png = bitmap.representation(using: .png, properties: [:]), png.count <= 4 * 1024 * 1024 {
            return (png, "image/png", width, height)
        }
        guard let jpeg = bitmap.representation(using: .jpeg, properties: [.compressionFactor: 0.8]) else {
            throw PreviewAgentError.failed("The screenshot could not be encoded.")
        }
        return (jpeg, "image/jpeg", width, height)
    }

    // MARK: - Page scripts (isolated world; each returns a JSON string)

    private static let inspectScript = #"""
    const describe = (el) => {
      const esc = (v) => CSS.escape(v);
      const unique = (sel) => { try { return document.querySelectorAll(sel).length === 1; } catch (e) { return false; } };
      if (el.id && unique('#' + esc(el.id))) return '#' + esc(el.id);
      for (const attr of ['data-testid', 'name', 'aria-label']) {
        const v = el.getAttribute(attr);
        if (v) { const sel = el.localName + '[' + attr + '="' + v.replace(/["\\]/g, '\\$&') + '"]'; if (unique(sel)) return sel; }
      }
      const parts = [];
      let node = el;
      while (node && node.nodeType === 1 && node !== document.documentElement) {
        if (node.id && unique('#' + esc(node.id))) { parts.unshift('#' + esc(node.id)); break; }
        let part = node.localName;
        const parent = node.parentElement;
        if (parent) {
          const same = Array.from(parent.children).filter((c) => c.localName === node.localName);
          if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
        }
        parts.unshift(part);
        node = parent;
      }
      return parts.join(' > ');
    };
    const visible = (el) => {
      const r = el.getBoundingClientRect();
      const s = getComputedStyle(el);
      return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
    };
    const clip = (v, n) => (v || '').replace(/\s+/g, ' ').trim().slice(0, n);
    const text = document.body ? document.body.innerText : '';
    const query = 'a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=tab],[role=menuitem],[contenteditable=""],[contenteditable=true]';
    const nodes = Array.from(document.querySelectorAll(query)).filter(visible);
    const elements = nodes.slice(0, 80).map((el) => {
      const tag = el.localName;
      const type = el.getAttribute('type') || undefined;
      const field = tag === 'input' || tag === 'textarea' || tag === 'select';
      return {
        selector: describe(el), tag, type,
        role: el.getAttribute('role') || undefined,
        text: clip(el.innerText || el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('alt'), 160),
        name: el.getAttribute('name') || undefined,
        // Never hand back what is typed into a password field.
        value: field && type !== 'password' ? clip(String(el.value), 160) : undefined,
        href: tag === 'a' ? el.href : undefined,
        disabled: el.disabled === true ? true : undefined,
      };
    });
    return JSON.stringify({
      url: location.href, title: document.title,
      text: text.slice(0, maxChars), truncated: text.length > maxChars || nodes.length > 80,
      elements,
    });
    """#

    private static let prelude = #"""
    const find = (selector) => {
      let el;
      try { el = document.querySelector(selector); } catch (e) { return { error: 'invalid', message: 'Invalid CSS selector: ' + selector }; }
      if (!el) return { error: 'not_found', message: 'No element matches ' + selector };
      return { el };
    };
    const flash = (el) => {
      const before = el.style.outline;
      el.style.outline = '2px solid #2f80ed';
      setTimeout(() => { el.style.outline = before; }, 700);
    };
    """#

    private static let clickScript = #"""
    const found = find(selector);
    if (found.error) return JSON.stringify(found);
    const el = found.el;
    if (el.disabled) return JSON.stringify({ error: 'invalid', message: selector + ' is disabled' });
    el.scrollIntoView({ block: 'center', inline: 'center' });
    flash(el);
    if (typeof el.focus === 'function') el.focus();
    el.click();
    return JSON.stringify({ ok: true });
    """#

    private static let fillScript = #"""
    const found = find(selector);
    if (found.error) return JSON.stringify(found);
    const el = found.el;
    el.scrollIntoView({ block: 'center' });
    flash(el);
    el.focus();
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      if (el.disabled || el.readOnly) return JSON.stringify({ error: 'invalid', message: selector + ' is not editable' });
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
    } else if (el.isContentEditable) {
      el.textContent = value;
    } else {
      return JSON.stringify({ error: 'invalid', message: selector + ' is not a text field' });
    }
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return JSON.stringify({ ok: true });
    """#

    private static let selectScript = #"""
    const found = find(selector);
    if (found.error) return JSON.stringify(found);
    const el = found.el;
    if (!(el instanceof HTMLSelectElement)) return JSON.stringify({ error: 'invalid', message: selector + ' is not a select element' });
    const option = Array.from(el.options).find((o) => o.value === value) || Array.from(el.options).find((o) => o.text.trim() === value.trim());
    if (!option) return JSON.stringify({ error: 'not_found', message: 'No option ' + value + ' in ' + selector });
    el.scrollIntoView({ block: 'center' });
    flash(el);
    el.value = option.value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return JSON.stringify({ ok: true });
    """#

    private static let scrollScript = #"""
    if (typeof selector === 'string') {
      const found = find(selector);
      if (found.error) return JSON.stringify(found);
      found.el.scrollIntoView({ block: 'center' });
      flash(found.el);
    } else {
      window.scrollBy(0, dy);
    }
    return JSON.stringify({ ok: true, scrollY: window.scrollY });
    """#
}
