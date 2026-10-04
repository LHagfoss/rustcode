use rusqlite::{Connection, params};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolInfo {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
}

fn get_db_path() -> PathBuf {
    crate::config::get_config_dir()
        .map(|d| d.join("symbols.db"))
        .unwrap_or_else(|| PathBuf::from("symbols.db"))
}

pub fn init_db() -> Result<Connection, String> {
    let db_path = get_db_path();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let connection = open_writable_db(&db_path).or_else(|_| {
        let fallback = std::env::temp_dir().join("rustcode").join("symbols.db");
        if let Some(parent) = fallback.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        open_writable_db(&fallback)
    })?;

    connection
        .execute(
            "CREATE TABLE IF NOT EXISTS symbols (
            project_root TEXT NOT NULL,
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            signature TEXT,
            last_modified INTEGER NOT NULL,
            PRIMARY KEY (project_root, path, name, kind, start_line)
        )",
            [],
        )
        .map_err(|e| format!("failed to create symbols table: {e}"))?;

    connection
        .execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
            [],
        )
        .map_err(|e| format!("failed to create index on symbol name: {e}"))?;

    Ok(connection)
}

/// Open a SQLite database only when it can accept writes. The normal location
/// is the user's config directory; a temporary fallback keeps symbol search
/// usable in read-only or sandboxed environments.
fn open_writable_db(path: &Path) -> Result<Connection, String> {
    let connection =
        Connection::open(path).map_err(|e| format!("failed to open symbols database: {e}"))?;
    connection
        // BEGIN IMMEDIATE alone can succeed against an existing read-only file;
        // changing this harmless header field proves the database is writable.
        .execute_batch("PRAGMA user_version = 1;")
        .map_err(|e| format!("symbols database is not writable: {e}"))?;
    Ok(connection)
}

fn extract_signature(node_text: &str) -> String {
    if let Some(idx) = node_text.find('{') {
        node_text[..idx].trim().to_string()
    } else {
        node_text.lines().next().unwrap_or("").trim().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedLanguage {
    Rust,
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
}

impl SupportedLanguage {
    pub fn for_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    pub fn tree_sitter_language(&self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    pub fn query_str(&self) -> &'static str {
        match self {
            Self::Rust => {
                r#"
                (function_item name: (identifier) @name) @function
                (struct_item name: (type_identifier) @name) @struct
                (enum_item name: (type_identifier) @name) @enum
                (trait_item name: (type_identifier) @name) @trait
                (impl_item type: (_) @name) @impl
                (mod_item name: (identifier) @name) @module
            "#
            }
            Self::Python => {
                r#"
                (function_definition name: (identifier) @name) @function
                (class_definition name: (identifier) @name) @class
            "#
            }
            Self::TypeScript | Self::Tsx => {
                r#"
                (function_declaration name: (identifier) @name) @function
                (class_declaration name: (type_identifier) @name) @class
                (interface_declaration name: (type_identifier) @name) @interface
                (type_alias_declaration name: (type_identifier) @name) @type
                (method_definition name: (property_identifier) @name) @method
            "#
            }
            Self::JavaScript => {
                r#"
                (function_declaration name: (identifier) @name) @function
                (class_declaration name: (identifier) @name) @class
                (method_definition name: (property_identifier) @name) @method
            "#
            }
            Self::Go => {
                r#"
                (function_declaration name: (identifier) @name) @function
                (method_declaration name: (field_identifier) @name) @method
                (type_spec name: (type_identifier) @name) @type
            "#
            }
        }
    }
}

#[derive(Default)]
struct IndexState {
    generation: u64,
    discovery: u64,
    git_revision: u64,
    files: BTreeMap<PathBuf, crate::workspace_intelligence::Stamp>,
}
static INDEXES: LazyLock<Mutex<HashMap<PathBuf, IndexState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn update_index(root_dir: &Path) -> Result<(), String> {
    let snapshot = crate::workspace_intelligence::snapshot(root_dir)?;
    let mut indexes = INDEXES.lock().unwrap_or_else(|e| e.into_inner());
    let state = indexes.entry(snapshot.root.clone()).or_default();
    let git_revision = crate::workspace_intelligence::git_revision(&snapshot.root);
    if state.generation == snapshot.generation && state.git_revision == git_revision {
        return Ok(());
    }
    let root_dir = snapshot.root.as_path();
    let root_str = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf())
        .to_string_lossy()
        .to_string();
    let mut conn = init_db()?;
    let conn = conn.transaction().map_err(|e| e.to_string())?;
    let mut current_files = BTreeMap::new();
    if state.discovery != snapshot.discovery || state.git_revision != git_revision {
        // Only directory topology changes require discovery. Preserve the
        // existing ignore policy; source edits require no project walk.
        for entry in ignore::WalkBuilder::new(root_dir)
            .standard_filters(true)
            .build()
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(SupportedLanguage::for_extension)
                    .is_some()
                && let Some(stamp) = snapshot.files.get(path)
            {
                current_files.insert(path.to_path_buf(), stamp.clone());
            }
        }
    } else {
        for path in state.files.keys() {
            if let Some(stamp) = snapshot.files.get(path) {
                current_files.insert(path.clone(), stamp.clone());
            }
        }
    }
    // On first use also remove rows left by earlier processes. Files without
    // symbols are represented in IndexState, so they are not reparsed forever.
    let existing_paths: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT path FROM symbols WHERE project_root = ?")
            .map_err(|e| e.to_string())?;
        stmt.query_map([&root_str], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    for old_path in existing_paths {
        if !current_files.contains_key(&root_dir.join(&old_path)) {
            conn.execute(
                "DELETE FROM symbols WHERE project_root = ? AND path = ?",
                params![&root_str, &old_path],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let files = current_files
        .iter()
        .filter_map(|(path, stamp)| {
            if state.files.get(path) == Some(stamp) {
                return None;
            }
            let lang = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(SupportedLanguage::for_extension)?;
            let relative = path
                .strip_prefix(root_dir)
                .ok()?
                .to_string_lossy()
                .to_string();
            Some((path.clone(), relative, snapshot.generation as i64, lang))
        })
        .collect::<Vec<_>>();

    // 3. Incrementally parse and update changed files
    let mut parser = Parser::new();
    let mut current_lang: Option<SupportedLanguage> = None;
    let mut query_cache: std::collections::HashMap<SupportedLanguage, (Query, usize)> =
        std::collections::HashMap::new();
    let mut cursor = QueryCursor::new();

    for (abs_path, rel_path, mtime_secs, lang) in files {
        let ts_lang = lang.tree_sitter_language();
        if current_lang != Some(lang) {
            if parser.set_language(&ts_lang).is_err() {
                continue;
            }
            current_lang = Some(lang);
        }

        let (query, name_capture_idx) = match query_cache.entry(lang) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let query_str = lang.query_str();
                match Query::new(&ts_lang, query_str) {
                    Ok(q) => {
                        let name_idx = match q.capture_names().iter().position(|r| *r == "name") {
                            Some(idx) => idx,
                            None => continue,
                        };
                        e.insert((q, name_idx))
                    }
                    Err(e) => {
                        eprintln!("failed to compile tree-sitter query for {:?}: {e}", lang);
                        continue;
                    }
                }
            }
        };

        // Parse and extract symbols
        let content = match std::fs::read_to_string(&abs_path) {
            Ok(c) => c,
            Err(e) => return Err(format!("cannot index {}: {e}", abs_path.display())),
        };

        let tree = match parser.parse(&content, None) {
            Some(t) => t,
            None => continue,
        };

        // Clear existing entries for this file before re-indexing
        conn.execute(
            "DELETE FROM symbols WHERE project_root = ? AND path = ?",
            params![&root_str, &rel_path],
        )
        .map_err(|e| e.to_string())?;

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            for cap in m.captures {
                let node = cap.node;
                let capture_name = &query.capture_names()[cap.index as usize];

                // We only want to process the main structural node itself, not just the @name sub-node
                if *capture_name == "name" {
                    continue;
                }

                // Locate the @name sibling or child node to get the symbol's name
                let mut name = String::new();
                for sibling_cap in m.captures {
                    if sibling_cap.index as usize == *name_capture_idx {
                        let name_node = sibling_cap.node;
                        if name_node.start_byte() >= node.start_byte()
                            && name_node.end_byte() <= node.end_byte()
                        {
                            name = name_node
                                .utf8_text(content.as_bytes())
                                .unwrap_or("")
                                .to_string();
                            break;
                        }
                    }
                }

                if name.is_empty() {
                    continue;
                }

                let kind = capture_name.to_string();
                let start_line = node.start_position().row + 1; // 1-indexed
                let end_line = node.end_position().row + 1;
                let node_text = node.utf8_text(content.as_bytes()).unwrap_or("");
                let signature = extract_signature(node_text);

                conn.execute(
                    "INSERT OR IGNORE INTO symbols (project_root, path, name, kind, start_line, end_line, signature, last_modified)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        &root_str,
                        &rel_path,
                        &name,
                        &kind,
                        start_line as i64,
                        end_line as i64,
                        &signature,
                        mtime_secs
                    ],
                ).map_err(|e| e.to_string())?;
            }
        }
    }

    conn.commit().map_err(|e| e.to_string())?;
    state.generation = snapshot.generation;
    state.discovery = snapshot.discovery;
    state.git_revision = git_revision;
    state.files = current_files;
    Ok(())
}

pub fn find_symbol(root_dir: &Path, query: &str) -> Result<Vec<SymbolInfo>, String> {
    let conn = init_db()?;
    let root_str = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf())
        .to_string_lossy()
        .to_string();

    let mut stmt = conn
        .prepare(
            "SELECT path, name, kind, start_line, end_line, signature
             FROM symbols
             WHERE project_root = ? AND name LIKE ?
             ORDER BY name ASC, path ASC, start_line ASC
             LIMIT 50",
        )
        .map_err(|e| e.to_string())?;

    let sql_query = format!("%{query}%");
    let rows = stmt
        .query_map([&root_str, &sql_query], |row| {
            Ok(SymbolInfo {
                path: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
                signature: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for sym in rows.flatten() {
        out.push(sym);
    }

    Ok(out)
}

/// Fuzzy-rank symbols for a query: exact/prefix/substring outrank weak
/// matches; ties break by name, then path. Returns at most `limit` items.
pub fn fuzzy_filter_symbols(symbols: &[SymbolInfo], query: &str, limit: usize) -> Vec<SymbolInfo> {
    let normalized: String = crate::tools::normalize_alphanumeric_lower(query);
    if normalized.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(&SymbolInfo, usize)> = symbols
        .iter()
        .filter_map(|sym| {
            let name: String = crate::tools::normalize_alphanumeric_lower(&sym.name);
            let score = if name == normalized {
                0
            } else if name.starts_with(normalized.as_str()) {
                1
            } else if name.contains(normalized.as_str()) {
                2
            } else {
                return None;
            };
            Some((sym, score))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then(a.0.name.cmp(&b.0.name))
            .then(a.0.path.cmp(&b.0.path))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(sym, _)| sym.clone()).collect()
}

pub fn get_project_map(root_dir: &Path) -> Result<String, String> {
    let conn = init_db()?;
    let root_str = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf())
        .to_string_lossy()
        .to_string();

    let mut stmt = conn
        .prepare(
            "SELECT path, name, kind, signature
             FROM symbols
             WHERE project_root = ?
             ORDER BY path ASC, start_line ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([&root_str], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut map_by_file = std::collections::BTreeMap::new();
    for (path, name, kind, signature) in rows.flatten() {
        map_by_file
            .entry(path)
            .or_insert_with(Vec::new)
            .push((name, kind, signature));
    }

    let mut out = String::new();
    if map_by_file.is_empty() {
        return Ok("Project Map is empty. Ensure codebase contains parsed source files (e.g. .rs, .py, .ts, .js, .go).".to_string());
    }

    out.push_str("Codebase Project Map:\n");
    for (path, symbols) in map_by_file {
        out.push_str(&format!("\n{}:\n", path));
        for (name, kind, signature) in symbols {
            let compressed = if signature.len() > 120 {
                format!("{}...", signature.chars().take(117).collect::<String>())
            } else {
                signature
            };
            out.push_str(&format!("  {} [{}] {}\n", name, kind, compressed));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual repeated-symbol benchmark"]
    fn benchmark_repeated_symbol_refresh() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..1000 {
            std::fs::write(
                dir.path().join(format!("module_{n}.rs")),
                format!("pub fn symbol_{n}() {{}}\n"),
            )
            .unwrap();
        }
        let cold = std::time::Instant::now();
        update_index(dir.path()).unwrap();
        eprintln!("symbol benchmark cold_ms={}", cold.elapsed().as_millis());
        let repeated = std::time::Instant::now();
        for _ in 0..20 {
            update_index(dir.path()).unwrap();
            assert!(!find_symbol(dir.path(), "symbol_42").unwrap().is_empty());
        }
        eprintln!(
            "symbol benchmark repeated_20_ms={}",
            repeated.elapsed().as_millis()
        );
    }

    #[test]
    fn quick_external_symbol_edit_is_visible() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "pub fn first() {}\n").unwrap();
        update_index(dir.path()).unwrap();
        std::fs::write(&file, "pub fn other() {}\n").unwrap();
        update_index(dir.path()).unwrap();
        assert!(find_symbol(dir.path(), "first").unwrap().is_empty());
        assert_eq!(find_symbol(dir.path(), "other").unwrap().len(), 1);
    }

    #[test]
    fn incremental_symbols_detect_create_delete_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.rs");
        std::fs::write(&empty, "// no symbols").unwrap();
        update_index(dir.path()).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let before = INDEXES.lock().unwrap().get(&root).unwrap().generation;
        update_index(dir.path()).unwrap();
        assert_eq!(
            before,
            INDEXES.lock().unwrap().get(&root).unwrap().generation
        );
        let file = dir.path().join("new.rs");
        std::fs::write(&file, "pub fn added() {}\n").unwrap();
        update_index(dir.path()).unwrap();
        assert_eq!(find_symbol(dir.path(), "added").unwrap().len(), 1);
        std::fs::remove_file(file).unwrap();
        update_index(dir.path()).unwrap();
        assert!(find_symbol(dir.path(), "added").unwrap().is_empty());
    }

    #[test]
    fn test_symbols_indexer_and_search_polyglot() {
        let dir = std::env::temp_dir()
            .join("rustcode-symbols-tests")
            .join(format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Rust file
        let rs_file = dir.join("main.rs");
        let rs_content = r#"
            struct Config {
                port: u16,
            }

            impl Config {
                fn new() -> Self {
                    Config { port: 8080 }
                }
            }

            fn run_server() -> Result<(), String> {
                Ok(())
            }
        "#;
        std::fs::write(&rs_file, rs_content).unwrap();

        // 2. Python file
        let py_file = dir.join("service.py");
        let py_content = r#"
class AuthManager:
    def __init__(self, secret: str):
        self.secret = secret

def authenticate_user(token: str) -> bool:
    return True
"#;
        std::fs::write(&py_file, py_content).unwrap();

        // 3. TypeScript file
        let ts_file = dir.join("client.ts");
        let ts_content = r#"
interface UserSession {
    id: string;
    token: string;
}

type AuthCallback = (session: UserSession) => void;

class ApiClient {
    connect(url: string): void {
        console.log(url);
    }
}

function createClient(): ApiClient {
    return new ApiClient();
}
"#;
        std::fs::write(&ts_file, ts_content).unwrap();

        // 4. Go file
        let go_file = dir.join("handler.go");
        let go_content = r#"
package main

type Router struct {
    routes []string
}

func HandleRequest(r *Router) error {
    return nil
}
"#;
        std::fs::write(&go_file, go_content).unwrap();

        update_index(&dir).unwrap();

        // Check Rust symbols
        let rs_results = find_symbol(&dir, "Config").unwrap();
        assert!(!rs_results.is_empty(), "should find Config symbol");
        let struct_match = rs_results.iter().find(|s| s.kind == "struct").unwrap();
        assert_eq!(struct_match.name, "Config");
        assert_eq!(struct_match.path, "main.rs");

        // Check Python symbols
        let py_results = find_symbol(&dir, "AuthManager").unwrap();
        assert!(
            !py_results.is_empty(),
            "should find AuthManager python class"
        );
        let py_class = py_results.iter().find(|s| s.kind == "class").unwrap();
        assert_eq!(py_class.name, "AuthManager");
        assert_eq!(py_class.path, "service.py");

        let py_fn = find_symbol(&dir, "authenticate_user").unwrap();
        assert!(
            !py_fn.is_empty(),
            "should find authenticate_user python function"
        );

        // Check TypeScript symbols
        let ts_iface = find_symbol(&dir, "UserSession").unwrap();
        assert!(!ts_iface.is_empty(), "should find UserSession TS interface");
        assert_eq!(ts_iface[0].kind, "interface");

        let ts_type = find_symbol(&dir, "AuthCallback").unwrap();
        assert!(!ts_type.is_empty(), "should find AuthCallback TS type");

        let ts_class = find_symbol(&dir, "ApiClient").unwrap();
        assert!(!ts_class.is_empty(), "should find ApiClient TS class");

        // Check Go symbols
        let go_type = find_symbol(&dir, "Router").unwrap();
        assert!(!go_type.is_empty(), "should find Router Go type");

        let go_fn = find_symbol(&dir, "HandleRequest").unwrap();
        assert!(!go_fn.is_empty(), "should find HandleRequest Go function");

        // Check overall project map
        let map = get_project_map(&dir).unwrap();
        assert!(map.contains("main.rs:"), "map should contain main.rs");
        assert!(map.contains("service.py:"), "map should contain service.py");
        assert!(map.contains("client.ts:"), "map should contain client.ts");
        assert!(map.contains("handler.go:"), "map should contain handler.go");
        assert!(
            map.contains("AuthManager [class]"),
            "map should contain AuthManager"
        );
        assert!(
            map.contains("UserSession [interface]"),
            "map should contain UserSession"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_symbol(name: &str, path: &str) -> SymbolInfo {
        SymbolInfo {
            path: path.to_string(),
            name: name.to_string(),
            kind: "fn".to_string(),
            start_line: 0,
            end_line: 1,
            signature: String::new(),
        }
    }

    #[test]
    fn fuzzy_filter_ranks_exact_before_prefix_before_substring() {
        let symbols = vec![
            test_symbol("my_handler", "b.rs"),
            test_symbol("handler", "a.rs"),
            test_symbol("handle_request", "c.rs"),
        ];
        let ranked = fuzzy_filter_symbols(&symbols, "handler", 10);
        assert_eq!(ranked[0].name, "handler");
        assert!(ranked.iter().any(|s| s.name == "handle_request"));
        assert_eq!(fuzzy_filter_symbols(&symbols, "", 10).len(), 0);
        assert_eq!(fuzzy_filter_symbols(&symbols, "handler", 1).len(), 1);
    }
}
