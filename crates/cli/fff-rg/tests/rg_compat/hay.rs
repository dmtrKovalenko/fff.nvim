/// Standard test corpus (borrowed from ripgrep's test suite).
pub const SHERLOCK: &str = "\
For the Doctor Watsons of this world, as opposed to the Sherlock
Holmeses, success in the province of detective work must always
be, to a very large extent, the result of luck. Sherlock Holmes
can extract a clew from a wisp of straw or a flake of cigar ash;
but Doctor Watson has to have it taken out for him and dusted,
and exhibited clearly, with a label attached.
";

pub const RUST_MAIN: &str = "\
use std::collections::HashMap;
use std::io;

fn main() {
    if let Err(e) = run() {
        eprintln!(\"error: {}\", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), io::Error> {
    let mut map = HashMap::new();
    map.insert(\"hello\", 1);
    map.insert(\"world\", 2);

    for (key, value) in &map {
        println!(\"{}: {}\", key, value);
    }

    println!(\"done\");
    Ok(())
}
";

pub const RUST_LIB: &str = "\
pub struct Config {
    name: String,
    timeout: u64,
    verbose: bool,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Config {
            name: name.to_string(),
            timeout: 30,
            verbose: false,
        }
    }

    pub fn with_timeout(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::new(\"default\")
    }
}
";

pub const RUST_TEST: &str = "\
use super::*;

#[test]
fn test_config_default() {
    let config = Config::default();
    assert_eq!(config.timeout, 30);
    assert!(!config.is_verbose());
}

#[test]
fn test_config_with_timeout() {
    let config = Config::new(\"test\").with_timeout(60);
    assert_eq!(config.timeout, 60);
    assert_eq!(config.name, \"test\");
}
";

pub const JSON_CONFIG: &str = "\
{
  \"name\": \"my-project\",
  \"version\": \"1.0.0\",
  \"settings\": {
    \"timeout\": 30,
    \"verbose\": false,
    \"max_retries\": 3
  },
  \"features\": [\"search\", \"preview\", \"git\"]
}
";

pub const UNICODE_README: &str = "\
# Prójéct Dócs

A café-inspired résumé builder for the naïve developer.

## Features

- 日本語サポート (Japanese support)
- 中文文档 (Chinese docs)
- العربية (Arabic)

Built with care and résumé-quality output.
";

pub const INDENTED: &str = "\
    fn process() {
        let x = 42;
        if x > 0 {
            println!(\"positive: {}\", x);
        }
    }
";

pub const REPEATED: &str = "\
foo bar foo baz foo
hello world
foo foo foo foo foo
bar foo baz foo
";

pub const NO_NEWLINE: &str = "last line has no newline";

pub struct Hay {
    pub rust_main: &'static str,
    pub rust_lib: &'static str,
    pub rust_test: &'static str,
    pub json_config: &'static str,
    pub unicode_readme: &'static str,
    pub indented: &'static str,
    pub repeated: &'static str,
    pub no_newline: &'static str,
}

pub const PROJECT: Hay = Hay {
    rust_main: RUST_MAIN,
    rust_lib: RUST_LIB,
    rust_test: RUST_TEST,
    json_config: JSON_CONFIG,
    unicode_readme: UNICODE_README,
    indented: INDENTED,
    repeated: REPEATED,
    no_newline: NO_NEWLINE,
};
