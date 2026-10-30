import AppKit
import RustCodeKit
import SwiftUI

/// Light, dark or follow the system, stored as the raw value of `NSAppearance.Name`
/// (empty for the system).
enum AppearanceSetting {
    static let key = "appearance"

    @MainActor
    static func apply(_ value: String?) {
        guard let value, !value.isEmpty else {
            NSApp?.appearance = nil
            return
        }
        NSApp?.appearance = NSAppearance(named: NSAppearance.Name(value))
    }
}

/// ⌘, — Settings as a page of the window, in place of the chats and the
/// chat (#2553): its own sidebar of sections on the left, the chosen section
/// on the right. The chat stays alive under it, so Back (or Esc) returns to
/// it with its draft and scroll position. The section sits in a centered
/// column with margins that follow the window, and in a narrow window the
/// sidebar keeps only the sections' icons (`SettingsLayout`, #2593).
struct SettingsView: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.closePanel) private var close
    @StateObject private var capabilities = CapabilitiesStore()
    @StateObject private var connections = ConnectionsStore()
    @AppStorage(SidebarResizeHandle.key) private var sidebarWidth = SidebarResizeHandle.defaultWidth
    /// What the sidebar's search field holds; the sections listed match it.
    @State private var query = ""
    @State private var showsCompactSearch = false
    @FocusState private var searchFocused: Bool
    typealias Section = SettingsSection

    private var section: Section { model.settingsSection }

    var body: some View {
        GeometryReader { proxy in
            let layout = SettingsLayout(width: proxy.size.width,
                                        sidebar: Double(SidebarResizeHandle.clamp(sidebarWidth)))
            HStack(spacing: 0) {
                sidebar(compact: layout.compact)
                    .frame(width: layout.sidebar)
                    .background(Color.sidebarBackground)
                    // The chat sidebar's edge line, so the two read as one.
                    .overlay(alignment: .trailing) {
                        Rectangle().fill(Color.primary.opacity(0.1)).frame(width: 1)
                    }
                detail(layout)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                    .background(Color.chatBackground)
            }
        }
        .ignoresSafeArea()
        .tint(.brand)
        // Esc goes back to the chat from wherever focus is, once a search
        // in the sidebar is cleared.
        .background {
            Button("Back to Thread") { if query.isEmpty { close() } else { query = "" } }
                .keyboardShortcut(.cancelAction).hidden()
        }
        .task(id: section) {
            if section == .general { await model.loadPresence() }
            if section == .skills || section == .mcp { await capabilities.load(model.engineConnection) }
            if section == .connections {
                await connections.load(model.engineConnection)
                await connections.wait(model.engineConnection)
            }
        }
    }

    private var chatTitle: String { model.settingsBackTitle }

    /// Laid out like the chat sidebar it replaces: Back in a toolbar cluster
    /// where that sidebar's buttons sit, a caption header like "Chats", and
    /// rows with its fills, so opening Settings changes only the list.
    private func sidebar(compact: Bool) -> some View {
        VStack(alignment: compact ? .center : .leading, spacing: 0) {
            // The title bar's room, beside the traffic lights, holding Back.
            HStack(spacing: 2) {
                Spacer()
                if !compact { backButton }
            }
            .padding(.leading, compact ? 0 : 80)
            .padding(.trailing, 10)
            .frame(height: SidebarMetrics.barHeight)
            .padding(.top, SidebarMetrics.barTopInset)
            .background(WindowDragArea())
            if compact {
                // Too narrow beside the traffic lights: Back goes under them.
                backButton.padding(.top, 8)
            }
            if !compact {
                searchField
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                Text("Settings")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.leading, 20)
                    .padding(.top, 12)
                    .padding(.bottom, 4)
                    .accessibilityAddTraits(.isHeader)
            } else {
                // Too narrow for a field: a button opens it over the page.
                Button { showsCompactSearch = true } label: {
                    Image(systemName: "magnifyingglass").frame(width: 32, height: 28).contentShape(.rect)
                }
                .buttonStyle(.plain)
                .foregroundStyle(query.isEmpty ? .secondary : Color.brand)
                .help("Search settings")
                .accessibilityLabel("Search settings")
                .padding(.top, 8)
                .popover(isPresented: $showsCompactSearch, arrowEdge: .trailing) {
                    VStack(alignment: .leading, spacing: 4) {
                        searchField
                        ForEach(matches) { item in
                            SectionRow(section: item, isSelected: item == section, compact: false) { choose(item) }
                        }
                        if matches.isEmpty { noMatches }
                    }
                    .padding(10)
                    .frame(width: 220)
                    .onAppear { searchFocused = true }
                }
                Divider().padding(.vertical, 8)
            }
            VStack(alignment: compact ? .center : .leading, spacing: 2) {
                ForEach(matches) { item in
                    SectionRow(section: item, isSelected: item == section, compact: compact) { model.settingsSection = item }
                }
                if matches.isEmpty, !compact { noMatches }
            }
            .padding(.horizontal, compact ? 8 : 12)
            .animation(.snappy(duration: 0.15), value: matches)
            Spacer()
        }
        .padding(.bottom, 12)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Settings sections")
    }

    /// The sections that match the search, all of them while it is empty.
    private var matches: [Section] { Section.matching(query) }

    /// Patterned on the chat sidebar's search: ⏎ opens the first match, Esc
    /// clears the field before it goes back to the chat (#2625).
    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Search settings", text: $query)
                .textFieldStyle(.plain)
                .focused($searchFocused)
                .onSubmit { if let first = matches.first { choose(first) } }
                .onKeyPress(.escape) {
                    guard !query.isEmpty else { return .ignored }
                    query = ""
                    return .handled
                }
                .accessibilityHint("Filters the settings sections")
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .frame(height: 30)
        .glassSurface(in: .capsule)
    }

    private var noMatches: some View {
        Text("No settings match “\(query)”")
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .accessibilityAddTraits(.updatesFrequently)
    }

    private func choose(_ item: Section) {
        model.settingsSection = item
        showsCompactSearch = false
    }

    private var backButton: some View {
        ToolbarCluster {
            ClusterButton(systemImage: "chevron.left", help: "Back to \(chatTitle) (Esc)", action: close)
        }
    }

    /// The section in a column centered in the room right of the sidebar.
    private func detail(_ layout: SettingsLayout) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(section.rawValue)
                .font(.title2.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
                .frame(width: layout.column, alignment: .leading)
                .padding(.top, 40)
                .padding(.bottom, 12)
                .frame(maxWidth: .infinity)
                .background(WindowDragArea())
            ScrollView {
                Group {
                    switch section {
                    case .general: GeneralSettings(model: model)
                    case .keybinds: KeybindingsSettings(store: model.keybindings)
                    case .connections: ConnectionsSettings(model: model, store: connections)
                    case .skills: SkillsSettings(model: model, store: capabilities)
                    case .mcp: MCPSettings(model: model, store: capabilities)
                    case .engine: EngineSettings(model: model)
                    case .about: AboutSettings(updater: model.updater)
                    }
                }
                .frame(width: layout.column, alignment: .leading)
                .padding(.bottom, 32)
                .frame(maxWidth: .infinity)
            }
            // Each section opens at its top, not where the last one was left.
            .id(section)
        }
    }
}

private struct SectionRow: View {
    let section: SettingsView.Section
    let isSelected: Bool
    /// Only the icon, in a narrow window.
    var compact = false
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                // A plain symbol, as in the window's sidebar: no coloured tile.
                Image(systemName: section.systemImage)
                    .font(.system(size: 14, weight: .medium))
                    .symbolRenderingMode(.monochrome)
                    .foregroundStyle(isSelected ? AnyShapeStyle(Color.brand) : AnyShapeStyle(.secondary))
                    .frame(width: 22, height: 22)
                if !compact {
                    Text(section.rawValue)
                    Spacer()
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            // The chat rows' fills: selection strong, hover a faint wash.
            .background(isSelected ? Color.primary.opacity(0.1) : isHovered ? Color.primary.opacity(0.035) : .clear,
                        in: .rect(cornerRadius: 9))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(compact ? section.rawValue : "")
        .accessibilityLabel(section.rawValue)
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
    }
}

// MARK: - Building blocks

/// Rows on one soft card; `RowDivider` goes between them.
struct SettingsGroup<Content: View>: View {
    var title: String?
    var footer: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let title {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary).padding(.leading, 4)
            }
            VStack(spacing: 0) { content }
            .background(Color.primary.opacity(0.045), in: .rect(cornerRadius: 12))
            if let footer {
                Text(footer).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 4)
            }
        }
        .padding(.bottom, 18)
    }
}

struct SettingsRow<Trailing: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                if let subtitle {
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 12)
            trailing
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(minHeight: 44)
    }
}

struct RowDivider: View {
    var body: some View { Divider().padding(.leading, 14).opacity(0.5) }
}

/// A quiet capsule button, the counterpart of the composer's controls.
struct PillButtonStyle: ButtonStyle {
    var prominent = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.callout.weight(.medium))
            .foregroundStyle(prominent ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
            .padding(.horizontal, 12)
            .frame(height: 26)
            .background(prominent ? AnyShapeStyle(Color.brandFill) : AnyShapeStyle(.fill.tertiary), in: .capsule)
            .opacity(configuration.isPressed ? 0.7 : 1)
            .contentShape(.capsule)
    }
}

/// A choice from a list, as a pill that opens a menu: the current value and
/// an up-down chevron, in the same capsule as the page's buttons, with a
/// check by the current value in the menu (#2631).
struct SettingsMenu<Value: Hashable>: View {
    let title: String
    let selection: Value
    let options: [(value: Value, name: String)]
    let choose: (Value) -> Void

    init(_ title: String, selection: Value, options: [(Value, String)], choose: @escaping (Value) -> Void) {
        self.title = title
        self.selection = selection
        self.options = options.map { (value: $0.0, name: $0.1) }
        self.choose = choose
    }

    var body: some View {
        let current = options.first { $0.value == selection }?.name ?? "—"
        Menu {
            ForEach(options, id: \.value) { option in
                Button { choose(option.value) } label: {
                    if option.value == selection {
                        Label(option.name, systemImage: "checkmark")
                    } else {
                        Text(option.name)
                    }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Text(current).lineLimit(1).truncationMode(.middle)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: 260)
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .buttonStyle(PillButtonStyle())
        .fixedSize()
        .help(title)
        .accessibilityLabel(title)
        .accessibilityValue(current)
    }
}

/// A few exclusive choices side by side: a capsule track whose selected
/// segment carries a raised thumb that slides between them, in place of
/// the boxy system segmented control (#2631).
struct SettingsSegments<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [(value: Value, name: String)]
    @Namespace private var thumb
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    init(_ title: String, selection: Binding<Value>, options: [(Value, String)]) {
        self.title = title
        _selection = selection
        self.options = options.map { (value: $0.0, name: $0.1) }
    }

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.value) { option in
                let selected = option.value == selection
                Button {
                    withAnimation(reduceMotion ? nil : .snappy(duration: 0.2)) { selection = option.value }
                } label: {
                    Text(option.name)
                        .font(.callout.weight(selected ? .semibold : .regular))
                        .foregroundStyle(selected ? .primary : .secondary)
                        .padding(.horizontal, 12)
                        .frame(height: 22)
                        .background {
                            if selected {
                                Capsule()
                                    .fill(Color(nsColor: .controlBackgroundColor))
                                    .shadow(color: .black.opacity(0.12), radius: 1.5, y: 0.5)
                                    .matchedGeometryEffect(id: "thumb", in: thumb)
                            }
                        }
                        .contentShape(.capsule)
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .padding(2)
        .background(.fill.tertiary, in: .capsule)
        .fixedSize()
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
    }
}

// MARK: - Sections

private struct GeneralSettings: View {
    @ObservedObject var model: DesktopModel
    @AppStorage(AppearanceSetting.key) private var appearance = ""
    @AppStorage(TurnNotifier.enabledKey) private var notifyTurns = true
    @AppStorage(DesktopModel.autoApproveKey) private var autoApprove = false
    @AppStorage(StreamingProse.revealKey) private var revealsText = true
    @AppStorage(ToolVerbosity.key) private var verbosity = ToolVerbosity.terminalDefault
    @AppStorage(ComposerPredictions.key) private var predictions = true
    @AppStorage(SmartUI.enabledKey) private var smartUI = false
    @AppStorage(DesktopModel.groupsByAttentionKey) private var groupsByAttention = true
    @AppStorage(TurnCompletionCard.enabledKey) private var showsTurnCompletion = true
    @AppStorage(NewThreadBanner.key) private var banner = ""
    @AppStorage(NewThreadBanner.styleKey) private var bannerStyle = "ascii"

    private var verbositySubtitle: String {
        switch verbosity {
        case .low: "A finished turn folds its work into one row; open it for the steps, and a step for its calls"
        case .high: "Every tool call is its own card and every thought shows"
        case .edits: "Like Low, but edits are their own step with the diff open"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsGroup(title: "Appearance") {
                HStack(spacing: 14) {
                    AppearanceTile(title: "System", value: "", selection: $appearance)
                    AppearanceTile(title: "Light", value: NSAppearance.Name.aqua.rawValue, selection: $appearance)
                    AppearanceTile(title: "Dark", value: NSAppearance.Name.darkAqua.rawValue, selection: $appearance)
                    Spacer(minLength: 0)
                }
                .padding(14)
                RowDivider()
                SettingsRow(title: "New thread banner",
                            subtitle: banner.isEmpty
                                ? "A wide picture over new threads, drawn in text or as is. Cropped to at least 3:1"
                                : "Over new threads, cropped to at least 3:1") {
                    HStack(spacing: 8) {
                        if !banner.isEmpty {
                            SettingsSegments("Banner style", selection: $bannerStyle,
                                             options: [("ascii", "Text"), ("picture", "Picture")])
                            Button("Remove") {
                                NewThreadBanner.remove(banner)
                                banner = ""
                            }
                            .buttonStyle(PillButtonStyle())
                        }
                        Button(banner.isEmpty ? "Choose…" : "Change…") {
                            if let name = NewThreadBanner.choose(replacing: banner) { banner = name }
                        }
                        .buttonStyle(PillButtonStyle())
                    }
                }
                if !banner.isEmpty {
                    NewThreadBannerView(name: banner, style: bannerStyle)
                        .id(bannerStyle)
                        .frame(maxHeight: 140)
                        .clipped()
                        .padding(.horizontal, 14)
                        .padding(.bottom, 14)
                }
            }
            .onChange(of: appearance) { _, value in AppearanceSetting.apply(value) }

            SettingsGroup(title: "Thread") {
                SettingsRow(title: "Default project folder",
                            subtitle: "\(model.workspace.path.abbreviatingHome) · New threads start here. Open threads keep their own") {
                    Button("Choose…") { model.chooseWorkspace() }.buttonStyle(PillButtonStyle())
                        .help(model.keybindings.help("Choose Default Project", .chooseProject))
                }
                if !ProjectFolder.same(model.chatProject, model.workspace) {
                    RowDivider()
                    SettingsRow(title: "This thread's folder",
                                subtitle: "\(model.chatProject.path.abbreviatingHome) · Only the thread on screen works here") {
                        Button("Show in Finder") { NSWorkspace.shared.open(model.chatProject) }.buttonStyle(PillButtonStyle())
                    }
                }
                RowDivider()
                SettingsRow(title: "Notifications", subtitle: "When a thread finishes or needs you while RustCode is in the background") {
                    Toggle("Notifications", isOn: $notifyTurns)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .onChange(of: notifyTurns) { _, on in if on { TurnNotifier.shared.start() } }
                }
                RowDivider()
                SettingsRow(title: "Fade in new text",
                            subtitle: "Replies fade in softly as they stream. Off with Reduce Motion") {
                    Toggle("Fade in new text", isOn: $revealsText)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
                RowDivider()
                SettingsRow(title: "Word predictions",
                            subtitle: "Suggest the next word while you type a message; Tab accepts it. On this Mac only") {
                    Toggle("Word predictions", isOn: $predictions)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
                RowDivider()
                SettingsRow(title: "Smart UI (experimental)",
                            subtitle: smartUI == model.engineSmartUI
                                ? "Let the agent reply with small interactive views when you ask for one. They run sandboxed, with no network, file or app access"
                                : "The agent learns this when the engine restarts; threads in progress keep running until then") {
                    HStack(spacing: 8) {
                        // The engine reads the setting at initialize.
                        if smartUI != model.engineSmartUI, model.runningSessions.isEmpty {
                            Button("Restart Engine") { Task { await model.restartEngine() } }
                                .buttonStyle(PillButtonStyle())
                        }
                        Toggle("Smart UI", isOn: $smartUI)
                            .labelsHidden()
                            .toggleStyle(.switch)
                    }
                }
                RowDivider()
                SettingsRow(title: "Turn summary",
                            subtitle: "Under a finished answer, list the files the turn changed and the checks it ran, with how each ended") {
                    Toggle("Turn summary", isOn: $showsTurnCompletion)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
                RowDivider()
                SettingsRow(title: "Group threads by attention",
                            subtitle: "Threads that need you, are working or have a reply to read go to the top of the sidebar. Off keeps newest first") {
                    Toggle("Group threads by attention", isOn: $groupsByAttention)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
                RowDivider()
                SettingsRow(title: "Tool calls", subtitle: verbositySubtitle) {
                    SettingsSegments("Tool calls", selection: $verbosity,
                                     options: ToolVerbosity.allCases.map { ($0, $0.title) })
                }
                RowDivider()
                SettingsRow(title: "Auto-approve tools",
                            subtitle: "Run commands and edits without asking (YOLO). Questions still wait for you") {
                    Toggle("Auto-approve tools", isOn: Binding(
                        get: { autoApprove },
                        set: { on in Task { await model.setAutoApprove(on) } }))
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
            }

            if let presence = model.presence {
                SettingsGroup(title: "Discord",
                              footer: presence.enabled && !presence.discordDetected
                                  ? "Discord's desktop app is not running; your status appears once it is." : nil) {
                    SettingsRow(title: "Rich Presence",
                                subtitle: "Show the project's folder name and whether RustCode is working. Never prompts or paths") {
                        Toggle("Rich Presence", isOn: Binding(
                            get: { presence.enabled },
                            set: { on in Task { await model.setPresence(on) } }))
                            .labelsHidden()
                            .toggleStyle(.switch)
                    }
                }
            }

            if let defaults = model.modelDefaults {
                SettingsGroup(title: "New Threads",
                              footer: "New threads, in the app and the terminal UI, start with this model and effort. Changing the model inside a thread applies to that thread only.") {
                    SettingsRow(title: "Model") {
                        SettingsMenu("Model", selection: defaults.model,
                                     options: defaults.models.map { ($0.name, $0.name) }) { value in
                            Task { await model.setModelDefaults(model: value) }
                        }
                    }
                    let efforts = defaults.efforts(for: defaults.model)
                    if !efforts.isEmpty {
                        RowDivider()
                        SettingsRow(title: "Reasoning effort") {
                            SettingsMenu("Reasoning effort", selection: defaults.effort,
                                         options: efforts.map { ($0.id, $0.name) }) { value in
                                Task { await model.setModelDefaults(effort: value) }
                            }
                        }
                    }
                }
            }
        }
        .task { await model.loadModelDefaults() }
    }
}

/// A miniature window in the appearance it stands for, as System Settings draws it.
private struct AppearanceTile: View {
    let title: String
    let value: String
    @Binding var selection: String

    private var isSelected: Bool { selection == value }

    var body: some View {
        Button { selection = value } label: {
            VStack(spacing: 6) {
                preview
                    .frame(width: 96, height: 62)
                    .clipShape(.rect(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(isSelected ? Color.brand : Color.primary.opacity(0.1), lineWidth: isSelected ? 2.5 : 1)
                    }
                Text(title)
                    .font(.callout)
                    .foregroundStyle(isSelected ? .primary : .secondary)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var preview: some View {
        if value.isEmpty {
            HStack(spacing: 0) {
                MiniWindow(dark: false)
                MiniWindow(dark: true)
            }
        } else {
            MiniWindow(dark: value == NSAppearance.Name.darkAqua.rawValue)
        }
    }
}

private struct MiniWindow: View {
    let dark: Bool

    var body: some View {
        let base = dark ? Color(white: 0.12) : Color(white: 0.97)
        let side = dark ? Color(white: 0.2) : Color(white: 0.88)
        let line = dark ? Color(white: 0.32) : Color(white: 0.78)
        HStack(spacing: 0) {
            side.frame(width: 26)
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(line).frame(width: 34, height: 4)
                Capsule().fill(line).frame(width: 46, height: 4)
                Spacer()
                Capsule().fill(Color.brandFill).frame(height: 8)
            }
            .padding(7)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            .background(base)
        }
    }
}

// MARK: - Keybinds

private struct KeybindingsSettings: View {
    @ObservedObject var store: KeybindingStore
    @State private var query = ""
    @State private var recording: KeybindingAction?
    @State private var note: Note?
    @State private var noteTask: Task<Void, Never>?

    private struct Note: Equatable {
        let text: String
        let isError: Bool
        /// The note reports a change, so it offers Undo.
        var undoable = false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                search
                Spacer(minLength: 8)
                HStack(spacing: 6) {
                    Button(action: undo) {
                        Label("Undo", systemImage: "arrow.uturn.backward")
                    }
                    .buttonStyle(GlassPillButtonStyle())
                    .disabled(!store.canUndo)
                    .help("Undo the last shortcut change")
                    Button(action: resetAll) {
                        Label("Reset All", systemImage: "arrow.counterclockwise")
                    }
                    .buttonStyle(GlassPillButtonStyle())
                    .disabled(!store.hasCustomizations)
                    .help("Restore every shortcut to its default. Undo brings them back")
                }
            }
            .padding(.bottom, 12)

            if let recording {
                RecordingBanner(action: recording) { self.recording = nil }
                    .padding(.bottom, 12)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            } else if let note {
                NoteBanner(text: note.text, isError: note.isError, undo: note.undoable && store.canUndo ? undo : nil)
                    .padding(.bottom, 12)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }

            ForEach(KeybindingCategory.allCases) { category in
                let actions = category.actions.filter(matches)
                if !actions.isEmpty {
                    SettingsGroup(title: category.rawValue) {
                        ForEach(Array(actions.enumerated()), id: \.element.id) { index, action in
                            if index > 0 { RowDivider() }
                            KeybindingRow(action: action, store: store, recording: $recording, report: show)
                        }
                    }
                }
            }

            if KeybindingAction.allCases.allSatisfy({ !matches($0) }) {
                VStack(spacing: 8) {
                    Image(systemName: "keyboard")
                        .font(.system(size: 24, weight: .light))
                        .foregroundStyle(.secondary)
                    Text("No shortcuts match “\(query)”.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    Button("Clear Search") { query = "" }
                        .buttonStyle(GlassPillButtonStyle())
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 36)
            }

            Text("Shortcuts save automatically on this Mac. Click a shortcut to record a new one: Esc cancels, Delete clears it. Taking a shortcut another command uses leaves that command without one. Undo reverts the last change.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 4)
                .padding(.top, 2)
        }
        .animation(.snappy(duration: 0.2), value: recording)
        .onDisappear {
            recording = nil
            noteTask?.cancel()
        }
    }

    private var search: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Search shortcuts", text: $query)
                .textFieldStyle(.plain)
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
            }
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .frame(height: 30)
        .glassSurface(in: .capsule)
        .frame(maxWidth: 280)
        .simultaneousGesture(TapGesture().onEnded { recording = nil })
    }

    private func matches(_ action: KeybindingAction) -> Bool {
        guard !query.isEmpty else { return true }
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return true }
        return action.title.localizedCaseInsensitiveContains(needle)
            || action.detail.localizedCaseInsensitiveContains(needle)
            || action.category.rawValue.localizedCaseInsensitiveContains(needle)
            || store.display(for: action).localizedCaseInsensitiveContains(needle)
    }

    private func resetAll() {
        withAnimation(.snappy(duration: 0.2)) {
            recording = nil
            store.resetAll()
        }
        show("All shortcuts are back to their defaults.", isError: false)
    }

    private func undo() {
        recording = nil
        if store.undo() {
            show("Undid the last shortcut change.", isError: false, undoable: false)
        }
    }

    /// A message under the toolbar. One that reports a change offers Undo.
    private func show(_ text: String, isError: Bool) {
        show(text, isError: isError, undoable: !isError)
    }

    private func show(_ text: String, isError: Bool, undoable: Bool) {
        noteTask?.cancel()
        withAnimation(.snappy(duration: 0.2)) { note = Note(text: text, isError: isError, undoable: undoable) }
        noteTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(undoable ? 6 : 4))
            guard !Task.isCancelled else { return }
            withAnimation(.easeOut(duration: 0.2)) { note = nil }
        }
    }
}

/// Shown while a shortcut is being recorded, so it is plain what keys do now.
private struct RecordingBanner: View {
    let action: KeybindingAction
    let cancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "record.circle")
                .foregroundStyle(Color.brand)
                .symbolEffect(.pulse, options: .repeating)
            VStack(alignment: .leading, spacing: 1) {
                Text("Recording a shortcut for \(action.title)")
                    .font(.callout.weight(.medium))
                Text("Press the new keys. Delete clears the shortcut, Esc cancels.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Button("Cancel", action: cancel)
                .buttonStyle(GlassPillButtonStyle())
                .help("Keep the current shortcut (Esc)")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassSurface(in: .rect(cornerRadius: 14))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Color.brand.opacity(0.45), lineWidth: 1)
        }
    }
}

private struct NoteBanner: View {
    let text: String
    let isError: Bool
    var undo: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            Label(text, systemImage: isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .font(.callout.weight(.medium))
                .foregroundStyle(isError ? Color.attention : Color.brand)
            Spacer(minLength: 8)
            if let undo {
                Button("Undo", action: undo)
                    .buttonStyle(GlassPillButtonStyle())
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
        .glassSurface(in: .rect(cornerRadius: 14))
    }
}

/// A capsule button on Liquid Glass, for the controls above the keybinds.
private struct GlassPillButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.callout.weight(.medium))
            .labelStyle(.titleAndIcon)
            .foregroundStyle(isEnabled ? AnyShapeStyle(.primary) : AnyShapeStyle(.tertiary))
            .padding(.horizontal, 12)
            .frame(height: 28)
            .interactiveGlassSurface(in: .capsule)
            .opacity(configuration.isPressed ? 0.7 : 1)
            .contentShape(.capsule)
    }
}

private struct KeybindingRow: View {
    let action: KeybindingAction
    @ObservedObject var store: KeybindingStore
    @Binding var recording: KeybindingAction?
    let report: (String, Bool) -> Void

    private var isRecording: Bool { recording == action }

    var body: some View {
        SettingsRow(title: action.title, subtitle: action.detail) {
            HStack(spacing: 6) {
                if isRecording {
                    RowIconButton(systemImage: "xmark", help: "Cancel (Esc)") { recording = nil }
                } else if store.isCustomized(action) {
                    RowIconButton(systemImage: "arrow.counterclockwise",
                                  help: "Reset to \(action.defaultCombo?.display ?? "no shortcut")") { reset() }
                }
                ShortcutRecorder(action: action, store: store, recording: $recording, report: report)
            }
        }
        .background(isRecording ? Color.brand.opacity(0.07) : .clear)
        .contextMenu {
            Button("Record New Shortcut") { recording = action }
            Button("Clear Shortcut") {
                store.clear(action)
                report("\(action.title) has no shortcut now.", false)
            }
            .disabled(store.combo(for: action) == nil)
            Button("Reset to Default", action: reset)
                .disabled(!store.isCustomized(action))
        }
    }

    private func reset() {
        recording = nil
        store.reset(action)
        report("\(action.title) is back to \(store.display(for: action)).", false)
    }
}

private struct RowIconButton: View {
    let systemImage: String
    let help: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 11, weight: .semibold))
                .frame(width: 24, height: 24)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help(help)
        .accessibilityLabel(help)
    }
}

/// The keycap that records the next shortcut. While it is on, a local event
/// monitor takes key presses before they reach a menu, a text field or the
/// panel's own Esc, so Esc cancels the recording rather than closing Settings.
private struct ShortcutRecorder: View {
    let action: KeybindingAction
    @ObservedObject var store: KeybindingStore
    @Binding var recording: KeybindingAction?
    let report: (String, Bool) -> Void
    @State private var monitor: Any?
    @State private var held: KeyModifiers = []

    private var isRecording: Bool { recording == action }
    private var combo: KeyCombo? { store.combo(for: action) }

    var body: some View {
        Button { recording = isRecording ? nil : action } label: {
            ShortcutKeycap(text: labelText, isRecording: isRecording, hasShortcut: combo != nil)
        }
        .buttonStyle(.plain)
        .help(isRecording
              ? "Press a shortcut. Delete clears it, Esc cancels."
              : "Click to record a shortcut")
        .accessibilityLabel("Shortcut for \(action.title)")
        .accessibilityValue(isRecording ? "Recording" : combo?.display ?? "Not set")
        .onChange(of: isRecording) { _, on in
            if on { start() } else { stop() }
        }
        .onDisappear { stop() }
    }

    private var labelText: String {
        if isRecording {
            return held.symbols.isEmpty ? "Press keys…" : held.symbols + "…"
        }
        return combo?.display ?? "Not set"
    }

    private func start() {
        held = []
        stopMonitor()
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .flagsChanged]) { event in
            handle(event)
        }
    }

    private func stop() {
        stopMonitor()
        held = []
    }

    private func stopMonitor() {
        if let monitor {
            NSEvent.removeMonitor(monitor)
            self.monitor = nil
        }
    }

    private func handle(_ event: NSEvent) -> NSEvent? {
        guard isRecording else { return event }

        if event.type == .flagsChanged {
            held = KeyModifiers(event.modifierFlags)
            return nil
        }
        switch KeyRecorderInput(event: event) {
        case .cancel:
            // The old shortcut stays.
            recording = nil
        case .clear:
            store.clear(action)
            recording = nil
            report("\(action.title) has no shortcut now.", false)
        case .unusable:
            report("That key can't be used. Try a letter, number, arrow or punctuation key.", true)
        case .record(let combo):
            switch store.assign(combo, to: action) {
            case .assigned(let displaced):
                recording = nil
                if let displaced {
                    report("\(combo.display) is now \(action.title). \(displaced.title) has no shortcut.", false)
                } else {
                    report("\(action.title) is now \(combo.display).", false)
                }
            case .rejected(let reason):
                report(reason, true)
            }
        }
        return nil
    }
}

private struct ShortcutKeycap: View {
    let text: String
    let isRecording: Bool
    let hasShortcut: Bool
    @State private var isHovered = false

    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .medium, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.75)
            .foregroundStyle(foreground)
            .padding(.horizontal, 10)
            .frame(minWidth: 72, minHeight: 26)
            .background {
                Capsule().fill(isRecording ? Color.brand.opacity(0.16) : Color.primary.opacity(isHovered ? 0.1 : 0.06))
            }
            .overlay {
                Capsule().strokeBorder(isRecording ? Color.brand : Color.primary.opacity(0.12),
                                       style: StrokeStyle(lineWidth: isRecording ? 1.5 : 1,
                                                          dash: hasShortcut || isRecording ? [] : [3, 2]))
            }
            .contentShape(.capsule)
            .onHover { isHovered = $0 }
    }

    private var foreground: Color {
        isRecording ? Color.brand : hasShortcut ? .primary : .secondary
    }
}

private struct EngineSettings: View {
    @ObservedObject var model: DesktopModel
    @AppStorage("enginePath") private var enginePath = ""
    @State private var restarting = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsGroup(title: "Running") {
                SettingsRow(title: model.engineLocation?.source.rawValue ?? "Not running",
                            subtitle: model.engineLocation?.url.path.abbreviatingHome) {
                    HStack(spacing: 8) {
                        if restarting { ProgressView().controlSize(.small) }
                        Button("Restart") {
                            restarting = true
                            Task {
                                await model.restartEngine()
                                restarting = false
                            }
                        }
                        .buttonStyle(PillButtonStyle())
                        .disabled(restarting)
                    }
                }
            }

            SettingsGroup(title: "Custom engine",
                          footer: "Leave empty to use the engine bundled with the app, or the installed rustcode CLI. Restart to apply.") {
                HStack(spacing: 8) {
                    TextField("", text: $enginePath, prompt: Text("Path to a rustcode binary"))
                        .textFieldStyle(.plain)
                        .font(.callout.monospaced())
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .background(Color.chatBackground, in: .rect(cornerRadius: 7))
                    Button("Choose…", action: chooseEngine).buttonStyle(PillButtonStyle())
                    if !enginePath.isEmpty {
                        Button("Reset") { enginePath = "" }.buttonStyle(PillButtonStyle())
                    }
                }
                .padding(12)
            }
        }
    }

    private func chooseEngine() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.treatsFilePackagesAsDirectories = true
        panel.prompt = "Use Engine"
        if panel.runModal() == .OK, let url = panel.url { enginePath = url.path }
    }
}

private struct AboutSettings: View {
    @ObservedObject var updater: Updater
    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development"
    }

    var body: some View {
        VStack(spacing: 10) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 104, height: 104)
            Text("RustCode").font(.title.weight(.semibold))
            Text("Version \(version)").foregroundStyle(.secondary)
            Link(destination: URL(string: "https://github.com/LHagfoss/rustcode")!) {
                Label("View on GitHub", systemImage: "arrow.up.right")
            }
            .buttonStyle(PillButtonStyle())
            .padding(.top, 6)
            UpdateControl(updater: updater)
                .padding(.top, 14)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 30)
    }
}

enum SettingsSection: String, CaseIterable, Identifiable {
    case general = "General", keybinds = "Keybinds", connections = "Connections", skills = "Skills", mcp = "MCP Servers", engine = "Engine", about = "About"

    var id: Self { self }

    var systemImage: String {
        switch self {
        case .general: "gearshape"
        case .keybinds: "keyboard"
        case .connections: "person.2.badge.key"
        case .skills: "sparkles"
        case .mcp: "point.3.connected.trianglepath.dotted"
        case .engine: "terminal"
        case .about: "info.circle"
        }
    }

    /// Words for what a section holds, so a search for a setting finds the
    /// section it lives in, not only one named like it (#2625).
    var keywords: [String] {
        switch self {
        case .general: ["appearance", "theme", "dark", "light", "notifications", "project", "folder", "fade", "sidebar", "group",
                        "predictions", "smart ui", "tool calls", "verbosity", "auto-approve", "yolo",
                        "discord", "rich presence", "model", "effort", "reasoning", "new threads", "new chats",
                        "banner", "picture", "image", "ascii", "wallpaper"]
        case .keybinds: ["keyboard", "shortcuts", "hotkeys", "keys"]
        case .connections: ["accounts", "providers", "login", "sign in", "api key", "github", "copilot", "remote", "phone"]
        case .skills: ["skill", "agent skills"]
        case .mcp: ["mcp", "servers", "tools", "model context protocol"]
        case .engine: ["engine", "restart", "logs", "binary", "version", "path"]
        case .about: ["version", "updates", "license", "release"]
        }
    }

    /// The sections whose name or keywords contain every word of `query`,
    /// in sidebar order; all of them for an empty query.
    static func matching(_ query: String) -> [SettingsSection] {
        let words = query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
        guard !words.isEmpty else { return allCases }
        return allCases.filter { section in
            let text = ([section.rawValue] + section.keywords).joined(separator: " ").lowercased()
            return words.allSatisfy(text.contains)
        }
    }
}
