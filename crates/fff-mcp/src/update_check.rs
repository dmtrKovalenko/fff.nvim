//! Background update checker — compares the embedded build hash against
//! the latest GitHub release tag to surface upgrade notices in MCP instructions.

use std::sync::OnceLock;

const REPO: &str = "dmtrKovalenko/fff.nvim";
const BUILD_HASH: &str = env!("FFF_GIT_HASH");

/// Holds the result of the update check (empty string = up to date or check failed).
static UPDATE_NOTICE: OnceLock<String> = OnceLock::new();

/// Returns the update notice if the check has completed, empty string otherwise.
pub fn get_update_notice() -> &'static str {
    UPDATE_NOTICE.get().map(|s| s.as_str()).unwrap_or("")
}

/// Kick off the update check in a background thread so it never blocks the server.
pub fn spawn_update_check() {
    std::thread::spawn(|| {
        let notice = check_latest_release();
        let _ = UPDATE_NOTICE.set(notice);
    });
}

/// Fetch the latest release tag from GitHub and compare against the build hash.
fn check_latest_release() -> String {
    match fetch_latest_tag() {
        Ok(tag) => compare_versions(BUILD_HASH, &tag),
        Err(_) => String::new(),
    }
}

/// Compare a build hash against a release tag.
/// Returns an update notice string, or empty if up-to-date.
fn compare_versions(build_hash: &str, release_tag: &str) -> String {
    let tag = release_tag.trim();
    if tag.is_empty() || build_hash == "unknown" {
        return String::new();
    }

    let our_short = &build_hash[..build_hash.len().min(tag.len())];
    if our_short == tag {
        return String::new();
    }

    format!(
        "\n[fff update available: `curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install-mcp.sh | bash`]\n"
    )
}

/// Shell out to curl to fetch the latest release tag name from GitHub API.
fn fetch_latest_tag() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &format!("https://api.github.com/repos/{REPO}/releases?per_page=1"),
        ])
        .output()?;

    if !output.status.success() {
        return Err("curl failed".into());
    }

    let body = String::from_utf8(output.stdout)?;
    let releases: Vec<serde_json::Value> = serde_json::from_str(&body)?;
    let tag = releases
        .first()
        .and_then(|r| r.get("tag_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_check_end_to_end() {
        // Fetch the actual latest release tag from GitHub
        let tag = fetch_latest_tag().expect("Failed to fetch latest release tag from GitHub");
        assert!(!tag.is_empty(), "Release tag should not be empty");

        let notice = compare_versions(BUILD_HASH, &tag);
        let build_short = &BUILD_HASH[..BUILD_HASH.len().min(7)];

        if BUILD_HASH.starts_with(tag.trim()) || tag.trim().starts_with(BUILD_HASH) {
            // If by chance we're on the exact release commit
            assert!(notice.is_empty(), "Should be empty when hashes match");
        } else {
            assert!(
                notice.contains("fff update available"),
                "Expected update notice for mismatched hashes (build: {}, release: {}), got: '{}'",
                build_short,
                tag.trim(),
                notice
            );
            assert!(
                notice.contains(tag.trim()),
                "Notice should contain release tag"
            );
            assert!(
                notice.contains(build_short),
                "Notice should contain our short hash"
            );
            assert!(
                notice.contains("install-mcp.sh"),
                "Notice should contain install command"
            );
        }
    }
}
