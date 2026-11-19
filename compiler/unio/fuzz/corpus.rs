//! The corpus of one target (`WORK/corpus/TARGET/`), shared by the workers of the target and
//! kept between runs and between the stages of the workflow (R-1). Every file is an input,
//! named by its hash.
//!
//! Shared by the fuzzer (`main.rs`) and its tests (`tests/fuzz.rs`).

use std::collections::HashSet;
use std::path::PathBuf;

use crate::supervisor::fnv;

pub struct Corpus {
    pub dir: PathBuf,
    /// The names of the inputs this worker has seen: kept, run, or written by it.
    pub names: HashSet<String>,
    /// The inputs mutations start from.
    pub inputs: Vec<Vec<u8>>,
}

impl Corpus {
    pub fn new(dir: PathBuf) -> Corpus {
        Corpus {
            dir,
            names: HashSet::new(),
            inputs: Vec::new(),
        }
    }

    /// Keeps a new input and writes it to the directory.
    pub fn add(&mut self, data: Vec<u8>) {
        let name = fnv(&data);
        if self.names.insert(name.clone()) {
            let _ = std::fs::write(self.dir.join(&name), &data);
            self.inputs.push(data);
        }
    }

    /// Files of the directory not seen yet: those of earlier runs, then those the other workers
    /// added since the last look. Their names count as seen.
    pub fn unseen(&mut self) -> Vec<Vec<u8>> {
        let mut found = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !self.names.contains(&name) {
                    if let Ok(data) = std::fs::read(entry.path()) {
                        self.names.insert(name);
                        found.push(data);
                    }
                }
            }
        }
        found
    }

    /// The start of a worker: every input the directory holds, run and kept (it was chosen by
    /// coverage before, and is on disk already), then the seeds, run and kept when they reach
    /// something new, when there is no instrumentation to choose by, or while nothing is kept.
    /// `run` runs one input and says whether it reached new coverage; it is also given the
    /// number of inputs kept so far. The corpus is never left empty, so that the mutations have
    /// a base: with nothing else kept, it holds the empty input.
    pub fn start(
        &mut self,
        seeds: Vec<Vec<u8>>,
        instrumented: bool,
        run: &mut dyn FnMut(&[u8], usize) -> bool,
    ) {
        for data in self.unseen() {
            run(&data, self.inputs.len());
            self.inputs.push(data);
        }
        for data in seeds {
            let new = run(&data, self.inputs.len());
            if new || !instrumented || self.inputs.is_empty() {
                self.add(data);
            } else {
                self.names.insert(fnv(&data));
            }
        }
        if self.inputs.is_empty() {
            self.inputs.push(Vec::new());
        }
    }
}
