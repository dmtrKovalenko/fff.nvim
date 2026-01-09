//! Error handling for fff-nvim
//!
//! This module provides utilities for converting fff_core errors to mlua errors.

use fff_core::Error as CoreError;

/// Convert a fff_core::Error to mlua::Error
///
/// This function is used because we can't implement From<CoreError> for mlua::Error
/// due to Rust's orphan rules (both types are foreign to this crate).
pub fn to_lua_error(err: CoreError) -> mlua::Error {
    let string_value = err.to_string();
    ::tracing::error!(string_value);
    mlua::Error::RuntimeError(string_value)
}

/// Extension trait for Result<T, fff_core::Error> to convert to LuaResult<T>
pub trait IntoLuaResult<T> {
    fn into_lua_result(self) -> mlua::Result<T>;
}

impl<T> IntoLuaResult<T> for Result<T, CoreError> {
    fn into_lua_result(self) -> mlua::Result<T> {
        self.map_err(to_lua_error)
    }
}

/// Extension trait for Result<T, PoisonError> to convert to Result<T, CoreError>
pub trait IntoCoreError<T> {
    fn with_lock_error(self, err: CoreError) -> Result<T, CoreError>;
}

impl<T, G> IntoCoreError<T> for Result<T, std::sync::PoisonError<G>> {
    fn with_lock_error(self, err: CoreError) -> Result<T, CoreError> {
        self.map_err(|_| err)
    }
}
