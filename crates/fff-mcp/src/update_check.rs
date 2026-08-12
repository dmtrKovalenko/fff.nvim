use std::sync::OnceLock;

const REPO: &str = "dmtrKovalenko/fff";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

static UPDATE_NOTICE: OnceLock<String> = OnceLock::new();

pub fn get_update_notice() -> &'static str {
    UPDATE_NOTICE.get().map(|s| s.as_str()).unwrap_or("")
}

pub fn spawn_update_check() {
    std::thread::spawn(|| {
        let notice = check_latest_release();
        let _ = UPDATE_NOTICE.set(notice);
    });
}

fn check_latest_release() -> String {
    match fetch_latest_stable_tag() {
        Ok(tag) => compare_versions(CURRENT_VERSION, &tag),
        Err(_) => String::new(),
    }
}

fn compare_versions(current_version: &str, release_tag: &str) -> String {
    let tag = release_tag.trim();
    let tag_version = tag.strip_prefix('v').unwrap_or(tag);
    if tag.is_empty() || tag_version == current_version {
        return String::new();
    }

    format!(
        "\n[fff update available ({current_version} -> {tag_version}): `curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install-mcp.sh | bash`]\n"
    )
}

// Uses /releases/latest — GitHub excludes prereleases here, matching the
// stable channel that install-mcp.sh installs from.
fn fetch_latest_stable_tag() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()?;

    if !output.status.success() {
        return Err("curl failed".into());
    }

    let body = String::from_utf8(output.stdout)?;
    let release: serde_json::Value = serde_json::from_str(&body)?;
    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::compare_versions;

    #[test]
    fn same_version_with_v_prefix_is_silent() {
        assert_eq!(compare_versions("0.10.1", "v0.10.1"), "");
    }

    #[test]
    fn same_version_without_v_prefix_is_silent() {
        assert_eq!(compare_versions("0.10.1", "0.10.1"), "");
    }

    #[test]
    fn empty_tag_is_silent() {
        assert_eq!(compare_versions("0.10.1", ""), "");
        assert_eq!(compare_versions("0.10.1", "   "), "");
    }

    #[test]
    fn older_current_reports_update() {
        let notice = compare_versions("0.10.0", "v0.10.1");
        assert!(notice.contains("0.10.0 -> 0.10.1"), "got: {notice}");
        assert!(notice.contains("install-mcp.sh"));
    }

    #[test]
    fn nightly_tag_never_equals_stable_current() {
        let notice = compare_versions("0.10.1", "0.10.2-nightly.6a239e9");
        assert!(!notice.is_empty());
        assert!(notice.contains("0.10.1 -> 0.10.2-nightly.6a239e9"));
    }
}
