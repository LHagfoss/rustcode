use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
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
    let connection =
        Connection::open(&db_path).map_err(|e| format!("failed to open symbols database: {e}"))?;

    connection.execute(
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

    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
        [],
    )
    .map_err(|e| format!("failed to create index on symbol name: {e}"))?;

    Ok(connection)
}

fn extract_signature(node_text: &str) -> String {
    if let Some(idx) = node_text.find('{') {
        node_text[..idx].trim().to_string()
    } else {
        node_text.lines().next().unwrap_or("").trim().to_string()
    }
}

pub fn update_index(root_dir: &Path) -> Result<(), String> {
    let conn = init_db()?;
    let root_str = root_dir.to_string_lossy().to_string();

    // 1. Gather all files and track mtimes
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root_dir)
        .standard_filters(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |ext| ext == "rs") {
            let relative_path = path
                .strip_prefix(root_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let mtime_secs = mtime
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            files.push((path.to_path_buf(), relative_path, mtime_secs));
        }
    }

    // 2. Clear out any indexed files that no longer exist
    let mut stmt = conn
        .prepare("SELECT DISTINCT path FROM symbols WHERE project_root = ?")
        .map_err(|e| e.to_string())?;

    let existing_paths: Vec<String> = stmt
        .query_map([&root_str], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let new_paths_set: std::collections::HashSet<&str> =
        files.iter().map(|(_, rel, _)| rel.as_str()).collect();

    for old_path in existing_paths {
        if !new_paths_set.contains(old_path.as_str()) {
            let _ = conn.execute(
                "DELETE FROM symbols WHERE project_root = ? AND path = ?",
                params![&root_str, &old_path],
            );
        }
    }

    // 3. Incrementally parse and update changed files
    let rust_lang = tree_sitter_rust::LANGUAGE.into();
    let query_str = r#"
        (function_item name: (identifier) @name) @function
        (struct_item name: (type_identifier) @name) @struct
        (enum_item name: (type_identifier) @name) @enum
        (trait_item name: (type_identifier) @name) @trait
        (impl_item type: (_) @name) @impl
        (mod_item name: (identifier) @name) @module
    "#;
    let query = Query::new(&rust_lang, query_str)
        .map_err(|e| format!("failed to compile tree-sitter query: {e}"))?;

    for (abs_path, rel_path, mtime_secs) in files {
        // Check if we already indexed this version
        let already_indexed: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM symbols WHERE project_root = ? AND path = ? AND last_modified = ?)",
                params![&root_str, &rel_path, mtime_secs],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if already_indexed {
            continue;
        }

        // Parse and extract symbols
        let content = match std::fs::read_to_string(&abs_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut parser = Parser::new();
        if parser.set_language(&rust_lang).is_err() {
            continue;
        }

        let tree = match parser.parse(&content, None) {
            Some(t) => t,
            None => continue,
        };

        // Clear existing entries for this file before re-indexing
        let _ = conn.execute(
            "DELETE FROM symbols WHERE project_root = ? AND path = ?",
            params![&root_str, &rel_path],
        );

        let name_capture_idx = query
            .capture_names()
            .iter()
            .position(|r| *r == "name")
            .unwrap();

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());

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
                    if sibling_cap.index as usize == name_capture_idx {
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

                let _ = conn.execute(
                    "INSERT INTO symbols (project_root, path, name, kind, start_line, end_line, signature, last_modified)
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
                );
            }
        }
    }

    Ok(())
}

pub fn find_symbol(root_dir: &Path, query: &str) -> Result<Vec<SymbolInfo>, String> {
    let conn = init_db()?;
    let root_str = root_dir.to_string_lossy().to_string();

    let mut stmt = conn
        .prepare(
            "SELECT path, name, kind, start_line, end_line, signature
             FROM symbols
             WHERE project_root = ? AND name LIKE ?
             ORDER BY name ASC
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
    for row in rows {
        if let Ok(sym) = row {
            out.push(sym);
        }
    }

    Ok(out)
}

pub fn get_project_map(root_dir: &Path) -> Result<String, String> {
    let conn = init_db()?;
    let root_str = root_dir.to_string_lossy().to_string();

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
    for row in rows {
        if let Ok((path, name, kind, signature)) = row {
            map_by_file
                .entry(path)
                .or_insert_with(Vec::new)
                .push((name, kind, signature));
        }
    }

    let mut out = String::new();
    if map_by_file.is_empty() {
        return Ok("Project Map is empty. Ensure codebase contains parsed .rs files.".to_string());
    }

    out.push_str("Codebase Project Map:\n");
    for (path, symbols) in map_by_file {
        out.push_str(&format!("\n{}:\n", path));
        for (name, kind, signature) in symbols {
            let compressed = if signature.len() > 120 {
                format!("{}...", &signature[..117])
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
    fn test_symbols_indexer_and_search() {
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

        let file_path = dir.join("main.rs");
        let content = r#"
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
        std::fs::write(&file_path, content).unwrap();

        update_index(&dir).unwrap();

        let results = find_symbol(&dir, "Config").unwrap();
        assert!(!results.is_empty(), "should find Config symbol");
        let struct_match = results.iter().find(|s| s.kind == "struct").unwrap();
        assert_eq!(struct_match.name, "Config");
        assert_eq!(struct_match.path, "main.rs");
        assert_eq!(struct_match.signature, "struct Config");

        let fn_results = find_symbol(&dir, "run_server").unwrap();
        assert!(!fn_results.is_empty(), "should find run_server symbol");
        let fn_match = fn_results.iter().find(|s| s.kind == "function").unwrap();
        assert_eq!(fn_match.name, "run_server");
        assert_eq!(fn_match.signature, "fn run_server() -> Result<(), String>");

        let map = get_project_map(&dir).unwrap();
        assert!(map.contains("main.rs:"), "map should contain file path");
        assert!(
            map.contains("Config [struct]"),
            "map should contain Config struct"
        );
        assert!(
            map.contains("run_server [function]"),
            "map should contain run_server"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
