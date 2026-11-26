//! `_rustcode/memory/list` and `_rustcode/memory/forget`: the project and
//! global facts agents saved with `remember`, for Settings › Agents › Memory.
//! Thin wrappers over `crate::memory`. A file that cannot be read answers an
//! error rather than an empty list, so the page never claims nothing is
//! saved when it could not look.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::memory::{MemoryFact, ProjectMemory};

/// List a project's facts and the global ones.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/memory/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryListRequest {
    /// Any folder of the project: a worktree reads its repository's file.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Forget one fact, or every fact of the project.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/memory/forget", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryForgetRequest {
    #[serde(default)]
    pub cwd: Option<String>,
    /// With `key`, names exactly one fact.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    /// Every fact of the project in `cwd`; global facts stay.
    #[serde(default)]
    pub all: bool,
    /// The fact is a global one, not the project's.
    #[serde(default)]
    pub global: bool,
}

/// Answer one of the two methods, or hand anything else on as unhandled.
pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    let work: Box<dyn FnOnce() -> Result<Value, String> + Send> = match method.as_str() {
        "_rustcode/memory/list" => {
            let request: MemoryListRequest = super::parse_params(params)?;
            Box::new(move || list(request))
        }
        "_rustcode/memory/forget" => {
            let request: MemoryForgetRequest = super::parse_params(params)?;
            Box::new(move || forget(request))
        }
        _ => {
            *unhandled = Some(agent_client_protocol::Dispatch::Request(
                agent_client_protocol::UntypedMessage { method, params },
                responder,
            ));
            return Ok(());
        }
    };
    connection.spawn(async move {
        match tokio::task::spawn_blocking(work).await {
            Ok(Ok(answer)) => responder.respond(answer),
            Ok(Err(error)) => responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            Err(error) => responder.respond_with_error(
                agent_client_protocol::Error::internal_error().data(error.to_string()),
            ),
        }?;
        Ok(())
    })
}

fn folder(cwd: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(cwd);
    if !path.is_dir() {
        return Err(format!("{cwd} is not a folder on this computer"));
    }
    Ok(Some(path))
}

pub(crate) fn list(request: MemoryListRequest) -> Result<Value, String> {
    let project = match folder(request.cwd.as_deref())? {
        Some(root) => Some(listing(
            &crate::memory::location(Some(&root)).path,
            crate::memory::load(Some(&root))?,
        )),
        None => None,
    };
    let global = listing(
        &crate::memory::global_location(),
        crate::memory::load_global()?,
    );
    Ok(json!({ "project": project, "global": global }))
}

fn listing(path: &Path, memory: ProjectMemory) -> Value {
    json!({
        "path": path.display().to_string(),
        "facts": memory.facts.iter().map(fact_row).collect::<Vec<_>>(),
    })
}

fn fact_row(fact: &MemoryFact) -> Value {
    json!({
        "category": fact.category,
        "key": fact.key,
        "value": fact.value,
        "source": fact.source,
        "confidence": fact.confidence,
        "tentative": fact.confidence <= crate::memory::TENTATIVE_CONFIDENCE_MAX,
        "updatedAt": fact.updated_at,
    })
}

pub(crate) fn forget(request: MemoryForgetRequest) -> Result<Value, String> {
    let root = folder(request.cwd.as_deref())?;
    if request.all {
        if request.global {
            return Err("forgetting every global fact at once is not supported".into());
        }
        let root = root.ok_or("name the project whose facts to forget")?;
        let removed = crate::memory::load(Some(&root))?.facts.len();
        crate::memory::reset(Some(&root))?;
        return Ok(json!({ "removed": removed }));
    }
    let key = request
        .key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("name the fact to forget")?;
    let spec = match request.category.as_deref().map(str::trim) {
        Some(category) if !category.is_empty() => format!("{category}/{key}"),
        _ => key.to_owned(),
    };
    let removed = if request.global {
        crate::memory::remove_global(&spec)?
    } else {
        let root = root.ok_or("name the project whose fact to forget")?;
        crate::memory::remove(Some(&root), &spec)?
    };
    Ok(json!({ "removed": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forget_request(cwd: &Path) -> MemoryForgetRequest {
        MemoryForgetRequest {
            cwd: Some(cwd.display().to_string()),
            category: None,
            key: None,
            all: false,
            global: false,
        }
    }

    #[test]
    fn list_shows_project_facts_and_forget_removes_one_then_all() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        crate::memory::upsert(
            Some(root),
            crate::memory::fact("build", "runner", "Use bun", "test"),
        )
        .unwrap();
        crate::memory::upsert(
            Some(root),
            crate::memory::fact("style", "runner", "Same key, other category", "test"),
        )
        .unwrap();
        crate::memory::upsert(
            Some(root),
            crate::memory::fact("style", "quotes", "Double", "test"),
        )
        .unwrap();

        let cwd = Some(root.display().to_string());
        let listed = list(MemoryListRequest { cwd: cwd.clone() }).unwrap();
        let facts = listed["project"]["facts"].as_array().unwrap();
        assert_eq!(facts.len(), 3);
        assert!(
            listed["project"]["path"]
                .as_str()
                .unwrap()
                .ends_with(".json")
        );
        assert_eq!(facts[0]["tentative"], false);
        assert!(listed["global"]["facts"].is_array());

        let one = forget(MemoryForgetRequest {
            category: Some("build".into()),
            key: Some("runner".into()),
            ..forget_request(root)
        })
        .unwrap();
        assert_eq!(one["removed"], 1);
        let left = list(MemoryListRequest { cwd: cwd.clone() }).unwrap();
        assert_eq!(left["project"]["facts"].as_array().unwrap().len(), 2);

        let all = forget(MemoryForgetRequest {
            all: true,
            ..forget_request(root)
        })
        .unwrap();
        assert_eq!(all["removed"], 2);
        let empty = list(MemoryListRequest { cwd }).unwrap();
        assert!(empty["project"]["facts"].as_array().unwrap().is_empty());
    }

    #[test]
    fn an_unreadable_file_or_missing_folder_is_an_error_not_an_empty_list() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let path = crate::memory::location(Some(root)).path;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let cwd = Some(root.display().to_string());
        assert!(list(MemoryListRequest { cwd }).is_err());
        std::fs::remove_file(&path).unwrap();

        let gone = list(MemoryListRequest {
            cwd: Some(root.join("missing").display().to_string()),
        });
        assert!(gone.unwrap_err().contains("not a folder"));
        assert!(forget(forget_request(root)).is_err());
    }

    #[test]
    fn the_methods_match_their_names() {
        use agent_client_protocol::JsonRpcMessage;
        assert!(MemoryListRequest::matches_method("_rustcode/memory/list"));
        assert!(MemoryForgetRequest::matches_method(
            "_rustcode/memory/forget"
        ));
    }
}
