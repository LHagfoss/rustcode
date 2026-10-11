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
    /// One value for both fields, so they can never both claim the keyboard.
    @State private var focus: Field?

    private enum Field { case address, code }

    private static let errorID = "pairingError"
    private static let connectID = "pairingConnect"

    private var isPairing: Bool { store.pairing == .pairing }

    private var parsedAddress: HostAddress? {
        try? HostAddress.parse(address)
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                Form {
                    Section {
                        scanSection
                    } footer: {
                        Text("Run /remote on your Mac, then scan its QR code. Use the same Wi-Fi.")
                    }
                    nearbySection
                    manualSection
                    if case .failed(let message) = store.pairing {
                        Section {
                            Label(message, systemImage: "exclamationmark.circle")
                                .font(.subheadline)
                                .foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                                .accessibilityLabel("Pairing failed. \(message)")
                        }
                        .id(Self.errorID)
                    }
                }
                .onChange(of: store.pairing) { _, pairing in
                    // The reason sits below Connect; bring it on screen when it appears.
                    guard case .failed = pairing else { return }
                    withAnimation { proxy.scrollTo(Self.errorID, anchor: .bottom) }
                }
                .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidShowNotification)) { _ in
                    // The Form only reveals the field being edited. Bring Connect
                    // above the keyboard bar too, so the bar never covers it.
                    guard focus != nil else { return }
                    withAnimation { proxy.scrollTo(Self.connectID, anchor: .bottom) }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .background { PairingKeyboardDismissal(dismiss: dismissKeyboard) }
            .navigationTitle("Connect to Mac")
            .navigationBarTitleDisplayMode(.inline)
            .softNavigationChrome()
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Demo", systemImage: "play.circle") {
                        dismissKeyboard()
                        Task { await store.startDemo(deviceName: UIDevice.current.name) }
                    }
                    .accessibilityLabel("Try the Demo")
                    .accessibilityHint("Opens sample sessions without connecting to a Mac")
                    .disabled(isPairing)
                }
            }
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

    private func dismissKeyboard() { focus = nil }

    private func focusCode() { focus = .code }

    /// A field reporting that it lost focus must not clear the other field's claim.
    private func isFocused(_ field: Field) -> Binding<Bool> {
        Binding {
            focus == field
        } set: { focused in
            if focused { focus = field } else if focus == field { focus = nil }
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

    private var scanSection: some View {
        Button {
            dismissKeyboard()
            store.clearPairingError()
            showsScanner = true
        } label: {
            Label("Scan Pairing Code", systemImage: "qrcode.viewfinder")
                .frame(maxWidth: .infinity)
        }
        .brandProminentButtonStyle()
        .controlSize(.large)
        .disabled(isPairing)
        .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        .listRowBackground(Color.clear)
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
                    focusCode()
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
                    .contentShape(.rect)
                    .background { PairingKeyboardDismissal.PreservesFocus() }
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
            Text("Choose a Mac to fill its address and enter its /remote code.")
        }
        .sensoryFeedback(.selection, trigger: address) { _, _ in focus == .code }
    }

    private var manualSection: some View {
        Section {
            HStack(spacing: 12) {
                Text("Address").onTapGesture(perform: dismissKeyboard)
                PairingAddressField(text: $address, isFocused: isFocused(.address), onNext: focusCode, onPaste: pasteAddress)
                PasteButton(payloadType: String.self) { values in
                    if let value = values.first { pasteAddress(value) }
                }
                .labelStyle(.iconOnly).controlSize(.small)
                .accessibilityLabel("Paste address or QR payload")
            }
            HStack(spacing: 12) {
                Text("Code").onTapGesture(perform: dismissKeyboard)
                PairingCodeField(text: $code, isFocused: isFocused(.code), onPaste: pasteCode)
                PasteButton(payloadType: String.self) { values in
                    if let value = values.first { pasteCode(value) }
                }
                .labelStyle(.iconOnly).controlSize(.small)
                .accessibilityLabel("Paste pairing code")
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
            .listRowInsets(EdgeInsets(top: 8, leading: 16, bottom: 8, trailing: 16))
            .id(Self.connectID)
            .disabled(isPairing || address.isEmpty || (!PairingCodeInput.isComplete(code) && !address.hasPrefix("{")))
        } header: {
            Text(discovery.hosts.first { $0.address == address }.map { "Connect to \($0.name)" } ?? "Address and pairing code")
        } footer: {
            VStack(alignment: .leading, spacing: 8) {
                Text("Enter the eight-digit /remote code, not your Mac password. Codes expire after 2 minutes; run /remote again for a new one.")
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

    /// A paste that completes the form connects straight away; otherwise it
    /// moves on to the code, which is the only thing left to enter.
    private func pasteAddress(_ value: String) {
        address = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !isPairing else { return }
        if address.hasPrefix("{") || PairingCodeInput.isComplete(code) {
            pairManually()
        } else if parsedAddress != nil {
            focusCode()
        }
    }

    private func pasteCode(_ value: String) {
        code = PairingCodeInput.formatted(value)
        guard !isPairing else { return }
        if PairingCodeInput.isComplete(code), parsedAddress != nil {
            pairManually()
        } else {
            focusCode()
        }
    }

    private func pairManually() {
        store.clearPairingError()
        // The terminal prints the QR payload as text; pasting it works like scanning it.
        let pasted = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if pasted.hasPrefix("{") {
            dismissKeyboard()
            Task {
                await store.pair(.scanned(pasted), deviceName: UIDevice.current.name)
                if let host = store.host { recentAddress = host.address.description }
            }
            return
        }
        do {
            let parsed = try HostAddress.parse(address)
            let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
            guard PairingCodeInput.isComplete(code) else { return }
            dismissKeyboard()
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
