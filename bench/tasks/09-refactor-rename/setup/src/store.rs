use crate::Widget;

pub fn describe(w: &Widget) -> String {
    format!("gadget#{}", w.id)
}
