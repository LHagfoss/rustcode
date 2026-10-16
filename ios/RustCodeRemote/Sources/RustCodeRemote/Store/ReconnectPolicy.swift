import Foundation

/// Exponential backoff with jitter and a hard ceiling. Pure: the random input is supplied.
public struct BackoffPolicy: Sendable, Equatable {
    public var initialDelay: Duration
    public var multiplier: Double
    /// No delay is ever longer than this, jitter included.
    public var maximumDelay: Duration
    /// Fraction of the delay to vary by, in both directions. 0.2 means ±20 %.
    public var jitter: Double
    /// Automatic attempts before giving up and waiting for a manual retry.
    public var maximumAttempts: Int

    public init(
        initialDelay: Duration = .seconds(1),
        multiplier: Double = 2,
        maximumDelay: Duration = .seconds(30),
        jitter: Double = 0.2,
        maximumAttempts: Int = 8
    ) {
        self.initialDelay = initialDelay
        self.multiplier = multiplier
        self.maximumDelay = maximumDelay
        self.jitter = jitter
        self.maximumAttempts = maximumAttempts
    }

    public static let standard = BackoffPolicy()

    /// - Parameters:
    ///   - attempt: 1 for the delay before the first retry.
    ///   - randomUnit: a value in `0...1`; 0.5 means no jitter.
    public func delay(forAttempt attempt: Int, randomUnit: Double) -> Duration {
        let exponent = Double(max(attempt, 1) - 1)
        let ceiling = maximumDelay.seconds
        let base = min(initialDelay.seconds * pow(multiplier, exponent), ceiling)
        let unit = min(max(randomUnit, 0), 1)
        let factor = 1 + jitter * (2 * unit - 1)
        return .seconds(min(max(base * factor, 0), ceiling))
    }
}

/// Decides what to do after each connection failure.
public struct ReconnectState: Sendable, Equatable {
    public enum Decision: Sendable, Equatable {
        case retry(attempt: Int, after: Duration)
        /// The automatic attempts are used up; only a manual retry continues.
        case giveUp
        /// The host refused this device. Retrying cannot help until it is paired again.
        case stop
    }

    public let policy: BackoffPolicy
    public private(set) var failures = 0
    public private(set) var isStopped = false

    public init(policy: BackoffPolicy = .standard) {
        self.policy = policy
    }

    public mutating func registerFailure(unauthorized: Bool, randomUnit: Double) -> Decision {
        if unauthorized || isStopped {
            isStopped = true
            return .stop
        }
        failures += 1
        guard failures <= policy.maximumAttempts else { return .giveUp }
        return .retry(attempt: failures, after: policy.delay(forAttempt: failures, randomUnit: randomUnit))
    }

    /// A connection was established: the next failure starts from the shortest delay again.
    public mutating func registerSuccess() {
        failures = 0
    }

    /// The user asked to try again. Does not lift an unauthorized stop.
    public mutating func manualRetry() {
        failures = 0
    }
}

extension Duration {
    var seconds: Double {
        Double(components.seconds) + Double(components.attoseconds) / 1e18
    }
}
