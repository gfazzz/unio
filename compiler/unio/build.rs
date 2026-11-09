//! The identity of this compiler for the program image of `unio build` (docs/runtime.md 8.4):
//! a hash of the sources of the compiler and of the runtime, so that an executable is run only
//! by the runtime of the compiler that made its image. The hash depends on the contents of the
//! files and their paths below the crate, never on where the checkout is, so it is the same on
//! every machine (C-4).

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn main() {
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let roots = [
        ("unio", crate_dir.join("src")),
        ("unio-runtime", crate_dir.join("../unio-runtime/src")),
    ];
    // FNV-1a, 64 bits: no dependency, and the same on every platform.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (name, root) in &roots {
        println!("cargo:rerun-if-changed={}", root.display());
        let mut found = Vec::new();
        files(root, &mut found);
        let mut named: Vec<(String, PathBuf)> = found
            .into_iter()
            .map(|p| {
                let relative = p.strip_prefix(root).unwrap_or(&p).to_string_lossy();
                (format!("{name}/{}", relative.replace('\\', "/")), p)
            })
            .collect();
        named.sort();
        for (relative, path) in named {
            feed(relative.as_bytes());
            feed(&[0]);
            // Line ends as Git may check them out on Windows do not change the identity.
            let text = std::fs::read(&path).unwrap_or_default();
            let text: Vec<u8> = text.into_iter().filter(|b| *b != b'\r').collect();
            feed(&(text.len() as u64).to_le_bytes());
            feed(&text);
        }
    }
    println!("cargo:rustc-env=UNIO_SOURCE_HASH={hash:016x}");
}
