use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct Interval {
    
}

#[derive(Deserialize)]
struct Watch {
    binary: PathBuf,
    permitted: Vec<Interval>,
}

#[derive(Deserialize)]
struct Config {
    /// The list of processes to watch and kill.
    watch: Vec<Watch>
}
