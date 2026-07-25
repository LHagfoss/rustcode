fn main() {
    // Type error: a string literal assigned to an i32.
    let count: i32 = "42";
    println!("count = {}", count);
}
