import Foundation

/// What a tool card says: a readable name for the engine's tool identifier and
/// one line of detail, matching the iOS transcript.
public enum ToolPresentation {
    public static func title(_ name: String) -> String {
        if let known = titles[name] { return known }
        if let mcp = mcpTitle(name) { return mcp }
        // Titles that are already prose ("Read src/main.rs") pass through.
        guard !name.contains(" ") else { return name }
        let words = name.split(whereSeparator: { $0 == "_" || $0 == "-" }).joined(separator: " ")
        guard let first = words.first else { return name }
        return first.uppercased() + words.dropFirst()
    }

    /// The line under a tool's title: the argument that says what it acted on,
    /// else the first line of its output. Nil when there is nothing to add.
    public static func detail(input: JSONValue?, output: String?) -> String? {
        if let input {
            for key in detailKeys {
                if let value = input[key]?.stringValue, let line = firstLine(value) { return clipped(line) }
            }
        }
        if let output, let line = firstLine(output) { return clipped(line) }
        return nil
    }

    private static let detailKeys = ["command", "path", "file_path", "pattern", "query", "url", "task", "name", "description"]

    private static func firstLine(_ text: String) -> String? {
        text.split(whereSeparator: \.isNewline).lazy
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty }
    }

    private static func clipped(_ line: String) -> String {
        line.count > 160 ? String(line.prefix(159)) + "…" : line
    }

    /// `mcp__<server>__<tool>` is how the engine names a tool from an MCP server.
    private static func mcpTitle(_ name: String) -> String? {
        guard name.hasPrefix("mcp__") else { return nil }
        let rest = name.dropFirst("mcp__".count)
        guard let separator = rest.range(of: "__") else { return nil }
        let server = rest[..<separator.lowerBound]
        let tool = rest[separator.upperBound...]
        guard !server.isEmpty, !tool.isEmpty else { return nil }
        return "\(server) · \(tool)"
    }

    private static let titles: [String: String] = [
        "view_file": "Read",
        "read_file": "Read",
        "get_project_map": "Project map",
        "replace_file_content": "Edit",
        "multi_replace_file_content": "Edit",
        "edit_file": "Edit",
        "write_to_file": "Write",
        "write_file_chunk": "Write",
        "delete_file": "Delete",
        "move_file": "Move",
        "copy_file": "Copy",
        "list_directory": "List",
        "glob": "List",
        "grep": "Search",
        "find_symbol": "Search",
        "search_web": "Web search",
        "read_url": "Read URL",
        "run_command": "Run",
        "manage_task": "Task",
        "ask_question": "Question",
        "todo_write": "Plan",
        "use_skill": "Skill",
        "list_skills": "Skills",
        "remember": "Remember",
        "recall_memory": "Recall",
        "forget_memory": "Forget",
        "git_status": "Git status",
        "git_diff": "Git diff",
        "git_add": "Git add",
        "git_commit": "Git commit",
        "spawn_agent": "Start agent",
        "delegate_task": "Delegate",
        "complete_task": "Finish",
    ]
}
