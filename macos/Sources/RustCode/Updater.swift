import AppKit
import CryptoKit
import RustCodeKit
import SwiftUI

/// Checks GitHub for a newer RustCode release and installs it in place: download
/// the app zip, verify it against the release's SHA256SUMS, unpack it, then swap
/// the bundle and relaunch once this process has quit.
@MainActor
final class Updater: ObservableObject {
    enum State: Equatable {
        case idle
        case checking
        case upToDate
        case available(AppRelease)
        case downloading(AppRelease, Double)
        case installing(AppRelease)
        case failed(String)
    }

    @Published private(set) var state = State.idle

    /// The version of the running app; nil for a development build.
    let currentVersion = (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String)
        .flatMap(AppVersion.init)

    /// Every five minutes: a release takes longer than that to build.
    private var polling: Task<Void, Never>?
    static let pollInterval: Duration = .seconds(300)

    /// A newer release, once a check found one.
    var available: AppRelease? {
        switch state {
        case .available(let release), .downloading(let release, _), .installing(let release): release
        default: nil
        }
    }

    /// The app can replace itself only as a bundle in a folder it may write to.
    var canInstallInPlace: Bool {
        let bundle = Bundle.main.bundleURL
        return bundle.pathExtension == "app"
            && FileManager.default.isWritableFile(atPath: bundle.deletingLastPathComponent().path)
    }

    /// Check now and then every few minutes while the app runs, quietly: a failed
    /// poll leaves what is shown alone.
    func startPolling() {
        guard polling == nil else { return }
        polling = Task { [weak self] in
            while !Task.isCancelled {
                await self?.check(quietly: true)
                try? await Task.sleep(for: Self.pollInterval)
            }
        }
    }

    func check(quietly: Bool = false) async {
        if case .downloading = state { return }
        if case .installing = state { return }
        // A background poll must not hide the sidebar's update row while it runs.
        if !quietly { state = .checking }
        do {
            guard let release = try await Self.latest() else {
                throw UpdateError("GitHub did not answer the update check.")
            }
            if case .downloading = state { return }
            if case .installing = state { return }
            if let currentVersion, release.version <= currentVersion {
                state = .upToDate
            } else if try await Self.exists(release.appURL) {
                state = .available(release)
            } else {
                // Tagged, but its macOS app is not uploaded yet.
                state = .upToDate
            }
        } catch {
            if !quietly { state = .failed(Self.message(error)) }
        }
    }

    /// The latest release, from where `releases/latest` redirects.
    private nonisolated static func latest() async throws -> AppRelease? {
        var request = URLRequest(url: AppRelease.latestURL)
        request.httpMethod = "HEAD"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (_, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200, let url = response.url else { return nil }
        return AppRelease(releasePage: url)
    }

    private nonisolated static func exists(_ url: URL) async throws -> Bool {
        var request = URLRequest(url: url)
        request.httpMethod = "HEAD"
        let (_, response) = try await URLSession.shared.data(for: request)
        return (response as? HTTPURLResponse)?.statusCode == 200
    }

    func install() async {
        guard let release = available else { return }
        guard canInstallInPlace else {
            if let page = release.pageURL { NSWorkspace.shared.open(page) }
            return
        }
        do {
            state = .downloading(release, 0)
            let work = FileManager.default.temporaryDirectory
                .appendingPathComponent("rustcode-update-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
            let zip = try await Self.download(release.appURL, to: work.appendingPathComponent(AppRelease.appAssetName)) { progress in
                Task { @MainActor in
                    if case .downloading = self.state { self.state = .downloading(release, progress) }
                }
            }
            state = .installing(release)
            try await verify(zip, against: release)
            let unpacked = work.appendingPathComponent("unpacked", isDirectory: true)
            try await Self.run("/usr/bin/ditto", ["-x", "-k", zip.path, unpacked.path])
            let newApp = unpacked.appendingPathComponent("RustCode.app")
            guard FileManager.default.fileExists(atPath: newApp.path) else { throw UpdateError("The download has no RustCode.app.") }
            try await Self.run("/usr/bin/codesign", ["--verify", "--deep", "--strict", newApp.path])
            try relaunch(replacing: Bundle.main.bundleURL, with: newApp)
        } catch {
            state = .failed(Self.message(error))
        }
    }

    /// A download task writes the file off the main actor; reading the body
    /// byte by byte here made a 16 MB update crawl and starved the UI.
    private nonisolated static func download(_ url: URL, to destination: URL, progress: @escaping @Sendable (Double) -> Void) async throws -> URL {
        try await withCheckedThrowingContinuation { continuation in
            var observation: NSKeyValueObservation?
            let task = URLSession.shared.downloadTask(with: url) { location, response, error in
                observation?.invalidate()
                do {
                    if let error { throw error }
                    guard let location, (response as? HTTPURLResponse)?.statusCode == 200 else {
                        throw UpdateError("The download failed.")
                    }
                    try? FileManager.default.removeItem(at: destination)
                    try FileManager.default.moveItem(at: location, to: destination)
                    progress(1)
                    continuation.resume(returning: destination)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
            var reported = -1
            observation = task.progress.observe(\.fractionCompleted) { value, _ in
                let percent = Int(value.fractionCompleted * 100)
                guard percent != reported else { return }
                reported = percent
                progress(value.fractionCompleted)
            }
            task.resume()
        }
    }

    private func verify(_ zip: URL, against release: AppRelease) async throws {
        guard let sumsURL = release.checksumsURL else { throw UpdateError("The release has no checksums to verify the download.") }
        let (data, _) = try await URLSession.shared.data(from: sumsURL)
        guard let expected = AppRelease.checksum(for: AppRelease.appAssetName, in: String(decoding: data, as: UTF8.self)) else {
            throw UpdateError("The release checksums do not list the app.")
        }
        let digest = try await Task.detached {
            SHA256.hash(data: try Data(contentsOf: zip, options: .mappedIfSafe))
                .map { String(format: "%02x", $0) }.joined()
        }.value
        guard digest == expected else { throw UpdateError("The download does not match the release checksum.") }
    }

    /// A detached shell waits for this process to exit, swaps the bundle and
    /// opens the new one; the app then quits.
    private func relaunch(replacing current: URL, with newApp: URL) throws {
        let script = """
        while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
        rm -rf "$2.old" && mv "$2" "$2.old" && mv "$3" "$2" && rm -rf "$2.old"
        xattr -dr com.apple.quarantine "$2" 2>/dev/null
        open "$2"
        """
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "sh", String(ProcessInfo.processInfo.processIdentifier), current.path, newApp.path]
        try process.run()
        NSApp.terminate(nil)
    }

    private static func run(_ tool: String, _ arguments: [String]) async throws {
        try await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: tool)
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                throw UpdateError("\((tool as NSString).lastPathComponent) failed while installing the update.")
            }
        }.value
    }

    private static func message(_ error: Error) -> String {
        (error as? UpdateError)?.message ?? error.localizedDescription
    }
}

struct UpdateError: Error {
    let message: String
    init(_ message: String) { self.message = message }
}

/// The update's status and action, in Settings › About and the sidebar.
struct UpdateControl: View {
    @ObservedObject var updater: Updater

    var body: some View {
        switch updater.state {
        case .idle, .upToDate, .failed:
            VStack(spacing: 6) {
                Button("Check for Updates") { Task { await updater.check() } }
                    .buttonStyle(PillButtonStyle())
                if case .upToDate = updater.state {
                    Text("RustCode is up to date.").font(.caption).foregroundStyle(.secondary)
                }
                if case .failed(let message) = updater.state {
                    Text(message).font(.caption).foregroundStyle(Color.attention).multilineTextAlignment(.center)
                }
            }
        case .checking:
            ProgressView().controlSize(.small)
        case .available(let release):
            Button(updater.canInstallInPlace ? "Update to \(release.version)" : "Download \(release.version)") {
                Task { await updater.install() }
            }
            .buttonStyle(PillButtonStyle(prominent: true))
        case .downloading(_, let progress):
            ProgressView(value: progress) { Text("Downloading…").font(.caption) }
                .frame(width: 180)
        case .installing:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Installing…").font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}
