import Foundation

/// A tool call as the transcript shows it: one card that later updates fill in.
public struct ToolItem: Equatable, Sendable {
    public var callId: String
    public var title: String
    public var kind: String?
    public var status: ToolStatus?
    public var input: String?
    public var output: String?
    /// One line saying what the call acted on, taken from its arguments.
    public var inputDetail: String?

    public var displayTitle: String { ToolPresentation.title(title) }
    public var detail: String? { inputDetail ?? ToolPresentation.detail(input: nil, output: output) }
}

public struct TranscriptItem: Identifiable, Equatable, Sendable {
    public enum Content: Equatable, Sendable {
        case user(String)
        case assistant(String)
        case thinking(String)
        case tool(ToolItem)
        /// A host slash command the user ran and the engine's answer to it.
        case command(input: String, output: String)
    }

    public let id: Int
    public var content: Content

    /// Everything a find in the chat can match.
    public var searchableText: String {
        switch content {
        case .user(let text), .assistant(let text), .thinking(let text): text
        case .tool(let tool): [tool.title, tool.input, tool.output].compactMap { $0 }.joined(separator: "\n")
        case .command(let input, let output): input + "\n" + output
        }
    }
}

/// The rendered conversation of one session. Streamed chunks join the item
/// they continue, and tool updates change the card of their call in place.
public struct Transcript: Equatable, Sendable {
    public private(set) var items: [TranscriptItem] = []
    private var nextId = 0

    public init() {}

    public var isEmpty: Bool { items.isEmpty }

    /// Ids of the items containing `query`, ignoring case and diacritics, in order.
    public func matches(_ query: String) -> [Int] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return [] }
        return items.filter {
            $0.searchableText.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) != nil
        }.map(\.id)
    }

    public mutating func removeAll() {
        items.removeAll()
    }

    /// A prompt the user sent. The engine echoes user messages only when it
    /// replays a session, so the client adds its own.
    public mutating func appendUser(_ text: String) {
        append(.user(text))
    }

    /// A host command the user sent; the engine's reply fills it in.
    public mutating func appendCommand(_ input: String) {
        append(.command(input: input, output: ""))
    }

    public mutating func apply(_ update: SessionUpdate) {
        switch update {
        case .userText(let text):
            append(.user(text))
        case .agentText(let text):
            if case .command(let input, let output) = items.last?.content {
                items[items.count - 1].content = .command(input: input, output: output + text)
            } else if case .assistant(let previous) = items.last?.content {
                items[items.count - 1].content = .assistant(previous + text)
            } else {
                append(.assistant(text))
            }
        case .thoughtText(let text):
            if case .thinking(let previous) = items.last?.content {
                items[items.count - 1].content = .thinking(previous + text)
            } else {
                append(.thinking(text))
            }
        case .toolCall(let id, let title, let kind, let status, let input):
            let rendered = input.map { $0.prettyPrinted() }
            let detail = ToolPresentation.detail(input: input, output: nil)
            if let index = toolIndex(id), case .tool(var tool) = items[index].content {
                tool.title = title
                tool.kind = kind ?? tool.kind
                tool.status = status ?? tool.status
                tool.input = rendered ?? tool.input
                tool.inputDetail = detail ?? tool.inputDetail
                items[index].content = .tool(tool)
            } else {
                append(.tool(ToolItem(callId: id, title: title, kind: kind, status: status, input: rendered, inputDetail: detail)))
            }
        case .toolCallUpdate(let id, let title, let status, let output):
            if let index = toolIndex(id), case .tool(var tool) = items[index].content {
                tool.title = title ?? tool.title
                tool.status = status ?? tool.status
                tool.output = output ?? tool.output
                items[index].content = .tool(tool)
            } else {
                append(.tool(ToolItem(callId: id, title: title ?? "Tool", status: status, output: output)))
            }
        case .title, .configOptions, .availableCommands, .unknown:
            break
        }
    }

    private func toolIndex(_ callId: String) -> Int? {
        items.lastIndex { item in
            if case .tool(let tool) = item.content { return tool.callId == callId }
            return false
        }
    }

    private mutating func append(_ content: TranscriptItem.Content) {
        items.append(TranscriptItem(id: nextId, content: content))
        nextId += 1
    }
}
