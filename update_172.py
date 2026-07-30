import sys

def modify_ui_markdown():
    path = "src/ui/markdown.rs"
    with open(path, "r") as f:
        content = f.read()
    
    old_code = """    let lines = render_markdown_uncached(content, width, show_picker);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(key, lines.clone());
    lines"""

    new_code = """    let lines = render_markdown_uncached(content, width, show_picker);
    let mut cache = cache.lock().unwrap();
    cache.insert(key, lines.clone());
    lines"""

    content = content.replace(old_code, new_code)
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_markdown()
