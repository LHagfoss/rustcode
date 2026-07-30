import sys

def modify_ui_tool_result():
    path = "src/ui/tool_result.rs"
    with open(path, "r") as f:
        content = f.read()
    
    old_code = """    let embedded_diff = result
        .split_once("```diff")
        .and_then(|(_, body)| body.split_once("```").map(|(diff, _)| diff.trim()))
        .filter(|diff| !diff.is_empty());

    let lines = if let Some(diff) = embedded_diff {
        // The action row plus the structured diff is the useful confirmation;
        // the filesystem acknowledgement is redundant transcript noise.
        render_unified_diff(diff, width, show_picker)
    } else {
        vec![Line::from(Span::styled(
            format!("  {icon} {summary}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        ))]
    };
    lines"""

    new_code = """    let diffs: Vec<&str> = result
        .split("```diff")
        .skip(1)
        .filter_map(|block| block.split_once("```").map(|(diff, _)| diff.trim()))
        .filter(|diff| !diff.is_empty())
        .collect();

    let mut lines = Vec::new();
    if !diffs.is_empty() {
        for diff in diffs {
            lines.extend(render_unified_diff(diff, width, show_picker));
        }
    } else {
        lines.push(Line::from(Span::styled(
            format!("  {icon} {summary}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        )));
    }
    lines"""

    content = content.replace(old_code, new_code)
    
    with open(path, "w") as f:
        f.write(content)

modify_ui_tool_result()
