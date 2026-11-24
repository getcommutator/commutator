//! Pull requests of every repository the account's projects live in, read
//! from GitHub by the devices that hold them (`ListChangeRequestPage`), on
//! one list: the phone's Pull requests page.

use std::collections::HashSet;

use commutator_proto::{ChangeRequestFilter, ChangeRequestListItem, ChangeRequestPage};
use commutator_rpc::methods;

use crate::Client;
use crate::error::ClientError;

/// A repository whose pull requests couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestFailure {
    pub repository: String,
    pub device_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullRequestListing {
    /// Every repository's first page, most recently updated first.
    pub items: Vec<ChangeRequestListItem>,
    pub failures: Vec<PullRequestFailure>,
    /// No project repository on an online device that GitHub or GitLab knows.
    pub no_repositories: bool,
}

impl Client {
    /// First page of `filter`'s pull requests in each GitHub repository with a
    /// project on an online device, asked of that device. `refresh` reaches
    /// GitHub; otherwise the device's stored copy answers when it has one.
    // ponytail: first page per repository only; paging when someone has 50+ open in one repo.
    pub async fn pull_requests(
        &self,
        filter: ChangeRequestFilter,
        refresh: bool,
    ) -> PullRequestListing {
        if self.inner.is_demo() {
            return PullRequestListing::default();
        }
        // A GitHub repository by name; any other remote (GitLab) by one of
        // its checkouts, which that device resolves.
        let mut seen = HashSet::new();
        let targets: Vec<(Target, String)> = self
            .workspace()
            .projects
            .iter()
            .filter(|p| p.device_online && p.git_detected)
            .map(|p| match &p.github_repo {
                Some(repo) => (Target::Repository(repo.clone()), p.device_id.clone()),
                None => (
                    Target::Checkout(p.path.clone(), p.group_key.clone()),
                    p.device_id.clone(),
                ),
            })
            .filter(|(target, _)| seen.insert(target.key()))
            .collect();
        let pages = futures::future::join_all(targets.iter().map(|(target, device)| {
            let mut params = serde_json::json!({
                "targetDeviceId": device,
                "filter": filter,
                "refresh": refresh,
                "cached": !refresh,
            });
            match target {
                Target::Repository(repo) => params["repository"] = repo.as_str().into(),
                Target::Checkout(path, _) => params["cwd"] = path.as_str().into(),
            }
            async move {
                let value = self
                    .inner
                    .host_rpc(device, methods::LIST_CHANGE_REQUEST_PAGE, params)
                    .await?;
                serde_json::from_value::<ChangeRequestPage>(value)
                    .map_err(|e| ClientError::HostError(e.to_string()))
            }
        }))
        .await;

        let mut listing = PullRequestListing {
            no_repositories: true,
            ..Default::default()
        };
        for ((target, device_id), page) in targets.into_iter().zip(pages) {
            let error = match page {
                Ok(page) => {
                    listing.no_repositories = false;
                    listing.items.extend(page.items);
                    continue;
                }
                Err(error) => error.to_string(),
            };
            // A checkout on neither GitHub nor GitLab (or an engine that
            // predates `cwd`): nothing to list, not a failure.
            let silent =
                error.contains("pull_requests.unsupported") || error.contains("repository must be");
            if matches!(target, Target::Checkout(..)) && silent {
                continue;
            }
            listing.no_repositories = false;
            listing.failures.push(PullRequestFailure {
                repository: target.label(),
                device_id,
                error,
            });
        }
        listing
            .items
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        listing
    }
}

enum Target {
    /// GitHub `owner/repo`.
    Repository(String),
    /// A checkout's path and its repository group.
    Checkout(String, String),
}

impl Target {
    fn key(&self) -> String {
        match self {
            Self::Repository(repo) => repo.to_ascii_lowercase(),
            Self::Checkout(_, group) => format!("group:{group}"),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Repository(repo) => repo.clone(),
            Self::Checkout(path, _) => path.rsplit('/').next().unwrap_or(path).to_owned(),
        }
    }
}
