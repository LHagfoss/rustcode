import RustCodeRemote
import SwiftUI
import UIKit

struct PairingView: View {
    @Environment(RemoteStore.self) private var store
    @State private var address = ""
    @AppStorage("recentPairingAddress") private var recentAddress = ""
    @State private var code = ""
    @State private var addressError: String?
    @State private var showsScanner = false
    @State private var scannedPayload: String?
    @State private var discovery = NearbyHostDiscovery()
    @FocusState private var focus: Field?

    private enum Field {
        case address
        case code
    }

    private var isPairing: Bool { store.pairing == .pairing }

    private var parsedAddress: HostAddress? {
        try? HostAddress.parse(address)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    header
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                }
                Section {
                    scanSection
                } footer: {
                    Text("On your Mac, run /remote in RustCode. Use the same Wi-Fi, then scan the QR code or choose your Mac below.")
                }
                nearbySection
                manualSection
                if case .failed(let message) = store.pairing {
                    Section {
                        Label(message, systemImage: "exclamationmark.circle")
                            .font(.subheadline)
                            .foregroundStyle(.red)
                            .accessibilityLabel("Pairing failed. \(message)")
                    }
                }
                Section {
                    demoSection
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle("Connect to Mac")
            .navigationBarTitleDisplayMode(.inline)
            .onAppear {
                if address.isEmpty { address = recentAddress }
                discovery.start()
            }
            .onDisappear { discovery.stop() }
            .sheet(isPresented: $showsScanner, onDismiss: pairScannedPayload) {
                QRScannerSheet { payload in
                    scannedPayload = payload
                    showsScanner = false
                }
            }
            .sensoryFeedback(.selection, trigger: showsScanner) { _, presented in presented }
            .sensoryFeedback(trigger: store.pairing) { old, new in
                guard old == .pairing else { return nil }
                if case .failed = new { return .error }
                return nil
            }
        }
    }

    private func pairScannedPayload() {
        guard let payload = scannedPayload else { return }
        scannedPayload = nil
        Task {
            await store.pair(.scanned(payload), deviceName: UIDevice.current.name)
            if let host = store.host { recentAddress = host.address.description }
        }
    }

    private var header: some View {
        VStack(spacing: 12) {
            Image(.logo)
                .resizable()
                .scaledToFit()
                .frame(width: 56, height: 56)
                .clipShape(.rect(cornerRadius: 14))
                .accessibilityHidden(true)
            Text("RustCode Remote")
                .font(.title2.bold())
            Text("Watch and continue your terminal sessions from your iPhone.")
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
    }

    private var scanSection: some View {
        Button {
            focus = nil
            store.clearPairingError()
            showsScanner = true
        } label: {
            Label("Scan Pairing Code", systemImage: "qrcode.viewfinder")
                .frame(maxWidth: .infinity)
        }
        .brandProminentButtonStyle()
        .controlSize(.large)
        .disabled(isPairing)
    }

    private var nearbySection: some View {
        Section {
            if let error = discovery.error {
                Text(error).font(.subheadline).foregroundStyle(.secondary)
            } else if discovery.hosts.isEmpty {
                HStack(spacing: 12) {
                    if discovery.isSearching { ProgressView() }
                    Text("Looking for Macs on your Wi-Fi…")
                        .foregroundStyle(.secondary)
                }
            }
            ForEach(discovery.hosts) { host in
                Button {
                    if address != host.address { code = "" }
                    address = host.address
                    addressError = nil
                    store.clearPairingError()
                    focus = .code
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer").foregroundStyle(.tint)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(host.name).foregroundStyle(.primary)
                            Text(host.address).font(.caption).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                        }
                        Spacer(minLength: 4)
                        Image(systemName: address == host.address ? "checkmark" : "chevron.right")
                            .foregroundStyle(.tint)
                    }
                    .frame(minHeight: 44)
                }
                .buttonStyle(.plain)
                .disabled(isPairing)
                .accessibilityHint("Enter the code shown by /remote on this Mac to connect.")
            }
            if discovery.error != nil {
                Button("Search Again") { discovery.stop(); discovery.start() }
            }
        } header: {
            Text("Nearby Macs")
        } footer: {
            Text("Choosing a Mac fills in its address. Enter its /remote pairing code below. You only pair once.")
        }
        .sensoryFeedback(.selection, trigger: address) { _, _ in focus == .code }
    }

    private var manualSection: some View {
        Section {
            HStack(spacing: 12) {
                Text("Address")
                TextField("Address and port", text: $address, prompt: Text("192.168.1.20:17879"))
                    .textContentType(.URL)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .focused($focus, equals: .address)
                    .onSubmit { focus = .code }
                    .frame(minHeight: 44)
                    .accessibilityLabel("Host address and port")
                PasteButton(payloadType: String.self) { values in
                    if let value = values.first { address = value.trimmingCharacters(in: .whitespacesAndNewlines) }
                }
                .labelStyle(.iconOnly).controlSize(.small)
                .accessibilityLabel("Paste address or QR payload")
            }
            HStack(spacing: 12) {
                Text("Code")
                TextField("Pairing code", text: $code, prompt: Text("1234-5678"))
                    .textContentType(.oneTimeCode)
                    .keyboardType(.asciiCapable)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.go)
                    .focused($focus, equals: .code)
                    .onSubmit(pairManually)
                    .frame(minHeight: 44)
                    .accessibilityLabel("Pairing code")
                PasteButton(payloadType: String.self) { values in
                    if let value = values.first {
                        code = value.trimmingCharacters(in: .whitespacesAndNewlines)
                        focus = .code
                    }
                }
                .labelStyle(.iconOnly).controlSize(.small)
                .accessibilityLabel("Paste pairing code")
            }
            if focus != nil {
                Button { focus = nil } label: {
                    Label("Dismiss Keyboard", systemImage: "keyboard.chevron.compact.down")
                        .frame(minHeight: 44)
                }
                .accessibilityLabel("Dismiss pairing keyboard")
            }
            Button(action: pairManually) {
                HStack(spacing: 8) {
                    if isPairing { ProgressView() }
                    Text(isPairing ? "Connecting…" : "Connect")
                }
                .frame(maxWidth: .infinity)
            }
            .brandProminentButtonStyle()
            .controlSize(.large)
            .disabled(isPairing || address.isEmpty || (code.isEmpty && !address.hasPrefix("{")))
        } header: {
            Text(discovery.hosts.first { $0.address == address }.map { "Connect to \($0.name)" } ?? "Address and pairing code")
        } footer: {
            VStack(alignment: .leading, spacing: 8) {
                Text("Use the eight-digit code shown by /remote, not your Mac password. The dash is optional. You can also paste the QR payload into the address field.")
                if let addressError {
                    Text(addressError).foregroundStyle(.red)
                } else if let parsedAddress {
                    Label(parsedAddress.protection.explanation, systemImage: parsedAddress.protection.symbolName)
                        .foregroundStyle(parsedAddress.protection.isEncrypted ? Color.secondary : Color.attention)
                }
            }
        }
        .onChange(of: address) { addressError = nil }
    }

    private var demoSection: some View {
        VStack(spacing: 6) {
            Button("Try the Demo") {
                focus = nil
                Task { await store.startDemo(deviceName: UIDevice.current.name) }
            }
            .font(.body.weight(.medium))
            .accessibilityHint("Opens a simulated host with sample sessions")
            Text("No Mac needed. Shows sample sessions from a simulated host.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .padding(.top, 4)
    }

    private func pairManually() {
        store.clearPairingError()
        // The terminal prints the QR payload as text; pasting it works like scanning it.
        let pasted = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if pasted.hasPrefix("{") {
            focus = nil
            Task {
                await store.pair(.scanned(pasted), deviceName: UIDevice.current.name)
                if let host = store.host { recentAddress = host.address.description }
            }
            return
        }
        do {
            let parsed = try HostAddress.parse(address)
            let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !code.isEmpty else { return }
            focus = nil
            Task {
                await store.pair(.manual(address: parsed, code: code), deviceName: UIDevice.current.name)
                if store.host != nil { recentAddress = parsed.description }
            }
        } catch {
            addressError = switch error {
            case .empty: "Enter the address shown under the QR code."
            case .missingPort: "Include the port, for example 192.168.1.20:17879."
            case .invalidPort: "The port has to be a number between 1 and 65535."
            case .invalidHost: "That does not look like a host address."
            }
        }
    }
}
