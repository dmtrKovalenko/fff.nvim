/// Common interface for search backends. Returns `Ok(true)` on match,
/// `Ok(false)` on no match, or an error.
pub trait Search {
    /// Content search — find lines matching the query pattern.
    fn grep(&self) -> Result<bool, Box<dyn std::error::Error>>;
    /// File listing — enumerate indexed files matching the fuzzy query.
    fn files(&self) -> Result<bool, Box<dyn std::error::Error>>;
}
