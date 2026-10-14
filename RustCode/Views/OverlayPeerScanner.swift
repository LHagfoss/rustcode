import Darwin
import Foundation

/// Finds gateways on a NetBird mesh. Bonjour multicast does not cross the WireGuard tunnel,
/// so the scanner dials the gateway port on every address in this device's /16 instead.
/// WireGuard drops packets for addresses without a peer before they leave the phone, so
/// only actual mesh peers ever see a probe.
enum OverlayPeerScanner {
    struct Peer: Sendable, Equatable {
        let address: String
        /// The peer's NetBird DNS name, when its resolver answers for the address.
        let name: String?
    }

    /// How long a batch waits for peers to answer. Covers relayed peers too.
    private static let probeTimeout: Int32 = 600
    private static let preferredBatchSize = 1024

    /// This device's IPv4 address inside 100.64.0.0/10, in host byte order.
    static func localOverlayAddress() -> UInt32? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        for entry in sequence(first: first, next: { $0.pointee.ifa_next }) {
            guard let socketAddress = entry.pointee.ifa_addr,
                  socketAddress.pointee.sa_family == sa_family_t(AF_INET) else { continue }
            let address = socketAddress.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                UInt32(bigEndian: $0.pointee.sin_addr.s_addr)
            }
            if address & 0xFFC0_0000 == 0x6440_0000 { return address }
        }
        return nil
    }

    /// Every peer in `local`'s /16 that accepts a connection on `port`, as it answers.
    static func peers(around local: UInt32, port: Int) -> AsyncStream<Peer> {
        AsyncStream { continuation in
            let task = Task.detached(priority: .utility) {
                let network = local & 0xFFFF_0000
                let candidates = (1..<UInt32(0xFFFF)).map { network | $0 }.filter { $0 != local }
                let batchSize = reserveDescriptors()
                var start = 0
                while start < candidates.count, !Task.isCancelled {
                    let batch = Array(candidates[start..<min(start + batchSize, candidates.count)])
                    start += batch.count
                    for address in probe(batch, port: UInt16(port)) {
                        let text = dottedQuad(address)
                        continuation.yield(Peer(address: text, name: reverseName(text)))
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Raises the open-file limit if needed, and returns a batch size that leaves the app
    /// room for its own descriptors.
    private static func reserveDescriptors() -> Int {
        var limit = rlimit()
        guard getrlimit(RLIMIT_NOFILE, &limit) == 0 else { return 128 }
        let wanted = rlim_t(preferredBatchSize + 256)
        if limit.rlim_cur < wanted {
            limit.rlim_cur = min(wanted, limit.rlim_max)
            _ = setrlimit(RLIMIT_NOFILE, &limit)
            _ = getrlimit(RLIMIT_NOFILE, &limit)
        }
        return max(64, min(preferredBatchSize, Int(limit.rlim_cur) - 256))
    }

    /// Starts a non-blocking connect to every address, then collects those that complete.
    private static func probe(_ addresses: [UInt32], port: UInt16) -> [UInt32] {
        var pending: [(fd: Int32, address: UInt32)] = []
        defer { for socket in pending { close(socket.fd) } }
        for address in addresses {
            let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP)
            guard fd >= 0 else { break }
            _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
            var on: Int32 = 1
            _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
            var target = sockaddr_in()
            target.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            target.sin_family = sa_family_t(AF_INET)
            target.sin_port = port.bigEndian
            target.sin_addr = in_addr(s_addr: address.bigEndian)
            let result = withUnsafePointer(to: &target) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
            guard result == 0 || errno == EINPROGRESS else {
                close(fd)
                continue
            }
            pending.append((fd, address))
        }

        var polls = pending.map { pollfd(fd: $0.fd, events: Int16(POLLOUT), revents: 0) }
        var open: [UInt32] = []
        let deadline = ContinuousClock.now + .milliseconds(Int(probeTimeout))
        while !Task.isCancelled {
            let remaining = deadline - ContinuousClock.now
            let milliseconds = Int32(remaining.components.seconds * 1000
                + remaining.components.attoseconds / 1_000_000_000_000_000)
            guard milliseconds > 0, poll(&polls, nfds_t(polls.count), milliseconds) > 0 else { break }
            for index in polls.indices where polls[index].fd >= 0 && polls[index].revents != 0 {
                var error: Int32 = 0
                var length = socklen_t(MemoryLayout<Int32>.size)
                if getsockopt(polls[index].fd, SOL_SOCKET, SO_ERROR, &error, &length) == 0, error == 0 {
                    open.append(pending[index].address)
                }
                // A negative descriptor tells poll to skip the entry.
                polls[index].fd = -1
            }
        }
        return open
    }

    /// The first label of the peer's DNS name, e.g. `studio` for `studio.netbird.cloud`.
    private static func reverseName(_ address: String) -> String? {
        var target = sockaddr_in()
        target.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        target.sin_family = sa_family_t(AF_INET)
        guard inet_pton(AF_INET, address, &target.sin_addr) == 1 else { return nil }
        var buffer = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        let status = withUnsafePointer(to: &target) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getnameinfo($0, socklen_t(MemoryLayout<sockaddr_in>.size), &buffer, socklen_t(buffer.count), nil, 0, NI_NAMEREQD)
            }
        }
        guard status == 0 else { return nil }
        let label = String(cString: buffer).split(separator: ".").first.map(String.init)
        return label?.isEmpty == false ? label : nil
    }

    private static func dottedQuad(_ address: UInt32) -> String {
        [24, 16, 8, 0].map { String((address >> $0) & 0xFF) }.joined(separator: ".")
    }
}
