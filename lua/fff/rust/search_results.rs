use std::borrow::Cow;

use crate::types::{FileItem, MatchedFile, Score};
use mlua::prelude::*;

pub struct SearchResultsState {
    pub query: String,
    pub scores: Vec<Score>,
    pub matched_files: Vec<usize>,
}

impl SearchResultsState {
    pub fn all_files_to_sort<'a>(
        &self,
        query: &str,
        all_files: &'a [FileItem],
    ) -> Cow<'a, [FileItem]> {
        if self.query.starts_with(query) {
            Cow::Owned(
                self.matched_files
                    .iter()
                    .filter_map(|&index| all_files.get(index))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            Cow::Borrowed(all_files)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult<'a> {
    pub items: Vec<&'a FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
}

impl SearchResult<'_> {
    pub fn capture_and_truncate_search_results<'a>(
        query: String,
        results: Vec<MatchedFile<'a>>,
        last_search_results: &mut SearchResultsState,
        total_files: usize,
        max_results: usize,
    ) -> SearchResult<'a> {
        let total_matched = results.len();

        last_search_results.query = query;
        last_search_results.matched_files.clear();
        last_search_results.scores.clear();

        let mut items = Vec::with_capacity(max_results);
        let mut scores = Vec::with_capacity(max_results);

        for (i, matched) in results.into_iter().enumerate() {
            if i <= max_results {
                items.push(matched.file);
                scores.push(matched.score);
            }

            last_search_results.matched_files.push(matched.file_index);
            last_search_results.scores.push(matched.score);
        }

        SearchResult {
            items,
            scores,
            total_matched,
            total_files,
        }
    }
}

impl IntoLua for SearchResult<'_> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;
        table.set("items", self.items)?;
        table.set("scores", self.scores)?;
        table.set("total_matched", self.total_matched)?;
        table.set("total_files", self.total_files)?;

        Ok(LuaValue::Table(table))
    }
}
