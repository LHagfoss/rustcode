import RustCodeRemote
import SwiftUI

struct SettingsView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var confirmsUnpair = false

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
                            Text("Removes the device token from the keychain. To also revoke it on the host, run “rustcode remote revoke \(store.host?.device.id.prefix(8) ?? "<device>")” there.")
                        }
                    }
                }

                Section {
                    LabeledContent("Version", value: version)
                } header: {
                    Text("About")
                } footer: {
                    Text("RustCode Remote controls sessions you share with /remote from a RustCode terminal. Sessions keep running on your Mac; closing this app never cancels work. The gateway currently pairs and authenticates devices; sharing sessions through it is still in development on the RustCode side.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog(
                "Unpair from \(store.host?.name ?? "this host")?",
                isPresented: $confirmsUnpair,
                titleVisibility: .visible
            ) {
                Button("Unpair", role: .destructive, action: unpair)
            } message: {
                Text("You will need a new pairing code from the terminal to connect again.")
            }
        }
    }

    private func unpair() {
        Task {
            await store.unpair()
            dismiss()
        }
    }
}
