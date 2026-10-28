import AppKit
import RustCodeKit
import SwiftUI

/// The What's New card shown once after the app was updated, and its full
/// changelog (#2506). The update pill stays the way to install a newer
/// release; this describes the version already running.
@MainActor
final class WhatsNew: ObservableObject {
    /// The compact card, over the dimmed window until it is closed.
    @Published var card: ReleaseNotes?
    /// The full changelog, over the window as Settings is.
    @Published var full: ReleaseNotes?

    private let defaults: UserDefaults
    private var checked = false
    /// Whether the app had saved settings before this launch wrote any.
    private let launchedBefore: Bool

    init(defaults: UserDefaults = .standard, domain: String? = Bundle.main.bundleIdentifier) {
        self.defaults = defaults
        // Read before the launch saves window frames or sweep dates.
        let saved = domain.flatMap(defaults.persistentDomain(forName:)) ?? [:]
        launchedBefore = saved.keys.contains { $0 != WhatsNewGate.announcedKey }
    }

    /// At launch: show the card if this version was not announced yet. A
    /// fresh install records its version without one (an update from a
    /// release older than the card still shows it), and a development build
    /// never shows it.
    func checkOnLaunch(current: AppVersion?) async {
        guard !checked else { return }
        checked = true
        switch WhatsNewGate.decide(current: current, announced: defaults.string(forKey: WhatsNewGate.announcedKey),
                                   launchedBefore: launchedBefore) {
        case .nothing:
            return
        case .record(let version):
            defaults.set(version.description, forKey: WhatsNewGate.announcedKey)
        case .announce(let version):
            // Recorded before anything can fail, so it is shown at most once.
            defaults.set(version.description, forKey: WhatsNewGate.announcedKey)
            let notes = await notes(for: ReleaseNotes.tag(for: version))
            withAnimation(.snappy(duration: 0.3)) { card = notes }
        }
    }

    /// Help › What's New: the full changelog of the running version.
    func reopen(current: AppVersion?) async {
        card = nil
        guard let current else {
            full = ReleaseNotes(tag: "development", body: "")
            return
        }
        full = await notes(for: ReleaseNotes.tag(for: current))
    }

    #if DEBUG
    /// For the debug driver: show `tag`'s card, or its full changelog.
    func debugShow(tag: String, full showsFull: Bool) async {
        let notes = await notes(for: tag)
        if showsFull {
            card = nil
            full = notes
        } else {
            withAnimation(.snappy(duration: 0.3)) { card = notes }
        }
    }
    #endif

    func readMore() {
        full = card
        withAnimation(.snappy(duration: 0.2)) { card = nil }
    }

    func dismissCard() {
        withAnimation(.snappy(duration: 0.2)) { card = nil }
    }

    /// The notes of `tag`: cached after the first fetch, so a reopen makes
    /// no request; empty when GitHub cannot be reached or rate-limits, which
    /// the views show as a link to the release page instead.
    private func notes(for tag: String) async -> ReleaseNotes {
        let key = "whatsNewNotes.\(tag)"
        if let data = defaults.data(forKey: key), let cached = try? JSONDecoder().decode(ReleaseNotes.self, from: data) {
            return cached
        }
        var request = URLRequest(url: ReleaseNotes.apiURL(tag: tag), timeoutInterval: 10)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200,
              let notes = ReleaseNotes(apiJSON: data, expectedTag: tag) else {
            return ReleaseNotes(tag: tag, body: "")
        }
        if let encoded = try? JSONEncoder().encode(notes) { defaults.set(encoded, forKey: key) }
        return notes
    }
}

/// The compact card: artwork over the top half, then a few highlights. It is
/// shown as a panel over the dimmed window, as Settings is, so it is closed
/// before anything else in the window is used.
struct WhatsNewCard: View {
    let notes: ReleaseNotes
    let readMore: () -> Void
    let dismiss: () -> Void

    private var highlights: [String] { notes.highlights() }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ReleaseArt(version: notes.version?.description)
                .frame(height: 150)
                .overlay(alignment: .topTrailing) {
                    Button(action: dismiss) {
                        Image(systemName: "xmark")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 22, height: 22)
                            .background(.black.opacity(0.25), in: .circle)
                    }
                    .buttonStyle(.plain)
                    .help("Close")
                    .accessibilityLabel("Close What's New")
                    .padding(10)
                }
            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("What's new in RustCode").font(.title3.weight(.semibold))
                    if let version = notes.version {
                        Text("Version \(version.description)").font(.caption).foregroundStyle(.secondary)
                    }
                }
                if highlights.isEmpty {
                    Text("This update brings fixes and improvements. Read the release notes for the details.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(highlights, id: \.self) { line in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Image(systemName: "sparkle")
                                    .font(.system(size: 9, weight: .semibold))
                                    .foregroundStyle(Color.brand)
                                Text(line)
                                    .font(.callout)
                                    .lineLimit(2)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                }
                HStack(spacing: 8) {
                    Spacer()
                    Button("Later", action: dismiss).buttonStyle(PillButtonStyle())
                    Button("Read more", action: readMore)
                        .buttonStyle(PillButtonStyle(prominent: true))
                        .keyboardShortcut(.defaultAction)
                }
                .padding(.top, 2)
            }
            .padding(16)
        }
        .frame(width: 360)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("What's new in RustCode")
    }
}

/// The full changelog, styled as Settings: a header with the version and a
/// close button, the notes, and the release's own page on GitHub.
struct WhatsNewPanel: View {
    let notes: ReleaseNotes
    @Environment(\.closePanel) private var close

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ReleaseArt(version: notes.version?.description)
                .frame(height: 120)
                .overlay(alignment: .topTrailing) {
                    ChromeButton(systemImage: "xmark", help: "Close (Esc)") { close() }
                        .colorScheme(.dark)
                        .padding(10)
                }
            HStack(alignment: .firstTextBaseline) {
                Text("What's New").font(.title2.weight(.semibold))
                if let version = notes.version {
                    Text("Version \(version.description)").font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(.horizontal, 24)
            .padding(.top, 18)
            .padding(.bottom, 10)
            ScrollView {
                Group {
                    if notes.version == nil {
                        Text("This is a development build, so it has no release notes.")
                            .foregroundStyle(.secondary)
                    } else if notes.readableBody.isEmpty {
                        Text("The release notes could not be loaded. They are on the release's page on GitHub.")
                            .foregroundStyle(.secondary)
                    } else {
                        MarkdownText(text: notes.readableBody)
                            .textSelection(.enabled)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 24)
                .padding(.bottom, 16)
            }
            Divider()
            HStack(spacing: 8) {
                Spacer()
                if notes.version != nil {
                    Button("View on GitHub") { NSWorkspace.shared.open(notes.pageURL) }
                        .buttonStyle(PillButtonStyle())
                }
                Button("Done") { close() }
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 14)
        }
        .frame(minWidth: 420, idealWidth: 640, maxWidth: 680, minHeight: 320, idealHeight: 560, maxHeight: 640)
    }
}

/// The release artwork, drawn rather than downloaded so the card never
/// waits on the network: the app's slate blue in depth, with the version.
struct ReleaseArt: View {
    let version: String?

    var body: some View {
        ZStack {
            LinearGradient(colors: [Color(red: 0.16, green: 0.22, blue: 0.29), Color(red: 0.31, green: 0.43, blue: 0.53)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
            GeometryReader { geometry in
                let size = geometry.size
                Circle()
                    .fill(Color.white.opacity(0.07))
                    .frame(width: size.height * 1.6)
                    .position(x: size.width * 0.86, y: size.height * 0.15)
                Circle()
                    .fill(Color.white.opacity(0.05))
                    .frame(width: size.height * 1.1)
                    .position(x: size.width * 0.08, y: size.height * 1.05)
                Circle()
                    .strokeBorder(Color.white.opacity(0.12), lineWidth: 1)
                    .frame(width: size.height * 0.9)
                    .position(x: size.width * 0.72, y: size.height * 0.62)
            }
            VStack(spacing: 6) {
                Image(systemName: "sparkles")
                    .font(.system(size: 30, weight: .medium))
                    .symbolRenderingMode(.hierarchical)
                if let version {
                    Text(version)
                        .font(.system(size: 26, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                }
            }
            .foregroundStyle(.white)
            .shadow(color: .black.opacity(0.18), radius: 6, y: 2)
        }
        .clipped()
        .accessibilityHidden(true)
    }
}

/// The card and the changelog, each as a panel over the window.
/// Observes only `WhatsNew`, so the rest of the window does not redraw.
struct WhatsNewPresenter: ViewModifier {
    @ObservedObject var whatsNew: WhatsNew

    func body(content: Content) -> some View {
        content
            .clickAwayPanel(item: $whatsNew.card) { notes in
                WhatsNewCard(notes: notes, readMore: whatsNew.readMore, dismiss: whatsNew.dismissCard)
            }
            .clickAwayPanel(item: $whatsNew.full) { WhatsNewPanel(notes: $0) }
    }
}
