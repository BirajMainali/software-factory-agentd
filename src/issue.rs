use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn fetch_issue(url: &str, dir: &Path) -> Result<(), String> {
    let (owner, repo, number) = parse_issue_url(url)?;
    let target = format!("https://github.com/{owner}/{repo}/issues/{number}");

    let output = Command::new("gh")
        .args([
            "issue",
            "view",
            &target,
            "--json",
            "number,title,body,author,state,labels,createdAt,updatedAt,url,comments",
        ])
        .output()
        .map_err(|e| format!("failed to run gh (is GitHub CLI installed?): {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh issue view failed: {}", stderr.trim()));
    }

    let issue: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("gh returned invalid JSON: {e}"))?;

    if is_pull_request(&issue) {
        return Err(format!("{url} is a pull request, not an issue"));
    }

    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join("issue.md");
    fs::write(&path, render_issue(&issue))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    println!("wrote {}", path.display());
    Ok(())
}

fn parse_issue_url(url: &str) -> Result<(String, String, u64), String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or("expected a full URL like https://github.com/owner/repo/issues/123")?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() != 4 || parts[2] != "issues" {
        return Err("expected a full URL like https://github.com/owner/repo/issues/123".into());
    }

    let (owner, repo, number) = (parts[0], parts[1], parts[3]);
    for part in [owner, repo] {
        let valid = !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !valid {
            return Err(format!("invalid GitHub owner or repository: {part}"));
        }
    }

    let number: u64 = number
        .parse()
        .map_err(|_| format!("invalid issue number: {number}"))?;
    if number == 0 {
        return Err("issue number must be greater than zero".into());
    }

    Ok((owner.into(), repo.into(), number))
}

fn render_issue(issue: &Value) -> String {
    let mut out = format!("# {}\n\n", text(issue, "title"));
    out.push_str(&format!("- URL: {}\n", text(issue, "url")));
    out.push_str(&format!("- Author: @{}\n", author_login(&issue["author"])));
    out.push_str(&format!("- State: {}\n", text(issue, "state")));

    let labels = labels(issue);
    if !labels.is_empty() {
        out.push_str(&format!("- Labels: {labels}\n"));
    }

    out.push_str(&format!("- Created: {}\n", text(issue, "createdAt")));
    out.push_str(&format!("- Updated: {}\n", text(issue, "updatedAt")));
    out.push_str("\n## Body\n\n");
    out.push_str(text(issue, "body").trim_end());
    out.push('\n');

    if let Some(comments) = issue["comments"].as_array().filter(|c| !c.is_empty()) {
        out.push_str("\n## Comments\n");
        for comment in comments {
            out.push_str(&format!(
                "\n### @{} - {}\n\n",
                author_login(&comment["author"]),
                text(comment, "createdAt")
            ));
            out.push_str(text(comment, "body").trim_end());
            out.push('\n');
        }
    }

    out
}

fn is_pull_request(issue: &Value) -> bool {
    issue["url"].as_str().and_then(|url| url.split('/').nth(5)) == Some("pull")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn author_login(author: &Value) -> &str {
    author["login"].as_str().unwrap_or("ghost")
}

fn labels(issue: &Value) -> String {
    issue["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_full_issue_url() {
        assert_eq!(
            parse_issue_url("https://github.com/acme/widgets/issues/42").unwrap(),
            ("acme".into(), "widgets".into(), 42)
        );
        assert_eq!(
            parse_issue_url("https://github.com/acme/widgets/issues/42/")
                .unwrap()
                .2,
            42
        );
    }

    #[test]
    fn rejects_pull_requests_and_bad_urls() {
        assert!(parse_issue_url("https://github.com/acme/widgets/pull/42").is_err());
        assert!(parse_issue_url("acme/widgets#42").is_err());
        assert!(parse_issue_url("https://github.com/acme/widgets/issues/nope").is_err());
        assert!(parse_issue_url("https://github.com/acme/widgets/issues/0").is_err());
    }

    #[test]
    fn detects_pull_request_urls() {
        assert!(is_pull_request(&json!({
            "url": "https://github.com/acme/widgets/pull/42"
        })));
        assert!(!is_pull_request(&json!({
            "url": "https://github.com/acme/widgets/issues/42"
        })));
    }

    #[test]
    fn renders_issue_and_comments() {
        let issue = json!({
            "title": "Crash on start",
            "body": "Steps:\n1. run it",
            "url": "https://github.com/acme/widgets/issues/42",
            "author": {"login": "alice"},
            "state": "OPEN",
            "labels": [{"name": "bug"}, {"name": "runtime"}],
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "comments": [{
                "author": {"login": "bob"},
                "body": "Same here",
                "createdAt": "2026-01-03T00:00:00Z"
            }]
        });

        let md = render_issue(&issue);
        assert!(md.starts_with("# Crash on start\n"));
        assert!(md.contains("- Labels: bug, runtime"));
        assert!(md.contains("Steps:\n1. run it"));
        assert!(md.contains("### @bob - 2026-01-03T00:00:00Z"));
        assert!(md.contains("Same here"));
        assert!(md.ends_with('\n'));
    }
}
