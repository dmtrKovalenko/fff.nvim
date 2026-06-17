/// Bridges IPC domain types to their fff-core equivalents (orphan rule workaround).
pub trait IntoCoreExt {
    type Core;
    /// Converts this IPC type into the corresponding fff-core type.
    fn into_core(self) -> Self::Core;
}

impl IntoCoreExt for fff_ipc_domain::CaseMode {
    type Core = fff::CaseMode;

    fn into_core(self) -> fff::CaseMode {
        match self {
            Self::Smart => fff::CaseMode::Smart,
            Self::Sensitive => fff::CaseMode::Sensitive,
            Self::Insensitive => fff::CaseMode::Insensitive,
        }
    }
}
