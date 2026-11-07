// Benchmark (d): naive recursion and an arithmetic loop (checked arithmetic, as in Unio).
fn fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    fib(n - 1).checked_add(fib(n - 2)).expect("overflow")
}

fn main() {
    let mut total: i64 = 0;
    for i in 0..10_000_000i64 {
        total = total.checked_add(i.checked_mul(i).expect("overflow") % 7).expect("overflow");
    }
    println!("{} {}", fib(30), total);
}
