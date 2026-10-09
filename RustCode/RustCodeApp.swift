import RustCodeRemote
import SwiftUI
import UIKit

@main
struct RustCodeApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @State private var store = RemoteStore(
        // The schema-backed codec replaces this placeholder once the wire contract is final.
        client: LiveRemoteClient(codec: UnavailableRemoteCodec()),
        credentials: KeychainCredentialStore()
    )

    private let options = LaunchOptions(arguments: ProcessInfo.processInfo.arguments)

    var body: some Scene {
        WindowGroup {
            RootView(presentsAttentionOnLaunch: options.presentsAttention)
                .environment(store)
                .task { await start() }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                Task { await store.suspend() }
            case .active:
                store.resume()
            default:
                break
            }
        }
    }

    private func start() async {
        guard options.startsDemo else {
            store.bootstrap()
            return
        }
        await store.startDemo(deviceName: UIDevice.current.name)
        guard let sessionID = options.demoSessionID else { return }
        for _ in 0..<200 where store.sessions.isEmpty {
            try? await Task.sleep(for: .milliseconds(10))
        }
        store.select(sessionID)
    }
}
