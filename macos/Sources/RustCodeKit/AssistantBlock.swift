import Foundation

/// Presentation of the host's `<think>`-delimited assistant text. The raw transcript
/// stays intact for replay, history, copying and sharing.
public enum AssistantBlock: Sendable, Equatable {
    case markdown(String)
    case thinking(text: String, isComplete: Bool)

    public static func parse(_ text: String, isStreaming: Bool = false) -> [AssistantBlock] {
        let text = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        var blocks: [AssistantBlock] = []
        var buffer = ""
        var thinking = false
        var fence: MarkdownFence?
        var inlineTicks = 0
        var index = text.startIndex

        func flush(isComplete: Bool = false) {
            if thinking {
                blocks.append(.thinking(text: buffer, isComplete: isComplete))
            } else if !buffer.isEmpty {
                blocks.append(.markdown(buffer))
            }
            buffer = ""
        }

        while index < text.endIndex {
            let remaining = text[index...]
            let atLineStart = buffer.isEmpty || index == text.startIndex || text[text.index(before: index)] == "\n"
            if atLineStart, inlineTicks == 0 {
                let end = remaining.firstIndex(of: "\n") ?? text.endIndex
                let line = String(text[index..<end])
                let opening = MarkdownFence.opening(line)
                if fence != nil || opening != nil {
                    if let current = fence {
                        if current.closes(line) { fence = nil }
                    } else {
                        fence = opening
                    }
                    buffer += line
                    index = end
                    if index < text.endIndex {
                        buffer += "\n"
                        index = text.index(after: index)
                    }
                    continue
                }
            }

            if text[index] == "\\" {
                buffer.append(text[index])
                index = text.index(after: index)
                if index < text.endIndex {
                    buffer.append(text[index])
                    index = text.index(after: index)
                }
                continue
            }

            if text[index] == "`" {
                let ticks = remaining.prefix { $0 == "`" }
                if inlineTicks == 0, isStreaming || hasClosingBackticks(after: ticks, in: remaining) { inlineTicks = ticks.count }
                else if inlineTicks == ticks.count { inlineTicks = 0 }
                buffer += ticks
                index = text.index(index, offsetBy: ticks.count)
                continue
            }

            let tag = thinking ? "</think>" : "<think>"
            if inlineTicks == 0, remaining.hasPrefix(tag) {
                flush(isComplete: thinking)
                thinking.toggle()
                index = text.index(index, offsetBy: tag.count)
                continue
            }
            // Hold a delimiter split across deltas until the next chunk arrives.
            if inlineTicks == 0, isStreaming, remaining.first == "<", tag.hasPrefix(remaining) {
                break
            }
            buffer.append(text[index])
            index = text.index(after: index)
        }
        flush()
        // An empty reasoning block has nothing to show. Keep only the one still being
        // streamed, so the row can say it is thinking until its text arrives.
        return blocks.enumerated().filter { index, block in
            guard case .thinking(let text, let isComplete) = block,
                  text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return true }
            return isStreaming && !isComplete && index == blocks.count - 1
        }.map(\.element)
    }

    private static func hasClosingBackticks(after opening: Substring, in text: Substring) -> Bool {
        var remaining = text.dropFirst(opening.count)
        while let start = remaining.firstIndex(of: "`") {
            remaining = remaining[start...]
            let count = remaining.prefix { $0 == "`" }.count
            if count == opening.count { return true }
            remaining = remaining.dropFirst(count)
        }
        return false
    }
}
