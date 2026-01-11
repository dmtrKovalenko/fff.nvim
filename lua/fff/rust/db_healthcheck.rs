use crate::error::Result;
use mlua::{Lua, Table};

pub trait DbHealthChecker {
    fn get_env(&self) -> &heed::Env;
    fn count_entries(&self) -> Result<Vec<(&'static str, u64)>>;

    fn get_lua_helthcheckh(&self, lua: &Lua) -> std::result::Result<Table, mlua::Error> {
        let env = self.get_env();
        let table = lua.create_table()?;

        let size = env.real_disk_size().map_err(|e| {
            mlua::Error::RuntimeError(format!("Failed to read db disk size: {}", e))
        })?;
        let path = env.path().to_string_lossy().to_string();
        let entry_count = self
            .count_entries()
            .map_err(|e| mlua::Error::RuntimeError(format!("Failed to count db entries: {}", e)))?;

        table.set("path", path)?;
        table.set("disk_size", size)?;

        for (name, count) in entry_count {
            table.set(name, count)?;
        }

        Ok(table)
    }
}
