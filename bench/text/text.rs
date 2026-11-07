// Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
use std::env;
use std::fs;

fn main() {
    let name = env::args().nth(1).expect("file");
    let text = fs::read_to_string(name).expect("read");
    let mut total = 0;
    for (index, line) in text.lines().enumerate() {
        let words: Vec<&str> = line.split(' ').collect();
        let joined = words.join("-");
        let record = format!("{}:{}:{}", index, words.len(), joined.replace("the", "THE"));
        total += record.chars().count();
    }
    println!("{total}");
}
