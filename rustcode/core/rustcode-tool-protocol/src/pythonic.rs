//! Liquid AI (LFM2/LFM2.5) tool-call envelopes.
//!
//! These models are trained to wrap calls in
//! `<|tool_call_start|>[name(arg=value, ...), ...]<|tool_call_end|>`, with
//! Python-literal arguments, regardless of the textual contract in the system
//! prompt (#2587). Only text inside a closed envelope is parsed, so prose or
//! code that merely looks like a call is never executed.

use crate::ToolCall;
use serde_json::{Map, Number, Value};

pub const START: &str = "<|tool_call_start|>";
pub const END: &str = "<|tool_call_end|>";

/// Parse every closed envelope in `text`. An envelope whose body does not
/// parse contributes nothing, so the caller's malformed-call recovery runs.
pub(crate) fn parse_envelopes(text: &str, calls: &mut Vec<ToolCall>) {
    let mut rest = text;
    while let Some(start) = rest.find(START) {
        let after = &rest[start + START.len()..];
        let Some(end) = after.find(END) else {
            // Some servers drop the closing special token. Accept the tail
            // only when it is one complete call list; a truncated call fails
            // to parse and stays an incomplete envelope.
            if let Some(parsed) = Parser::new(after.trim()).calls() {
                calls.extend(parsed);
            }
            break;
        };
        let body = after[..end].trim();
        if let Some(parsed) = parse_body(body) {
            calls.extend(parsed);
        }
        rest = &after[end + END.len()..];
    }
}

/// True when the last envelope is unclosed and its tail is not a complete
/// call list, i.e. the model stopped mid-call.
pub(crate) fn has_unclosed_envelope(text: &str) -> bool {
    let Some(start) = text.rfind(START) else {
        return false;
    };
    let after = &text[start + START.len()..];
    !after.contains(END) && after.contains('(') && Parser::new(after.trim()).calls().is_none()
}

/// Byte ranges of each envelope, an unclosed one running to the end of text.
pub(crate) fn envelope_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(START) {
        let start = offset + relative;
        let body_start = start + START.len();
        match text[body_start..].find(END) {
            Some(end) => {
                let stop = body_start + end + END.len();
                spans.push((start, stop));
                offset = stop;
            }
            None => {
                spans.push((start, text.len()));
                break;
            }
        }
    }
    spans
}

fn parse_body(body: &str) -> Option<Vec<ToolCall>> {
    if let Some(calls) = Parser::new(body).calls() {
        return Some(calls);
    }
    // The model may also be told to write JSON inside the same envelope.
    let value: Value = serde_json::from_str(body).ok()?;
    let items = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    let calls = items
        .iter()
        .map(|item| {
            let (name, arguments) = crate::extract_tool_call(item)?;
            Some(ToolCall {
                name,
                arguments,
                call_id: None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (!calls.is_empty()).then_some(calls)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn eat(&mut self, expected: char) -> bool {
        self.skip_ws();
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// `[call, call]` or a bare `call, call`; the whole body must be consumed.
    fn calls(mut self) -> Option<Vec<ToolCall>> {
        let bracketed = self.eat('[');
        let mut calls = Vec::new();
        loop {
            self.skip_ws();
            if bracketed && self.peek() == Some(']') {
                break;
            }
            calls.push(self.call()?);
            if !self.eat(',') {
                break;
            }
        }
        if bracketed && !self.eat(']') {
            return None;
        }
        self.skip_ws();
        (self.pos == self.src.len() && !calls.is_empty()).then_some(calls)
    }

    fn ident(&mut self) -> Option<&'a str> {
        self.skip_ws();
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            self.bump();
        }
        let ident = &self.src[start..self.pos];
        ident
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            .then_some(ident)
    }

    fn call(&mut self) -> Option<ToolCall> {
        let name = self.ident()?.to_string();
        if !self.eat('(') {
            return None;
        }
        let mut arguments = Map::new();
        loop {
            if self.eat(')') {
                break;
            }
            // Keyword arguments only: a positional value has no schema name.
            let key = self.ident()?.to_string();
            if !self.eat('=') {
                return None;
            }
            let value = self.value()?;
            arguments.insert(key, value);
            if !self.eat(',') {
                if !self.eat(')') {
                    return None;
                }
                break;
            }
        }
        Some(ToolCall {
            name,
            arguments: Value::Object(arguments),
            call_id: None,
        })
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            '\'' | '"' => self.string().map(Value::String),
            '[' => self.sequence('[', ']'),
            '(' => self.sequence('(', ')'),
            '{' => self.dict(),
            c if c == '-' || c == '+' || c.is_ascii_digit() => self.number(),
            _ => match self.ident()? {
                "True" | "true" => Some(Value::Bool(true)),
                "False" | "false" => Some(Value::Bool(false)),
                "None" | "null" => Some(Value::Null),
                _ => None,
            },
        }
    }

    fn sequence(&mut self, open: char, close: char) -> Option<Value> {
        if !self.eat(open) {
            return None;
        }
        let mut items = Vec::new();
        loop {
            if self.eat(close) {
                break;
            }
            items.push(self.value()?);
            if !self.eat(',') {
                if !self.eat(close) {
                    return None;
                }
                break;
            }
        }
        Some(Value::Array(items))
    }

    fn dict(&mut self) -> Option<Value> {
        if !self.eat('{') {
            return None;
        }
        let mut map = Map::new();
        loop {
            if self.eat('}') {
                break;
            }
            self.skip_ws();
            let key = match self.peek()? {
                '\'' | '"' => self.string()?,
                _ => self.ident()?.to_string(),
            };
            if !self.eat(':') {
                return None;
            }
            map.insert(key, self.value()?);
            if !self.eat(',') {
                if !self.eat('}') {
                    return None;
                }
                break;
            }
        }
        Some(Value::Object(map))
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.pos;
        if matches!(self.peek(), Some('-' | '+')) {
            self.bump();
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '_'))
            || (matches!(self.peek(), Some('-' | '+'))
                && matches!(self.src[..self.pos].chars().last(), Some('e' | 'E')))
        {
            self.bump();
        }
        let raw = self.src[start..self.pos]
            .trim_start_matches('+')
            .replace('_', "");
        if let Ok(int) = raw.parse::<i64>() {
            return Some(Value::Number(int.into()));
        }
        raw.parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.bump()?;
        let triple = self.src[self.pos..].starts_with(&format!("{quote}{quote}"));
        if triple {
            self.bump();
            self.bump();
        }
        let mut out = String::new();
        loop {
            let ch = self.bump()?;
            if ch == quote {
                if !triple {
                    return Some(out);
                }
                if self.src[self.pos..].starts_with(&format!("{quote}{quote}")) {
                    self.bump();
                    self.bump();
                    return Some(out);
                }
                out.push(ch);
                continue;
            }
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            match self.bump()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\n' => {}
                'u' => {
                    let hex = self.src.get(self.pos..self.pos + 4)?;
                    let code = u32::from_str_radix(hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                    self.pos += 4;
                }
                other @ ('\\' | '\'' | '"') => out.push(other),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        parse_envelopes(text, &mut calls);
        calls
    }

    // Session 01a183f4 (#2587): the reply ended with this envelope and the
    // engine finished the turn without running either command.
    #[test]
    fn parses_the_session_envelope_into_both_calls() {
        let text = "I'll check your MacBook's memory status — total RAM and what's currently using it. Let me run a few system commands to get that info.<|tool_call_start|>[run_command(command='sysctl hw.memsize', timeout_ms=5000), run_command(command='top -l 1', timeout_ms=5000)]<|tool_call_end|>";
        let calls = parse(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "run_command");
        assert_eq!(
            calls[0].arguments,
            json!({"command": "sysctl hw.memsize", "timeout_ms": 5000})
        );
        assert_eq!(
            calls[1].arguments,
            json!({"command": "top -l 1", "timeout_ms": 5000})
        );
    }

    #[test]
    fn parses_python_literals_and_escapes() {
        let text = r#"<|tool_call_start|>[edit(path="a \"b\".rs", body='it\'s\nok', n=-1.5, big=1e3, on=True, off=False, none=None, items=[1, 'x', (2, 3)], opts={'k': "v", depth: 2}, empty=[],)]<|tool_call_end|>"#;
        let calls = parse(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].arguments,
            json!({
                "path": "a \"b\".rs",
                "body": "it's\nok",
                "n": -1.5,
                "big": 1000.0,
                "on": true,
                "off": false,
                "none": null,
                "items": [1, "x", [2, 3]],
                "opts": {"k": "v", "depth": 2},
                "empty": []
            })
        );
    }

    #[test]
    fn parses_unbracketed_calls_no_args_and_json_bodies() {
        assert_eq!(
            parse("<|tool_call_start|>get_time()<|tool_call_end|>")[0].arguments,
            json!({})
        );
        let calls = parse(
            "<|tool_call_start|>[{\"name\": \"grep\", \"arguments\": {\"pattern\": \"x\"}}]<|tool_call_end|>",
        );
        assert_eq!(calls[0].name, "grep");
        assert_eq!(calls[0].arguments, json!({"pattern": "x"}));
        // Separate envelopes each contribute their call.
        let calls = parse(
            "<|tool_call_start|>[a(x=1)]<|tool_call_end|> then <|tool_call_start|>[b(y='2')]<|tool_call_end|>",
        );
        assert_eq!(
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn a_complete_call_list_survives_a_dropped_end_marker() {
        let text = "<|tool_call_start|>[run_command(command='ls')]";
        assert_eq!(parse(text)[0].arguments, json!({"command": "ls"}));
        assert!(!has_unclosed_envelope(text));
        assert!(has_unclosed_envelope(
            "<|tool_call_start|>[write_to_file(path='a', content='partial"
        ));
    }

    #[test]
    fn rejects_malformed_or_unclosed_envelopes() {
        for text in [
            "<|tool_call_start|>[run_command('ls')]<|tool_call_end|>",
            "<|tool_call_start|>[run_command(command='ls]<|tool_call_end|>",
            "<|tool_call_start|>[run_command(command=ls)]<|tool_call_end|>",
            "<|tool_call_start|>[run_command(command='ls')] trailing<|tool_call_end|>",
            "<|tool_call_start|>[run_command(command='ls'",
            "Call run_command(command='ls') to list files.",
        ] {
            assert!(parse(text).is_empty(), "{text}");
        }
    }
}
