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
    @Environment(\.dismiss) private var dismiss
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
            .frame(width: 200)
            .background(Color.sidebarBackground)

            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text(section.rawValue).font(.title2.weight(.semibold))
                    Spacer()
                    ChromeButton(systemImage: "xmark", help: "Close (Esc)") { dismiss() }
                        .keyboardShortcut(.cancelAction)
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 12)
                ScrollView {
                    Group {
                        switch section {
                        case .general: GeneralSettings(model: model)
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
        .frame(width: 720, height: 480)
        .tint(.brand)
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
                Image(systemName: section.systemImage)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 24, height: 24)
                    .background(section.tile.gradient, in: .rect(cornerRadius: 6))
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
private struct SettingsGroup<Content: View>: View {
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

private struct SettingsRow<Trailing: View>: View {
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

private struct RowDivider: View {
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
                SettingsRow(title: "Project", subtitle: model.workspace.path.abbreviatingHome) {
                    Button("Choose…") { model.chooseWorkspace() }.buttonStyle(PillButtonStyle())
                }
                RowDivider()
                SettingsRow(title: "Notifications", subtitle: "When a chat finishes or needs you while RustCode is in the background") {
                    Toggle("Notifications", isOn: $notifyTurns)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .onChange(of: notifyTurns) { _, on in if on { TurnNotifier.shared.start() } }
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

            if let defaults = model.modelDefaults {
                SettingsGroup(title: "New Chats",
                              footer: "New chats start with this model and effort; each chat keeps its own once changed. The terminal UI uses the same defaults.") {
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
    case general = "General", engine = "Engine", about = "About"

    var id: Self { self }

    var systemImage: String {
        switch self {
        case .general: "gearshape.fill"
        case .engine: "terminal.fill"
        case .about: "info.circle.fill"
        }
    }

    var tile: Color {
        switch self {
        case .general: .gray
        case .engine: .orange
        case .about: .blue
        }
    }
}
