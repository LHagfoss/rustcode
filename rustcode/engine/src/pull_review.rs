//! The review a pull request receives from the app: its line comments, each
//! on one line or a range of lines, checked and shaped into the provider's
//! request so a review is posted whole or not at all.

use crate::pull_requests::PullError;
use serde::Serialize;
use serde_json::Value;

/// One line comment of a review: `line` in the file on `side` (`RIGHT`
/// for the pull request's version, `LEFT` for the base), from `start_line`
/// on `start_side` when it covers a range.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub side: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
}

fn refused(message: String) -> PullError {
    PullError {
        code: "failed",
        message,
    }
}

/// The provider's review request: one review at `commit_id`, with its
/// comments. A range that starts on its own last line is sent as a single
/// line, and one that starts after it is refused.
pub fn review_payload(
    commit_id: &str,
    event: &str,
    body: &str,
    comments: &[ReviewComment],
) -> Result<Value, PullError> {
    let event = match event {
        "APPROVE" | "REQUEST_CHANGES" | "COMMENT" => event,
        other => return Err(refused(format!("unknown review event {other}"))),
    };
    if commit_id.is_empty() {
        return Err(refused("the review names no commit".to_owned()));
    }
    let mut shaped = Vec::with_capacity(comments.len());
    for comment in comments {
        let side_ok = |side: &str| matches!(side, "LEFT" | "RIGHT");
        if !side_ok(&comment.side) || comment.line == 0 {
            return Err(refused(format!(
                "{} has no line to comment on",
                comment.path
            )));
        }
        let mut comment = comment.clone();
        match comment.start_line {
            Some(start) if start == comment.line => {
                comment.start_line = None;
                comment.start_side = None;
            }
            Some(start) if start == 0 || start > comment.line => {
                return Err(refused(format!(
                    "{} has a range that ends before it starts",
                    comment.path
                )));
            }
            Some(_) => {
                let side = comment
                    .start_side
                    .clone()
                    .unwrap_or_else(|| comment.side.clone());
                if !side_ok(&side) {
                    return Err(refused(format!(
                        "{} has no line to comment on",
                        comment.path
                    )));
                }
                comment.start_side = Some(side);
            }
            None => comment.start_side = None,
        }
        shaped.push(comment);
    }
    Ok(serde_json::json!({
        "commit_id": commit_id,
        "event": event,
        "body": body,
        "comments": shaped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(line: u64, start_line: Option<u64>) -> ReviewComment {
        ReviewComment {
            path: "a.rs".into(),
            line,
            side: "RIGHT".into(),
            body: "Why?".into(),
            start_line,
            start_side: None,
        }
    }

    #[test]
    fn a_review_payload_is_one_review_at_one_commit() {
        let comments = vec![comment(12, None)];
        let payload = review_payload("da4593b", "COMMENT", "Looks fine.", &comments).unwrap();
        assert_eq!(payload["commit_id"], "da4593b");
        assert_eq!(payload["event"], "COMMENT");
        assert_eq!(payload["comments"][0]["line"], 12);
        assert_eq!(payload["comments"][0]["side"], "RIGHT");
        assert!(payload["comments"][0].get("start_line").is_none());
        assert!(review_payload("da4593b", "MERGE", "", &[]).is_err());
        assert!(review_payload("", "COMMENT", "x", &[]).is_err());
        let mut bad = comments.clone();
        bad[0].side = "MIDDLE".into();
        assert!(review_payload("da4593b", "COMMENT", "", &bad).is_err());
    }

    #[test]
    fn a_range_comment_names_its_first_line_and_side() {
        let payload = review_payload("da4593b", "COMMENT", "", &[comment(18, Some(12))]).unwrap();
        assert_eq!(payload["comments"][0]["start_line"], 12);
        assert_eq!(payload["comments"][0]["start_side"], "RIGHT");
        assert_eq!(payload["comments"][0]["line"], 18);
        let single = review_payload("da4593b", "COMMENT", "", &[comment(7, Some(7))]).unwrap();
        assert!(single["comments"][0].get("start_line").is_none());
        assert!(single["comments"][0].get("start_side").is_none());
        assert!(review_payload("da4593b", "COMMENT", "", &[comment(7, Some(9))]).is_err());
        let parsed: ReviewComment = serde_json::from_value(
            serde_json::json!({"path": "a.rs", "line": 3, "side": "LEFT", "body": "x"}),
        )
        .unwrap();
        assert_eq!(parsed.start_line, None);
    }
}
