import sys

def modify_ui_mod():
    path = "src/ui/mod.rs"
    with open(path, "r") as f:
        content = f.read()
    
    content = content.replace("use unicode_width::UnicodeWidthStr;", "use unicode_width::{UnicodeWidthStr, UnicodeWidthChar};")
    
    content = content.replace("            col += 1;", "            col += c.width().unwrap_or(1);")
    content = content.replace("                col += 1;", "                col += c.width().unwrap_or(1);")
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_mod()
