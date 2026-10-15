import Foundation

/// ACP's stdio transport: one JSON-RPC 2.0 message per line, UTF-8, no
/// embedded newlines. `LineFramer` turns arbitrary pipe reads into lines.
public struct LineFramer {
    /// A single message larger than this is treated as a broken stream rather
    /// than buffered without bound.
    public static let defaultMaxLineBytes = 32 * 1024 * 1024

    public enum FramingError: Error, Equatable {
        case lineTooLong(Int)
    }

    private var buffer = Data()
    private let maxLineBytes: Int

    public init(maxLineBytes: Int = LineFramer.defaultMaxLineBytes) {
        self.maxLineBytes = maxLineBytes
    }

    public mutating func append(_ chunk: Data) throws -> [Data] {
        buffer.append(chunk)
        var lines: [Data] = []
        while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
            var line = buffer[buffer.startIndex..<newline]
            if line.last == UInt8(ascii: "\r") { line = line.dropLast() }
            if !line.isEmpty { lines.append(Data(line)) }
            buffer.removeSubrange(buffer.startIndex...newline)
        }
        if buffer.count > maxLineBytes {
            let size = buffer.count
            buffer.removeAll()
            throw FramingError.lineTooLong(size)
        }
        return lines
    }
}

public struct RPCError: Error, Equatable, Sendable, CustomStringConvertible {
    public var code: Int
    public var message: String
    public var data: JSONValue?

    public init(code: Int, message: String, data: JSONValue? = nil) {
        self.code = code
        self.message = message
        self.data = data
    }

    public static func methodNotFound(_ method: String) -> RPCError {
        RPCError(code: -32601, message: "Method not found: \(method)")
    }

    public static func invalidParams(_ method: String) -> RPCError {
        RPCError(code: -32602, message: "Invalid params: \(method)")
    }

    public var description: String {
        if let detail = data?.stringValue, !detail.isEmpty {
            return "\(message): \(detail)"
        }
        return message
    }
}

public enum RPCMessage: Equatable, Sendable {
    case request(id: JSONValue, method: String, params: JSONValue)
    case notification(method: String, params: JSONValue)
    case response(id: JSONValue, result: JSONValue)
    case error(id: JSONValue, error: RPCError)

    public enum DecodeError: Error, Equatable {
        case notJSON
        case notAnObject
        case invalidEnvelope
    }

    public static func decode(_ line: Data) throws -> RPCMessage {
        guard let value = try? JSONDecoder().decode(JSONValue.self, from: line) else {
            throw DecodeError.notJSON
        }
        guard let object = value.objectValue else { throw DecodeError.notAnObject }
        let id = object["id"]
        if let method = object["method"]?.stringValue {
            let params = object["params"] ?? .null
            if let id, id != .null {
                return .request(id: id, method: method, params: params)
            }
            return .notification(method: method, params: params)
        }
        guard let id else { throw DecodeError.invalidEnvelope }
        if let error = object["error"]?.objectValue {
            return .error(
                id: id,
                error: RPCError(
                    code: error["code"]?.intValue ?? -32603,
                    message: error["message"]?.stringValue ?? "Unknown error",
                    data: error["data"]))
        }
        if let result = object["result"] {
            return .response(id: id, result: result)
        }
        throw DecodeError.invalidEnvelope
    }

    /// One wire line, newline-terminated.
    public func encodedLine() throws -> Data {
        var envelope: [String: JSONValue] = ["jsonrpc": "2.0"]
        switch self {
        case .request(let id, let method, let params):
            envelope["id"] = id
            envelope["method"] = .string(method)
            envelope["params"] = params
        case .notification(let method, let params):
            envelope["method"] = .string(method)
            envelope["params"] = params
        case .response(let id, let result):
            envelope["id"] = id
            envelope["result"] = result
        case .error(let id, let error):
            var body: [String: JSONValue] = [
                "code": .number(Double(error.code)), "message": .string(error.message),
            ]
            if let data = error.data { body["data"] = data }
            envelope["id"] = id
            envelope["error"] = .object(body)
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        var data = try encoder.encode(JSONValue.object(envelope))
        data.append(UInt8(ascii: "\n"))
        return data
    }
}
