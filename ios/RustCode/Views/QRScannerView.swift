import AVFoundation
import SwiftUI
import RustCodeRemote
import OSLog

/// Scans one QR code and hands back its text. Falls back to an explanation where the
/// camera cannot be used (Simulator, denied access, restricted devices).
struct QRScannerSheet: View {
    let onScan: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var access: Access = .checking
    @State private var retryCount = 0

    private enum Access {
        case checking
        case ready
        case denied
        case unsupported
        case failed(PairingScannerFailure)
    }

    var body: some View {
        NavigationStack {
            Group {
                switch access {
                case .checking:
                    ProgressView("Opening Camera…")
                case .ready:
                    QRScanner(onScan: onScan, onFailure: { access = .failed($0) })
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
                case .failed(let failure):
                    ContentUnavailableView {
                        Label("Camera Couldn’t Start", systemImage: "camera")
                    } description: {
                        Text(failure.message)
                    } actions: {
                        // Retrying re-checks access and builds a fresh capture session in this sheet.
                        Button("Try Again") { access = .checking; retryCount += 1 }
                        if failure.offersSettings {
                            Button("Open Settings") {
                                if let url = URL(string: UIApplication.openSettingsURLString) {
                                    UIApplication.shared.open(url)
                                }
                            }
                        }
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
                // permission request; only backgrounding pauses capture.
                if case .ready = access { return }
                // Apply the result even if SwiftUI restarted this task while the
                // sheet settled; dropping it would leave "Opening Camera…" up forever.
                access = await resolveAccess()
            }
            // UIKit's notification, not `scenePhase`: inside a sheet the environment
            // value can be stale, which used to leave the camera paused on a black view.
            .onReceive(NotificationCenter.default.publisher(for: UIApplication.didBecomeActiveNotification)) { _ in
                // Returning from Settings can grant permission without reopening
                // this sheet. Only refresh a result Settings can change, not a pending prompt.
                switch access {
                case .denied, .failed(.accessRevoked): break
                default: return
                }
                access = .checking
                retryCount += 1
            }
        }
    }

    private func resolveAccess() async -> Access {
        if LaunchOptions.current.opensScannerWithoutCamera { return .ready }
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
    let onScan: (String) -> Void
    let onFailure: (PairingScannerFailure) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        ScannerController(onScan: onScan, onFailure: onFailure)
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}

    static func dismantleUIViewController(_ controller: ScannerController, coordinator: ()) {
        controller.stop()
    }
}

/// Own the preview surface directly. Camera configuration and startup run on a
/// serial background queue; presentation, preview readiness and delivery stay here.
private final class ScannerController: UIViewController {
    private let onScan: (String) -> Void
    private let onFailure: (PairingScannerFailure) -> Void
    private var lifecycle = PairingScannerLifecycle()
    private let guide = CAShapeLayer()
    private let loading = UIActivityIndicatorView(style: .large)
    private var previewObservation: NSKeyValueObservation?
    private var previewTimeout: Task<Void, Never>?
    private let logger = Logger(subsystem: "com.lhagfoss.rustcode", category: "PairingCamera")
    private lazy var capture = QRCodeCapture(
        onStart: { [weak self] in self?.captureStarted(run: $0) },
        onCode: { [weak self] in self?.deliver($0) },
        onFailure: { [weak self] in self?.reportFailure($0) }
    )
    private lazy var preview = AVCaptureVideoPreviewLayer(session: capture.session)

    init(onScan: @escaping (String) -> Void, onFailure: @escaping (PairingScannerFailure) -> Void) {
        self.onScan = onScan
        self.onFailure = onFailure
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func loadView() {
        let view = ScannerView()
        view.windowChanged = { [weak self] attached in self?.setVisible(attached) }
        self.view = view
    }

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
        previewObservation = preview.observe(\.isPreviewing, options: [.initial, .new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                // Read the layer now: queued change values can arrive out of order.
                lifecycle.previewChanged(preview.isPreviewing)
                updatePreviewLoading()
            }
        }
        // Capture follows the app's own foreground state. An inactive scene (the
        // permission prompt, Control Center) keeps the camera, so a brief
        // interruption cannot stop and restart it underneath the preview.
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(didEnterBackground), name: UIApplication.didEnterBackgroundNotification, object: nil)
        center.addObserver(self, selector: #selector(willEnterForeground), name: UIApplication.willEnterForegroundNotification, object: nil)
    }

    @objc private func didEnterBackground() { setForeground(false) }
    @objc private func willEnterForeground() { setForeground(true) }

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

    // Visibility follows the view's window, not appearance callbacks, which reach a
    // representable controller only as SwiftUI forwards them. A viewWillDisappear
    // with no later viewDidAppear (a cancelled sheet swipe, a transition while the
    // sheet presents) used to stop capture under a visible view: black, with no
    // spinner, no watchdog and no error to retry from.
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        setVisible(view.window != nil)
    }

    func stop() {
        setVisible(false)
    }

    private func setVisible(_ visible: Bool) {
        guard visible != lifecycle.isVisible else { return }
        // A newly shown scanner reads the app's state; afterwards the foreground
        // notifications keep it current.
        let active = visible ? UIApplication.shared.applicationState != .background : lifecycle.isActive
        lifecycle.update(visible: visible, active: active)
        updateCapture()
    }

    private func setForeground(_ foreground: Bool) {
        guard foreground != lifecycle.isActive else { return }
        lifecycle.update(visible: lifecycle.isVisible, active: foreground)
        updateCapture()
    }

    private func updateCapture() {
        capture.setRunning(lifecycle.shouldRun, run: lifecycle.run)
        lifecycle.previewChanged(preview.isPreviewing)
        updatePreviewLoading()
    }

    private func captureStarted(run: Int) {
        let wasStarting = lifecycle.isStarting
        lifecycle.sessionStarted(run: run)
        // A start queued before a pause or restart is not this run's start.
        guard wasStarting, lifecycle.hasStarted else { return }
        view.setNeedsLayout()
        // Replace the startup bound with the shorter wait for the first frame.
        previewTimeout?.cancel()
        previewTimeout = nil
        lifecycle.previewChanged(preview.isPreviewing)
        updatePreviewLoading()
    }

    private func updatePreviewLoading() {
        if lifecycle.shouldRun && !lifecycle.hasPreview { loading.startAnimating() }
        else { loading.stopAnimating() }
        if !lifecycle.shouldRun || lifecycle.hasPreview {
            previewTimeout?.cancel()
            previewTimeout = nil
        } else {
            guard previewTimeout == nil else { return }
            // Startup and the first frame have separate bounds; see PairingScannerLifecycle.
            let timeout = lifecycle.timeout
            previewTimeout = Task { @MainActor [weak self] in
                // Poll the layer as well as observing it: a missed observation must
                // not leave the spinner over a working camera, or end in an error.
                let deadline = ContinuousClock.now + timeout
                while ContinuousClock.now < deadline {
                    do { try await Task.sleep(for: .milliseconds(100)) } catch { return }
                    guard let self, lifecycle.shouldRun else { return }
                    lifecycle.previewChanged(preview.isPreviewing)
                    guard lifecycle.hasPreview else { continue }
                    updatePreviewLoading()
                    return
                }
                guard !Task.isCancelled else { return }
                self?.previewTimedOut()
            }
        }
    }

    private func previewTimedOut() {
        previewTimeout = nil
        lifecycle.previewChanged(preview.isPreviewing)
        let running = capture.session.isRunning
        logger.error("Preview timeout: running=\(running) connected=\(self.preview.connection != nil) enabled=\(self.preview.connection?.isEnabled ?? false) active=\(self.preview.connection?.isActive ?? false) attached=\(self.view.window != nil) width=\(self.preview.bounds.width) height=\(self.preview.bounds.height)")
        switch lifecycle.previewTimedOut() {
        case .ignore:
            updatePreviewLoading()
        case .restart:
            capture.setRunning(true, run: lifecycle.run, restarting: true)
            updatePreviewLoading()
        case .fail:
            let authorized = AVCaptureDevice.authorizationStatus(for: .video) == .authorized
            reportFailure(.timedOut(authorized: authorized, sessionRunning: running))
        }
    }

    private func reportFailure(_ failure: PairingScannerFailure) {
        guard lifecycle.shouldRun else { return }
        capture.setRunning(false)
        previewTimeout?.cancel()
        previewTimeout = nil
        onFailure(failure)
    }

    private func deliver(_ payload: String) {
        guard lifecycle.accept(payload) else { return }
        updateCapture()
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        onScan(payload)
    }
}

/// Reports joining and leaving a window, which is the one signal that reliably says
/// the preview can be seen.
private final class ScannerView: UIView {
    var windowChanged: ((Bool) -> Void)?

    override func didMoveToWindow() {
        super.didMoveToWindow()
        windowChanged?(window != nil)
    }
}
