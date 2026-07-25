pub mod store;

pub struct Widget {
    pub id: u32,
}

impl Widget {
    pub fn new(id: u32) -> Self {
        Widget { id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn works() {
        let w = Widget::new(7);
        assert_eq!(store::describe(&w), "gadget#7");
    }
}
