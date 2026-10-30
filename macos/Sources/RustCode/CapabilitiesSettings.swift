import RustCodeKit
import SwiftUI

/// Skills and MCP servers for Settings (#2165), read from and toggled through
/// the app's engine so "connected" means the engine really has the server up.
@MainActor
final class CapabilitiesStore: ObservableObject {
    @Published private(set) var snapshot: CapabilitySnapshot?
    @Published private(set) var loading = false
    @Published var error: String?
    /// Toggles waiting for the engine, by `kind:name`.
    @Published private(set) var pending: Set<String> = []

    func load(_ connection: ACPConnection?) async {
        guard let connection else {
            error = "The RustCode engine is not running."
            return
        }
        loading = true
        defer { loading = false }
        do {
            let result = try await connection.request(ACPMethod.capabilities, params: .object([:]), timeout: .seconds(15))
            snapshot = try CapabilitySnapshot(json: result)
            error = nil
        } catch {
            self.error = Self.message(for: error)
        }
    }

    func set(_ kind: CapabilityKind, name: String, enabled: Bool, connection: ACPConnection?) async {
        guard let connection else { return }
        let key = "\(kind.rawValue):\(name)"
        pending.insert(key)
        defer { pending.remove(key) }
        apply(kind, name: name, enabled: enabled)   // optimistic; replaced by the answer
        do {
            // Starting an MCP server can take up to its 10 s handshake limit.
            let result = try await connection.request(
                ACPMethod.setCapability, params: ACPRequests.setCapability(kind, name: name, enabled: enabled),
                timeout: .seconds(30))
            snapshot = try CapabilitySnapshot(json: result)
            error = nil
        } catch {
            apply(kind, name: name, enabled: !enabled)
            self.error = Self.message(for: error)
        }
    }

    /// Save and start a new server. Returns the engine's error message, or
    /// nil once it is in the config; a server that fails to start is still
    /// added and shows its error on its row.
    func addServer(_ params: JSONValue, connection: ACPConnection?) async -> String? {
        guard let connection else { return "The RustCode engine is not running." }
        do {
            // Starting the server can take up to its 10 s handshake limit.
            let result = try await connection.request(ACPMethod.addMCPServer, params: params, timeout: .seconds(30))
            snapshot = try CapabilitySnapshot(json: result)
            error = nil
            return nil
        } catch {
            return Self.message(for: error)
        }
    }

    func removeServer(_ name: String, connection: ACPConnection?) async {
        guard let connection else { return }
        let key = "\(CapabilityKind.mcp.rawValue):\(name)"
        pending.insert(key)
        defer { pending.remove(key) }
        do {
            let result = try await connection.request(
                ACPMethod.removeMCPServer, params: ACPRequests.removeMCPServer(name: name), timeout: .seconds(15))
            snapshot = try CapabilitySnapshot(json: result)
            error = nil
        } catch {
            self.error = Self.message(for: error)
        }
    }

    func isPending(_ kind: CapabilityKind, _ name: String) -> Bool {
        pending.contains("\(kind.rawValue):\(name)")
    }

    private func apply(_ kind: CapabilityKind, name: String, enabled: Bool) {
        switch kind {
        case .skill:
            if let index = snapshot?.skills.firstIndex(where: { $0.name == name }) { snapshot?.skills[index].enabled = enabled }
        case .mcp:
            if let index = snapshot?.mcpServers.firstIndex(where: { $0.name == name }) { snapshot?.mcpServers[index].enabled = enabled }
        }
    }

    private static func message(for error: Error) -> String {
        let text = String(describing: error)
        if text.localizedCaseInsensitiveContains("method not found") {
            return "This engine is too old to manage skills and MCP servers. Update RustCode or choose a newer engine."
        }
        return text
    }
}

/// A search field for a settings list.
private struct SettingsSearchField: View {
    let prompt: String
    @Binding var text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField(prompt, text: $text).textFieldStyle(.plain)
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                    .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(Color.primary.opacity(0.045), in: .rect(cornerRadius: 8))
        .padding(.bottom, 14)
    }
}

/// Error, loading and empty states shared by both sections.
private struct CapabilityStatus: View {
    @ObservedObject var store: CapabilitiesStore
    let retry: () -> Void

    var body: some View {
        if let error = store.error {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                Text(error).font(.callout).textSelection(.enabled)
                Spacer(minLength: 8)
                Button("Retry", action: retry).buttonStyle(PillButtonStyle())
            }
            .padding(12)
            .background(Color.orange.opacity(0.08), in: .rect(cornerRadius: 10))
            .padding(.bottom, 14)
        } else if store.snapshot == nil {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading…").foregroundStyle(.secondary)
            }
            .padding(.vertical, 20)
        }
    }
}

struct SkillsSettings: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: CapabilitiesStore
    @State private var query = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            CapabilityStatus(store: store) { Task { await store.load(model.engineConnection) } }
            if let snapshot = store.snapshot {
                SettingsSearchField(prompt: "Search skills", text: $query)
                let skills = snapshot.skills(matching: query)
                let enabled = snapshot.skills.filter(\.enabled).count
                SettingsGroup(title: "\(enabled) of \(snapshot.skills.count) on",
                              footer: footer(snapshot)) {
                    if skills.isEmpty {
                        Text(snapshot.skills.isEmpty
                             ? "No skills installed. Put a SKILL.md in ~/.agents/skills/<name>/ or .rustcode/skills/<name>/ in a project."
                             : "No skills match “\(query)”.")
                            .font(.callout).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(14)
                    }
                    ForEach(Array(skills.enumerated()), id: \.element.id) { index, skill in
                        if index > 0 { RowDivider() }
                        SkillRow(skill: skill, pending: store.isPending(.skill, skill.name)) { on in
                            Task { await store.set(.skill, name: skill.name, enabled: on, connection: model.engineConnection) }
                        }
                    }
                }
            }
        }
    }

    private func footer(_ snapshot: CapabilitySnapshot) -> String {
        var text = "A skill that is off stays installed but is never offered to the model. Changes apply from the next message, in open threads too, and to every project."
        if let path = snapshot.configPath { text += " Saved in \(path.abbreviatingHome)." }
        return text
    }
}

private struct SkillRow: View {
    let skill: CapabilitySnapshot.Skill
    let pending: Bool
    let toggle: (Bool) -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(skill.name).foregroundStyle(skill.enabled ? .primary : .secondary)
                if !skill.description.isEmpty {
                    Text(skill.description).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
                Text(skill.sourceLabel).font(.caption2).foregroundStyle(.tertiary)
                    .help(skill.path)
            }
            Spacer(minLength: 12)
            if pending { ProgressView().controlSize(.small) }
            Toggle(skill.name, isOn: Binding(get: { skill.enabled }, set: toggle))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(pending)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }
}

struct MCPSettings: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: CapabilitiesStore
    @State private var query = ""
    @State private var adding = false
    @State private var removing: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            CapabilityStatus(store: store) { Task { await store.load(model.engineConnection) } }
            if let snapshot = store.snapshot {
                HStack(spacing: 8) {
                    SettingsSearchField(prompt: "Search servers and tools", text: $query)
                    Button { Task { await store.load(model.engineConnection) } } label: {
                        if store.loading { ProgressView().controlSize(.small) } else { Text("Refresh") }
                    }
                    .buttonStyle(PillButtonStyle())
                    .padding(.bottom, 14)
                    Button { adding = true } label: { Label("Add Server", systemImage: "plus") }
                        .buttonStyle(PillButtonStyle())
                        .padding(.bottom, 14)
                        .help("Add an MCP server; it starts right away")
                }
                let servers = snapshot.mcpServers(matching: query)
                let connected = snapshot.mcpServers.filter { $0.connection == .connected }.count
                SettingsGroup(title: "\(connected) of \(snapshot.mcpServers.count) connected",
                              footer: "A server that is off stays configured but is not started. Header and environment values are never shown. Changes are saved to the user config and take effect right away in this app; a terminal running RustCode picks them up on its next message. Right-click a server to remove it.") {
                    if servers.isEmpty {
                        Text(snapshot.mcpServers.isEmpty ? "No MCP servers configured." : "No servers match “\(query)”.")
                            .font(.callout).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(14)
                    }
                    ForEach(Array(servers.enumerated()), id: \.element.id) { index, server in
                        if index > 0 { RowDivider() }
                        MCPServerRow(server: server, pending: store.isPending(.mcp, server.name)) { on in
                            Task { await store.set(.mcp, name: server.name, enabled: on, connection: model.engineConnection) }
                        }
                        .contextMenu {
                            if server.canToggle {
                                Button("Remove “\(server.name)”…", role: .destructive) { removing = server.name }
                            }
                        }
                    }
                }
            }
        }
        .sheet(isPresented: $adding) {
            AddMCPServerSheet(existing: Set(store.snapshot?.mcpServers.map(\.name) ?? [])) { params in
                await store.addServer(params, connection: model.engineConnection)
            }
        }
        .confirmationDialog("Remove “\(removing ?? "")”?", isPresented: Binding(
            get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button("Remove", role: .destructive) {
                if let name = removing { Task { await store.removeServer(name, connection: model.engineConnection) } }
                removing = nil
            }
        } message: {
            Text("The server is stopped and deleted from the user config, with its environment values and headers.")
        }
    }
}

/// Collects a new server; the engine validates it like `rustcode mcp add`
/// and starts it before the sheet closes.
private struct AddMCPServerSheet: View {
    let existing: Set<String>
    let add: (JSONValue) async -> String?
    @Environment(\.dismiss) private var dismiss
    @State private var draft = MCPServerDraft()
    @State private var saving = false
    @State private var error: String?

    private var problem: String? {
        if existing.contains(draft.trimmedName) { return "A server named “\(draft.trimmedName)” already exists." }
        return draft.problem
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add MCP Server").font(.headline)
            Form {
                TextField("Name", text: $draft.name, prompt: Text("files"))
                Picker("Type", selection: $draft.transport) {
                    Text("Local command").tag(MCPServerDraft.Transport.stdio)
                    Text("Remote URL").tag(MCPServerDraft.Transport.http)
                }
                .pickerStyle(.segmented)
                switch draft.transport {
                case .stdio:
                    TextField("Command", text: $draft.command, prompt: Text("npx -y @example/files-mcp"))
                        .font(.body.monospaced())
                    LabeledContent("Environment") {
                        TextEditor(text: $draft.environment)
                            .font(.callout.monospaced())
                            .frame(height: 64)
                            .overlay(alignment: .topLeading) {
                                if draft.environment.isEmpty {
                                    Text("API_KEY=… (one per line)").foregroundStyle(.tertiary)
                                        .font(.callout.monospaced()).padding(.leading, 5).allowsHitTesting(false)
                                }
                            }
                    }
                case .http:
                    TextField("URL", text: $draft.url, prompt: Text("https://mcp.example.com/mcp"))
                        .font(.body.monospaced())
                    LabeledContent("Headers") {
                        TextEditor(text: $draft.headers)
                            .font(.callout.monospaced())
                            .frame(height: 64)
                            .overlay(alignment: .topLeading) {
                                if draft.headers.isEmpty {
                                    Text("Authorization: Bearer … (one per line)").foregroundStyle(.tertiary)
                                        .font(.callout.monospaced()).padding(.leading, 5).allowsHitTesting(false)
                                }
                            }
                    }
                }
            }
            Text("Saved to the user config. Environment and header values are stored there and never shown again.")
                .font(.caption).foregroundStyle(.secondary)
            if let error {
                Text(error).font(.callout).foregroundStyle(.red).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button {
                    Task { await save() }
                } label: {
                    if saving { ProgressView().controlSize(.small) } else { Text("Add") }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(saving || problem != nil)
                .help(problem ?? "")
            }
        }
        .padding(20)
        .frame(width: 460)
    }

    private func save() async {
        guard problem == nil, case .success(let params) = draft.request() else { return }
        saving = true
        defer { saving = false }
        error = await add(params)
        if error == nil { dismiss() }
    }
}

private struct MCPServerRow: View {
    let server: CapabilitySnapshot.MCPServer
    let pending: Bool
    let toggle: (Bool) -> Void
    @State private var showsTools = false

    private var dot: Color {
        switch server.connection {
        case .connected: .green
        case .failed: .red
        case .notConnected: .orange
        case .disabled: .secondary.opacity(0.5)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(server.name)
                        Text(server.transport).font(.caption2.weight(.medium)).foregroundStyle(.secondary)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(.fill.tertiary, in: .capsule)
                        if server.scope == "project" {
                            Text("project").font(.caption2.weight(.medium)).foregroundStyle(.secondary)
                                .padding(.horizontal, 5).padding(.vertical, 1)
                                .background(.fill.tertiary, in: .capsule)
                        }
                    }
                    if !server.target.isEmpty {
                        Text(server.target).font(.caption.monospaced()).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                    }
                    HStack(spacing: 5) {
                        Circle().fill(dot).frame(width: 7, height: 7)
                        Text(server.statusLabel).font(.caption).foregroundStyle(.secondary)
                        if !server.tools.isEmpty {
                            Button(showsTools ? "Hide tools" : "Show tools") { showsTools.toggle() }
                                .buttonStyle(.link).font(.caption)
                        }
                    }
                    if let error = server.error {
                        Text(error).font(.caption).foregroundStyle(.red).lineLimit(3).textSelection(.enabled)
                    }
                }
                Spacer(minLength: 12)
                if pending { ProgressView().controlSize(.small) }
                Toggle(server.name, isOn: Binding(get: { server.enabled }, set: toggle))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(pending || !server.canToggle)
                    .help(server.canToggle ? "" : "Declared in this project's config file; change it there.")
            }
            if showsTools {
                Text(server.tools.joined(separator: ", "))
                    .font(.caption.monospaced()).foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }
}
