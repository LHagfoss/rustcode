import RustCodeRemote
import SwiftUI

struct RootView: View {
    @Environment(RemoteStore.self) private var store
    var presentsAttentionOnLaunch = false

    var body: some View {
        @Bindable var store = store
        Group {
            if store.host == nil {
                PairingView()
            } else {
                SessionsView(presentsAttentionOnLaunch: presentsAttentionOnLaunch)
            }
        }
        .animation(.default, value: store.host == nil)
        .keepScreenAwake(whileRunning: store.host != nil && store.isWorking)
        .alert(
            "RustCode",
            isPresented: Binding(get: { store.notice != nil }, set: { if !$0 { store.notice = nil } }),
            presenting: store.notice
        ) { _ in
            Button("OK", role: .cancel) {}
        } message: { notice in
            Text(notice)
        }
    }
}

extension TransportProtection {
    var symbolName: String {
        switch self {
        case .loopback: "desktopcomputer"
        case .tunnelEncrypted: "lock.fill"
        case .unencryptedLAN: "lock.open"
        case .unencryptedOther: "exclamationmark.triangle"
        }
    }

    /// Says plainly whether the connection is encrypted. Pairing authenticates the device;
    /// it does not protect the traffic.
    var explanation: String {
        switch self {
        case .loopback:
            "This address only reaches the device itself."
        case .tunnelEncrypted:
            "NetBird address. The NetBird tunnel encrypts this connection."
        case .unencryptedLAN:
            "Local network address. This connection is not encrypted. Use it only on a network you trust."
        case .unencryptedOther:
            "This address is neither on your local network nor on NetBird. The connection is not encrypted."
        }
    }
}

/// A quiet tag for an address on the NetBird mesh.
struct NetBirdBadge: View {
    var body: some View {
        Label("NetBird", systemImage: "point.3.connected.trianglepath.dotted")
            .labelStyle(.titleAndIcon)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(.fill.tertiary, in: .capsule)
            .fixedSize()
            .accessibilityLabel("On NetBird")
    }
}

extension HostAddress {
    /// Whether a typed or discovered `host:port` string reaches a NetBird peer.
    static func isNetBird(_ address: String) -> Bool {
        (try? parse(address))?.isNetBird ?? false
    }
}
