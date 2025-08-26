use mlua::prelude::*;
use std::path::PathBuf;

use crate::git::format_git_status;

#[derive(Debug, Clone)]
pub struct FileItem {
    pub path: PathBuf,
    pub relative_path: String,
    pub file_name: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: Option<git2::Status>,
}

#[derive(Debug, Clone, Copy)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
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

#[derive(Debug)]
pub struct MatchedFile<'a> {
    pub file: &'a FileItem,
    pub file_index: usize,
    pub score: Score,
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
        Ok(LuaValue::Table(table))
    }
}
