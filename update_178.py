import sys

def modify_ui_mod():
    path = "src/ui/mod.rs"
    with open(path, "r") as f:
        content = f.read()
    
    new_func = """use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;

thread_local! {
    static MARKER_CACHE: RefCell<(u64, String)> = RefCell::new((0, String::new()));
}

fn collapse_image_markers(text: &str) -> String {
    const MARK: &str = "![image](file://";
    if !text.contains(MARK) {
        return text.to_string();
    }
    
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let hash = hasher.finish();

    MARKER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.0 == hash {
            return cache.1.clone();
        }
        let mut out = String::new();
        let mut rest = text;
        let mut n = 0;
        while let Some(start) = rest.find(MARK) {
            out.push_str(&rest[..start]);
            let after = &rest[start + MARK.len()..];
            if let Some(close) = after.find(')') {
                n += 1;
                out.push_str(&format!("[Image #{n}]"));
                rest = &after[close + 1..];
            } else {
                out.push_str(&rest[start..]);
                *cache = (hash, out.clone());
                return out;
            }
        }
        out.push_str(rest);
        *cache = (hash, out.clone());
        out
    })
}"""

    import re
    old_func_pattern = re.compile(r"fn collapse_image_markers\(text: &str\) -> String \{.*?^\}", re.MULTILINE | re.DOTALL)
    content = old_func_pattern.sub(new_func, content)
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_mod()
