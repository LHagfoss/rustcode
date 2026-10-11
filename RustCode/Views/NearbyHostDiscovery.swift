import Foundation
import Observation

/// Bonjour discovery is local-network only. A discovered address still needs a pairing code.
@MainActor
@Observable
final class NearbyHostDiscovery: NSObject {
    struct Host: Identifiable, Equatable {
        let id: String
        let name: String
        let address: String
    }

    private(set) var hosts: [Host] = []
    private(set) var isSearching = false
    private(set) var error: String?
    private var browser: NetServiceBrowser?
    private var services: [String: NetService] = [:]

    func start() {
        guard browser == nil else { return }
        hosts = []
        error = nil
        isSearching = true
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-pairingNearbyHost") {
            hosts = [Host(id: "fixture", name: "Fixture Mac", address: "fixture.local:17879")]
            isSearching = false
            return
        }
        #endif
        let browser = NetServiceBrowser()
        browser.delegate = self
        self.browser = browser
        browser.searchForServices(ofType: "_rustcode._tcp.", inDomain: "local.")
    }

    func stop() {
        browser?.stop()
        browser?.delegate = nil
        browser = nil
        for service in services.values {
            service.stop()
            service.delegate = nil
        }
        services = [:]
        isSearching = false
    }

    private func key(_ service: NetService) -> String {
        "\(service.name).\(service.type)\(service.domain)"
    }
}

extension NearbyHostDiscovery: @preconcurrency NetServiceBrowserDelegate {
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        let id = key(service)
        services[id] = service
        service.delegate = self
        service.resolve(withTimeout: 8)
    }

    func netServiceBrowser(_ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool) {
        let id = key(service)
        services.removeValue(forKey: id)?.stop()
        hosts.removeAll { $0.id == id }
    }

    func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String: NSNumber]) {
        isSearching = false
        error = "Nearby search is unavailable. Allow Local Network access in Settings, or use a QR code or address."
    }
}

extension NearbyHostDiscovery: @preconcurrency NetServiceDelegate {
    func netServiceDidResolveAddress(_ sender: NetService) {
        guard let hostName = sender.hostName, sender.port > 0,
              services[key(sender)] === sender else { return }
        // NetService returns an absolute DNS name; omit the terminal dot for address classification.
        let addressName = hostName.hasSuffix(".") ? String(hostName.dropLast()) : hostName
        // The host listens on one IPv4 address, while its .local name also resolves to
        // IPv6 and other interfaces that refuse the connection.
        let target = Self.ipv4Address(in: sender.addresses ?? []) ?? addressName
        let host = Host(id: key(sender), name: sender.name, address: "\(target):\(sender.port)")
        hosts.removeAll { $0.id == host.id }
        hosts.append(host)
        hosts.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        // A stale advertisement must not become a selectable but unusable address.
        hosts.removeAll { $0.id == key(sender) }
    }
}

extension NearbyHostDiscovery {
    /// The first routable IPv4 address among a resolved service's socket addresses.
    nonisolated static func ipv4Address(in addresses: [Data]) -> String? {
        for data in addresses {
            let address: String? = data.withUnsafeBytes { raw in
                guard raw.count >= MemoryLayout<sockaddr_in>.size,
                      raw.loadUnaligned(as: sockaddr.self).sa_family == sa_family_t(AF_INET) else { return nil }
                var ip = raw.loadUnaligned(as: sockaddr_in.self).sin_addr
                var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
                guard inet_ntop(AF_INET, &ip, &buffer, socklen_t(INET_ADDRSTRLEN)) != nil else { return nil }
                return String(cString: buffer)
            }
            if let address, !address.hasPrefix("127."), !address.hasPrefix("169.254.") { return address }
        }
        return nil
    }
}
