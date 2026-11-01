import Foundation

/// What a tool approval asks for, in the words and shapes a person reads:
/// the command to run, the edit as a diff, the file to delete. The engine
/// sends the tool's name and raw arguments; this turns them into a headline
/// and a body so the card never shows JSON for the tools it knows.
public struct PermissionPreview: Equatable, Sendable {
    public enum Body: Equatable, Sendable {
        /// A shell command, with the folder it runs in when not the project's.
        case command(String, cwd: String?)
        /// The edits to one file, as the transcript draws them.
        case edit(FileChange)
        /// Labelled values for tools without a dedicated shape.
        case fields([Field])
    }

    public struct Field: Equatable, Sendable, Identifiable {
        public var label: String
        public var value: String
        public var id: String { label }
    }

    /// A short line naming the action: "Run a command", "Edit calc.py".
    public var headline: String
    /// The model's own one-line reason for the call, when it gave one.
    public var summary: String?
    public var body: Body?
    /// Extra powers the call asks for, such as network access.
    public var notes: [String]

    public init(headline: String, summary: String? = nil, body: Body? = nil, notes: [String] = []) {
        self.headline = headline
        self.summary = summary
        self.body = body
        self.notes = notes
    }

    public init(tool: String, input: JSONValue?) {
        let summary = input?["summary"]?.stringValue.flatMap { $0.isEmpty ? nil : $0 }
        let path = input?["path"]?.stringValue ?? input?["file_path"]?.stringValue
        let name = path.map { URL(fileURLWithPath: $0).lastPathComponent }
        switch tool {
        case "run_command":
            var notes: [String] = []
            if input?["network_access"]?.boolValue == true { notes.append("Needs network access") }
            if let write = input?["filesystem_write_path"]?.stringValue { notes.append("Needs write access to \(write)") }
            if input?["detached"]?.boolValue == true { notes.append("Keeps running in the background") }
            self.init(headline: "Run a command", summary: summary,
                      body: input?["command"]?.stringValue.map { .command($0, cwd: input?["cwd"]?.stringValue) },
                      notes: notes)
        case "replace_file_content", "multi_replace_file_content", "write_to_file", "write_file_chunk":
            let change = path.flatMap { Self.change(tool: tool, path: $0, input: input) }
            let creates = tool == "write_to_file" && change?.isNewFile == true
            self.init(headline: "\(creates ? "Write" : "Edit") \(name ?? "a file")", summary: summary,
                      body: change.map { .edit($0) })
        case "delete_file":
            self.init(headline: "Delete \(name ?? "a file")", summary: summary,
                      body: path.map { .fields([Field(label: "File", value: $0)]) })
        case "move_file", "copy_file":
            let source = input?["src"]?.stringValue
            let from = source.map { URL(fileURLWithPath: $0).lastPathComponent }
            self.init(headline: "\(tool == "move_file" ? "Move" : "Copy") \(from ?? "a file")", summary: summary,
                      body: .fields([source.map { Field(label: "From", value: $0) },
                                     input?["dest"]?.stringValue.map { Field(label: "To", value: $0) }].compactMap { $0 }))
        default:
            self.init(headline: Self.words(tool), summary: summary, body: Self.fields(input))
        }
    }

    /// `git_commit` or `GitCommit` as "Git commit".
    static func words(_ tool: String) -> String {
        var out = ""
        for (index, char) in tool.enumerated() {
            if char == "_" || char == "-" {
                if !out.isEmpty, out.last != " " { out.append(" ") }
                continue
            }
            if char.isUppercase, index > 0, out.last != " " { out.append(" ") }
            out.append(out.isEmpty ? Character(char.uppercased()) : Character(char.lowercased()))
        }
        return out.isEmpty ? "Tool call" : out
    }

    /// Top-level string, number and boolean arguments as rows, nested ones
    /// as compact JSON; the summary is shown on its own.
    static func fields(_ input: JSONValue?) -> Body? {
        guard let object = input?.objectValue else { return nil }
        let rows = object.keys.sorted().compactMap { key -> Field? in
            guard key != "summary", let value = object[key] else { return nil }
            let text: String
            switch value {
            case .null: return nil
            case .string(let string): text = string
            case .bool(let bool): text = bool ? "Yes" : "No"
            case .number(let number): text = number.rounded() == number ? String(Int(number)) : String(number)
            default: text = value.prettyPrinted()
            }
            return Field(label: words(key), value: text)
        }
        return rows.isEmpty ? nil : .fields(rows)
    }

    /// The call's edits as one diff. Replacements carry no line numbers, so
    /// the gutter stays empty instead of guessing.
    static func change(tool: String, path: String, input: JSONValue?) -> FileChange? {
        var pairs: [(old: String?, new: String)] = []
        func pair(_ edit: JSONValue?) {
            guard let edit else { return }
            let old = edit["target_content"]?.stringValue ?? edit["old_string"]?.stringValue
            let new = edit["replacement_content"]?.stringValue ?? edit["new_string"]?.stringValue
            if let old, let new { pairs.append((old, new)) }
        }
        switch tool {
        case "write_to_file", "write_file_chunk":
            guard let content = input?["content"]?.stringValue else { return nil }
            // A chunk past the start appends; only a whole write is a new file's text.
            pairs.append((nil, content))
        case "multi_replace_file_content":
            input?["replacements"]?.arrayValue?.forEach { pair($0) }
        default:
            pair(input)
            input?["edits"]?.arrayValue?.forEach { pair($0) }
        }
        guard !pairs.isEmpty else { return nil }
        var rows: [DiffRow] = []
        for (index, edit) in pairs.enumerated() {
            if index > 0 { rows.append(.gap(nil)) }
            let lines = LineDiff.lines(old: LineDiff.split(edit.old ?? ""), new: LineDiff.split(edit.new))
            rows += FileChange.folded(lines).map { row in
                guard case .line(var line) = row else { return row }
                line.oldNumber = nil
                line.newNumber = nil
                return .line(line)
            }
        }
        let added = rows.filter { if case .line(let line) = $0 { line.kind == .added } else { false } }.count
        let removed = rows.filter { if case .line(let line) = $0 { line.kind == .removed } else { false } }.count
        return FileChange(path: path, rows: rows, added: added, removed: removed,
                          isNewFile: pairs.allSatisfy { $0.old == nil })
    }
}

public extension PermissionRequest {
    /// This approval as a person reads it.
    var preview: PermissionPreview { PermissionPreview(tool: title, input: input) }
}
