pub mod a;
pub mod b;
pub mod c;

#[cfg(test)]
mod tests {
    #[test]
    fn all() {
        assert_eq!(crate::a::area_square(3), 9);
        assert_eq!(crate::b::triple(4), 12);
        assert_eq!(crate::c::perimeter_square(3), 12);
    }
}
