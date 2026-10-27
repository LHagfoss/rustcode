import AppKit
import RustCodeKit
import SwiftUI

/// GitHub and model accounts for Settings, read from and signed in and out
/// through the app's engine (`_rustcode/connections`).
@MainActor
final class ConnectionsStore: ObservableObject {
    @Published private(set) var snapshot: ConnectionsSnapshot?
    @Published private(set) var loading = false
    @Published var error: String?
    /// The code to enter at GitHub while a sign-in waits for the browser.
    @Published private(set) var signIn: ConnectionsSnapshot.DeviceCode?
    @Published private(set) var startingSignIn = false
    /// Sign-outs waiting for the engine, by account id.
    @Published private(set) var pending: Set<String> = []

    func load(_ connection: ACPConnection?) async {
        guard let connection else {
            error = "The RustCode engine is not running."
            return
        }
        loading = true
        defer { loading = false }
        do {
            let result = try await connection.request(ACPMethod.connections, params: .object([:]), timeout: .seconds(15))
            snapshot = try ConnectionsSnapshot(json: result)
            signIn = snapshot?.github.pending
            error = nil
        } catch {
            self.error = Self.message(for: error)
        }
    }

    /// Start `gh auth login --web`, show its code, and wait for the browser step.
    func signInToGitHub(_ connection: ACPConnection?) async {
        guard let connection else { return }
        startingSignIn = true
        do {
            let result = try await connection.request(ACPMethod.githubLogin, params: .object([:]), timeout: .seconds(30))
            startingSignIn = false
            let code = try JSONDecoder().decode(ConnectionsSnapshot.DeviceCode.self, from: JSONEncoder().encode(result))
            withAnimation(.snappy) { signIn = code }
            Self.copy(code.code)
            if let url = URL(string: code.url) { NSWorkspace.shared.open(url) }
            await wait(connection)
        } catch {
            startingSignIn = false
            self.error = Self.message(for: error)
        }
    }

    /// Wait for a sign-in already running, as after reopening Settings.
    func wait(_ connection: ACPConnection?) async {
        guard let connection, signIn != nil else { return }
        do {
            // GitHub device codes expire after 15 minutes; gh exits then.
            let result = try await connection.request(ACPMethod.githubWait, params: .object([:]), timeout: .seconds(16 * 60))
            snapshot = try ConnectionsSnapshot(json: result)
            error = nil
        } catch {
            if !String(describing: error).contains("cancelled") { self.error = Self.message(for: error) }
            await load(connection)
        }
        withAnimation(.snappy) { signIn = nil }
    }

    func cancelSignIn(_ connection: ACPConnection?) async {
        withAnimation(.snappy) { signIn = nil }
        _ = try? await connection?.request(ACPMethod.githubCancel, params: .object([:]), timeout: .seconds(10))
    }

    func signOut(_ account: ConnectionsSnapshot.GitHubAccount, _ connection: ACPConnection?) async {
        await run(account.id, connection) {
            try await $0.request(ACPMethod.githubLogout, params: ACPRequests.githubLogout(host: account.host, login: account.login),
                                 timeout: .seconds(30))
        }
    }

    func signOut(_ account: ConnectionsSnapshot.ModelAccount, _ connection: ACPConnection?) async {
        await run(account.id, connection) {
            try await $0.request(ACPMethod.accountLogout,
                                 params: ACPRequests.accountLogout(provider: account.provider, account: account.account),
                                 timeout: .seconds(30))
        }
    }

    func isPending(_ id: String) -> Bool { pending.contains(id) }

    private func run(_ id: String, _ connection: ACPConnection?, _ call: (ACPConnection) async throws -> JSONValue) async {
        guard let connection else { return }
        pending.insert(id)
        defer { pending.remove(id) }
        do {
            snapshot = try ConnectionsSnapshot(json: try await call(connection))
            error = nil
        } catch {
            self.error = Self.message(for: error)
        }
    }

    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    private static func message(for error: Error) -> String {
        let text = String(describing: error)
        if text.localizedCaseInsensitiveContains("method not found") {
            return "This engine is too old to manage connections. Update RustCode or choose a newer engine."
        }
        return text
    }
}

struct ConnectionsSettings: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: ConnectionsStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let error = store.error {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    Text(error).font(.callout).textSelection(.enabled)
                    Spacer(minLength: 8)
                    Button("Retry") { Task { await store.load(model.engineConnection) } }.buttonStyle(PillButtonStyle())
                }
                .padding(12)
                .background(Color.orange.opacity(0.08), in: .rect(cornerRadius: 10))
                .padding(.bottom, 14)
            }
            if let snapshot = store.snapshot {
                SettingsGroup(title: "GitHub", footer: "RustCode uses the GitHub CLI's sign-in, so the agent's gh and git commands and RustCode share one account. The token stays in the system keychain, where gh keeps it.") {
                    GitHubRows(model: model, store: store, github: snapshot.github)
                }
                SettingsGroup(title: "Model accounts",
                              footer: "Accounts you signed in to with /login. Signing out removes the saved credential from the keychain; sign in again with /login <provider> in a chat.") {
                    if snapshot.accounts.isEmpty {
                        Text("No saved accounts. API keys from environment variables are not listed.")
                            .font(.callout).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(14)
                    }
                    ForEach(Array(snapshot.accounts.enumerated()), id: \.element.id) { index, account in
                        if index > 0 { RowDivider() }
                        ModelAccountRow(account: account, pending: store.isPending(account.id)) {
                            Task { await store.signOut(account, model.engineConnection) }
                        }
                    }
                }
            } else if store.error == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading…").foregroundStyle(.secondary)
                }
                .padding(.vertical, 20)
            }
        }
        .animation(.snappy, value: store.snapshot)
    }
}

private struct GitHubRows: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: ConnectionsStore
    let github: ConnectionsSnapshot.GitHub

    var body: some View {
        if let code = store.signIn {
            SignInCodeRow(code: code) { Task { await store.cancelSignIn(model.engineConnection) } }
        } else if !github.cliInstalled {
            SettingsRow(title: "GitHub CLI not installed", subtitle: "Install it with: brew install gh") {
                Button("Copy command") { ConnectionsStore.copy("brew install gh") }.buttonStyle(PillButtonStyle())
            }
        } else if github.accounts.isEmpty {
            SettingsRow(title: "Not signed in", subtitle: "Sign in to let the agent read issues, pull requests and CI") {
                connectButton
            }
        } else {
            ForEach(Array(github.accounts.enumerated()), id: \.element.id) { index, account in
                if index > 0 { RowDivider() }
                GitHubAccountRow(account: account, pending: store.isPending(account.id),
                                 reconnect: { Task { await store.signInToGitHub(model.engineConnection) } },
                                 signOut: { Task { await store.signOut(account, model.engineConnection) } })
            }
            if github.active == nil {
                RowDivider()
                SettingsRow(title: "github.com", subtitle: "No active account") { connectButton }
            }
        }
    }

    private var connectButton: some View {
        Button {
            Task { await store.signInToGitHub(model.engineConnection) }
        } label: {
            if store.startingSignIn { ProgressView().controlSize(.small) } else { Text("Connect GitHub") }
        }
        .buttonStyle(PillButtonStyle(prominent: true))
        .disabled(store.startingSignIn)
    }
}

/// The one-time code while GitHub waits for the browser step.
private struct SignInCodeRow: View {
    let code: ConnectionsSnapshot.DeviceCode
    let cancel: () -> Void
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Enter this code at GitHub").font(.callout.weight(.medium))
            HStack(spacing: 12) {
                Text(code.code)
                    .font(.system(size: 26, weight: .semibold, design: .monospaced))
                    .textSelection(.enabled)
                Button(copied ? "Copied" : "Copy") {
                    ConnectionsStore.copy(code.code)
                    withAnimation(.snappy) { copied = true }
                }
                .buttonStyle(PillButtonStyle())
                Spacer()
            }
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Waiting for you to approve in the browser…").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Open GitHub") { if let url = URL(string: code.url) { NSWorkspace.shared.open(url) } }
                    .buttonStyle(PillButtonStyle(prominent: true))
                Button("Cancel", action: cancel).buttonStyle(PillButtonStyle())
            }
        }
        .padding(14)
        .transition(.opacity.combined(with: .scale(scale: 0.97)))
    }
}

private struct GitHubAccountRow: View {
    let account: ConnectionsSnapshot.GitHubAccount
    let pending: Bool
    let reconnect: () -> Void
    let signOut: () -> Void

    private var subtitle: String {
        if !account.valid { return "The saved token no longer works. Reconnect to sign in again." }
        var parts = [account.host]
        if !account.active { parts.append("not active") }
        if !account.missingScopes.isEmpty { parts.append("missing \(account.missingScopes.joined(separator: ", "))") }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(spacing: 12) {
            AsyncImage(url: URL(string: "https://github.com/\(account.login).png?size=64")) { image in
                image.resizable()
            } placeholder: {
                Image(systemName: "person.crop.circle.fill").resizable().foregroundStyle(.tertiary)
            }
            .frame(width: 28, height: 28)
            .clipShape(.circle)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(account.login)
                    Circle().fill(account.valid ? Color.green : .orange).frame(width: 7, height: 7)
                }
                Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    .help(account.scopes.isEmpty ? "" : "Scopes: \(account.scopes.joined(separator: ", "))")
            }
            Spacer(minLength: 12)
            if pending { ProgressView().controlSize(.small) }
            if !account.valid {
                Button("Reconnect", action: reconnect).buttonStyle(PillButtonStyle(prominent: true))
            }
            Button("Sign out", action: signOut).buttonStyle(PillButtonStyle()).disabled(pending)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }
}

private struct ModelAccountRow: View {
    let account: ConnectionsSnapshot.ModelAccount
    let pending: Bool
    let signOut: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(account.display.isEmpty ? account.account : account.display)
                        .foregroundStyle(account.active ? .primary : .secondary)
                    Text(account.provider).font(.caption2.weight(.medium)).foregroundStyle(.secondary)
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(.fill.tertiary, in: .capsule)
                }
                Text(account.active ? account.methodLabel : "\(account.methodLabel) · signed out")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            if pending { ProgressView().controlSize(.small) }
            if account.active {
                Button("Sign out", action: signOut).buttonStyle(PillButtonStyle()).disabled(pending)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }
}
