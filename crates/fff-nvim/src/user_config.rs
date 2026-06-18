use fff::{GrepConfig, ParserConfig};
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, Default)]
pub struct UserConfigOptions {
    pub enable_filename_constraint: bool,
}

static USER_CONFIG: RwLock<UserConfigOptions> = RwLock::new(UserConfigOptions {
    enable_filename_constraint: false,
});

pub fn set_global_user_config(config: UserConfigOptions) {
    if let Ok(mut guard) = USER_CONFIG.write() {
        *guard = config;
    }
}

fn get_global_user_config() -> UserConfigOptions {
    USER_CONFIG.read().map(|c| *c).unwrap_or_default()
}

/// Grep query parser config for Neovim configured by user
/// Same as `GrepConfig` but lets the user configure some behavior of query parsing
pub struct NvimGrepConfig;

impl ParserConfig for NvimGrepConfig {
    fn enable_path_segments(&self) -> bool {
        true
    }

    fn enable_git_status(&self) -> bool {
        false
    }

    fn enable_location(&self) -> bool {
        false
    }

    fn enable_filename_constraint(&self) -> bool {
        get_global_user_config().enable_filename_constraint
    }

    fn is_glob_pattern(&self, token: &str) -> bool {
        GrepConfig.is_glob_pattern(token)
    }
}
