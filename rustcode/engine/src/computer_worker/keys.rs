//! Key combos for `computer_key`: "return", "cmd+t", "ctrl+shift+tab".
//! Key codes are macOS virtual key codes for the ANSI layout.

/// Modifier flags, as `CGEventFlags` bits.
pub const COMMAND: u64 = 0x0010_0000;
pub const SHIFT: u64 = 0x0002_0000;
pub const CONTROL: u64 = 0x0004_0000;
pub const OPTION: u64 = 0x0008_0000;

/// One parsed combo: the key's code and the modifier flags held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub code: u16,
    pub flags: u64,
}

/// Parse "cmd+shift+t" into a key code and modifier flags.
pub fn parse(combo: &str) -> Result<Combo, String> {
    let parts: Vec<String> = combo
        .split('+')
        .map(|part| part.trim().to_ascii_lowercase())
        .collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return Err("key is empty".into());
    };
    let mut flags = 0;
    for modifier in modifiers {
        flags |= match modifier.as_str() {
            "cmd" | "command" | "meta" | "super" => COMMAND,
            "shift" => SHIFT,
            "ctrl" | "control" => CONTROL,
            "alt" | "option" | "opt" => OPTION,
            other => return Err(format!("unknown modifier '{other}'")),
        };
    }
    let code = code(key).ok_or_else(|| format!("unknown key '{key}'"))?;
    Ok(Combo { code, flags })
}

fn code(key: &str) -> Option<u16> {
    let named = match key {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "forwarddelete" => 117,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        _ => u16::MAX,
    };
    if named != u16::MAX {
        return Some(named);
    }
    let mut chars = key.chars();
    let (Some(single), None) = (chars.next(), chars.next()) else {
        return None;
    };
    Some(match single {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '=' => 24,
        '9' => 25,
        '7' => 26,
        '-' => 27,
        '8' => 28,
        '0' => 29,
        ']' => 30,
        'o' => 31,
        'u' => 32,
        '[' => 33,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        '\'' => 39,
        'k' => 40,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        'n' => 45,
        'm' => 46,
        '.' => 47,
        '`' => 50,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_keys_letters_and_modifiers() {
        assert_eq!(parse("Return").unwrap(), Combo { code: 36, flags: 0 });
        assert_eq!(
            parse("cmd+t").unwrap(),
            Combo {
                code: 17,
                flags: COMMAND
            }
        );
        assert_eq!(
            parse("ctrl + shift + tab").unwrap(),
            Combo {
                code: 48,
                flags: CONTROL | SHIFT
            }
        );
        assert_eq!(parse("option+left").unwrap().flags, OPTION);
    }

    #[test]
    fn refuses_unknown_keys_and_modifiers() {
        assert!(parse("hyper+a").unwrap_err().contains("modifier"));
        assert!(parse("cmd+launch").unwrap_err().contains("unknown key"));
        assert!(parse("").is_err());
    }
}
