import RustCodeRemote
import SwiftUI
import UIKit

struct PairingView: View {
    @Environment(RemoteStore.self) private var store
    @State private var address = ""
    @State private var code = ""
    @State private var addressError: String?
    @State private var showsScanner = false
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
            ScrollView {
                VStack(spacing: 28) {
                    header
                    scanSection
                    manualSection
                    if case .failed(let message) = store.pairing {
                        Label(message, systemImage: "exclamationmark.circle")
                            .font(.subheadline)
                            .foregroundStyle(.red)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .accessibilityLabel("Pairing failed. \(message)")
                    }
                    demoSection
                }
                .padding(.horizontal, 24)
                .padding(.vertical, 32)
                .frame(maxWidth: 520)
                .frame(maxWidth: .infinity)
            }
            .scrollDismissesKeyboard(.interactively)
            .sheet(isPresented: $showsScanner) {
                QRScannerSheet { payload in
                    showsScanner = false
                    Task { await store.pair(.scanned(payload), deviceName: UIDevice.current.name) }
                }
            }
        }
    }

    private var header: some View {
        VStack(spacing: 12) {
            Image(.logo)
                .resizable()
                .scaledToFit()
                .frame(width: 92, height: 92)
                .clipShape(.rect(cornerRadius: 22))
                .accessibilityHidden(true)
            Text("RustCode Remote")
                .font(.largeTitle.bold())
            Text("Watch and continue your terminal sessions from your iPhone.")
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
    }

    private var scanSection: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label {
                Text("Run **rustcode remote pair** on your Mac, then scan the code it shows or enter the address and code.")
            } icon: {
                Image(systemName: "1.circle.fill").foregroundStyle(.tint)
            }
            .font(.subheadline)

            Button {
                store.clearPairingError()
                showsScanner = true
            } label: {
                Label("Scan Pairing Code", systemImage: "qrcode.viewfinder")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(isPairing)
        }
    }

    private var manualSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Or enter it manually")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            VStack(spacing: 0) {
                TextField("Address, for example 192.168.1.20:17879", text: $address)
                    .textContentType(.URL)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .focused($focus, equals: .address)
                    .onSubmit { focus = .code }
                    .padding(14)
                    .accessibilityLabel("Host address and port")
                Divider().padding(.leading, 14)
                TextField("Pairing code, for example 1234-5678", text: $code)
                    .keyboardType(.numbersAndPunctuation)
                    .autocorrectionDisabled()
                    .font(code.isEmpty ? .body : .body.monospaced())
                    .submitLabel(.go)
                    .focused($focus, equals: .code)
                    .onSubmit(pairManually)
                    .padding(14)
                    .accessibilityLabel("Pairing code")
            }
            .background(.fill.quaternary, in: .rect(cornerRadius: 14))

            if let addressError {
                Text(addressError)
                    .font(.footnote)
                    .foregroundStyle(.red)
            } else if let parsedAddress {
                Label(parsedAddress.protection.explanation, systemImage: parsedAddress.protection.symbolName)
                    .font(.footnote)
                    .foregroundStyle(parsedAddress.protection.isEncrypted ? Color.secondary : Color.orange)
            }

            Button(action: pairManually) {
                HStack(spacing: 8) {
                    if isPairing { ProgressView() }
                    Text(isPairing ? "Pairing…" : "Pair")
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .controlSize(.large)
            .disabled(isPairing || address.isEmpty || (code.isEmpty && !address.hasPrefix("{")))

            Text("You can also paste the QR payload that the terminal prints into the address field.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .onChange(of: address) { addressError = nil }
    }

    private var demoSection: some View {
        VStack(spacing: 6) {
            Button("Try the Demo") {
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
            Task { await store.pair(.scanned(pasted), deviceName: UIDevice.current.name) }
            return
        }
        do {
            let parsed = try HostAddress.parse(address)
            let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !code.isEmpty else { return }
            focus = nil
            Task { await store.pair(.manual(address: parsed, code: code), deviceName: UIDevice.current.name) }
        } catch {
            addressError = switch error {
            case .empty: "Enter the address shown by rustcode remote pair."
            case .missingPort: "Include the port, for example 192.168.1.20:17879."
            case .invalidPort: "The port has to be a number between 1 and 65535."
            case .invalidHost: "That does not look like a host address."
            }
        }
    }
}
