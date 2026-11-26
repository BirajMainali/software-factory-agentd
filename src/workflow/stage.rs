//! The stage an agent reports when it finishes, in the agent-facing JSON format:
//! `{"stage":"planning","status":"completed"}`,
//! `{"stage":"review","review":"code","status":"change_requested","text":"..."}`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub(crate) enum Stage {
    /// The plan is written.
    Planning,
    /// The change is implemented and committed.
    Development,
    Review(ReviewKind, Verdict),
}

/// Reviews run in this order: security, code, ui/ux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReviewKind {
    Security,
    Code,
    UiUx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Completed,
    ChangeRequested(String),
    NeedMoreEvidence(String),
}

#[derive(Debug, thiserror::Error)]
#[error("not a stage report")]
pub(crate) struct NotAStage;

impl ReviewKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Code => "code",
            Self::UiUx => "uiux",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "security" => Some(Self::Security),
            "code" => Some(Self::Code),
            "uiux" => Some(Self::UiUx),
            _ => None,
        }
    }
}

impl TryFrom<Value> for Stage {
    type Error = NotAStage;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let field = |name: &str| value.get(name).and_then(Value::as_str);
        let status = field("status").ok_or(NotAStage)?;
        match (field("stage").ok_or(NotAStage)?, status) {
            ("planning", "completed") => Ok(Self::Planning),
            ("development", "completed") => Ok(Self::Development),
            ("review", _) => {
                let kind = field("review")
                    .and_then(ReviewKind::parse)
                    .ok_or(NotAStage)?;
                let text = field("text").unwrap_or_default().to_string();
                let verdict = match status {
                    "completed" => Verdict::Completed,
                    "change_requested" => Verdict::ChangeRequested(text),
                    "need_more_evidence" => Verdict::NeedMoreEvidence(text),
                    _ => return Err(NotAStage),
                };
                Ok(Self::Review(kind, verdict))
            }
            _ => Err(NotAStage),
        }
    }
}

impl From<Stage> for Value {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Planning => json!({ "stage": "planning", "status": "completed" }),
            Stage::Development => json!({ "stage": "development", "status": "completed" }),
            Stage::Review(kind, Verdict::Completed) => {
                json!({ "stage": "review", "review": kind.as_str(), "status": "completed" })
            }
            Stage::Review(kind, Verdict::ChangeRequested(text)) => json!({
                "stage": "review",
                "review": kind.as_str(),
                "status": "change_requested",
                "text": text,
            }),
            Stage::Review(kind, Verdict::NeedMoreEvidence(text)) => json!({
                "stage": "review",
                "review": kind.as_str(),
                "status": "need_more_evidence",
                "text": text,
            }),
        }
    }
}
