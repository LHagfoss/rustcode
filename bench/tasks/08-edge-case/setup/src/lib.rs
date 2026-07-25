pub fn count_vowels(s: &str) -> usize {
    s.chars()
        .filter(|c| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u'))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts() {
        assert_eq!(count_vowels("hello"), 2);
        assert_eq!(count_vowels("HELLO"), 2);
        assert_eq!(count_vowels("Sky"), 0);
        assert_eq!(count_vowels(""), 0);
    }
}
