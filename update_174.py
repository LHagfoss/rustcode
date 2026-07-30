import sys

def modify_ui_mod():
    path = "src/ui/mod.rs"
    with open(path, "r") as f:
        content = f.read()
    
    content = content.replace("use std::hash::{Hash, Hasher};\nuse unicode_width::UnicodeWidthStr;", "use std::hash::{Hash, Hasher};\nuse unicode_width::UnicodeWidthStr;\n\nfn safe_byte_index(s: &str, char_pos: usize) -> usize {\n    s.char_indices().nth(char_pos).map(|(i, _)| i).unwrap_or(s.len())\n}")
    
    content = content.replace(
        "        let raw_prefix =\n            &state.input_buffer[..state.cursor_position.min(state.input_buffer.len())];",
        "        let safe_end = safe_byte_index(&state.input_buffer, state.cursor_position);\n        let raw_prefix = &state.input_buffer[..safe_end];"
    )
    
    content = content.replace(
        "state.input_buffer[..state.cursor_position.min(state.input_buffer.len())].ends_with('@')",
        "state.input_buffer[..safe_byte_index(&state.input_buffer, state.cursor_position)].ends_with('@')"
    )
    
    with open(path, "w") as f:
        f.write(content)

def modify_app_suggestion():
    path = "src/app/suggestion.rs"
    with open(path, "r") as f:
        content = f.read()
        
    content = content.replace(
        "pub fn get_at_word_query(input_buffer: &str, cursor_pos: usize) -> Option<(usize, String)> {\n    let pos = cursor_pos.min(input_buffer.len());",
        "fn safe_byte_index(s: &str, char_pos: usize) -> usize {\n    s.char_indices().nth(char_pos).map(|(i, _)| i).unwrap_or(s.len())\n}\n\npub fn get_at_word_query(input_buffer: &str, cursor_pos: usize) -> Option<(usize, String)> {\n    let pos = safe_byte_index(input_buffer, cursor_pos);"
    )
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_mod()
modify_app_suggestion()
