import AVFoundation
import SwiftUI
import RustCodeRemote

/// Scans one QR code and hands back its text. Falls back to an explanation where the
/// camera cannot be used (Simulator, denied access, restricted devices).
struct QRScannerSheet: View {
    let onScan: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var access: Access = .checking
    @State private var retryCount = 0

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
                    ProgressView("Opening Camera…")
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
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .navigationTitle("Scan Pairing Code")
            .navigationBarTitleDisplayMode(.inline)
            .softNavigationChrome()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task(id: retryCount) {
                // A permission prompt makes the scene inactive. Do not cancel that
                // permission request; scene changes only pause/resume capture.
                if case .ready = access { return }
                let resolved = await resolveAccess()
                guard !Task.isCancelled else { return }
                access = resolved
            }
            .onChange(of: scenePhase) { _, phase in
                // Returning from Settings can grant permission without reopening
                // this sheet. Only refresh a denied result, not a pending prompt.
                guard phase == .active, case .denied = access else { return }
                access = .checking
                retryCount += 1
            }
        }
    }

    private func resolveAccess() async -> Access {
        guard AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back) != nil else { return .unsupported }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            break
        case .notDetermined:
            guard await AVCaptureDevice.requestAccess(for: .video) else { return .denied }
        default:
            return .denied
        }
        return .ready
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

/// Own the preview surface directly. Camera configuration and startup run on a
/// serial background queue; presentation, preview readiness and delivery stay here.
private final class ScannerController: UIViewController {
    private let onScan: (String) -> Void
    private let onFailure: (String) -> Void
    private var lifecycle = PairingScannerLifecycle()
    private let guide = CAShapeLayer()
    private let loading = UIActivityIndicatorView(style: .large)
    private var previewObservation: NSKeyValueObservation?
    private var previewTimeout: Task<Void, Never>?
    private lazy var capture = QRCodeCapture(
        onStart: { [weak self] in self?.view.setNeedsLayout() },
        onCode: { [weak self] in self?.deliver($0) },
        onFailure: { [weak self] in self?.reportFailure($0) }
    )
    private lazy var preview = AVCaptureVideoPreviewLayer(session: capture.session)

    init(onScan: @escaping (String) -> Void, onFailure: @escaping (String) -> Void) {
        self.onScan = onScan
        self.onFailure = onFailure
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        guide.fillColor = UIColor.clear.cgColor
        guide.strokeColor = UIColor.white.cgColor
        guide.lineWidth = 3
        view.layer.addSublayer(guide)
        view.accessibilityLabel = "Position the pairing QR code inside the square"
        loading.color = .white
        loading.accessibilityLabel = "Starting camera preview"
        view.addSubview(loading)
        loading.startAnimating()
        previewObservation = preview.observe(\.isPreviewing, options: [.initial, .new]) { [weak self] _, change in
            let ready = change.newValue ?? false
            Task { @MainActor [weak self] in
                guard let self else { return }
                lifecycle.previewChanged(ready)
                updatePreviewLoading()
            }
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        preview.frame = view.bounds
        let orientation = view.window?.windowScene?.effectiveGeometry.interfaceOrientation ?? .portrait
        let angle: CGFloat = switch orientation {
        case .landscapeLeft: 0
        case .landscapeRight: 180
        case .portraitUpsideDown: 270
        default: 90
        }
        if let connection = preview.connection, connection.isVideoRotationAngleSupported(angle) {
            connection.videoRotationAngle = angle
        }
        let available = view.bounds.inset(by: view.safeAreaInsets)
        let side = min(320, min(available.width * 0.78, available.height * 0.65))
        let target = CGRect(x: available.midX - side / 2, y: available.midY - side / 2, width: side, height: side)
        guide.path = UIBezierPath(roundedRect: target, cornerRadius: 16).cgPath
        loading.center = CGPoint(x: available.midX, y: available.midY)
        CATransaction.commit()
        capture.setRegion(preview.metadataOutputRectConverted(fromLayerRect: target))
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        lifecycle.update(visible: true, active: lifecycle.isActive)
        updateCapture()
    }

    override func viewWillDisappear(_ animated: Bool) {
        lifecycle.update(visible: false, active: lifecycle.isActive)
        updateCapture()
        super.viewWillDisappear(animated)
    }

    func setActive(_ active: Bool) {
        guard active != lifecycle.isActive else { return }
        lifecycle.update(visible: lifecycle.isVisible, active: active)
        updateCapture()
    }

    private func updateCapture() {
        capture.setRunning(lifecycle.shouldRun)
        updatePreviewLoading()
    }

    private func updatePreviewLoading() {
        if !lifecycle.needsPreviewRecovery {
            loading.stopAnimating()
            previewTimeout?.cancel()
            previewTimeout = nil
        } else {
            loading.startAnimating()
            guard previewTimeout == nil else { return }
            previewTimeout = Task { @MainActor [weak self] in
                do { try await Task.sleep(for: .seconds(8)) } catch { return }
                guard let self, lifecycle.needsPreviewRecovery else { return }
                reportFailure("The camera preview did not start. Tap Try Again, or close other camera apps and retry.")
            }
        }
    }

    private func reportFailure(_ message: String) {
        guard lifecycle.shouldRun else { return }
        capture.setRunning(false)
        previewTimeout?.cancel()
        previewTimeout = nil
        onFailure(message)
    }

    private func deliver(_ payload: String) {
        guard lifecycle.accept(payload) else { return }
        updateCapture()
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        onScan(payload)
    }
}
