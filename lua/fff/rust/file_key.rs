#[derive(Debug, Clone)]
pub struct FileKey {
    pub path: String,
}

impl FileKey {
    pub fn new(path: String) -> Self {
        Self { path }
    }

    pub fn into_path_buf(self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.path)
    }

    pub fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(&self.path)
    }

    pub fn into_string(self) -> String {
        self.path
    }
}
