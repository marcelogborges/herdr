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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub status: String,
    pub title: String,
    pub url: String,
    pub repository: String,
}

impl PullRequest {
    pub(crate) fn label(&self) -> String {
        let number = self.url.rsplit('/').next().unwrap_or_default();
        if self.repository.is_empty() || number.is_empty() {
            self.url.clone()
        } else {
            format!("{}#{number}", self.repository)
        }
    }
}

pub(crate) fn pull_request_instance_types(summary: &Value) -> Vec<String> {
    summary
        .get("summary")
        .and_then(|summary| summary.get("pullrequest"))
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
            })
        })
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
                .and_then(development_summary),
            pull_requests: Vec::new(),
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

fn development_summary(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let json_start = text.find("json=")? + "json=".len();
    let json_text = text.get(json_start..)?.trim_end().strip_suffix('}')?;
    let parsed: Value = serde_json::from_str(json_text).ok()?;
    let summary = parsed.get("cachedValue")?.get("summary")?;
    let parts = [
        ("pullrequest", "PR", "PRs"),
        ("branch", "branch", "branches"),
        ("commit", "commit", "commits"),
        ("repository", "repositório", "repositórios"),
    ]
    .into_iter()
    .filter_map(|(kind, singular, plural)| {
        let overall = summary.get(kind)?.get("overall")?;
        let count = overall.get("count")?.as_u64().filter(|count| *count > 0)?;
        let noun = if count == 1 { singular } else { plural };
        let state = overall
            .get("state")
            .and_then(Value::as_str)
            .map(|state| format!(" ({})", state.to_lowercase()))
            .unwrap_or_default();
        Some(format!("{count} {noun}{state}"))
    })
    .collect::<Vec<_>>();
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
            Some("2 PRs (open) · 1 repositório")
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
            pull_request_instance_types(&summary),
            ["oAuth-com.github.integration.production"]
        );
        let detail = serde_json::json!({"detail": [{"pullRequests": [
            {"status": "OPEN", "name": "VK25-1: x", "url": "https://github.com/vakinha/vakinha-web/pull/5783", "repositoryName": "vakinha/vakinha-web"},
            {"status": "MERGED", "name": "sem url"}
        ]}]});
        let prs = pull_requests_from_detail(&detail);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].label(), "vakinha/vakinha-web#5783");
        assert_eq!(prs[0].status, "OPEN");
        assert!(pull_request_instance_types(&serde_json::json!({})).is_empty());
    }
}
