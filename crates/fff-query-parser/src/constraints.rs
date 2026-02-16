use smallvec::SmallVec;

/// Constraint types that can be extracted from a query
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint<'a> {
    /// Match file extension: *.rs -> Extension("rs")
    Extension(&'a str),

    /// Glob pattern: **/*.rs -> Glob("**/*.rs")
    Glob(&'a str),

    /// Multiple text search parts: ["src", "name"]
    /// Uses slice to avoid allocation
    Parts(&'a [&'a str]),

    /// Single text token (optimized case)
    Text(&'a str),

    /// Exclude pattern: !test -> Exclude(&["test"])
    Exclude(&'a [&'a str]),

    /// Path constraint: /src/ -> PathSegment("src")
    PathSegment(&'a str),

    /// File type constraint: type:rust -> FileType("rust")
    FileType(&'a str),

    /// Git status constraint: status:modified -> GitStatus(Modified)
    GitStatus(GitStatusFilter),

    /// Negation constraint: !extension:rs -> Not(Extension("rs"))
    /// Negates the inner constraint
    Not(Box<Constraint<'a>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatusFilter {
    Modified,
    Untracked,
    Staged,
    Unmodified,
}

/// Stack-allocated buffer for text parts (up to 16 parts without heap allocation)
pub(crate) type TextPartsBuffer<'a> = SmallVec<[&'a str; 16]>;
