use crate::{ObjectId, ObjectKind, StoredObject};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IssueAction {
    Create {
        title: String,
        body: String,
    },
    Edit {
        title: Option<String>,
        body: Option<String>,
    },
    Comment {
        body: String,
    },
    Close,
    Reopen,
    Link {
        target: IssueLink,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum IssueLink {
    Commit(ObjectId),
    Asset(ObjectId),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IssueEvent {
    pub issue: Uuid,
    pub parent: Option<ObjectId>,
    pub action: IssueAction,
    pub author: Option<String>,
    pub timestamp: String,
}
impl StoredObject for IssueEvent {
    const KIND: ObjectKind = ObjectKind::IssueEvent;
    fn validate(&self) -> crate::Result<()> {
        crate::validate_timestamp(&self.timestamp)?;
        if self.issue.get_version_num() != 4 {
            return Err(crate::Error::Integrity(
                "issue identity must be UUIDv4".into(),
            ));
        }
        if self.parent.is_none() != matches!(self.action, IssueAction::Create { .. }) {
            return Err(crate::Error::Integrity(
                "only create events may have no parent".into(),
            ));
        }
        match &self.action {
            IssueAction::Create { title, .. } if title.trim().is_empty() => {
                return Err(crate::Error::Integrity("empty issue title".into()));
            }
            IssueAction::Edit { title, body }
                if (title.is_none() && body.is_none())
                    || title.as_ref().is_some_and(|t| t.trim().is_empty()) =>
            {
                return Err(crate::Error::Integrity("invalid issue edit".into()));
            }
            IssueAction::Comment { body } if body.trim().is_empty() => {
                return Err(crate::Error::Integrity("empty issue comment".into()));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueComment {
    pub body: String,
    pub author: Option<String>,
    pub timestamp: String,
    pub event: ObjectId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    pub id: Uuid,
    pub head: ObjectId,
    pub title: String,
    pub body: String,
    pub status: String,
    pub comments: Vec<IssueComment>,
    pub links: Vec<IssueLink>,
    pub history: Vec<ObjectId>,
}

use crate::{
    Asset, Backend, Commit, Error, MetaChange, Repository, Result, Transaction, parse_pointer,
    pointer, timestamp,
};

impl<B: Backend> Repository<B> {
    pub fn create_issue(&self, title: &str, body: &str) -> Result<Issue> {
        self.ensure_ready()?;
        if title.trim().is_empty() {
            return Err(Error::Invalid("issue title must not be empty".into()));
        }
        let issue = Uuid::new_v4();
        let event = IssueEvent {
            issue,
            parent: None,
            action: IssueAction::Create {
                title: title.into(),
                body: body.into(),
            },
            author: self.backend.author()?,
            timestamp: timestamp()?,
        };
        let head = self.put(&event)?;
        let mut tx = Transaction::new("issue create");
        tx.metadata.push(MetaChange {
            name: format!("refs/issues/{issue}"),
            before: None,
            after: Some(pointer(&head)),
        });
        self.execute(tx)?;
        self.issue(&issue.to_string())
    }

    pub fn list_issues(&self, include_closed: bool) -> Result<Vec<Issue>> {
        let mut issues = Vec::new();
        for name in self.backend.list_meta("refs/issues/")? {
            let issue = self.issue(&name["refs/issues/".len()..])?;
            if include_closed || issue.status == "open" {
                issues.push(issue);
            }
        }
        Ok(issues)
    }

    pub fn issue(&self, selector: &str) -> Result<Issue> {
        if selector.len() < 4 || !selector.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(Error::Invalid(
                "issue ID must be a UUID or an unambiguous prefix of at least four characters"
                    .into(),
            ));
        }
        let selector = selector.to_lowercase();
        let matches: Vec<_> = self
            .backend
            .list_meta("refs/issues/")?
            .into_iter()
            .filter(|n| n["refs/issues/".len()..].starts_with(&selector))
            .collect();
        if matches.len() != 1 {
            return Err(if matches.is_empty() {
                Error::NotFound(format!("issue {selector}"))
            } else {
                Error::Conflict(format!("ambiguous issue ID: {selector}"))
            });
        }
        let name = &matches[0];
        let identity = Uuid::parse_str(&name["refs/issues/".len()..])
            .map_err(|e| Error::Integrity(e.to_string()))?;
        let head = parse_pointer(
            &self
                .backend
                .read_meta(name)?
                .ok_or_else(|| Error::Integrity(format!("missing {name}")))?,
        )?;
        let mut chain = Vec::new();
        let mut next = Some(head.clone());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = next {
            if !seen.insert(id.clone()) {
                return Err(Error::Integrity("issue ancestry cycle".into()));
            }
            let event = self.load::<IssueEvent>(&id)?;
            if event.issue != identity || event.timestamp.parse::<u64>().is_err() {
                return Err(Error::Integrity(
                    "invalid issue event identity or timestamp".into(),
                ));
            }
            next = event.parent.clone();
            chain.push((id, event));
        }
        chain.reverse();
        let mut issue = Issue {
            id: identity,
            head,
            title: String::new(),
            body: String::new(),
            status: "open".into(),
            comments: vec![],
            links: vec![],
            history: vec![],
        };
        for (position, (id, event)) in chain.into_iter().enumerate() {
            if (position == 0) != matches!(event.action, IssueAction::Create { .. }) {
                return Err(Error::Integrity(
                    "issue history must start with exactly one create event".into(),
                ));
            }
            match event.action {
                IssueAction::Create { title, body } => {
                    issue.title = title;
                    issue.body = body;
                }
                IssueAction::Edit { title, body } => {
                    if let Some(title) = title {
                        issue.title = title;
                    }
                    if let Some(body) = body {
                        issue.body = body;
                    }
                }
                IssueAction::Comment { body } => issue.comments.push(IssueComment {
                    body,
                    author: event.author,
                    timestamp: event.timestamp,
                    event: id.clone(),
                }),
                IssueAction::Close => {
                    if issue.status == "closed" {
                        return Err(Error::Integrity(
                            "issue closed twice without reopening".into(),
                        ));
                    }
                    issue.status = "closed".into();
                }
                IssueAction::Reopen => {
                    if issue.status == "open" {
                        return Err(Error::Integrity("open issue reopened".into()));
                    }
                    issue.status = "open".into();
                }
                IssueAction::Link { target } => {
                    self.validate_issue_link(&target)?;
                    if !issue.links.contains(&target) {
                        issue.links.push(target);
                    }
                }
            }
            if issue.title.trim().is_empty() {
                return Err(Error::Integrity("empty issue title".into()));
            }
            issue.history.push(id);
        }
        Ok(issue)
    }

    pub fn update_issue(
        &self,
        selector: &str,
        action: IssueAction,
        expected_head: Option<&ObjectId>,
    ) -> Result<Issue> {
        self.ensure_ready()?;
        let issue = self.issue(selector)?;
        if expected_head.is_some_and(|head| head != &issue.head) {
            return Err(Error::Conflict("issue changed since it was read".into()));
        }
        match &action {
            IssueAction::Create { .. } => return Err(Error::Invalid("use issue create".into())),
            IssueAction::Edit { title, body } => {
                if title.is_none() && body.is_none() {
                    return Err(Error::Invalid("provide a title or body to edit".into()));
                }
                if title.as_ref().is_some_and(|t| t.trim().is_empty()) {
                    return Err(Error::Invalid("issue title must not be empty".into()));
                }
            }
            IssueAction::Comment { body } if body.trim().is_empty() => {
                return Err(Error::Invalid("comment must not be empty".into()));
            }
            IssueAction::Close if issue.status == "closed" => {
                return Err(Error::Conflict("issue is already closed".into()));
            }
            IssueAction::Reopen if issue.status == "open" => {
                return Err(Error::Conflict("issue is already open".into()));
            }
            IssueAction::Link { target } => {
                self.validate_issue_link(target)?;
                if issue.links.contains(target) {
                    return Err(Error::Conflict("issue already has this link".into()));
                }
            }
            _ => {}
        }
        let event = IssueEvent {
            issue: issue.id,
            parent: Some(issue.head.clone()),
            action,
            author: self.backend.author()?,
            timestamp: timestamp()?,
        };
        let head = self.put(&event)?;
        let mut tx = Transaction::new("issue update");
        tx.metadata.push(MetaChange {
            name: format!("refs/issues/{}", issue.id),
            before: Some(pointer(&issue.head)),
            after: Some(pointer(&head)),
        });
        self.execute(tx)?;
        self.issue(&issue.id.to_string())
    }

    pub(crate) fn validate_issue_link(&self, link: &IssueLink) -> Result<()> {
        match link {
            IssueLink::Commit(id) => {
                self.load::<Commit>(id)?;
            }
            IssueLink::Asset(id) => {
                self.load::<Asset>(id)?;
            }
        }
        Ok(())
    }
}
