// TODO: implement a public `factorial(n: u64) -> u64` function so the tests pass.

#[cfg(test)]
mod tests {
    #[test]
    fn factorials() {
        assert_eq!(crate::factorial(0), 1);
        assert_eq!(crate::factorial(1), 1);
        assert_eq!(crate::factorial(5), 120);
        assert_eq!(crate::factorial(10), 3_628_800);
    }
}
