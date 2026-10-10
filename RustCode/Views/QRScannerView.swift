import AVFoundation
import SwiftUI
import VisionKit

/// Scans one QR code and hands back its text. Falls back to an explanation where the
/// camera cannot be used (Simulator, denied access, restricted devices).
struct QRScannerSheet: View {
    let onScan: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var access: Access = .checking
    @State private var retryCount = 0

    private struct AccessCheck: Equatable {
        var phase: ScenePhase
        var retry: Int
    }

    private enum Access {
        case checking
        case ready
        case denied
        case unsupported
        case failed(String)
    }

    var body: some View {
        NavigationStack {
            Group {
                switch access {
                case .checking:
                    ProgressView()
                case .ready:
                    QRScanner(isActive: scenePhase == .active, onScan: onScan, onFailure: { access = .failed($0) })
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
                case .failed(let message):
                    ContentUnavailableView {
                        Label("Camera Couldn’t Start", systemImage: "camera")
                    } description: {
                        Text(message)
                    } actions: {
                        Button("Try Again") { access = .checking; retryCount += 1 }
                        Button("Enter Code Manually") { dismiss() }
                    }
                case .unsupported:
                    ContentUnavailableView {
                        Label("Scanning Unavailable", systemImage: "qrcode.viewfinder")
                    } description: {
                        Text("This device cannot scan codes. Enter the address and code manually.")
                    } actions: {
                        Button("Enter Code Manually") { dismiss() }
                    }
                }
            }
            .navigationTitle("Scan Pairing Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task(id: AccessCheck(phase: scenePhase, retry: retryCount)) {
                guard scenePhase == .active else { return }
                // Permission prompts and app switching change scenePhase. Keep a ready
                // camera alive rather than replacing its controller on every return.
                if case .ready = access { return }
                let resolved = await resolveAccess()
                guard !Task.isCancelled else { return }
                access = resolved
            }
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
        return DataScannerViewController.isAvailable ? .ready : .failed("The camera is temporarily unavailable. Close other camera apps and try again, or enter the code manually.")
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    let isActive: Bool
    let onScan: (String) -> Void
    let onFailure: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        ScannerController(onScan: onScan, onFailure: onFailure)
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {
        controller.setActive(isActive)
    }

    static func dismantleUIViewController(_ controller: ScannerController, coordinator: ()) {
        controller.setActive(false)
    }
}

/// Start after presentation, when VisionKit has a camera surface. Keep the visual
/// target and recognition region identical, including after rotation.
private final class ScannerController: UIViewController, DataScannerViewControllerDelegate {
    let scanner = DataScannerViewController(
        recognizedDataTypes: [.barcode(symbologies: [.qr])],
        qualityLevel: .balanced,
        recognizesMultipleItems: false,
        isGuidanceEnabled: false,
        isHighlightingEnabled: true
    )
    private let guide = CAShapeLayer()
    private let onScan: (String) -> Void
    private let onFailure: (String) -> Void
    private var hasDelivered = false
    private var isActive = false
    private var isVisible = false

    init(onScan: @escaping (String) -> Void, onFailure: @escaping (String) -> Void) {
        self.onScan = onScan
        self.onFailure = onFailure
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidLoad() {
        super.viewDidLoad()
        addChild(scanner)
        view.addSubview(scanner.view)
        scanner.didMove(toParent: self)
        scanner.delegate = self
        guide.fillColor = UIColor.clear.cgColor
        guide.strokeColor = UIColor.white.cgColor
        guide.lineWidth = 3
        scanner.overlayContainerView.layer.addSublayer(guide)
        scanner.view.accessibilityLabel = "Position the pairing QR code inside the square"
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        scanner.view.frame = view.bounds
        let available = scanner.view.bounds.inset(by: scanner.view.safeAreaInsets)
        let side = min(320, min(available.width * 0.78, available.height * 0.65))
        let target = CGRect(x: available.midX - side / 2, y: available.midY - side / 2, width: side, height: side)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        guide.path = UIBezierPath(roundedRect: target, cornerRadius: 16).cgPath
        CATransaction.commit()
        scanner.regionOfInterest = target
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        isVisible = true
        updateScanning()
    }

    override func viewWillDisappear(_ animated: Bool) {
        isVisible = false
        scanner.stopScanning()
        super.viewWillDisappear(animated)
    }

    func dataScanner(_ dataScanner: DataScannerViewController, becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable) {
        // Backgrounding temporarily makes the camera unavailable. Resume it on return.
        if isActive && isVisible && UIApplication.shared.applicationState == .active { reportFailure() }
    }

    func setActive(_ active: Bool) {
        isActive = active
        updateScanning()
    }

    private func updateScanning() {
        guard isActive, isVisible, !hasDelivered else {
            scanner.stopScanning()
            return
        }
        guard !scanner.isScanning else { return }
        do { try scanner.startScanning() }
        catch { reportFailure() }
    }

    private func reportFailure() {
        scanner.stopScanning()
        // Defer the SwiftUI state change out of representable updates.
        Task { @MainActor [weak self] in
            guard let self, isActive, isVisible, !hasDelivered else { return }
            onFailure("Scanning is unavailable right now. Try again, or enter the address and pairing code manually.")
        }
    }

    func dataScanner(_ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
        deliver(addedItems)
    }

    func dataScanner(_ dataScanner: DataScannerViewController, didUpdate updatedItems: [RecognizedItem], allItems: [RecognizedItem]) {
        deliver(updatedItems)
    }

    func dataScanner(_ dataScanner: DataScannerViewController, didTapOn item: RecognizedItem) {
        deliver([item])
    }

    private func deliver(_ items: [RecognizedItem]) {
        guard isActive, isVisible, !hasDelivered else { return }
        for case .barcode(let barcode) in items {
            if let payload = barcode.payloadStringValue, !payload.isEmpty {
                hasDelivered = true
                scanner.stopScanning()
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                onScan(payload)
                return
            }
        }
    }
}
