// Implement `pub fn sum_csv(input: &str) -> i32` that parses a comma-separated
// list of integers and returns their sum. Whitespace around numbers is ignored,
// and an empty string returns 0.

#[cfg(test)]
mod tests {
    #[test]
    fn sums() {
        assert_eq!(crate::sum_csv("1,2,3"), 6);
        assert_eq!(crate::sum_csv(" 10, 20 , 30 "), 60);
        assert_eq!(crate::sum_csv("42"), 42);
        assert_eq!(crate::sum_csv(""), 0);
    }
}
