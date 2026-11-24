//! GitLab merge requests for the pull request lists, through `glab api`
//! (the GitLab CLI, signed in on this device) — GitHub's `gh` counterpart.

use chrono::{DateTime, Utc};
use commutator_proto::{
    ChangeRequestActor, ChangeRequestFilter, ChangeRequestListItem, ChangeRequestMergeability,
    ChangeRequestPage, ChangeRequestState,
};
use serde::Deserialize;

use super::{
    ChangeRequestError, GITHUB_OUTPUT_LIMIT, GITHUB_TIMEOUT, GitHubCli, ProcessRequest,
    classify_run_error, host_from_authority,
};

/// A GitLab project: its host and full path (`group/sub/project`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabProject {
    pub host: String,
    pub path: String,
}

/// The project behind a non-GitHub remote, nested groups included.
pub fn parse_remote(remote_url: &str) -> Option<GitLabProject> {
    let remote_url = remote_url.trim();
    let (authority, path) = match remote_url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme.to_ascii_lowercase().as_str(), "ssh" | "http" | "https") {
                return None;
            }
            rest.split_once('/')?
        }
        None => {
            let (authority, path) = remote_url.split_once(':')?;
            if authority.contains('/') || path.starts_with('/') {
                return None;
            }
            (authority, path)
        }
    };
    let host = host_from_authority(authority)?.to_ascii_lowercase();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let valid = path.split('/').count() >= 2
        && path.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        });
    (valid && host != "github.com").then(|| GitLabProject {
        host,
        path: path.to_owned(),
    })
}

#[derive(Deserialize)]
struct MergeRequest {
    iid: u64,
    title: String,
    web_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    author: Option<User>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    has_conflicts: bool,
    #[serde(default)]
    detailed_merge_status: Option<String>,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Deserialize)]
struct User {
    username: String,
}

impl GitHubCli {
    /// Whether `project`'s host is a GitLab this device can reach: named so,
    /// or signed in to with `glab`.
    pub async fn is_gitlab(&self, project: &GitLabProject) -> bool {
        if project.host.contains("gitlab") {
            return true;
        }
        let request = glab_request(vec![
            "auth".into(),
            "status".into(),
            "--hostname".into(),
            project.host.clone(),
        ]);
        self.runner
            .run(request)
            .await
            .is_ok_and(|output| output.success)
    }

    /// The open merge requests of `project`, most recently updated first.
    // ponytail: first 50 only, no CI/approval state (the list endpoint has none).
    pub async fn list_gitlab_page(
        &self,
        project: &GitLabProject,
        filter: ChangeRequestFilter,
    ) -> Result<ChangeRequestPage, ChangeRequestError> {
        let scope = match filter {
            ChangeRequestFilter::Authored => "&scope=created_by_me".to_owned(),
            ChangeRequestFilter::All => "&scope=all".to_owned(),
            ChangeRequestFilter::Reviewing => {
                let me: User = self.glab_json(project, "user".into()).await?;
                format!("&scope=all&reviewer_username={}", me.username)
            }
        };
        let endpoint = format!(
            "projects/{}/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page=50{scope}",
            project.path.replace('/', "%2F"),
        );
        let requests: Vec<MergeRequest> = self.glab_json(project, endpoint).await?;
        let items = requests
            .into_iter()
            .map(|mr| item(project, filter, mr))
            .collect();
        Ok(ChangeRequestPage {
            items,
            next_cursor: None,
            total_count: None,
            fetched_at: Some(Utc::now()),
        })
    }

    async fn glab_json<T: serde::de::DeserializeOwned>(
        &self,
        project: &GitLabProject,
        endpoint: String,
    ) -> Result<T, ChangeRequestError> {
        let request = glab_request(vec![
            "api".into(),
            "--hostname".into(),
            project.host.clone(),
            endpoint,
        ]);
        let output = self.runner.run(request).await.map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)
    }
}

fn glab_request(args: Vec<String>) -> ProcessRequest {
    ProcessRequest {
        program: "glab".into(),
        args,
        stdin: None,
        cwd: None,
        env: vec![("NO_PROMPT".into(), "1".into())],
        timeout: GITHUB_TIMEOUT,
        output_limit: GITHUB_OUTPUT_LIMIT,
    }
}

fn classify_failure(stderr: &[u8]) -> ChangeRequestError {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("429") || stderr.contains("rate limit") {
        ChangeRequestError::RateLimited
    } else if stderr.contains("401")
        || stderr.contains("unauthorized")
        || stderr.contains("glab auth login")
        || stderr.contains("not logged")
    {
        ChangeRequestError::Authentication
    } else {
        ChangeRequestError::CommandFailed
    }
}

fn item(project: &GitLabProject, filter: ChangeRequestFilter, mr: MergeRequest) -> ChangeRequestListItem {
    ChangeRequestListItem {
        provider: "gitlab".into(),
        author: ChangeRequestActor {
            login: mr.author.map(|a| a.username).unwrap_or_default(),
        },
        repository: project.path.clone(),
        head_ref_oid: mr.sha.unwrap_or_default(),
        ci: Default::default(),
        viewer_did_author: (filter == ChangeRequestFilter::Authored).then_some(true),
        viewer_review_requested: (filter == ChangeRequestFilter::Reviewing).then_some(true),
        number: mr.iid,
        title: mr.title.split_whitespace().collect::<Vec<_>>().join(" "),
        url: mr.web_url,
        state: ChangeRequestState::Open,
        is_draft: mr.draft,
        review_decision: Default::default(),
        additions: 0,
        deletions: 0,
        mergeability: if mr.has_conflicts {
            ChangeRequestMergeability::Conflicting
        } else if mr.detailed_merge_status.as_deref() == Some("mergeable") {
            ChangeRequestMergeability::Mergeable
        } else {
            ChangeRequestMergeability::Unknown
        },
        created_at: mr.created_at,
        updated_at: mr.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_keep_nested_groups_and_skip_github() {
        let want = Some(GitLabProject {
            host: "gitlab.com".into(),
            path: "acme/platform/app".into(),
        });
        assert_eq!(parse_remote("git@gitlab.com:acme/platform/app.git"), want);
        assert_eq!(parse_remote("https://oauth2:t@gitlab.com/acme/platform/app/"), want);
        assert_eq!(parse_remote("https://github.com/acme/app"), None);
        assert_eq!(parse_remote("https://gitlab.com/app"), None);
    }

    #[test]
    fn merge_requests_become_list_items() {
        let mr: MergeRequest = serde_json::from_value(serde_json::json!({
            "iid": 7, "title": "Fix  it", "web_url": "https://gitlab.com/a/b/-/merge_requests/7",
            "draft": false, "author": { "username": "ann" },
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00.000+03:00",
            "has_conflicts": true, "detailed_merge_status": "broken_status", "sha": "abc",
        }))
        .unwrap();
        let project = parse_remote("git@gitlab.com:a/b.git").unwrap();
        let item = item(&project, ChangeRequestFilter::Authored, mr);
        assert_eq!((item.number, item.title.as_str(), item.author.login.as_str()), (7, "Fix it", "ann"));
        assert_eq!(item.repository, "a/b");
        assert_eq!(item.provider, "gitlab");
        assert_eq!(item.mergeability, ChangeRequestMergeability::Conflicting);
        assert_eq!(item.viewer_did_author, Some(true));
    }
}
