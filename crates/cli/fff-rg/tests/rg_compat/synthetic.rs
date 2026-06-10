use crate::util::Dir;

const EXTENSIONS: &[&str] = &["rs", "ts", "json", "md", "txt", "toml", "yaml"];

const DIRS: &[&str] = &[
    "src",
    "src/core",
    "src/core/utils",
    "src/api",
    "src/api/handlers",
    "src/db",
    "tests",
    "tests/integration",
    "docs",
    "config",
    "scripts",
    "lib",
    "lib/helpers",
];

const DOMAINS: &[&str] = &[
    r#"use std::net::{TcpStream, SocketAddr};
fn establish_connection(addr: SocketAddr) -> Result<TcpStream, std::io::Error> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    Ok(stream)
}"#,
    r#"use sqlx::{PgPool, Row};
async fn query_users(pool: &PgPool, limit: i64) -> Vec<String> {
    sqlx::query("SELECT name FROM users ORDER BY created_at DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool).await.unwrap()
        .iter().map(|row| row.get("name")).collect()
}"#,
    r#"fn verify_jwt_token(token: &str, secret: &[u8]) -> Result<Claims, AuthError> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() != 3 { return Err(AuthError::MalformedToken); }
    let payload = base64_decode(parts[1])?;
    Ok(serde_json::from_slice(&payload)?)
}"#,
    r#"struct Renderer { framebuffer: Vec<u32>, width: usize, height: usize }
impl Renderer {
    fn clear(&mut self, color: u32) { self.framebuffer.fill(color); }
    fn draw_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            self.framebuffer[y * self.width + x] = color;
        }
    }
}"#,
    r#"use serde::{Serialize, Deserialize};
#[derive(Serialize, Deserialize)]
struct ConfigFile { log_level: String, max_retries: u32, timeout_ms: u64 }
fn load_config(path: &std::path::Path) -> Result<ConfigFile, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&contents)?)
}"#,
    r#"struct PhysicsBody { position: [f64; 3], velocity: [f64; 3], mass: f64 }
fn apply_gravity(bodies: &mut [PhysicsBody], dt: f64) {
    let gravity_constant = 6.674e-11;
    for body in bodies.iter_mut() {
        body.velocity[1] -= gravity_constant * dt;
        for k in 0..3 { body.position[k] += body.velocity[k] * dt; }
    }
}"#,
    r#"use std::collections::BTreeMap;
struct LFUCache<K: Ord, V> { map: BTreeMap<K, (V, u64)>, capacity: usize }
impl<K: Ord, V> LFUCache<K, V> {
    fn new(capacity: usize) -> Self { Self { map: BTreeMap::new(), capacity } }
    fn get(&mut self, key: &K) -> Option<&V> {
        let (val, freq) = self.map.get_mut(key)?;
        *freq += 1;
        Some(val)
    }
}"#,
];

pub struct SyntheticRepo {
    pub file_count: usize,
    pub unique_needle_prefix: &'static str,
    pub common_needle: &'static str,
    pub files_with_common: usize,
}

impl SyntheticRepo {
    pub fn populate(&self, dir: &Dir) -> Vec<FileSpec> {
        let mut specs = Vec::with_capacity(self.file_count);

        for i in 0..self.file_count {
            let dir_name = DIRS[i % DIRS.len()];
            let ext = EXTENSIONS[i % EXTENSIONS.len()];
            let path = format!("{dir_name}/file_{i:04}.{ext}");
            let domain = DOMAINS[i % DOMAINS.len()];
            let unique = format!("{}_{i:04}", self.unique_needle_prefix);
            let has_common = i < self.files_with_common;

            let mut content = String::with_capacity(512);
            content.push_str(&format!("// {unique}\n"));
            content.push_str(domain);
            content.push('\n');
            if has_common {
                content.push_str(&format!("// {}\n", self.common_needle));
            }
            // Pad with filler to make files multi-line
            for j in 0..5 {
                content.push_str(&format!("// filler line {j} for {path}\n"));
            }

            dir.create(&path, &content);
            specs.push(FileSpec {
                path,
                unique_needle: unique,
                unique_line: 1,
                has_common,
            });
        }

        specs
    }
}

pub struct FileSpec {
    pub path: String,
    pub unique_needle: String,
    pub unique_line: u64,
    pub has_common: bool,
}

pub const SMALL_REPO: SyntheticRepo = SyntheticRepo {
    file_count: 50,
    unique_needle_prefix: "NEEDLE",
    common_needle: "COMMON_MARKER_XYZ",
    files_with_common: 30,
};

pub const MEDIUM_REPO: SyntheticRepo = SyntheticRepo {
    file_count: 200,
    unique_needle_prefix: "NEEDLE",
    common_needle: "COMMON_MARKER_XYZ",
    files_with_common: 120,
};

pub const LARGE_REPO: SyntheticRepo = SyntheticRepo {
    file_count: 500,
    unique_needle_prefix: "NEEDLE",
    common_needle: "COMMON_MARKER_XYZ",
    files_with_common: 300,
};
