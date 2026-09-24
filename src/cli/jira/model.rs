use serde_json::Value;

use super::api::DetailFields;

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum StatusCategory {
    New,
    InProgress,
    Done,
}

impl StatusCategory {
    fn from_key(key: &str) -> Self {
        match key {
            "new" => Self::New,
            "done" => Self::Done,
            _ => Self::InProgress,
        }
    }

    fn rank(self) -> usize {
        match self {
            Self::InProgress => 0,
            Self::New => 1,
            Self::Done => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Issue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub category: StatusCategory,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub issue_type: String,
    pub updated: String,
    pub parent: Option<(String, String)>,
}

impl Issue {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let key = value.get("key")?.as_str()?.to_owned();
        let fields = value.get("fields")?;
        let status = fields.get("status");
        Some(Self {
            key,
            summary: str_at(fields, &["summary"]).unwrap_or_default(),
            status: status
                .and_then(|status| str_at(status, &["name"]))
                .unwrap_or_else(|| "?".to_owned()),
            category: status
                .and_then(|status| str_at(status, &["statusCategory", "key"]))
                .map(|key| StatusCategory::from_key(&key))
                .unwrap_or(StatusCategory::InProgress),
            priority: str_at(fields, &["priority", "name"]),
            assignee: str_at(fields, &["assignee", "displayName"]),
            issue_type: str_at(fields, &["issuetype", "name"]).unwrap_or_default(),
            updated: str_at(fields, &["updated"]).unwrap_or_default(),
            parent: fields.get("parent").and_then(|parent| {
                Some((
                    parent.get("key")?.as_str()?.to_owned(),
                    str_at(parent, &["fields", "summary"]).unwrap_or_default(),
                ))
            }),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Comment {
    pub author: String,
    pub created: String,
    pub body: Value,
}

impl Comment {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            author: str_at(value, &["author", "displayName"]).unwrap_or_default(),
            created: str_at(value, &["created"]).unwrap_or_default(),
            body: value.get("body")?.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IssueDetail {
    pub issue: Issue,
    pub description: Option<Value>,
    pub comments: Vec<Comment>,
    pub sprint: Option<String>,
    pub story_points: Option<f64>,
    pub development: Option<String>,
    pub pull_requests: Vec<PullRequest>,
    pub worktrees: Vec<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    pub repo: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}

impl Worktree {
    pub(crate) fn pull_request<'a>(
        &self,
        pull_requests: &'a [PullRequest],
    ) -> Option<&'a PullRequest> {
        let branch = self.branch.as_deref()?;
        let same_repo = |repository: &str| {
            repository.is_empty()
                || repository == self.repo
                || repository.ends_with(&format!("/{}", self.repo))
        };
        pull_requests
            .iter()
            .filter(|pull_request| {
                pull_request.branch.as_deref() == Some(branch)
                    && same_repo(&pull_request.repository)
            })
            .min_by_key(|pull_request| !pull_request.status.eq_ignore_ascii_case("open"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub status: String,
    pub title: String,
    pub url: String,
    pub repository: String,
    pub branch: Option<String>,
}

impl PullRequest {
    pub(crate) fn number(&self) -> Option<&str> {
        self.url
            .rsplit('/')
            .next()
            .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    }

    pub(crate) fn label(&self) -> String {
        let number = self.url.rsplit('/').next().unwrap_or_default();
        if self.repository.is_empty() || number.is_empty() {
            self.url.clone()
        } else {
            format!("{}#{number}", self.repository)
        }
    }
}

pub(crate) fn instance_types(summary: &Value, kind: &str) -> Vec<String> {
    summary
        .get("summary")
        .and_then(|summary| summary.get(kind))
        .and_then(|pr| pr.get("byInstanceType"))
        .and_then(Value::as_object)
        .map(|types| types.keys().cloned().collect())
        .unwrap_or_default()
}

pub(crate) fn pull_requests_from_detail(detail: &Value) -> Vec<PullRequest> {
    detail
        .get("detail")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("pullRequests").and_then(Value::as_array))
        .flatten()
        .filter_map(|pr| {
            Some(PullRequest {
                status: str_at(pr, &["status"]).unwrap_or_default(),
                title: str_at(pr, &["name"]).unwrap_or_default(),
                url: str_at(pr, &["url"])?,
                repository: str_at(pr, &["repositoryName"]).unwrap_or_default(),
                branch: str_at(pr, &["source", "branch"]),
            })
        })
        .collect()
}

pub(crate) fn repository_names(detail: &Value) -> Vec<String> {
    detail
        .get("detail")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("repositories").and_then(Value::as_array))
        .flatten()
        .filter_map(|repository| str_at(repository, &["name"]))
        .collect()
}

impl IssueDetail {
    pub(crate) fn from_json(value: &Value, fields_ids: &DetailFields) -> Option<Self> {
        let issue = Issue::from_json(value)?;
        let fields = value.get("fields")?;
        Some(Self {
            issue,
            description: fields
                .get("description")
                .filter(|value| !value.is_null())
                .cloned(),
            comments: fields
                .get("comment")
                .and_then(|comment| comment.get("comments"))
                .and_then(Value::as_array)
                .map(|comments| comments.iter().filter_map(Comment::from_json).collect())
                .unwrap_or_default(),
            sprint: fields.get(&fields_ids.sprint).and_then(sprint_name),
            story_points: fields.get(&fields_ids.story_points).and_then(Value::as_f64),
            development: fields
                .get(&fields_ids.development)
                .and_then(cached_development_summary),
            pull_requests: Vec::new(),
            worktrees: Vec::new(),
        })
    }
}

fn sprint_name(value: &Value) -> Option<String> {
    let sprints = value.as_array()?;
    let active = sprints
        .iter()
        .find(|sprint| sprint.get("state").and_then(Value::as_str) == Some("active"))
        .or_else(|| sprints.last())?;
    str_at(active, &["name"])
}

fn cached_development_summary(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let json_start = text.find("json=")? + "json=".len();
    let json_text = text.get(json_start..)?.trim_end().strip_suffix('}')?;
    let parsed: Value = serde_json::from_str(json_text).ok()?;
    development_label(parsed.get("cachedValue")?.get("summary")?, None)
}

fn plural(count: u64, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

pub(crate) fn development_label(summary: &Value, repositories: Option<usize>) -> Option<String> {
    let overall = |kind: &str| {
        let overall = summary.get(kind)?.get("overall")?;
        let count = overall.get("count")?.as_u64().filter(|count| *count > 0)?;
        Some((count, overall))
    };
    let mut parts = Vec::new();
    if let Some((count, overall)) = overall("pullrequest") {
        let state = overall
            .get("state")
            .and_then(Value::as_str)
            .map(|state| format!(" ({})", state.to_lowercase()))
            .unwrap_or_default();
        parts.push(format!("{}{state}", plural(count, "PR", "PRs")));
    }
    if let Some((count, _)) = overall("branch") {
        parts.push(plural(count, "branch", "branches"));
    }
    if let Some((count, _)) = overall("repository").or_else(|| overall("commit")) {
        let commits = plural(count, "commit", "commits");
        parts.push(
            match repositories.filter(|repositories| *repositories > 0) {
                Some(repositories) => format!(
                    "{commits} em {}",
                    plural(repositories as u64, "repositório", "repositórios")
                ),
                None => commits,
            },
        );
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transition {
    pub id: String,
    pub name: String,
    pub to: String,
    pub required_fields: Vec<String>,
}

impl Transition {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let mut required_fields = value
            .get("fields")
            .and_then(Value::as_object)
            .map(|fields| {
                fields
                    .iter()
                    .filter(|(_, field)| {
                        field.get("required").and_then(Value::as_bool) == Some(true)
                    })
                    .map(|(id, field)| str_at(field, &["name"]).unwrap_or_else(|| id.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        required_fields.sort();
        Some(Self {
            id: value.get("id")?.as_str()?.to_owned(),
            name: str_at(value, &["name"]).unwrap_or_default(),
            to: str_at(value, &["to", "name"]).unwrap_or_default(),
            required_fields,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StatusGroup {
    pub status: String,
    pub issues: Vec<Issue>,
}

pub(crate) fn group_issues(issues: &[Issue], status_order: &[String]) -> Vec<StatusGroup> {
    let mut groups: Vec<StatusGroup> = Vec::new();
    for issue in issues {
        match groups.iter_mut().find(|group| group.status == issue.status) {
            Some(group) => group.issues.push(issue.clone()),
            None => groups.push(StatusGroup {
                status: issue.status.clone(),
                issues: vec![issue.clone()],
            }),
        }
    }
    let rank = |group: &StatusGroup| {
        let position = status_order
            .iter()
            .position(|status| status.eq_ignore_ascii_case(&group.status));
        let category = group.issues[0].category.rank();
        (
            position.is_none(),
            position.unwrap_or(usize::MAX),
            category,
            group.status.to_lowercase(),
        )
    };
    groups.sort_by_key(rank);
    groups
}

pub(crate) fn key_matches(text: &str, key: &str) -> bool {
    let Some(rest) = strip_prefix_ignore_case(text, key) else {
        return false;
    };
    !rest.starts_with(|c: char| c.is_ascii_digit())
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

pub(crate) fn priority_icon(priority: Option<&str>) -> &'static str {
    match priority.map(str::to_lowercase).as_deref() {
        Some("highest") | Some("blocker") | Some("critical") => "⇈",
        Some("high") => "↑",
        Some("medium") => "=",
        Some("low") => "↓",
        Some("lowest") | Some("trivial") => "⇊",
        _ => " ",
    }
}

fn str_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue(key: &str, status: &str, category: &str) -> Issue {
        Issue::from_json(&json!({
            "key": key,
            "fields": {
                "summary": format!("resumo {key}"),
                "status": {"name": status, "statusCategory": {"key": category}},
            }
        }))
        .unwrap()
    }

    #[test]
    fn groups_follow_configured_order_then_category() {
        let issues = vec![
            issue("A-1", "Backlog", "new"),
            issue("A-2", "Code Review", "indeterminate"),
            issue("A-3", "Development", "indeterminate"),
            issue("A-4", "Algo novo", "indeterminate"),
            issue("A-5", "Code Review", "indeterminate"),
            issue("A-6", "Outro novo", "new"),
        ];
        let order = ["Development", "Code Review", "Backlog"].map(str::to_owned);

        let groups = group_issues(&issues, &order);

        let names: Vec<_> = groups.iter().map(|group| group.status.as_str()).collect();
        assert_eq!(
            names,
            [
                "Development",
                "Code Review",
                "Backlog",
                "Algo novo",
                "Outro novo"
            ]
        );
        assert_eq!(groups[1].issues.len(), 2);
    }

    #[test]
    fn key_matching_rejects_longer_numbers() {
        assert!(key_matches("VK25-2727", "VK25-2727"));
        assert!(key_matches("VK25-2727-api", "VK25-2727"));
        assert!(key_matches("vk25-2727_web", "VK25-2727"));
        assert!(!key_matches("VK25-27270", "VK25-2727"));
        assert!(!key_matches("projects", "VK25-2727"));
    }

    #[test]
    fn detail_reads_sprint_points_and_pull_requests() {
        let value = json!({
            "key": "VK25-9",
            "fields": {
                "summary": "s",
                "status": {"name": "Code Review", "statusCategory": {"key": "indeterminate"}},
                "description": {"type": "doc", "content": []},
                "comment": {"comments": [{"author": {"displayName": "Ana"}, "created": "2026-09-01T00:00:00.000+0000", "body": {"type": "doc"}}]},
                "customfield_10020": [{"name": "Sprint 1", "state": "closed"}, {"name": "Sprint 2", "state": "active"}],
                "customfield_10016": 3.0,
                "customfield_10000": "{pullrequest={dataType=pullrequest, state=OPEN, stateCount=1}, json={\"cachedValue\":{\"errors\":[],\"summary\":{\"pullrequest\":{\"overall\":{\"count\":2,\"state\":\"OPEN\"}},\"repository\":{\"overall\":{\"count\":1}}}},\"isStale\":true}}",
                "parent": {"key": "VK25-1", "fields": {"summary": "pai"}}
            }
        });

        let detail =
            IssueDetail::from_json(&value, &super::super::api::tests::test_fields()).unwrap();

        assert_eq!(detail.sprint.as_deref(), Some("Sprint 2"));
        assert_eq!(detail.story_points, Some(3.0));
        assert_eq!(
            detail.development.as_deref(),
            Some("2 PRs (open) · 1 commit")
        );
        assert_eq!(detail.comments[0].author, "Ana");
        assert_eq!(detail.issue.parent, Some(("VK25-1".into(), "pai".into())));
    }

    #[test]
    fn transitions_collect_required_fields() {
        let value = json!({
            "id": "5",
            "name": "Resolver",
            "to": {"name": "Concluído"},
            "fields": {
                "resolution": {"required": true, "name": "Resolução"},
                "comment": {"required": false, "name": "Comentário"}
            }
        });

        let transition = Transition::from_json(&value).unwrap();

        assert_eq!(transition.to, "Concluído");
        assert_eq!(transition.required_fields, ["Resolução"]);
    }

    #[test]
    fn priority_icons_cover_common_names() {
        assert_eq!(priority_icon(Some("High")), "↑");
        assert_eq!(priority_icon(Some("Lowest")), "⇊");
        assert_eq!(priority_icon(None), " ");
    }

    #[test]
    fn pull_requests_come_from_dev_status_summary_and_detail() {
        let summary = serde_json::json!({"summary": {"pullrequest": {"byInstanceType": {"oAuth-com.github.integration.production": {}}}}});
        assert_eq!(
            instance_types(&summary, "pullrequest"),
            ["oAuth-com.github.integration.production"]
        );
        let detail = serde_json::json!({"detail": [{"pullRequests": [
            {"status": "OPEN", "name": "VK25-1: x", "url": "https://github.com/vakinha/vakinha-web/pull/5783", "repositoryName": "vakinha/vakinha-web", "source": {"branch": "task/VK25-1/x"}},
            {"status": "MERGED", "name": "sem url"}
        ]}]});
        let prs = pull_requests_from_detail(&detail);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].label(), "vakinha/vakinha-web#5783");
        assert_eq!(prs[0].status, "OPEN");
        assert_eq!(prs[0].branch.as_deref(), Some("task/VK25-1/x"));
        assert_eq!(prs[0].number(), Some("5783"));
        assert!(instance_types(&serde_json::json!({}), "pullrequest").is_empty());
    }

    #[test]
    fn development_label_counts_commits_and_repositories() {
        let summary = serde_json::json!({
            "pullrequest": {"overall": {"count": 1, "state": "MERGED"}},
            "branch": {"overall": {"count": 0}},
            "repository": {"overall": {"count": 5}}
        });
        assert_eq!(
            development_label(&summary, Some(1)).as_deref(),
            Some("1 PR (merged) · 5 commits em 1 repositório")
        );
        assert_eq!(
            development_label(&summary, None).as_deref(),
            Some("1 PR (merged) · 5 commits")
        );
        let many = serde_json::json!({
            "pullrequest": {"overall": {"count": 3, "state": "OPEN"}},
            "branch": {"overall": {"count": 2}},
            "repository": {"overall": {"count": 1}}
        });
        assert_eq!(
            development_label(&many, Some(2)).as_deref(),
            Some("3 PRs (open) · 2 branches · 1 commit em 2 repositórios")
        );
        let branch_only = serde_json::json!({"branch": {"overall": {"count": 1}}});
        assert_eq!(
            development_label(&branch_only, Some(0)).as_deref(),
            Some("1 branch")
        );
        assert_eq!(development_label(&serde_json::json!({}), None), None);
        assert!(!development_label(&summary, Some(3))
            .unwrap()
            .contains("5 repositórios"));
    }

    #[test]
    fn repository_names_come_from_dev_status_repository_detail() {
        let detail = serde_json::json!({"detail": [{"repositories": [
            {"name": "vakinha/vakinha-web", "commits": [{}, {}]},
            {"name": "vakinha/vakinha-api"}
        ]}]});
        assert_eq!(
            repository_names(&detail),
            ["vakinha/vakinha-web", "vakinha/vakinha-api"]
        );
    }

    #[test]
    fn worktree_matches_pull_request_by_exact_source_branch() {
        let pull_request = |branch: &str| PullRequest {
            status: "OPEN".into(),
            title: String::new(),
            url: "https://github.com/v/api/pull/12".into(),
            repository: "v/api".into(),
            branch: Some(branch.into()),
        };
        let prs = [pull_request("task/VK25-1/a"), pull_request("task/VK25-1/b")];
        let other_repo = PullRequest {
            repository: "v/admin-api".into(),
            url: "https://github.com/v/admin-api/pull/9".into(),
            ..pull_request("task/VK25-1/b")
        };
        let declined = PullRequest {
            status: "DECLINED".into(),
            url: "https://github.com/v/api/pull/11".into(),
            ..pull_request("task/VK25-1/b")
        };
        let mixed = [declined, other_repo.clone(), pull_request("task/VK25-1/b")];
        let worktree = |branch: Option<&str>| Worktree {
            repo: "api".into(),
            name: "VK25-1".into(),
            path: "/r/api-worktrees/VK25-1".into(),
            branch: branch.map(str::to_owned),
        };
        assert_eq!(
            worktree(Some("task/VK25-1/b")).pull_request(&prs),
            Some(&prs[1])
        );
        assert_eq!(worktree(Some("task/VK25-1")).pull_request(&prs), None);
        assert_eq!(
            worktree(Some("task/VK25-1/b"))
                .pull_request(&mixed)
                .and_then(PullRequest::number),
            Some("12")
        );
        let admin = Worktree {
            repo: "admin-api".into(),
            ..worktree(Some("task/VK25-1/b"))
        };
        assert_eq!(admin.pull_request(&mixed), Some(&other_repo));
        assert_eq!(worktree(None).pull_request(&prs), None);
    }
}
