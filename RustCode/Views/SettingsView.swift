import RustCodeRemote
import SwiftUI

struct SettingsView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var confirmsUnpair = false
    var onUnpair: () -> Void

    private var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "–"
        let build = info?["CFBundleVersion"] as? String ?? "–"
        return "\(short) (\(build))"
    }

    var body: some View {
        NavigationStack {
            Form {
                if let host = store.host {
                    Section {
                        LabeledContent("Name", value: host.name)
                        if !store.isDemo {
                            LabeledContent("Address", value: host.address.description)
                            LabeledContent("Paired", value: host.pairedAt.formatted(date: .abbreviated, time: .shortened))
                            if let expiresAt = host.expiresAt {
                                LabeledContent("Linked Until", value: expiresAt.formatted(date: .abbreviated, time: .shortened))
                            }
                        }
                        LabeledContent("This Device", value: host.device.name)
                        LabeledContent("Status", value: store.connection.summary)
                        if store.canRetry {
                            Button("Retry Connection") { store.retryNow() }
                        }
                    } header: {
                        Text(store.isDemo ? "Demo Host" : "Paired Host")
                    } footer: {
                        if store.isDemo {
                            Text("A simulated host that runs inside the app. Nothing leaves this device.")
                        } else {
                            Label(host.address.protection.explanation, systemImage: host.address.protection.symbolName)
                        }
                    }

                    Section {
                        Button(store.isDemo ? "Exit Demo" : "Unpair This Device", role: .destructive) {
                            if store.isDemo {
                                unpair()
                            } else {
                                confirmsUnpair = true
                            }
                        }
                    } footer: {
                        if !store.isDemo {
                            Text("Disconnects this iPhone. Sessions keep running on your Mac.")
                        }
                    }
                }

                Section {
                    LabeledContent("Version", value: version)
                } header: {
                    Text("About")
                } footer: {
                    Text("Every RustCode session open in a terminal on your Mac appears here while /remote is on, and you can start new ones from this app. Sessions keep running on your Mac when you close the app. Use /remote off on the Mac to turn remote access off.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .softNavigationChrome()
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .alert(
                "Unpair from \(store.host?.name ?? "this host")?",
                isPresented: $confirmsUnpair
            ) {
                Button("Cancel", role: .cancel) {}
                Button("Unpair", role: .destructive, action: unpair)
            } message: {
                Text("You will need a new pairing code from the terminal to connect again.")
            }
        }
    }

    private func unpair() {
        // Let the Settings presentation finish closing before replacing the root with pairing.
        onUnpair()
        dismiss()
    }
}
