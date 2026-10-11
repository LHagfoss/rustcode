import AVFoundation
import Foundation
import RustCodeRemote

/// All session configuration/start/stop operations and mutable fields below are
/// confined to `queue`. Every scanner shares that queue, so a dismissed scanner
/// releases the camera before a reopened one tries to start it. Only the preview layer shares the session, as AVFoundation
/// supports. Delegate and notification callbacks forward values to the main actor.
final class QRCodeCapture: NSObject, @unchecked Sendable, AVCaptureMetadataOutputObjectsDelegate {
    let session = AVCaptureSession()
    private let output = AVCaptureMetadataOutput()
    private static let queue = DispatchQueue(label: "RustCode.pairing-camera", qos: .userInitiated)
    private var configured = false
    private var wantsRunning = false
    private var observers: [NSObjectProtocol] = []
    private let onStart: @MainActor @Sendable (Int) -> Void
    private let onCode: @MainActor @Sendable (String) -> Void
    private let onFailure: @MainActor @Sendable (PairingScannerFailure) -> Void

    /// `onStart` returns the run number its `setRunning` call was given, so the
    /// caller can tell a current start from one queued before a pause.
    init(onStart: @escaping @MainActor @Sendable (Int) -> Void,
         onCode: @escaping @MainActor @Sendable (String) -> Void,
         onFailure: @escaping @MainActor @Sendable (PairingScannerFailure) -> Void) {
        self.onStart = onStart
        self.onCode = onCode
        self.onFailure = onFailure
        super.init()
        observers.append(NotificationCenter.default.addObserver(forName: AVCaptureSession.runtimeErrorNotification, object: session, queue: nil) { [weak self] _ in
            self?.fail(.interrupted)
        })
        observers.append(NotificationCenter.default.addObserver(forName: AVCaptureSession.wasInterruptedNotification, object: session, queue: nil) { [weak self] note in
            let reason = (note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int).flatMap(AVCaptureSession.InterruptionReason.init)
            switch reason {
            // Capture resumes by itself on returning to the foreground.
            case .videoDeviceNotAvailableInBackground: return
            case .videoDeviceInUseByAnotherClient: self?.fail(.inUseByAnotherApp)
            case .videoDeviceNotAvailableWithMultipleForegroundApps: self?.fail(.unavailableWhileMultitasking)
            case .videoDeviceNotAvailableDueToSystemPressure: self?.fail(.systemPressure)
            default: self?.fail(.interrupted)
            }
        })
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }

    /// `restarting` stops a session that runs without showing video before starting it again.
    func setRunning(_ running: Bool, run: Int = 0, restarting: Bool = false) {
        Self.queue.async { [self] in
            wantsRunning = running
            guard running else {
                if session.isRunning { session.stopRunning() }
                return
            }
            do {
                try configure()
                if restarting, session.isRunning { session.stopRunning() }
                if !session.isRunning { session.startRunning() }
                guard session.isRunning else {
                    fail(.couldNotStart)
                    return
                }
                Task { @MainActor [onStart] in onStart(run) }
            } catch {
                fail(.couldNotConfigure)
            }
        }
    }

    func setRegion(_ region: CGRect) {
        guard region.width > 0, region.height > 0 else { return }
        Self.queue.async { [self] in output.rectOfInterest = region }
    }

    private func configure() throws {
        guard !configured else { return }
        guard let camera = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back) else {
            throw CaptureError.unavailable
        }
        let input = try AVCaptureDeviceInput(device: camera)
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        if session.canSetSessionPreset(.hd1280x720) { session.sessionPreset = .hd1280x720 }
        guard session.canAddInput(input) else { throw CaptureError.unavailable }
        session.addInput(input)
        guard session.canAddOutput(output) else {
            session.removeInput(input)
            throw CaptureError.unavailable
        }
        session.addOutput(output)
        guard output.availableMetadataObjectTypes.contains(.qr) else {
            session.removeOutput(output)
            session.removeInput(input)
            throw CaptureError.unavailable
        }
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        configured = true
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard let payload = metadataObjects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first else { return }
        Task { @MainActor [onCode] in onCode(payload) }
    }

    private func fail(_ failure: PairingScannerFailure) {
        Self.queue.async { [self] in
            guard wantsRunning else { return }
            Task { @MainActor [onFailure] in onFailure(failure) }
        }
    }

    private enum CaptureError: Error { case unavailable }
}
