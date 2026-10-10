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
        let host = Host(id: key(sender), name: sender.name, address: "\(addressName):\(sender.port)")
        hosts.removeAll { $0.id == host.id }
        hosts.append(host)
        hosts.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        // A stale advertisement must not become a selectable but unusable address.
        hosts.removeAll { $0.id == key(sender) }
    }
}
