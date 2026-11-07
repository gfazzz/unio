// Benchmark (a): lines, words, and UTF-8 bytes of a large text.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let text = std::fs::read_to_string(path).expect("read");
    let lines = text.matches('\n').count();
    let words = text.split_whitespace().count();
    println!("{} {} {}", lines, words, text.len());
}
