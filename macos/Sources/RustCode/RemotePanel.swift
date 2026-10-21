import AppKit
import CoreImage.CIFilterBuiltins
import RustCodeKit
import SwiftUI

/// `/remote`: a pairing card for the shown chat. The engine owns the gateway
/// and the offer; this panel only shows what it reports and copies the code
/// and address a device needs.
struct RemotePanel: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.closePanel) private var dismiss

    private var status: RemoteShareStatus? { model.remoteStatus }
    private var shared: Bool { status?.shared == true }
    private var offer: RemoteShareStatus.Offer? {
        guard let offer = status?.offer, !(status?.pairingExpired ?? false) else { return nil }
        return offer
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header
            if let error = status?.error, !error.isEmpty {
                banner(error, systemImage: "exclamationmark.triangle.fill", tint: .red)
            }
            if shared {
                sharedBody
            } else {
                offBody
            }
            Divider()
            footer
        }
        .padding(22)
        .frame(width: 470)
        .onDisappear { model.remotePanelClosed() }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "qrcode")
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(Color.brand)
                .frame(width: 30, height: 30)
                .background(Color.brand.opacity(0.12), in: .rect(cornerRadius: 9))
            VStack(alignment: .leading, spacing: 3) {
                Text("Remote access").font(.title3.weight(.semibold))
                Text(shared
                     ? "Scan the code with the RustCode app on your phone."
                     : "Share this chat with a phone or another device.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            statusPill
        }
    }

    private var statusPill: some View {
        let (label, color): (String, Color) = if !shared {
            ("Off", .secondary)
        } else if status?.pairingPending == true && offer == nil {
            ("Starting", .orange)
        } else {
            ("Shared", .green)
        }
        return HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(label).font(.caption.weight(.medium)).foregroundStyle(color)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(color.opacity(0.1), in: .capsule)
        .accessibilityLabel("Remote access \(label)")
    }

    @ViewBuilder
    private var sharedBody: some View {
        if let offer {
            pairingCard(offer)
        } else if let error = status?.pairingError, !error.isEmpty {
            banner(error, systemImage: "exclamationmark.triangle.fill", tint: .red)
        } else if status?.pairingPending == true || model.remoteBusy {
            HStack(spacing: 10) {
                ProgressView().controlSize(.small)
                Text("Starting the remote gateway and making a code…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 28)
        } else if status?.pairingExpired == true {
            banner("The pairing code expired. Make a new one to connect a device.",
                   systemImage: "clock.badge.exclamationmark", tint: .orange)
        } else {
            banner("Remote access is on, but there is no pairing code yet.",
                   systemImage: "qrcode", tint: .secondary)
        }
        connectionSummary
    }

    private func pairingCard(_ offer: RemoteShareStatus.Offer) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 18) {
                qrCode(offer)
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("PAIRING CODE")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                        Spacer(minLength: 4)
                        CopyButton(help: "Copy pairing code") { Clipboard.copy(offer.code) }
                    }
                    Text(offer.code)
                        .font(.system(size: 25, weight: .semibold, design: .monospaced))
                        .tracking(1.5)
                        .lineLimit(1)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                    Text("Or type the address below in the app.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    addressRow(offer)
                    expiryRow(offer)
                }
            }
            if offer.loopbackOnly {
                banner("This Mac is only reachable on localhost. Set a LAN address in remote settings so a phone can connect.",
                       systemImage: "wifi.exclamationmark", tint: .orange)
            }
        }
        .padding(14)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
    }

    private func qrCode(_ offer: RemoteShareStatus.Offer) -> some View {
        PairingQRCode(payload: offer.qrPayload)
    }

    private func addressRow(_ offer: RemoteShareStatus.Offer) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "network")
                .foregroundStyle(.secondary)
                .font(.caption)
            Text(offer.address)
                .font(.system(.callout, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
            CopyButton(help: "Copy address") { Clipboard.copy(offer.address) }
        }
    }

    private func expiryRow(_ offer: RemoteShareStatus.Offer) -> some View {
        HStack(spacing: 5) {
            Image(systemName: "clock")
            Text(offer.expiresInSecs > 0
                 ? "Expires in \(Self.countdown(offer.expiresInSecs))"
                 : "Code expired")
        }
        .font(.caption)
        .foregroundStyle(offer.expiresInSecs > 30 ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
    }

    @ViewBuilder
    private var connectionSummary: some View {
        let devices = status?.connectedDevices ?? []
        let attached = status?.attachedDevices ?? []
        let names = devices.isEmpty ? attached : devices
        if !names.isEmpty {
            HStack(spacing: 8) {
                Image(systemName: "iphone.gen3")
                    .foregroundStyle(.green)
                Text(names.joined(separator: ", "))
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer()
                Text("connected").font(.caption).foregroundStyle(.secondary)
            }
        } else if shared {
            HStack(spacing: 8) {
                Image(systemName: "iphone.gen3")
                    .foregroundStyle(.secondary)
                Text("No device connected yet.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
            }
        }
    }

    private var offBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Remote access is off")
                .font(.headline)
            Text("Turn it on to share this chat with a paired device. Remote access stays on for this Mac until you turn it off.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let count = status?.pairedDevices, count > 0 {
                Label("\(count) paired device\(count == 1 ? "" : "s")", systemImage: "checkmark.shield")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if let count = status?.pairedDevices, count > 0, shared {
                Text("\(count) paired device\(count == 1 ? "" : "s")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if shared {
                Button {
                    Task { await model.refreshRemoteCode() }
                } label: {
                    Label("New code", systemImage: "arrow.clockwise")
                }
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(model.remoteBusy)

                Button("Turn off") {
                    Task { await model.turnOffRemote() }
                }
                .buttonStyle(PillButtonStyle())
                .disabled(model.remoteBusy)
            } else {
                Button {
                    Task { await model.turnOnRemote() }
                } label: {
                    Label("Turn on", systemImage: "qrcode")
                }
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(model.remoteBusy)
            }
            Button("Done") { dismiss() }
                .buttonStyle(PillButtonStyle())
                .keyboardShortcut(.defaultAction)
        }
    }

    private func banner(_ text: String, systemImage: String, tint: Color) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: systemImage).foregroundStyle(tint)
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(10)
        .background(tint.opacity(0.09), in: .rect(cornerRadius: 9))
    }

    private static func countdown(_ seconds: Int) -> String {
        String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}

/// The QR image is rendered once per payload, not on every status poll.
private struct PairingQRCode: View {
    let payload: String
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 164, height: 164)
                    .padding(9)
                    .background(.white, in: .rect(cornerRadius: 9))
            } else {
                RoundedRectangle(cornerRadius: 9)
                    .fill(.white)
                    .frame(width: 182, height: 182)
                    .overlay { ProgressView().controlSize(.small) }
            }
        }
        .task(id: payload) {
            image = QRCode.image(for: payload)
        }
        .accessibilityLabel("Pairing QR code")
    }
}

/// Core Image's QR generator, rendered at a whole-pixel scale so the modules
/// stay crisp.
private enum QRCode {
    static func image(for payload: String) -> NSImage? {
        guard !payload.isEmpty else { return nil }
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(payload.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        guard let cgImage = CIContext().createCGImage(scaled, from: scaled.extent) else { return nil }
        return NSImage(cgImage: cgImage,
                       size: NSSize(width: scaled.extent.width, height: scaled.extent.height))
    }
}
