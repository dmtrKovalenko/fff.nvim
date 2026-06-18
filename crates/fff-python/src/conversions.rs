use fff::file_picker::FilePicker;

use crate::types::{DirItem, FileItem, GrepMatch, MatchRange, MixedDirItem, MixedFileItem, Score};

pub enum MixedItem {
    File(MixedFileItem),
    Dir(MixedDirItem),
}

impl From<&fff::Score> for Score {
    fn from(s: &fff::Score) -> Self {
        Self {
            total: s.total,
            base_score: s.base_score,
            filename_bonus: s.filename_bonus,
            special_filename_bonus: s.special_filename_bonus,
            frecency_boost: s.frecency_boost,
            distance_penalty: s.distance_penalty,
            current_file_penalty: s.current_file_penalty,
            combo_match_boost: s.combo_match_boost,
            path_alignment_bonus: s.path_alignment_bonus,
            exact_match: s.exact_match,
            match_type: s.match_type.to_string(),
        }
    }
}

impl From<(&fff::FileItem, &FilePicker)> for FileItem {
    fn from((item, picker): (&fff::FileItem, &FilePicker)) -> Self {
        Self {
            relative_path: item.relative_path(picker),
            file_name: item.file_name(picker),
            git_status: fff::git::format_git_status(item.git_status).to_string(),
            size: item.size,
            modified: item.modified,
            access_frecency_score: item.access_frecency_score as i64,
            modification_frecency_score: item.modification_frecency_score as i64,
            total_frecency_score: item.total_frecency_score() as i64,
            is_binary: item.is_binary(),
        }
    }
}

impl From<(&fff::DirItem, &FilePicker)> for DirItem {
    fn from((item, picker): (&fff::DirItem, &FilePicker)) -> Self {
        Self {
            relative_path: item.relative_path(picker),
            dir_name: item.dir_name(picker),
            max_access_frecency: item.max_access_frecency(),
        }
    }
}

impl From<(&fff::FileItem, &FilePicker)> for MixedFileItem {
    fn from((item, picker): (&fff::FileItem, &FilePicker)) -> Self {
        Self {
            relative_path: item.relative_path(picker),
            file_name: item.file_name(picker),
            git_status: fff::git::format_git_status(item.git_status).to_string(),
            size: item.size,
            modified: item.modified,
            access_frecency_score: item.access_frecency_score as i64,
            modification_frecency_score: item.modification_frecency_score as i64,
            total_frecency_score: item.total_frecency_score() as i64,
            is_binary: item.is_binary(),
        }
    }
}

impl From<(&fff::DirItem, &FilePicker)> for MixedDirItem {
    fn from((item, picker): (&fff::DirItem, &FilePicker)) -> Self {
        Self {
            relative_path: item.relative_path(picker),
            dir_name: item.dir_name(picker),
            max_access_frecency: item.max_access_frecency(),
        }
    }
}

impl From<(&fff::GrepMatch, &fff::FileItem, &FilePicker)> for GrepMatch {
    fn from((m, file, picker): (&fff::GrepMatch, &fff::FileItem, &FilePicker)) -> Self {
        Self {
            relative_path: file.relative_path(picker),
            file_name: file.file_name(picker),
            git_status: fff::git::format_git_status(file.git_status).to_string(),
            line_content: m.line_content.clone(),
            match_ranges: m
                .match_byte_offsets
                .iter()
                .map(|&(s, e)| MatchRange { start: s, end: e })
                .collect(),
            context_before: m.context_before.clone(),
            context_after: m.context_after.clone(),
            size: file.size,
            modified: file.modified,
            total_frecency_score: file.total_frecency_score() as i64,
            access_frecency_score: file.access_frecency_score as i64,
            modification_frecency_score: file.modification_frecency_score as i64,
            line_number: m.line_number,
            byte_offset: m.byte_offset,
            col: m.col as u32,
            fuzzy_score: m.fuzzy_score,
            is_definition: m.is_definition,
            is_binary: file.is_binary(),
        }
    }
}
