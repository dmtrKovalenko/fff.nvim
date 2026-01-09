# fff-query-parser

Fast, zero-allocation query parser for file search pickers.

## Features

- **Blazingly Fast**: Sub-microsecond parsing (<100ns for most queries)
- **Zero Allocations**: Uses stack-allocated SmallVec for ≤8 constraints
- **Configurable**: Trait-based configuration for different picker types
- **Well-tested**: Comprehensive unit tests and benchmarks

## Performance

Benchmark results on Apple M-series:

- Simple extension (`*.rs`): **~9.5ns**
- Glob pattern (`**/*.rs`): **~20ns**  
- Text with extension (`name *.rs`): **~39ns**
- Complex mixed query (`src name *.rs !test /lib/ status:modified`): **~106ns**

## Usage

```rust
use fff_query_parser::{QueryParser, Constraint};

let parser = QueryParser::default();

// Parse extension constraint
let result = parser.parse("name *.rs");
assert_eq!(result.fuzzy_text, "name");
assert!(matches!(result.constraints[0], Constraint::Extension("rs")));

// Parse glob pattern  
let result = parser.parse("**/*.rs");
assert!(matches!(result.constraints[0], Constraint::Glob("**/*.rs")));

// Parse complex query
let result = parser.parse("src name *.rs !test /lib/ status:modified");
// fuzzy_text = "src name"
// constraints = [Extension, Exclude, PathSegment, GitStatus, Parts]
```

## Constraint Types

- `*.rs` → `Constraint::Extension("rs")` - File extension
- `**/*.rs` → `Constraint::Glob("**/*.rs")` - Glob pattern
- `!test` → `Constraint::Exclude(["test"])` - Exclusion pattern
- `/src/` → `Constraint::PathSegment("src")` - Path segment
- `type:rust` → `Constraint::FileType("rust")` - File type filter
- `status:modified` → `Constraint::GitStatus(Modified)` - Git status filter
- `hello world` → `Constraint::Parts(["hello", "world"])` - Text for fuzzy matching

## Configuration

```rust
use fff_query_parser::{QueryParser, GrepConfig};

// Use grep-specific configuration (no extension/glob parsing)
let parser = QueryParser::new(GrepConfig);
let result = parser.parse("*.rs");  // Treated as text, not extension
```

## Testing

```bash
cargo test
```

## Benchmarking

```bash
cargo bench
```

## License

MIT
