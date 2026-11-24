//! The phone's Pull requests page: every project repository's pull (GitHub)
//! and merge (GitLab) requests
//! (`commutator_client::pull_requests`), grouped and labelled the way the
//! desktop board does, so both platforms only paint rows.

use commutator_proto::change_request_assessment::{AttentionReason, Blocker, CiState, Facts};
use commutator_proto::{
    ChangeRequestFilter, ChangeRequestListItem, ChangeRequestMergeability,
    ChangeRequestReviewDecision,
};

use super::CoreClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PullRequestFilter {
    /// Opened by you.
    Authored,
    /// Your review requested.
    Reviewing,
    All,
}

/// The desktop board's sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PullRequestSection {
    Attention,
    Review,
    Approved,
    Drafts,
}

/// The glyph's color: open (success), draft (muted), conflict (danger),
/// changes requested (warning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PullRequestTone {
    Open,
    Draft,
    Conflict,
    ChangesRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PullRequestCi {
    Passed,
    Pending,
    Failed,
    /// No checks, skipped, or unknown.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PullRequestItem {
    pub url: String,
    /// `owner/repo` (GitLab: `group/sub/project`).
    pub repository: String,
    /// `github` · `gitlab`.
    pub provider: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub section: PullRequestSection,
    pub tone: PullRequestTone,
    pub is_draft: bool,
    /// "Changes requested · Your review requested"; empty when nothing to say.
    pub status: String,
    pub ci: PullRequestCi,
    pub additions: u64,
    pub deletions: u64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PullRequestFailureInfo {
    pub repository: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PullRequestList {
    /// Most recently updated first.
    pub items: Vec<PullRequestItem>,
    pub failures: Vec<PullRequestFailureInfo>,
    /// No project with a GitHub or GitLab repository on an online computer.
    pub no_repositories: bool,
}

#[uniffi::export]
impl CoreClient {
    /// Pull requests across every project repository. `refresh` asks GitHub;
    /// otherwise each computer's stored copy answers at once when it has one.
    pub async fn list_pull_requests(
        &self,
        filter: PullRequestFilter,
        refresh: bool,
    ) -> PullRequestList {
        let client = self.client.clone();
        let filter = match filter {
            PullRequestFilter::Authored => ChangeRequestFilter::Authored,
            PullRequestFilter::Reviewing => ChangeRequestFilter::Reviewing,
            PullRequestFilter::All => ChangeRequestFilter::All,
        };
        let listing = commutator_client::runtime::run(async move {
            Ok::<_, commutator_client::ClientError>(client.pull_requests(filter, refresh).await)
        })
        .await
        .unwrap_or_default();
        PullRequestList {
            items: listing.items.iter().map(item).collect(),
            failures: listing
                .failures
                .into_iter()
                .map(|f| PullRequestFailureInfo {
                    message: failure_message(&f.error),
                    repository: f.repository,
                })
                .collect(),
            no_repositories: listing.no_repositories,
        }
    }
}

fn item(p: &ChangeRequestListItem) -> PullRequestItem {
    let assessment = Facts::from_item(p).assess();
    let section = if p.is_draft {
        PullRequestSection::Drafts
    } else if !assessment.blockers.is_empty() || !assessment.attention.is_empty() {
        PullRequestSection::Attention
    } else if p.review_decision == ChangeRequestReviewDecision::Approved {
        PullRequestSection::Approved
    } else {
        PullRequestSection::Review
    };
    let tone = if p.is_draft {
        PullRequestTone::Draft
    } else if p.mergeability == ChangeRequestMergeability::Conflicting {
        PullRequestTone::Conflict
    } else if p.review_decision == ChangeRequestReviewDecision::ChangesRequested {
        PullRequestTone::ChangesRequested
    } else {
        PullRequestTone::Open
    };
    // The desktop row's status line; CI has its own mark.
    let mut labels: Vec<&str> = assessment
        .blockers
        .iter()
        .filter(|b| **b != Blocker::FailingCi)
        .map(|b| b.label())
        .collect();
    if p.review_decision == ChangeRequestReviewDecision::Approved {
        labels.push("Approved");
    }
    labels.extend(
        assessment
            .attention
            .iter()
            .filter(|r| **r != AttentionReason::FailingCi)
            .map(|r| r.label()),
    );
    PullRequestItem {
        url: p.url.clone(),
        repository: p.repository.clone(),
        provider: p.provider.clone(),
        number: p.number,
        title: p.title.split_whitespace().collect::<Vec<_>>().join(" "),
        author: p.author.login.clone(),
        section,
        tone,
        is_draft: p.is_draft,
        status: labels.join(" · "),
        ci: match p.ci.state {
            CiState::Passed => PullRequestCi::Passed,
            CiState::Pending => PullRequestCi::Pending,
            CiState::Failed => PullRequestCi::Failed,
            CiState::NoChecks | CiState::Skipped | CiState::Unknown => PullRequestCi::None,
        },
        additions: p.additions,
        deletions: p.deletions,
        updated_at_ms: p.updated_at.timestamp_millis(),
    }
}

/// The desktop board's words for a computer's GitHub errors.
fn failure_message(error: &str) -> String {
    use commutator_rpc::capability_errors as codes;
    if error.contains(codes::PULL_REQUESTS_CLI_UNAVAILABLE) {
        "Install gh (GitHub) or glab (GitLab) on that computer".into()
    } else if error.contains(codes::PULL_REQUESTS_AUTHENTICATION) {
        "Sign in on that computer (gh auth login / glab auth login)".into()
    } else if error.contains(codes::PULL_REQUESTS_RATE_LIMITED) {
        "The rate limit was reached".into()
    } else if error.contains("not supported") {
        "Update Commutator on that computer".into()
    } else {
        "Couldn’t load pull requests".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_follow_the_desktop_board() {
        let base: ChangeRequestListItem = serde_json::from_value(serde_json::json!({
            "provider": "github", "repository": "o/r", "number": 1, "title": "  a\n b ",
            "url": "https://github.com/o/r/pull/1", "state": "open", "isDraft": false,
            "reviewDecision": "approved", "additions": 1, "deletions": 2,
            "mergeability": "mergeable", "headRefOid": "a".repeat(40),
            "ci": { "state": "passed", "totalCount": 1 },
            "viewerDidAuthor": true, "viewerReviewRequested": false,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
        }))
        .unwrap();
        let approved = item(&base);
        assert_eq!(approved.section, PullRequestSection::Approved);
        assert_eq!(approved.title, "a b");
        assert_eq!(approved.tone, PullRequestTone::Open);

        let mut draft = base.clone();
        draft.is_draft = true;
        assert_eq!(item(&draft).section, PullRequestSection::Drafts);

        let mut conflict = base;
        conflict.mergeability = ChangeRequestMergeability::Conflicting;
        let conflict = item(&conflict);
        assert_eq!(conflict.section, PullRequestSection::Attention);
        assert_eq!(conflict.tone, PullRequestTone::Conflict);
        assert!(
            conflict.status.contains("Merge conflicts"),
            "{}",
            conflict.status
        );
    }
}
