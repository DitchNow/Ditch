//! Closed desktop integration operations. No arbitrary CLI or API execution.
use crate::{ProjectId, TaskId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum GitHubRequest {
    Status,
    Install,
    Connect,
    Disconnect,
    Repositories {
        page: u32,
    },
    Link {
        project_id: ProjectId,
        repository: String,
    },
    Links {
        project_id: Option<ProjectId>,
    },
    Issues {
        project_id: ProjectId,
        repository_id: u64,
        state: IssueFilter,
        page: u32,
    },
    Import {
        project_id: ProjectId,
        repository_id: u64,
        numbers: Vec<u64>,
    },
    Refresh {
        task_id: TaskId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IssueFilter {
    Open,
    Closed,
    All,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubRepository {
    pub id: u64,
    pub full_name: String,
    pub html_url: String,
    pub has_issues: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubIssue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub state_reason: Option<String>,
    pub html_url: String,
    pub updated_at: String,
    #[serde(default)]
    pub created_at: String,
    pub closed_at: Option<String>,
    pub user: Option<GitHubUser>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    #[serde(default)]
    pub assignees: Vec<GitHubUser>,
    #[serde(default)]
    pub comments: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubLabel {
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubTaskSource {
    pub repository_id: u64,
    pub issue_id: u64,
    pub repository: String,
    pub number: u64,
    pub url: String,
    pub state: String,
    pub last_synced_at: String,
}

pub fn repository_name(input: &str) -> Result<String, String> {
    let input = input.trim();
    let name = input
        .strip_prefix("https://github.com/")
        .or_else(|| input.strip_prefix("ssh://git@github.com/"))
        .or_else(|| input.strip_prefix("git@github.com:"))
        .unwrap_or(input)
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 100
                || *p == "."
                || *p == ".."
                || p.starts_with('-')
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err("Enter OWNER/REPO or a github.com repository URL".into());
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_inputs_cannot_be_commands_or_other_hosts() {
        for good in [
            "DitchNow/Ditch",
            "https://github.com/DitchNow/Ditch.git",
            "git@github.com:DitchNow/Ditch.git",
        ] {
            assert_eq!(repository_name(good).unwrap(), "DitchNow/Ditch");
        }
        for bad in [
            "../repo",
            "--help/x",
            "https://evil.test/a/b",
            "https://token@github.com/a/b",
            "a/b?x=y",
            "a/b;whoami",
            "a/b/c",
        ] {
            assert!(repository_name(bad).is_err());
        }
    }
}
