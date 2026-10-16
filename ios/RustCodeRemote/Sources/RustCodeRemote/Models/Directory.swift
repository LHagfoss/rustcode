import Foundation

public struct RemoteDirectory: Codable, Hashable, Sendable, Identifiable {
    public var name: String
    public var path: String
    public var id: String { path }
    public init(name: String, path: String) { self.name = name; self.path = path }
}

public struct DirectoryListing: Codable, Equatable, Sendable {
    public var path: String
    public var parent: String?
    public var directories: [RemoteDirectory]
    public var truncated: Bool
    public init(path: String, parent: String?, directories: [RemoteDirectory], truncated: Bool = false) {
        self.path = path; self.parent = parent; self.directories = directories; self.truncated = truncated
    }
}
