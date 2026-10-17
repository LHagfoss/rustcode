import Foundation

/// A small lexical highlighter for code in diffs and code blocks: comments,
/// strings, numbers, keywords and type names, one line at a time. It is not a
/// parser; it only has to make code easier to read at a glance.
public struct SyntaxHighlighter: Sendable {
    public enum Kind: Equatable, Sendable { case keyword, string, comment, number, type }

    public struct Token: Equatable, Sendable {
        public var range: Range<String.Index>
        public var kind: Kind
    }

    let keywords: Set<String>
    let lineComments: [String]
    let blockComment: (open: String, close: String)?
    /// Rust's `'` starts a lifetime unless it closes as a char literal.
    let apostropheIsCharOnly: Bool

    /// The highlighter for a file name or a code fence's language, or nil
    /// when the language is unknown.
    public init?(language: String) {
        let key = language.lowercased().split(separator: ".").last.map(String.init) ?? ""
        guard let family = Self.families.first(where: { $0.names.contains(key) }) else { return nil }
        keywords = family.keywords
        lineComments = family.lineComments
        blockComment = family.blockComment
        apostropheIsCharOnly = family.names.contains("rs")
    }

    /// The language of a path, by its extension or well-known name.
    public init?(path: String) {
        let name = (path as NSString).lastPathComponent.lowercased()
        if ["makefile", "dockerfile", "gemfile", "rakefile"].contains(name) {
            self.init(language: name == "makefile" ? "sh" : name == "dockerfile" ? "sh" : "rb")
        } else {
            let ext = (name as NSString).pathExtension
            guard !ext.isEmpty else { return nil }
            self.init(language: ext)
        }
    }

    /// The highlighted ranges of one line. `inBlockComment` carries an open
    /// `/* … */` from one line to the next.
    public func tokens(in line: String, inBlockComment: inout Bool) -> [Token] {
        var tokens: [Token] = []
        var index = line.startIndex
        let end = line.endIndex

        func starts(_ prefix: String, at position: String.Index) -> Bool {
            line[position...].hasPrefix(prefix)
        }

        if inBlockComment, let block = blockComment {
            if let close = line.range(of: block.close) {
                tokens.append(Token(range: line.startIndex..<close.upperBound, kind: .comment))
                index = close.upperBound
                inBlockComment = false
            } else {
                return [Token(range: line.startIndex..<end, kind: .comment)]
            }
        }

        while index < end {
            let character = line[index]
            if lineComments.contains(where: { starts($0, at: index) }) {
                tokens.append(Token(range: index..<end, kind: .comment))
                break
            }
            if let block = blockComment, starts(block.open, at: index) {
                let searchStart = line.index(index, offsetBy: block.open.count)
                if let close = line[searchStart...].range(of: block.close) {
                    tokens.append(Token(range: index..<close.upperBound, kind: .comment))
                    index = close.upperBound
                } else {
                    tokens.append(Token(range: index..<end, kind: .comment))
                    inBlockComment = true
                    break
                }
                continue
            }
            if character == "\"" || character == "'" || character == "`" {
                let start = index
                var cursor = line.index(after: index)
                while cursor < end, line[cursor] != character {
                    if line[cursor] == "\\" { cursor = line.index(after: cursor) }
                    if cursor < end { cursor = line.index(after: cursor) }
                }
                if cursor < end { cursor = line.index(after: cursor) }
                // In Rust a `'` that does not close as a char literal is a lifetime.
                if character == "'", apostropheIsCharOnly,
                   line.distance(from: start, to: cursor) > 4 || line[line.index(before: cursor)] != "'" {
                    index = line.index(after: start)
                    continue
                }
                tokens.append(Token(range: start..<cursor, kind: .string))
                index = cursor
                continue
            }
            if character.isNumber {
                let previous = index > line.startIndex ? line[line.index(before: index)] : " "
                let start = index
                while index < end, line[index].isNumber || line[index].isLetter || line[index] == "_" || line[index] == "." {
                    index = line.index(after: index)
                }
                if !(previous.isLetter || previous == "_") {
                    tokens.append(Token(range: start..<index, kind: .number))
                }
                continue
            }
            if character.isLetter || character == "_" || character == "@" || character == "#" {
                let start = index
                index = line.index(after: index)
                while index < end, line[index].isLetter || line[index].isNumber || line[index] == "_" {
                    index = line.index(after: index)
                }
                let word = String(line[start..<index])
                if keywords.contains(word) {
                    tokens.append(Token(range: start..<index, kind: .keyword))
                } else if let first = word.first, first.isUppercase, word.count > 1 {
                    tokens.append(Token(range: start..<index, kind: .type))
                }
                continue
            }
            index = line.index(after: index)
        }
        return tokens
    }

    private struct Family {
        let names: Set<String>
        let keywords: Set<String>
        var lineComments = ["//"]
        var blockComment: (open: String, close: String)? = ("/*", "*/")
    }

    private static let families: [Family] = [
        Family(names: ["swift"], keywords: [
            "actor", "any", "as", "async", "await", "break", "case", "catch", "class", "continue", "default", "defer",
            "do", "else", "enum", "extension", "fallthrough", "false", "fileprivate", "for", "func", "guard", "if",
            "import", "in", "init", "inout", "internal", "is", "let", "nil", "nonisolated", "open", "operator",
            "private", "protocol", "public", "repeat", "rethrows", "return", "self", "Self", "some", "static",
            "struct", "subscript", "super", "switch", "throw", "throws", "true", "try", "typealias", "var",
            "where", "while", "@MainActor", "@Published", "@State", "@Binding", "@ObservedObject",
            "@StateObject", "@Environment", "@ViewBuilder", "@escaping", "@available", "#if", "#else", "#endif",
        ]),
        Family(names: ["rs", "rust"], keywords: [
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
            "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
            "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            "Some", "None", "Ok", "Err",
        ]),
        Family(names: ["ts", "tsx", "js", "jsx", "mjs", "cjs", "typescript", "javascript"], keywords: [
            "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do",
            "else", "export", "extends", "false", "finally", "for", "from", "function", "if", "implements", "import",
            "in", "instanceof", "interface", "let", "new", "null", "of", "private", "protected", "public", "readonly",
            "return", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined",
            "var", "void", "while", "yield",
        ]),
        Family(names: ["py", "python"], keywords: [
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
            "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None",
            "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while", "with", "yield", "self",
        ], lineComments: ["#"], blockComment: nil),
        Family(names: ["go"], keywords: [
            "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "false", "for",
            "func", "go", "goto", "if", "import", "interface", "map", "nil", "package", "range", "return", "select",
            "struct", "switch", "true", "type", "var",
        ]),
        Family(names: ["c", "h", "cc", "cpp", "hpp", "m", "mm", "java", "kt", "kts", "cs", "kotlin", "java", "objc"],
               keywords: [
            "auto", "bool", "break", "case", "catch", "char", "class", "const", "continue", "default", "delete", "do",
            "double", "else", "enum", "extern", "false", "final", "float", "for", "fun", "if", "import", "int",
            "interface", "long", "namespace", "new", "null", "nullptr", "override", "package", "private",
            "protected", "public", "return", "short", "static", "struct", "switch", "this", "throw", "true", "try",
            "typedef", "union", "unsigned", "using", "val", "var", "void", "while", "#include", "#define",
        ]),
        Family(names: ["sh", "bash", "zsh", "shell", "console", "fish"], keywords: [
            "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "case", "esac", "function",
            "return", "export", "local", "set", "unset", "echo", "cd", "exit",
        ], lineComments: ["#"], blockComment: nil),
        Family(names: ["rb", "ruby"], keywords: [
            "def", "end", "class", "module", "if", "elsif", "else", "unless", "while", "do", "return", "true",
            "false", "nil", "self", "require", "yield", "begin", "rescue", "ensure",
        ], lineComments: ["#"], blockComment: nil),
        Family(names: ["toml", "yaml", "yml", "ini", "conf"], keywords: ["true", "false", "null"],
               lineComments: ["#"], blockComment: nil),
        Family(names: ["json", "jsonc"], keywords: ["true", "false", "null"]),
        Family(names: ["sql"], keywords: [
            "SELECT", "FROM", "WHERE", "INSERT", "INTO", "UPDATE", "DELETE", "CREATE", "TABLE", "JOIN", "ON", "AND",
            "OR", "NOT", "NULL", "AS", "ORDER", "BY", "GROUP", "LIMIT", "VALUES", "SET",
            "select", "from", "where", "insert", "into", "update", "delete", "create", "table", "join", "on", "and",
            "or", "not", "null", "as", "order", "by", "group", "limit", "values", "set",
        ], lineComments: ["--"], blockComment: ("/*", "*/")),
        Family(names: ["css", "scss"], keywords: ["important", "media", "import"]),
    ]
}
