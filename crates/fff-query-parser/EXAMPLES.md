# fff-query-parser Examples

## Basic Usage

```rust
use fff_query_parser::{QueryParser, Constraint, FuzzyQuery};

fn main() {
    let parser = QueryParser::default();
    
    // Example 1: Extension filter
    let result = parser.parse("*.rs");
    println!("Query: '*.rs'");
    println!("  Fuzzy query: {:?}", result.fuzzy_query);
    println!("  Constraints: {:?}", result.constraints);
    // Output: Fuzzy query: Empty
    //         Constraints: [Extension("rs")]
    
    // Example 2: Text with extension
    let result = parser.parse("test *.rs");
    println!("\nQuery: 'test *.rs'");
    println!("  Fuzzy query: {:?}", result.fuzzy_query);
    println!("  Constraints: {:?}", result.constraints);
    // Output: Fuzzy query: Text("test")
    //         Constraints: [Extension("rs")]
    
    // Example 3: Negation constraints (NEW!)
    let result = parser.parse("!*.rs");
    println!("\nQuery: '!*.rs'");
    println!("  Constraints: {:?}", result.constraints);
    // Output: Constraints: [Not(Extension("rs"))]
    
    let result = parser.parse("!test");
    println!("\nQuery: '!test'");
    println!("  Constraints: {:?}", result.constraints);
    // Output: Constraints: [Not(Text("test"))]
    
    let result = parser.parse("!/src/");
    println!("\nQuery: '!/src/'");
    println!("  Constraints: {:?}", result.constraints);
    // Output: Constraints: [Not(PathSegment("src"))]
    
    // Example 4: Complex query
    let result = parser.parse("src name *.rs !test /lib/ status:modified");
    println!("\nQuery: 'src name *.rs !test /lib/ status:modified'");
    println!("  Fuzzy query: {:?}", result.fuzzy_query);
    println!("  Constraints: {:?}", result.constraints);
    // Output: Fuzzy query: Parts(["src", "name"])
    //         Constraints: [Extension("rs"), Not(Text("test")),
    //                       PathSegment("lib"), GitStatus(Modified)]
    
    // Example 5: Glob pattern
    let result = parser.parse("**/*.rs");
    println!("\nQuery: '**/*.rs'");
    println!("  Fuzzy query: {:?}", result.fuzzy_query);
    println!("  Constraints: {:?}", result.constraints);
    // Output: Fuzzy query: Empty
    //         Constraints: [Glob("**/*.rs")]
    
    // Example 6: Negate complex constraints
    let result = parser.parse("!status:modified");
    println!("\nQuery: '!status:modified'");
    println!("  Constraints: {:?}", result.constraints);
    // Output: Constraints: [Not(GitStatus(Modified))]
}
```

## Custom Configuration

```rust
use fff_query_parser::{QueryParser, GrepConfig, ParserConfig, Constraint};

// Use grep-specific configuration
let grep_parser = QueryParser::new(GrepConfig);
let result = grep_parser.parse("*.rs");
// With GrepConfig, *.rs is treated as text, not an extension

// Create custom configuration
struct CustomConfig;

impl ParserConfig for CustomConfig {
    fn enable_extension(&self) -> bool { false }
    fn enable_glob(&self) -> bool { false }
    
    // Only allow exclude and path segments
    fn enable_exclude(&self) -> bool { true }
    fn enable_path_segments(&self) -> bool { true }
}

let custom_parser = QueryParser::new(CustomConfig);
```

## Integration Example

```rust
use fff_query_parser::{QueryParser, Constraint};

struct FileItem {
    path: String,
    extension: String,
    // ... other fields
}

fn filter_files(files: &[FileItem], query: &str) -> Vec<&FileItem> {
    let parser = QueryParser::default();
    let result = parser.parse(query);
    
    files.iter()
        .filter(|file| {
            // Apply each constraint
            result.constraints.iter().all(|constraint| {
                match constraint {
                    Constraint::Extension(ext) => file.extension == *ext,
                    Constraint::PathSegment(seg) => file.path.contains(seg),
                    Constraint::Not(inner) => {
                        // Negate the inner constraint
                        !matches_constraint(file, inner)
                    }
                    Constraint::Glob(pattern) => {
                        // Use glob crate to match pattern
                        glob_match(pattern, &file.path)
                    }
                    _ => true, // Other constraints don't filter
                }
            })
        })
        .collect()
}

fn matches_constraint(file: &FileItem, constraint: &Constraint) -> bool {
    match constraint {
        Constraint::Extension(ext) => file.extension == *ext,
        Constraint::PathSegment(seg) => file.path.contains(seg),
        Constraint::Text(text) => file.path.contains(text),
        Constraint::Not(inner) => !matches_constraint(file, inner),
        _ => true,
    }
}
```

## Benchmark Results

Run benchmarks to see performance:

```bash
cargo bench --package fff-query-parser
```

Expected results on modern hardware:
- Extension parsing: **<10ns**
- Glob parsing: **~20ns**
- Complex mixed queries: **<110ns**

## Memory Usage

The parser uses SmallVec with capacity 8 for constraints, which means:
- Queries with ≤8 constraints: **Zero heap allocations** for constraint storage
- Text parts buffer: Stack-allocated up to 16 tokens
- Total stack usage: **~256 bytes** for typical queries

Allocations:
1. `Box` for `Not` constraint inner value (when using negation)
2. SmallVec spills to heap if >8 constraints

The new `Not` constraint uses `Box` to wrap the inner constraint, which is a small heap allocation but allows for proper recursive constraint handling.
