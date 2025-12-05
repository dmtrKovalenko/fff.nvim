use mlua::prelude::*;
use std::path::PathBuf;

use crate::{git::format_git_status, location::Location};

#[derive(Debug, Clone)]
pub struct FileItem {
    pub path: PathBuf,
    pub relative_path: String,
    pub relative_path_lower: String,
    pub file_name: String,
    pub file_name_lower: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: Option<git2::Status>,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub exact_match: bool,
    pub match_type: &'static str,
}

#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    pub query: &'a str,
    pub current_file: Option<&'a str>,
    pub max_results: usize,
    pub max_typos: u16,
    pub max_threads: usize,
    pub reverse_order: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult<'a> {
    pub items: Vec<&'a FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
    pub location: Option<Location>,
}

impl IntoLua for &FileItem {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;
        table.set("path", self.path.to_string_lossy().to_string())?;
        table.set("relative_path", self.relative_path.clone())?;
        table.set("name", self.file_name.clone())?;
        table.set("size", self.size)?;
        table.set("modified", self.modified)?;
        table.set("access_frecency_score", self.access_frecency_score)?;
        table.set(
            "modification_frecency_score",
            self.modification_frecency_score,
        )?;
        table.set("total_frecency_score", self.total_frecency_score)?;
        table.set("git_status", format_git_status(self.git_status))?;
        Ok(LuaValue::Table(table))
    }
}

impl IntoLua for Score {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;
        table.set("total", self.total)?;
        table.set("base_score", self.base_score)?;
        table.set("filename_bonus", self.filename_bonus)?;
        table.set("special_filename_bonus", self.special_filename_bonus)?;
        table.set("frecency_boost", self.frecency_boost)?;
        table.set("distance_penalty", self.distance_penalty)?;
        table.set("current_file_penalty", self.current_file_penalty)?;
        table.set("match_type", self.match_type)?;
        table.set("exact_match", self.exact_match)?;
        Ok(LuaValue::Table(table))
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

impl IntoLua for SearchResult<'_> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let table = lua.create_table()?;
        table.set("items", self.items)?;
        table.set("scores", self.scores)?;
        table.set("total_matched", self.total_matched)?;
        table.set("total_files", self.total_files)?;

        if let Some(location) = &self.location {
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
