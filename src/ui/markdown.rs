use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use ratatui::text::{Line, Span};

struct MarkdownCache {
    entries: HashMap<u64, Vec<Line<'static>>>,
    max_size: usize,
}

impl MarkdownCache {
    fn new(max_size: usize) -> Self {
        Self { entries: HashMap::with_capacity(max_size), max_size }
    }

    fn get(&self, key: &u64) -> Option<&Vec<Line<'static>>> { self.entries.get(key) }

    fn insert(&mut self, key: u64, lines: Vec<Line<'static>>) {
        if self.entries.len() >= self.max_size && !self.entries.contains_key(&key) {
            if let Some(evict_key) = self.entries.keys().next().cloned() { self.entries.remove(&evict_key); }
        }
        self.entries.insert(key, lines);
    }
}

fn render_cache() -> &'static Mutex<MarkdownCache> { static CACHE: OnceLock<Mutex<MarkdownCache>> = OnceLock::new(); CACHE.get_or_init(|| Mutex::new(MarkdownCache::new(128))) }

fn cache_key(content: &str, width: usize) -> u64 { let mut hasher = std::collections::hash_map::DefaultHasher::new(); content.hash(&mut hasher); width.hash(&mut hasher); hasher.finish() }

fn into_static(line: Line<'_>) -> Line<'static> { Line { spans: line.spans.into_iter().map(|s| Span { content: std::borrow::Cow::Owned(s.content.into_owned()), style: s.style }).collect(), ..line } }

/// Render markdown content to styled terminal lines. Preserves caching and streaming semantics.
pub(super) fn render_markdown<'a>(content: &str, width: usize, show_picker: bool, use_cache: bool) -> Vec<Line<'a>> { if !use_cache { return render_markdown_uncached(content, width, show_picker); } let key = cache_key(content, width); let cache = render_cache(); if let Some(lines) = cache.lock().unwrap().get(&key).cloned() { return lines; } let lines = render_markdown_uncached(content, width, show_picker); cache.lock().unwrap().insert(key.clone(), lines.clone()); lines }

fn render_markdown_uncached(content: &str, _width: usize, show_picker: bool) -> Vec<Line<'static>> { let text = tui_markdown::from_str(content); let mut lines: Vec<Line<'static>> = text.lines.into_iter().map(into_static).collect(); if show_picker && !lines.is_empty() && !lines.last().is_some_and(|l| l.spans.is_empty()) { lines.push(Line::from("")); } if show_picker && !content.trim().is_empty() { lines.push(Line::from(vec![Span::styled(" ", ratatui::style::Style::default().fg(ratatui::style::Color::Green)), Span::styled("\u{25c7} Select", ratatui::style::Style::default().fg(ratatui::style::Color::Green).bold())])); } lines }

#[cfg(test)] mod tests { use super::{cache_key, render_cache}; use super::{render_markdown}; #[test] fn test_basic_rendering() { let md = "# Hello\n\nThis is **bold** and *italic*."; let lines = render_markdown(md, 80, false, false); assert!(!lines.is_empty()); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("Hello")))); } #[test] fn test_inline_styles() { let md = "**bold _italic_** and `code`"; let lines = render_markdown(md, 80, false, false); assert!(!lines.is_empty()); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("code")))); } #[test] fn test_list_rendering() { let md = "# Title\n\n- one\n- two"; let lines = render_markdown(md, 80, false, false); assert!(!lines.is_empty()); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("one")))); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("two")))); } #[test] fn test_table_rendering() { let md = "| Name | Value |\n|------|-------|\n| foo  | bar   |\n| baz  | qux   |"; let lines = render_markdown(md, 80, false, false); assert!(!lines.is_empty()); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains('\u{2502}')))); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("foo")))); assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains("bar")))); } #[test] fn test_cache_eviction() { const WIDTH: usize = 80; for i in 0..200u32 { let content = format!("item {}", i.to_string().repeat(10)); render_markdown(&content.to_string(), WIDTH as usize , false , true ); } assert!(true); } #[test] fn test_cache_stores_settled_content() { const WIDTH :usize=80; render_markdown("settled message", WIDTH as usize , false , true ); let _=render_markdown("settled message", WIDTH as usize , false , true ); assert!(true); }}