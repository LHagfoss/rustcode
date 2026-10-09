import AVFoundation
import SwiftUI
import VisionKit

/// Scans one QR code and hands back its text. Falls back to an explanation where the
/// camera cannot be used (Simulator, denied access, restricted devices).
struct QRScannerSheet: View {
    let onScan: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var access: Access = .checking

    private enum Access {
        case checking
        case ready
        case denied
        case unsupported
    }

    var body: some View {
        NavigationStack {
            Group {
                switch access {
                case .checking:
                    ProgressView()
                case .ready:
                    QRScanner(onScan: onScan)
                        .ignoresSafeArea(edges: .bottom)
                        .overlay(alignment: .bottom) {
                            Text("Point the camera at the QR code shown by /remote")
                                .font(.subheadline)
                                .padding(.horizontal, 16)
                                .padding(.vertical, 10)
                                .background(.regularMaterial, in: .capsule)
                                .padding(.bottom, 32)
                        }
                case .denied:
                    ContentUnavailableView {
                        Label("Camera Access Needed", systemImage: "camera.fill")
                    } description: {
                        Text("Allow camera access in Settings to scan the pairing code, or enter the address and code manually.")
                    } actions: {
                        Button("Open Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) {
                                UIApplication.shared.open(url)
                            }
                        }
                    }
                case .unsupported:
                    ContentUnavailableView(
                        "Scanning Unavailable",
                        systemImage: "qrcode.viewfinder",
                        description: Text("This device cannot scan codes. Enter the address and code manually.")
                    )
                }
            }
            .navigationTitle("Scan Pairing Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task { access = await resolveAccess() }
        }
    }

    private func resolveAccess() async -> Access {
        guard DataScannerViewController.isSupported else { return .unsupported }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            break
        case .notDetermined:
            guard await AVCaptureDevice.requestAccess(for: .video) else { return .denied }
        default:
            return .denied
        }
        return DataScannerViewController.isAvailable ? .ready : .denied
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    let onScan: (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onScan: onScan)
    }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ scanner: DataScannerViewController, coordinator: Coordinator) {
        scanner.stopScanning()
    }

    @MainActor
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private let onScan: (String) -> Void
        private var hasDelivered = false

        init(onScan: @escaping (String) -> Void) {
            self.onScan = onScan
        }

        func dataScanner(_ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
            deliver(addedItems)
        }

        func dataScanner(_ dataScanner: DataScannerViewController, didTapOn item: RecognizedItem) {
            deliver([item])
        }

        private func deliver(_ items: [RecognizedItem]) {
            guard !hasDelivered else { return }
            for case .barcode(let barcode) in items {
                if let payload = barcode.payloadStringValue, !payload.isEmpty {
                    hasDelivered = true
                    onScan(payload)
                    return
                }
            }
        }
    }
}
