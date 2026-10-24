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

/// ⌘, — a sheet over the chat: sections on the left, the chosen one on the right.
struct SettingsView: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.closePanel) private var dismiss
    @StateObject private var capabilities = CapabilitiesStore()
    typealias Section = SettingsSection

    private var section: Section { model.settingsSection }

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Settings")
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                ForEach(Section.allCases) { item in
                    SectionRow(section: item, isSelected: item == section) { model.settingsSection = item }
                }
                Spacer()
            }
            .padding(12)
            .padding(.top, 8)
            .frame(width: 220)
            .background(Color.sidebarBackground)

            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text(section.rawValue).font(.title2.weight(.semibold))
                    Spacer()
                    ChromeButton(systemImage: "xmark", help: "Close (Esc)") { dismiss() }
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 12)
                ScrollView {
                    Group {
                        switch section {
                        case .general: GeneralSettings(model: model)
                        case .keybinds: KeybindingsSettings(store: model.keybindings)
                        case .skills: SkillsSettings(model: model, store: capabilities)
                        case .mcp: MCPSettings(model: model, store: capabilities)
                        case .engine: EngineSettings(model: model)
                        case .about: AboutSettings(updater: model.updater)
                        }
                    }
                    .padding(.horizontal, 24)
                    .padding(.bottom, 24)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color.chatBackground)
        }
        .frame(maxWidth: 880, maxHeight: 620)
        .tint(.brand)
        .task(id: section) {
            if section == .general { await model.loadPresence() }
            if section == .skills || section == .mcp { await capabilities.load(model.engineConnection) }
        }
    }
}

private struct SectionRow: View {
    let section: SettingsView.Section
    let isSelected: Bool
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
                Text(section.rawValue)
                Spacer()
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(isSelected ? Color.primary.opacity(0.09) : isHovered ? Color.primary.opacity(0.04) : .clear,
                        in: .rect(cornerRadius: 9))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
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

// MARK: - Sections

private struct GeneralSettings: View {
    @ObservedObject var model: DesktopModel
    @AppStorage(AppearanceSetting.key) private var appearance = ""
    @AppStorage(TurnNotifier.enabledKey) private var notifyTurns = true
    @AppStorage(DesktopModel.autoApproveKey) private var autoApprove = false
    @AppStorage(StreamingProse.revealKey) private var revealsText = true
    @AppStorage(ToolVerbosity.key) private var verbosity = ToolVerbosity.terminalDefault
    @AppStorage(ComposerPredictions.key) private var predictions = true

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
            }
            .onChange(of: appearance) { _, value in AppearanceSetting.apply(value) }

            SettingsGroup(title: "Chat") {
                SettingsRow(title: "Default project folder",
                            subtitle: "\(model.workspace.path.abbreviatingHome) · New chats start here. Open chats keep their own") {
                    Button("Choose…") { model.chooseWorkspace() }.buttonStyle(PillButtonStyle())
                        .help(model.keybindings.help("Choose Default Project", .chooseProject))
                }
                if !ProjectFolder.same(model.chatProject, model.workspace) {
                    RowDivider()
                    SettingsRow(title: "This chat's folder",
                                subtitle: "\(model.chatProject.path.abbreviatingHome) · Only the chat on screen works here") {
                        Button("Show in Finder") { NSWorkspace.shared.open(model.chatProject) }.buttonStyle(PillButtonStyle())
                    }
                }
                RowDivider()
                SettingsRow(title: "Notifications", subtitle: "When a chat finishes or needs you while RustCode is in the background") {
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
                SettingsRow(title: "Tool calls", subtitle: verbositySubtitle) {
                    Picker("Tool calls", selection: $verbosity) {
                        ForEach(ToolVerbosity.allCases) { Text($0.title).tag($0) }
                    }
                    .labelsHidden()
                    .pickerStyle(.segmented)
                    .fixedSize()
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
                SettingsGroup(title: "New Chats",
                              footer: "New chats and new terminal UI sessions start with this model and effort. Changing the model inside a chat or a terminal session applies to that session only.") {
                    SettingsRow(title: "Model") {
                        Picker("Model", selection: Binding(get: { defaults.model },
                                                           set: { value in Task { await model.setModelDefaults(model: value) } })) {
                            ForEach(defaults.models, id: \.name) { profile in Text(profile.name).tag(profile.name) }
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                        .fixedSize()
                    }
                    let efforts = defaults.efforts(for: defaults.model)
                    if !efforts.isEmpty {
                        RowDivider()
                        SettingsRow(title: "Reasoning effort") {
                            Picker("Reasoning effort", selection: Binding(get: { defaults.effort },
                                                                          set: { value in Task { await model.setModelDefaults(effort: value) } })) {
                                ForEach(efforts) { effort in Text(effort.name).tag(effort.id) }
                            }
                            .labelsHidden()
                            .pickerStyle(.menu)
                            .fixedSize()
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
    case general = "General", keybinds = "Keybinds", skills = "Skills", mcp = "MCP Servers", engine = "Engine", about = "About"

    var id: Self { self }

    var systemImage: String {
        switch self {
        case .general: "gearshape"
        case .keybinds: "keyboard"
        case .skills: "sparkles"
        case .mcp: "point.3.connected.trianglepath.dotted"
        case .engine: "terminal"
        case .about: "info.circle"
        }
    }
}
