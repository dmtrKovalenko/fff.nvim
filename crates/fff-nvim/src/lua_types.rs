//! Lua type conversions for fff-core types
//!
//! This module provides IntoLua implementations for core types.

use fff_core::git::format_git_status;
use fff_core::{FileItem, Location, Score, SearchResult};
use mlua::prelude::*;

/// Wrapper for SearchResult that implements IntoLua
pub struct SearchResultLua<'a> {
    inner: SearchResult<'a>,
}

impl<'a> From<SearchResult<'a>> for SearchResultLua<'a> {
    fn from(inner: SearchResult<'a>) -> Self {
        Self { inner }
    }
}

struct LuaPosition((i32, i32));

impl IntoLua for LuaPosition {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;
        table.set("line", self.0.0)?;
        table.set("col", self.0.1)?;
        Ok(LuaValue::Table(table))
    }
}

fn file_item_into_lua(item: &FileItem, lua: &Lua) -> LuaResult<LuaValue> {
    let table = lua.create_table()?;
    table.set("path", item.path.to_string_lossy().to_string())?;
    table.set("relative_path", item.relative_path.clone())?;
    table.set("name", item.file_name.clone())?;
    table.set("size", item.size)?;
    table.set("modified", item.modified)?;
    table.set("access_frecency_score", item.access_frecency_score)?;
    table.set(
        "modification_frecency_score",
        item.modification_frecency_score,
    )?;
    table.set("total_frecency_score", item.total_frecency_score)?;
    table.set("git_status", format_git_status(item.git_status))?;
    Ok(LuaValue::Table(table))
}

fn score_into_lua(score: &Score, lua: &Lua) -> LuaResult<LuaValue> {
    let table = lua.create_table()?;
    table.set("total", score.total)?;
    table.set("base_score", score.base_score)?;
    table.set("filename_bonus", score.filename_bonus)?;
    table.set("special_filename_bonus", score.special_filename_bonus)?;
    table.set("frecency_boost", score.frecency_boost)?;
    table.set("distance_penalty", score.distance_penalty)?;
    table.set("current_file_penalty", score.current_file_penalty)?;
    table.set("combo_match_boost", score.combo_match_boost)?;
    table.set("match_type", score.match_type)?;
    table.set("exact_match", score.exact_match)?;
    Ok(LuaValue::Table(table))
}

impl IntoLua for SearchResultLua<'_> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;

        // Convert items
        let items_table = lua.create_table()?;
        for (i, item) in self.inner.items.iter().enumerate() {
            items_table.set(i + 1, file_item_into_lua(item, lua)?)?;
        }
        table.set("items", items_table)?;

        // Convert scores
        let scores_table = lua.create_table()?;
        for (i, score) in self.inner.scores.iter().enumerate() {
            scores_table.set(i + 1, score_into_lua(score, lua)?)?;
        }
        table.set("scores", scores_table)?;

        table.set("total_matched", self.inner.total_matched)?;
        table.set("total_files", self.inner.total_files)?;

        if let Some(location) = &self.inner.location {
            let location_table = lua.create_table()?;

            match location {
                Location::Line(line) => {
                    location_table.set("line", *line)?;
                }
                Location::Position { line, col } => {
                    location_table.set("line", *line)?;
                    location_table.set("col", *col)?;
                }
                Location::Range { start, end } => {
                    location_table.set("start", LuaPosition(*start))?;
                    location_table.set("end", LuaPosition(*end))?;
                }
            }

            table.set("location", location_table)?;
        }

        Ok(LuaValue::Table(table))
    }
}
