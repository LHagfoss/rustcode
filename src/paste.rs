const PASTE_PREFIX: &str = "<!--PASTE:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PasteMarker<'a> {
    pub(crate) end: usize,
    pub(crate) char_count: usize,
    pub(crate) payload: &'a str,
}

/// Parse a marker at `start`, using its declared character count instead of
/// searching for the first closing delimiter inside the pasted payload.
pub(crate) fn parse_at(text: &str, start: usize) -> Option<PasteMarker<'_>> {
    let after_prefix = text.get(start..)?.strip_prefix(PASTE_PREFIX)?;
    let (count_text, payload_with_end) = after_prefix.split_once(':')?;
    let char_count = count_text.parse().ok()?;
    let payload_end = payload_with_end
        .char_indices()
        .nth(char_count)
        .map(|(index, _)| index)
        .unwrap_or_else(|| payload_with_end.len());
    if payload_with_end[payload_end..].starts_with("-->")
        && payload_with_end[..payload_end].chars().count() == char_count
    {
        let end = start + PASTE_PREFIX.len() + count_text.len() + 1 + payload_end + 3;
        Some(PasteMarker {
            end,
            char_count,
            payload: &payload_with_end[..payload_end],
        })
    } else if let Some(close) = payload_with_end.find("-->") {
        // Older persisted markers may have an inaccurate count. Keep them
        // displayable and provider-compatible while preferring the declared
        // length whenever it describes a complete payload.
        let end = start + PASTE_PREFIX.len() + count_text.len() + 1 + close + 3;
        Some(PasteMarker {
            end,
            char_count,
            payload: &payload_with_end[..close],
        })
    } else {
        None
    }
}

pub(crate) fn find(text: &str, from: usize) -> Option<(usize, PasteMarker<'_>)> {
    let relative = text.get(from..)?.find(PASTE_PREFIX)?;
    let start = from + relative;
    parse_at(text, start).map(|marker| (start, marker))
}

pub(crate) fn compact(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut number = 0;
    while let Some((start, marker)) = find(text, cursor) {
        output.push_str(&text[cursor..start]);
        number += 1;
        output.push_str(&format!(
            "[Pasted Text #{number} ({} chars)]",
            marker.char_count
        ));
        cursor = marker.end;
    }
    output.push_str(&text[cursor..]);
    output
}

pub(crate) fn expand(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some((start, marker)) = find(text, cursor) {
        output.push_str(&text[cursor..start]);
        output.push_str(marker.payload);
        cursor = marker.end;
    }
    output.push_str(&text[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::{compact, expand, parse_at};

    #[test]
    fn marker_parser_uses_declared_unicode_length() {
        let text = "before <!--PASTE:4:å-->-->{after}";
        let start = text.find("<!--PASTE:").unwrap();
        let marker = parse_at(text, start).unwrap();
        assert_eq!(marker.char_count, 4);
        assert_eq!(marker.payload, "å-->");
        assert_eq!(expand(text), "before å-->".to_string() + "{after}");
    }

    #[test]
    fn malformed_markers_are_preserved() {
        let text = "<!--PASTE:8:short";
        assert_eq!(compact(text), text);
        assert_eq!(expand(text), text);
    }
}
