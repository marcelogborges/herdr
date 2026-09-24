use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};

use super::model::{
    pull_request_instance_types, pull_requests_from_detail, Comment, Issue, IssueDetail,
    PullRequest, Transition,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const SEARCH_FIELDS: &str = "summary,status,priority,assignee,issuetype,updated,parent";
const MAX_RESULTS: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JiraError {
    Unauthorized,
    Forbidden,
    NotFound,
    RateLimited,
    Http(u16, String),
    Network(String),
    Parse(String),
}

impl std::fmt::Display for JiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(
                f,
                "Jira recusou as credenciais (401): confira email e token"
            ),
            Self::Forbidden => write!(f, "sem permissão no Jira para esta ação (403)"),
            Self::NotFound => write!(f, "não encontrado no Jira (404)"),
            Self::RateLimited => write!(
                f,
                "Jira limitou as requisições (429); tente de novo em instantes"
            ),
            Self::Http(status, message) if message.is_empty() => {
                write!(f, "Jira respondeu {status}")
            }
            Self::Http(status, message) => write!(f, "Jira respondeu {status}: {message}"),
            Self::Network(message) => write!(f, "falha de rede ao falar com o Jira: {message}"),
            Self::Parse(message) => write!(f, "resposta inesperada do Jira: {message}"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct JiraApi {
    site: String,
    email: String,
    token: String,
    fields: DetailFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailFields {
    pub sprint: String,
    pub story_points: String,
    pub development: String,
}

impl std::fmt::Debug for JiraApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraApi")
            .field("site", &self.site)
            .field("email", &self.email)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl JiraApi {
    pub(crate) fn new(site: &str, email: &str, token: String, fields: DetailFields) -> Self {
        Self {
            site: site.trim_end_matches('/').to_owned(),
            email: email.to_owned(),
            token,
            fields,
        }
    }

    pub(crate) fn myself(&self) -> Result<(String, String), JiraError> {
        let value = self.request("GET", "/rest/api/3/myself", None)?;
        let account_id = value
            .get("accountId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| JiraError::Parse("myself sem accountId".into()))?;
        let name = value
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok((account_id, name))
    }

    pub(crate) fn search(&self, jql: &str) -> Result<Vec<Issue>, JiraError> {
        let path = format!(
            "/rest/api/3/search/jql?jql={}&fields={SEARCH_FIELDS}&maxResults={MAX_RESULTS}",
            percent_encode(jql)
        );
        let value = self.request("GET", &path, None)?;
        let issues = value
            .get("issues")
            .and_then(Value::as_array)
            .ok_or_else(|| JiraError::Parse("busca sem lista de issues".into()))?;
        Ok(issues.iter().filter_map(Issue::from_json).collect())
    }

    pub(crate) fn issue_detail(&self, key: &str) -> Result<IssueDetail, JiraError> {
        let fields = format!(
            "{SEARCH_FIELDS},description,comment,{},{},{}",
            self.fields.sprint, self.fields.story_points, self.fields.development
        );
        let path = format!("/rest/api/3/issue/{}?fields={fields}", percent_encode(key));
        let value = self.request("GET", &path, None)?;
        let mut detail = IssueDetail::from_json(&value, &self.fields)
            .ok_or_else(|| JiraError::Parse(format!("issue {key} incompleta")))?;
        if let Some(issue_id) = value.get("id").and_then(Value::as_str) {
            detail.pull_requests = self.pull_requests(issue_id);
        }
        Ok(detail)
    }

    fn pull_requests(&self, issue_id: &str) -> Vec<PullRequest> {
        let summary_path = format!(
            "/rest/dev-status/latest/issue/summary?issueId={}",
            percent_encode(issue_id)
        );
        let Ok(summary) = self.request("GET", &summary_path, None) else {
            return Vec::new();
        };
        pull_request_instance_types(&summary)
            .into_iter()
            .filter_map(|application| {
                let path = format!(
                    "/rest/dev-status/latest/issue/detail?issueId={}&applicationType={}&dataType=pullrequest",
                    percent_encode(issue_id),
                    percent_encode(&application)
                );
                self.request("GET", &path, None).ok()
            })
            .flat_map(|detail| pull_requests_from_detail(&detail))
            .collect()
    }

    pub(crate) fn transitions(&self, key: &str) -> Result<Vec<Transition>, JiraError> {
        let path = format!(
            "/rest/api/3/issue/{}/transitions?expand=transitions.fields",
            percent_encode(key)
        );
        let value = self.request("GET", &path, None)?;
        let transitions = value
            .get("transitions")
            .and_then(Value::as_array)
            .ok_or_else(|| JiraError::Parse("resposta sem transições".into()))?;
        Ok(transitions
            .iter()
            .filter_map(Transition::from_json)
            .collect())
    }

    pub(crate) fn apply_transition(&self, key: &str, transition_id: &str) -> Result<(), JiraError> {
        let path = format!("/rest/api/3/issue/{}/transitions", percent_encode(key));
        self.request(
            "POST",
            &path,
            Some(json!({ "transition": { "id": transition_id } })),
        )
        .map(|_| ())
    }

    pub(crate) fn add_comment(&self, key: &str, text: &str) -> Result<Comment, JiraError> {
        let path = format!("/rest/api/3/issue/{}/comment", percent_encode(key));
        let value = self.request(
            "POST",
            &path,
            Some(json!({ "body": super::adf::text_to_adf(text) })),
        )?;
        Ok(Comment::from_json(&value).unwrap_or_else(|| Comment {
            author: String::new(),
            created: String::new(),
            body: super::adf::text_to_adf(text),
        }))
    }

    pub(crate) fn assign(&self, key: &str, account_id: &str) -> Result<(), JiraError> {
        let path = format!("/rest/api/3/issue/{}/assignee", percent_encode(key));
        self.request("PUT", &path, Some(json!({ "accountId": account_id })))
            .map(|_| ())
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, JiraError> {
        let config = curl_config(
            method,
            &format!("{}{path}", self.site),
            &self.email,
            &self.token,
            body.as_ref(),
        );
        let mut child = crate::noninteractive_process::curl_command()
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| JiraError::Network(format!("curl indisponível: {err}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|err| JiraError::Network(err.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| JiraError::Network(err.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let Some((body, status)) = split_status(&stdout) else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr.lines().last().unwrap_or("sem resposta").trim();
            return Err(JiraError::Network(message.to_owned()));
        };
        parse_response(status, body)
    }
}

pub(crate) fn curl_config(
    method: &str,
    url: &str,
    email: &str,
    token: &str,
    body: Option<&Value>,
) -> String {
    let mut lines = vec![
        "silent".to_owned(),
        "show-error".to_owned(),
        format!("max-time = {}", REQUEST_TIMEOUT.as_secs()),
        format!("request = {}", quote(method)),
        format!("url = {}", quote(url)),
        format!("user = {}", quote(&format!("{email}:{token}"))),
        format!("header = {}", quote("Accept: application/json")),
        format!("write-out = {}", quote("\n%{http_code}")),
    ];
    if let Some(body) = body {
        lines.push(format!(
            "header = {}",
            quote("Content-Type: application/json")
        ));
        lines.push(format!("data-binary = {}", quote(&body.to_string())));
    }
    lines.join("\n") + "\n"
}

fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn split_status(stdout: &str) -> Option<(&str, u16)> {
    let (body, status) = stdout.rsplit_once('\n')?;
    let status = status
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|status| *status != 0)?;
    Some((body, status))
}

pub(crate) fn parse_response(status: u16, body: &str) -> Result<Value, JiraError> {
    match status {
        200..=299 => {
            if body.trim().is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_str(body).map_err(|err| JiraError::Parse(err.to_string()))
            }
        }
        401 => Err(JiraError::Unauthorized),
        403 => Err(JiraError::Forbidden),
        404 => Err(JiraError::NotFound),
        429 => Err(JiraError::RateLimited),
        other => Err(JiraError::Http(other, error_message(body))),
    }
}

fn error_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    let mut messages = value
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(errors) = value.get("errors").and_then(Value::as_object) {
        messages.extend(
            errors
                .iter()
                .filter_map(|(field, message)| Some(format!("{field}: {}", message.as_str()?))),
        );
    }
    messages.join("; ")
}

pub(crate) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub(crate) struct Recorded {
        pub method: String,
        pub path: String,
        pub authorization: String,
        pub body: String,
    }

    pub(crate) struct MockJira {
        pub site: String,
        pub requests: Arc<Mutex<Vec<Recorded>>>,
    }

    pub(crate) fn mock_jira(routes: Vec<(&'static str, &'static str, u16, String)>) -> MockJira {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let mut content_length = 0;
                let mut authorization = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        authorization = header["authorization:".len()..].trim().to_owned();
                    }
                }
                let mut body = vec![0; content_length];
                let _ = reader.read_exact(&mut body);
                let (status, response) = routes
                    .iter()
                    .find(|(route_method, prefix, _, _)| {
                        *route_method == method && path.starts_with(prefix)
                    })
                    .map(|(_, _, status, response)| (*status, response.clone()))
                    .unwrap_or((404, "{\"errorMessages\":[\"no route\"]}".to_owned()));
                recorded.lock().unwrap().push(Recorded {
                    method,
                    path,
                    authorization,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        MockJira { site, requests }
    }

    pub(crate) fn test_fields() -> DetailFields {
        DetailFields {
            sprint: "customfield_10020".into(),
            story_points: "customfield_10016".into(),
            development: "customfield_10000".into(),
        }
    }

    pub(crate) fn api_for(mock: &MockJira) -> JiraApi {
        JiraApi::new(
            &mock.site,
            "me@example.com",
            "s3cr3t-token".into(),
            test_fields(),
        )
    }

    const SEARCH: &str = r#"{"issues":[{"key":"VK25-1","fields":{"summary":"Primeira","status":{"name":"Code Review","statusCategory":{"key":"indeterminate"}},"priority":{"name":"High"},"assignee":{"displayName":"Eu"},"issuetype":{"name":"Task"},"updated":"2026-09-23T10:00:00.000-0300"}}]}"#;

    #[test]
    fn search_encodes_jql_and_parses_issues() {
        let mock = mock_jira(vec![("GET", "/rest/api/3/search/jql", 200, SEARCH.into())]);
        let issues = api_for(&mock)
            .search("project = VK25 AND statusCategory != Done")
            .unwrap();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "VK25-1");
        assert_eq!(issues[0].status, "Code Review");
        let requests = mock.requests.lock().unwrap();
        assert!(requests[0]
            .path
            .contains("jql=project%20%3D%20VK25%20AND%20statusCategory%20%21%3D%20Done"));
        assert!(requests[0].authorization.starts_with("Basic "));
    }

    #[test]
    fn mutations_send_expected_payloads() {
        let mock = mock_jira(vec![
            ("POST", "/rest/api/3/issue/VK25-1/transitions", 204, String::new()),
            (
                "POST",
                "/rest/api/3/issue/VK25-1/comment",
                201,
                r#"{"author":{"displayName":"Eu"},"created":"2026-09-23T10:00:00.000-0300","body":{"type":"doc","version":1,"content":[]}}"#.into(),
            ),
            ("PUT", "/rest/api/3/issue/VK25-1/assignee", 204, String::new()),
        ]);
        let api = api_for(&mock);

        api.apply_transition("VK25-1", "31").unwrap();
        let comment = api.add_comment("VK25-1", "linha 1\nlinha 2").unwrap();
        api.assign("VK25-1", "acc-1").unwrap();

        assert_eq!(comment.author, "Eu");
        let requests = mock.requests.lock().unwrap();
        let bodies: Vec<Value> = requests
            .iter()
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect();
        assert_eq!(bodies[0], json!({"transition": {"id": "31"}}));
        assert_eq!(bodies[1]["body"]["content"].as_array().unwrap().len(), 2);
        assert_eq!(bodies[2], json!({"accountId": "acc-1"}));
        assert_eq!(requests[2].method, "PUT");
    }

    #[test]
    fn http_errors_map_to_friendly_variants() {
        let mock = mock_jira(vec![
            ("GET", "/rest/api/3/myself", 401, "{}".into()),
            ("GET", "/rest/api/3/issue/FORBID", 403, "{}".into()),
            ("GET", "/rest/api/3/issue/LIMIT", 429, "{}".into()),
            (
                "GET",
                "/rest/api/3/issue/BAD",
                400,
                r#"{"errorMessages":["ruim"],"errors":{"field":"obrigatório"}}"#.into(),
            ),
        ]);
        let api = api_for(&mock);

        assert_eq!(api.myself(), Err(JiraError::Unauthorized));
        assert_eq!(
            api.issue_detail("FORBID").unwrap_err(),
            JiraError::Forbidden
        );
        assert_eq!(
            api.issue_detail("MISSING").unwrap_err(),
            JiraError::NotFound
        );
        assert_eq!(
            api.issue_detail("LIMIT").unwrap_err(),
            JiraError::RateLimited
        );
        assert_eq!(
            api.issue_detail("BAD").unwrap_err(),
            JiraError::Http(400, "ruim; field: obrigatório".into())
        );
    }

    #[test]
    fn network_failures_are_reported_without_the_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let api = JiraApi::new(
            &site,
            "me@example.com",
            "s3cr3t-token".into(),
            test_fields(),
        );

        let error = api.myself().unwrap_err();

        assert!(matches!(error, JiraError::Network(_)));
        assert!(!error.to_string().contains("s3cr3t-token"));
        assert!(!format!("{api:?}").contains("s3cr3t-token"));
    }

    #[test]
    fn curl_config_quotes_values_and_keeps_token_off_argv() {
        let config = curl_config(
            "POST",
            "https://x.atlassian.net/rest",
            "a@b.c",
            "to\"k\\en",
            Some(&json!({"text": "linha \"1\"\n"})),
        );

        assert!(config.contains(r#"user = "a@b.c:to\"k\\en""#));
        assert!(config.contains(r#"request = "POST""#));
        assert!(config.contains(r#"data-binary = "{\"text\":\"linha \\\"1\\\"\\n\"}""#));
        assert!(config.lines().all(|line| !line.contains('\n')));
    }

    #[test]
    fn percent_encoding_keeps_unreserved_characters() {
        assert_eq!(percent_encode("VK25-1_a.b~"), "VK25-1_a.b~");
        assert_eq!(percent_encode("a b=\"c\""), "a%20b%3D%22c%22");
        assert_eq!(percent_encode("ç"), "%C3%A7");
    }
}
