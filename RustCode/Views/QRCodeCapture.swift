import AVFoundation
import Foundation

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
    private let onStart: @MainActor @Sendable () -> Void
    private let onCode: @MainActor @Sendable (String) -> Void
    private let onFailure: @MainActor @Sendable (String) -> Void

    init(onStart: @escaping @MainActor @Sendable () -> Void,
         onCode: @escaping @MainActor @Sendable (String) -> Void,
         onFailure: @escaping @MainActor @Sendable (String) -> Void) {
        self.onStart = onStart
        self.onCode = onCode
        self.onFailure = onFailure
        super.init()
        for name in [AVCaptureSession.runtimeErrorNotification, AVCaptureSession.wasInterruptedNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: session, queue: nil) { [weak self] _ in
                self?.fail("The camera was interrupted. Tap Try Again when it is available.")
            })
        }
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }

    func setRunning(_ running: Bool) {
        Self.queue.async { [self] in
            wantsRunning = running
            guard running else {
                if session.isRunning { session.stopRunning() }
                return
            }
            do {
                try configure()
                if !session.isRunning { session.startRunning() }
                guard session.isRunning else {
                    fail("The camera could not start. Tap Try Again, or check camera access in Settings.")
                    return
                }
                Task { @MainActor [onStart] in onStart() }
            } catch {
                fail("The camera could not be configured. Check camera access in Settings and try again.")
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

    private func fail(_ message: String) {
        Self.queue.async { [self] in
            guard wantsRunning else { return }
            Task { @MainActor [onFailure] in onFailure(message) }
        }
    }

    private enum CaptureError: Error { case unavailable }
}
