import Foundation

/// Turns a system networking error into a reason the user can act on. The
/// system's own text ("Could not connect to the server.") does not say why.
public enum ConnectionFailure {
    public static func describe(_ error: any Error) -> String {
        let error = error as NSError
        guard error.domain == NSURLErrorDomain else { return error.localizedDescription }
        let url = error.userInfo[NSURLErrorFailingURLErrorKey] as? URL
        let target = url?.host().map { host in url?.port.map { "\(host):\($0)" } ?? host } ?? "the Mac"
        let reason: String? = switch URLError.Code(rawValue: error.code) {
        case .cannotConnectToHost:
            "Nothing is listening at \(target). Check that /remote is still running on the Mac and that the port matches."
        case .timedOut:
            "\(target) did not answer. Check that both devices are on the same Wi-Fi and that the Mac is awake."
        case .cannotFindHost, .dnsLookupFailed:
            "Could not find \(target) on this network. Check the address, or use the Mac's IP address."
        case .notConnectedToInternet, .dataNotAllowed:
            "This iPhone could not reach the local network. Check Wi-Fi, and allow Local Network for RustCode in Settings."
        case .networkConnectionLost:
            "The connection to \(target) dropped. Check that /remote is still running, then try again."
        case .secureConnectionFailed, .serverCertificateUntrusted, .serverCertificateHasBadDate,
             .serverCertificateHasUnknownRoot, .serverCertificateNotYetValid, .clientCertificateRejected:
            "Could not make a secure connection to \(target)."
        case .appTransportSecurityRequiresSecureConnection:
            "iOS blocked an unencrypted connection to \(target)."
        case .badServerResponse:
            "\(target) answered, but not as a RustCode host. Check the port."
        default:
            nil
        }
        return "\(reason ?? error.localizedDescription) (URL error \(error.code))"
    }
}
