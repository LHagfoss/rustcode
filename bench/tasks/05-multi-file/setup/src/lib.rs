pub mod math;

#[cfg(test)]
mod tests {
    #[test]
    fn doubles() {
        assert_eq!(crate::math::double(21), 42);
        assert_eq!(crate::math::double(0), 0);
    }
}
