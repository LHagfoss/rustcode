import sys

def modify_ui_markdown():
    path = "src/ui/markdown.rs"
    with open(path, "r") as f:
        content = f.read()
    
    old_code = """        for word in text.split_inclusive(|c: char| c.is_whitespace()) {
            let word_width = word.width();
            if current_width > 0 && current_width + word_width > width {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            current.push(Span::styled(word.to_string(), style));
            current_width += word_width;
        }"""

    new_code = """        for word in text.split_inclusive(|c: char| c.is_whitespace()) {
            let word_width = word.width();
            if word_width > width {
                use unicode_width::UnicodeWidthChar;
                for ch in word.chars() {
                    let ch_width = ch.width().unwrap_or(1);
                    if current_width + ch_width > width && current_width > 0 {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                    }
                    current.push(Span::styled(ch.to_string(), style));
                    current_width += ch_width;
                }
            } else {
                if current_width > 0 && current_width + word_width > width {
                    lines.push(Line::from(std::mem::take(&mut current)));
                    current_width = 0;
                }
                current.push(Span::styled(word.to_string(), style));
                current_width += word_width;
            }
        }"""

    content = content.replace(old_code, new_code)
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_markdown()
