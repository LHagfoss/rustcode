// `bun run perf:native`: the perf scenarios in a real WKWebView, the engine
// the desktop app ships with, in a window set up like the app's (transparent
// web view over sidebar vibrancy, 60 fps preference off as src/webview.rs does).
// Prints rAF rate, frame intervals, the main thread's rendering work per
// frame, long tasks, and the CPU the WebContent and GPU processes spend.
//
//   swiftc -O scripts/perf-native.swift -o <out> && <out> <url> <scenario.js> [--near60]

import Cocoa
import WebKit

let args = CommandLine.arguments
guard args.count >= 3 else {
    FileHandle.standardError.write("usage: perf-native <url> <scenario.js> [--near60] [--css <file>]\n".data(using: .utf8)!)
    exit(2)
}
let url = URL(string: args[1])!
let scenario = try! String(contentsOfFile: args[2], encoding: .utf8)
let keepNear60 = args.contains("--near60")
// The app's Content-Security-Policy, as a meta tag, to check what it allows (`--csp "<policy>"`).
let csp = args.firstIndex(of: "--csp").map { args[$0 + 1] }
let extraCss = args.firstIndex(of: "--css").map { try! String(contentsOfFile: args[$0 + 1], encoding: .utf8) } ?? ""

// Frame intervals, the work after each rAF (style, layout, paint until the next task),
// and gaps in a 10 ms heartbeat (long tasks; WebKit has no Long Tasks API).
let probe = """
(() => {
  const p = window.__pn = { frames: [], work: [], gaps: [], commits: 0 };
  // React commits, through a devtools hook stub (works in production builds too).
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = { renderers: new Map(), supportsFiber: true, isDisabled: false,
    inject() { return 1; }, onCommitFiberRoot() { p.commits++; }, onCommitFiberUnmount() {},
    onPostCommitFiberRoot() {}, onScheduleFiberRoot() {}, checkDCE() {} };
  const ch = new MessageChannel();
  let at = 0;
  ch.port1.onmessage = () => { p.work.push(performance.now() - at); };
  let last = 0;
  const loop = (t) => { if (last) p.frames.push(t - last); last = t; at = performance.now(); ch.port2.postMessage(0); requestAnimationFrame(loop); };
  requestAnimationFrame(loop);
  let beat = performance.now();
  setInterval(() => { const now = performance.now(); if (now - beat > 50) p.gaps.push(now - beat); beat = now; }, 10);
  p.reset = () => { p.frames = []; p.work = []; p.gaps = []; p.commits = 0; };
})();
"""

final class Cpu {
    static var timebase: mach_timebase_info_data_t = {
        var t = mach_timebase_info_data_t()
        mach_timebase_info(&t)
        return t
    }()
    /** CPU seconds a process has used. */
    static func seconds(_ pid: pid_t) -> Double {
        guard pid > 0 else { return 0 }
        var info = rusage_info_v2()
        let ok = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V2, $0) }
        }
        guard ok == 0 else { return 0 }
        let ticks = Double(info.ri_user_time + info.ri_system_time)
        return ticks * Double(timebase.numer) / Double(timebase.denom) / 1e9
    }
}

/** CPU seconds of WindowServer, which draws CoreAnimation backdrop layers (backdrop-filter). */
let windowServer: pid_t = {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
    p.arguments = ["-x", "WindowServer"]
    let out = Pipe()
    p.standardOutput = out
    try? p.run()
    p.waitUntilExit()
    return pid_t(String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? "") ?? 0
}()

func psSeconds(_ pid: pid_t) -> Double {
    guard pid > 0 else { return 0 }
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/bin/ps")
    p.arguments = ["-o", "time=", "-p", String(pid)]
    let out = Pipe()
    p.standardOutput = out
    try? p.run()
    p.waitUntilExit()
    let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return text.split(separator: ":").reduce(0.0) { $0 * 60 + (Double($1) ?? 0) }
}

func setNear60(_ prefs: WKPreferences, _ on: Bool) {
    guard let features = (WKPreferences.self as AnyObject).perform(NSSelectorFromString("_features"))?
        .takeUnretainedValue() as? [NSObject] else { return }
    let sel = NSSelectorFromString("_setEnabled:forFeature:")
    guard prefs.responds(to: sel) else { return }
    for f in features where (f.value(forKey: "key") as? String) == "PreferPageRenderingUpdatesNear60FPSEnabled" {
        typealias Fn = @convention(c) (AnyObject, Selector, Bool, AnyObject) -> Void
        unsafeBitCast(prefs.method(for: sel)!, to: Fn.self)(prefs, sel, on, f)
    }
}

func pid(_ object: AnyObject, _ name: String) -> pid_t {
    let sel = NSSelectorFromString(name)
    guard object.responds(to: sel) else { return 0 }
    typealias Fn = @convention(c) (AnyObject, Selector) -> pid_t
    return unsafeBitCast(object.method(for: sel)!, to: Fn.self)(object, sel)
}

final class Runner: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
    let web: WKWebView
    var marks: [String: (Double, Double, Date, Double)] = [:]
    var cpu: [String: Any] = [:]
    var alwaysVisible = true
    init(web: WKWebView) { self.web = web }

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        guard let body = m.body as? [String: Any], let kind = body["kind"] as? String,
              let name = body["name"] as? String else { return }
        let content = pid(web, "_webProcessIdentifier")
        let gpu = pid(web, "_gpuProcessIdentifier")
        let now = (Cpu.seconds(content), Cpu.seconds(gpu), Date(), kind == "log" ? 0 : psSeconds(windowServer))
        if !(web.window?.occlusionState.contains(.visible) ?? false) { alwaysVisible = false }
        if kind == "start" { marks[name] = now }
        if kind == "end", let s = marks[name] {
            let wall = now.2.timeIntervalSince(s.2)
            cpu[name] = [
                "webContentCpuPct": ((now.0 - s.0) / wall * 1000).rounded() / 10,
                "gpuCpuPct": ((now.1 - s.1) / wall * 1000).rounded() / 10,
                "windowServerCpuPct": ((now.3 - s.3) / wall * 1000).rounded() / 10,
            ]
        }
        if kind == "log" { print(name); fflush(stdout) }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        print("webcontent pid \(pid(webView, "_webProcessIdentifier"))")
        fflush(stdout)
        // The scenario file is an async arrow function; call it and wait for its result.
        webView.callAsyncJavaScript("return await (\(scenario))();", arguments: [:], in: nil, in: .page) { result in
            switch result {
            case .success(let value):
                var out = (value as? [String: Any]) ?? [:]
                out["cpu"] = self.cpu
                out["screenMaxFps"] = webView.window?.screen?.maximumFramesPerSecond ?? 0
                out["lowPowerMode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
                out["near60"] = keepNear60
                out["visible"] = self.alwaysVisible
                let data = try! JSONSerialization.data(withJSONObject: out, options: [.prettyPrinted, .sortedKeys])
                print(String(data: data, encoding: .utf8)!)
            case .failure(let error):
                print("scenario failed: \(error)")
            }
            fflush(stdout)
            NSApp.terminate(nil)
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let config = WKWebViewConfiguration()
if !keepNear60 { setNear60(config.preferences, false) }
config.userContentController.addUserScript(WKUserScript(source: probe, injectionTime: .atDocumentStart, forMainFrameOnly: true))
if let csp {
    let quoted = csp.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
    config.userContentController.addUserScript(WKUserScript(
        source: "{ const m = document.createElement('meta'); m.httpEquiv = 'Content-Security-Policy'; m.content = \"\(quoted)\"; document.documentElement.prepend(m); }",
        injectionTime: .atDocumentStart, forMainFrameOnly: true))
}
if !extraCss.isEmpty {
    let css = extraCss.replacingOccurrences(of: "`", with: "\\`")
    config.userContentController.addUserScript(WKUserScript(
        source: "addEventListener('DOMContentLoaded', () => { const s = document.createElement('style'); s.textContent = `\(css)`; document.head.append(s); });",
        injectionTime: .atDocumentStart, forMainFrameOnly: true))
}
let window = NSWindow(contentRect: NSRect(x: 80, y: 80, width: 1440, height: 900),
                      styleMask: [.titled, .closable, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
window.titlebarAppearsTransparent = true
window.isOpaque = false
window.backgroundColor = .clear
let effect = NSVisualEffectView(frame: window.contentView!.bounds)
effect.material = .sidebar
effect.blendingMode = .behindWindow
effect.state = .followsWindowActiveState
effect.autoresizingMask = [.width, .height]
let web = WKWebView(frame: effect.bounds, configuration: config)
web.autoresizingMask = [.width, .height]
web.setValue(false, forKey: "drawsBackground")
effect.addSubview(web)
window.contentView = effect
let runner = Runner(web: web)
config.userContentController.add(runner, name: "perf")
web.navigationDelegate = runner
// Floating and on every Space (a full-screen app's too), so nothing covers it: WebKit
// throttles an occluded window to a few frames a second. `visible` in the output says
// whether it stayed visible; a run where it did not measures nothing.
window.level = .floating
window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
DispatchQueue.main.asyncAfter(deadline: .now() + 150) {
    print("timed out")
    exit(1)
}
web.load(URLRequest(url: url))
app.run()
