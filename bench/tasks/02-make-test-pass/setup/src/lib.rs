/// Returns the sum of two integers.
pub fn add(a: i32, b: i32) -> i32 {
    // Bug: subtracts instead of adding.
    a - b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 3), 5);
        assert_eq!(add(10, 5), 15);
    }
}
