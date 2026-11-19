use super::*;

#[test]
fn a_one_time_pass_remembers_that_it_ran() {
    assert_eq!(meta("test_one_time_pass"), None);
    set_meta("test_one_time_pass", "1").unwrap();
    assert_eq!(meta("test_one_time_pass").as_deref(), Some("1"));
    set_meta("test_one_time_pass", "2").unwrap();
    assert_eq!(meta("test_one_time_pass").as_deref(), Some("2"));
}

fn turn(thread: &str, started: &str, tokens: (i64, i64), status: &str) -> TurnRow {
    TurnRow {
        thread_id: thread.to_owned(),
        started_at: started.to_owned(),
        finished_at: Some(started.to_owned()),
        harness: Some("codex".to_owned()),
        model: Some("gpt-6-luna".to_owned()),
        input_tokens: Some(tokens.0),
        output_tokens: Some(tokens.1),
        cached_tokens: None,
        cache_write_tokens: None,
        cost_usd: None,
        status: status.to_owned(),
        duration_ms: None,
        time_to_first_token_ms: None,
        model_time_ms: None,
        tool_time_ms: None,
        context_tokens: None,
        context_window: None,
        stop_reason: None,
        tool_calls: vec![ToolCallRow {
            tool_name: "execute".to_owned(),
            success: Some(false),
            exit_code: Some(1),
            at: Some(started.to_owned()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A store with two threads in two projects and three turns, one of them
/// with no reported tokens and none with a cost.
fn usage_store(dir: &Path) -> PathBuf {
    let path = dir.join(DATA_FILE);
    let connection = open(&path).unwrap();
    for (id, project) in [("a", "/repo/a"), ("b", "/repo/b")] {
        upsert_thread(
            &connection,
            &ThreadRow {
                id: id.to_owned(),
                title: Some(format!("Thread {id}")),
                project: Some(project.to_owned()),
                cwd: Some(project.to_owned()),
                harness: Some("codex".to_owned()),
                model: Some("gpt-6-luna".to_owned()),
                effort: None,
                parent_id: (id == "b").then(|| "a".to_owned()),
                parent_title: (id == "b").then(|| "Thread a".to_owned()),
                created_at: Some("2026-11-01T10:00:00Z".to_owned()),
                updated_at: Some("2026-11-01T10:00:00Z".to_owned()),
            },
        )
        .unwrap();
    }
    for spent in [
        turn("a", "2026-11-01T10:00:00Z", (100, 10), "completed"),
        turn("b", "2026-11-02T10:00:00Z", (200, 20), "completed"),
    ] {
        write_turn(&connection, &spent).unwrap();
    }
    // A turn whose harness reported nothing at all.
    write_turn(
        &connection,
        &TurnRow {
            thread_id: "b".to_owned(),
            started_at: "2026-11-03T10:00:00Z".to_owned(),
            finished_at: Some("2026-11-03T10:00:01Z".to_owned()),
            harness: Some("claude-code".to_owned()),
            model: Some("sonnet".to_owned()),
            input_tokens: None,
            output_tokens: None,
            cached_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
            status: "completed".to_owned(),
            duration_ms: None,
            time_to_first_token_ms: None,
            model_time_ms: None,
            tool_time_ms: None,
            context_tokens: None,
            context_window: None,
            stop_reason: None,
            tool_calls: Vec::new(),
            ..Default::default()
        },
    )
    .unwrap();
    path
}

fn rows(answer: &Value) -> Vec<Value> {
    answer["rows"].as_array().cloned().unwrap_or_default()
}

#[test]
fn model_groups_strip_provider_namespaces_and_keep_raw_labels() {
    let dir = tempfile::tempdir().unwrap();
    let path = usage_store(dir.path());
    let connection = open(&path).unwrap();
    connection.execute("DELETE FROM turns", []).unwrap();
    for (index, model) in [
        "gpt-6-luna",
        "opencode/gpt-6-luna",
        "openrouter/openai/gpt-6-luna",
        "vercel-ai-gateway/openai/gpt-6-luna",
    ]
    .iter()
    .enumerate()
    {
        let mut row = turn(
            "a",
            &format!("2026-11-01T10:00:0{index}Z"),
            (10, 1),
            "completed",
        );
        row.model = Some((*model).to_owned());
        write_turn(&connection, &row).unwrap();
    }
    let result = usage_at(&path, None, GroupBy::Model, None).unwrap();
    assert_eq!(result["rows"].as_array().unwrap().len(), 1);
    assert_eq!(result["rows"][0]["model"], "gpt-6-luna");
    assert_eq!(result["rows"][0]["turns"], 4);
    assert!(
        result["rows"][0]["raw_models"]
            .as_str()
            .unwrap()
            .contains("openrouter/openai/gpt-6-luna")
    );
}

#[test]
fn usage_groups_totals_and_scopes_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = usage_store(dir.path());

    let by_harness = usage_at(&path, None, GroupBy::Harness, None).unwrap();
    let listed = rows(&by_harness);
    let harnesses: Vec<&str> = listed
        .iter()
        .map(|row| row["harness"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(harnesses, ["codex", "claude-code"], "{by_harness}");
    let codex = &rows(&by_harness)[0];
    assert_eq!(codex["input_tokens"], 300);
    assert_eq!(codex["output_tokens"], 30);
    assert_eq!(codex["turns"], 2);
    assert_eq!(codex["turns_without_tokens"], 0);
    assert_eq!(codex["turns_without_input_tokens"], 0);
    assert_eq!(codex["turns_without_output_tokens"], 0);
    // Cache columns are NULL on both codex turns; no count is invented.
    assert_eq!(codex["turns_without_cached_tokens"], 2);
    assert_eq!(codex["turns_without_cache_write_tokens"], 2);
    assert_eq!(codex["turns_without_any_tokens"], 2);
    // No turn in this store reported a cost, so both of codex's are missing one.
    assert_eq!(codex["turns_without_cost"], 2);
    // A harness that reported nothing is NULL, never a made-up zero.
    let claude = &rows(&by_harness)[1];
    assert!(claude["input_tokens"].is_null(), "{claude}");
    assert_eq!(claude["turns_without_tokens"], 1);
    assert_eq!(claude["turns_without_input_tokens"], 1);
    assert_eq!(claude["turns_without_output_tokens"], 1);
    assert_eq!(claude["turns_without_cached_tokens"], 1);
    assert_eq!(claude["turns_without_cache_write_tokens"], 1);
    assert_eq!(claude["turns_without_any_tokens"], 1);
    assert_eq!(claude["turns_without_cost"], 1);
    assert!(
        by_harness["totals"]["cost_usd"].is_null(),
        "nothing was priced"
    );
    // Every cache count in the store is NULL: the total stays absent, not 0.
    assert!(
        by_harness["totals"]["cached_tokens"].is_null(),
        "{by_harness}"
    );
    assert!(
        by_harness["totals"]["cache_write_tokens"].is_null(),
        "{by_harness}"
    );
    assert_eq!(by_harness["totals"]["input_tokens"], 300.0);
    assert_eq!(by_harness["totals"]["turns"], 3.0);
    assert_eq!(by_harness["totals"]["turns_without_tokens"], 1.0);
    assert_eq!(by_harness["totals"]["turns_without_any_tokens"], 3.0);
    assert_eq!(by_harness["totals"]["turns_without_input_tokens"], 1.0);
    assert_eq!(by_harness["totals"]["turns_without_output_tokens"], 1.0);
    assert_eq!(by_harness["totals"]["turns_without_cached_tokens"], 3.0);
    assert_eq!(
        by_harness["totals"]["turns_without_cache_write_tokens"],
        3.0
    );
    assert_eq!(by_harness["totals"]["turns_without_cost"], 3.0);
    assert!(
        by_harness["note"]
            .as_str()
            .unwrap()
            .contains("never a price")
    );

    // One project, read from `threads`.
    let scoped = usage_at(&path, None, GroupBy::Project, Some("/repo/a")).unwrap();
    assert_eq!(rows(&scoped).len(), 1);
    assert_eq!(rows(&scoped)[0]["project"], "/repo/a");
    assert_eq!(rows(&scoped)[0]["input_tokens"], 100);

    // `since` is "from that day on", and a month works too, comparing the
    // RFC 3339 timestamp's prefix.
    let day = usage_at(&path, Some("2026-11-02"), GroupBy::Day, None).unwrap();
    assert_eq!(rows(&day).len(), 2, "{day}");
    assert_eq!(rows(&day)[0]["day"], "2026-11-02");
    let month = usage_at(&path, Some("2026-11"), GroupBy::Day, None).unwrap();
    assert_eq!(rows(&month).len(), 3);
    let later = usage_at(&path, Some("2026-12"), GroupBy::Day, None).unwrap();
    assert!(rows(&later).is_empty());

    // Per thread, with its title so a row says which thread it is.
    let threads = usage_at(&path, None, GroupBy::Thread, None).unwrap();
    let first = &rows(&threads)[0];
    assert_eq!(first["thread"], "b");
    assert_eq!(first["title"], "Thread b");
    assert_eq!(first["project"], "/repo/b");
    // The thread ran two harness/model pairs (codex/gpt-6-luna and
    // claude-code/sonnet), so it has no single pair to price and the key
    // stays NULL rather than naming the latest one.
    assert!(first["model"].is_null(), "{first}");
    assert!(first["model_harness"].is_null(), "{first}");
    assert_eq!(first["parent_id"], "a");
    assert_eq!(first["parent_title"], "Thread a");
}

#[test]
fn a_thread_is_priced_only_when_its_turns_share_one_harness_and_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = usage_store(dir.path());
    let answer = usage_at(&path, None, GroupBy::Thread, None).unwrap();
    let listed = rows(&answer);
    let row = |id: &str| {
        listed
            .iter()
            .find(|row| row["thread"] == id)
            .unwrap_or_else(|| panic!("no row for {id} in {answer}"))
    };

    // "a" ran one turn, so its harness and model are the pair to price.
    let single = row("a");
    assert_eq!(single["model"], "gpt-6-luna", "{single}");
    assert_eq!(single["model_harness"], "codex", "{single}");
    // Its display harness still comes from `threads`/the latest turn.
    assert_eq!(single["harness"], "codex", "{single}");

    // "b" mixes codex/gpt-6-luna with claude-code/sonnet, so neither key
    // is named: one rate would be wrong for the other harness's turns.
    let mixed = row("b");
    assert!(mixed["model"].is_null(), "{mixed}");
    assert!(mixed["model_harness"].is_null(), "{mixed}");
    // The display harness is unchanged (the store's latest one).
    assert_eq!(mixed["harness"], "claude-code", "{mixed}");
}

#[test]
fn usage_counts_each_missing_token_column() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    upsert_thread(
        &connection,
        &ThreadRow {
            id: "t".to_owned(),
            title: Some("Thread t".to_owned()),
            project: Some("/repo/t".to_owned()),
            cwd: Some("/repo/t".to_owned()),
            harness: Some("codex".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            effort: None,
            parent_id: None,
            parent_title: None,
            created_at: None,
            updated_at: None,
        },
    )
    .unwrap();
    let spent = |started: &str,
                 input: Option<i64>,
                 output: Option<i64>,
                 cached: Option<i64>,
                 cache_write: Option<i64>| TurnRow {
        thread_id: "t".to_owned(),
        started_at: started.to_owned(),
        finished_at: Some(started.to_owned()),
        harness: Some("codex".to_owned()),
        model: Some("gpt-6-luna".to_owned()),
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached,
        cache_write_tokens: cache_write,
        cost_usd: None,
        status: "completed".to_owned(),
        duration_ms: None,
        time_to_first_token_ms: None,
        model_time_ms: None,
        tool_time_ms: None,
        context_tokens: None,
        context_window: None,
        stop_reason: None,
        tool_calls: Vec::new(),
        ..Default::default()
    };
    for turn in [
        spent("2026-11-01T10:00:00Z", Some(10), Some(2), Some(3), Some(1)),
        spent("2026-11-02T10:00:00Z", Some(5), None, None, None),
        spent("2026-11-03T10:00:00Z", None, Some(4), None, Some(2)),
        spent("2026-11-04T10:00:00Z", None, None, None, None),
    ] {
        write_turn(&connection, &turn).unwrap();
    }

    let answer = usage_at(&path, None, GroupBy::Harness, None).unwrap();
    let row = &rows(&answer)[0];
    assert_eq!(row["turns"], 4);
    assert_eq!(row["turns_without_tokens"], 1, "only the all-NULL turn");
    assert_eq!(row["turns_without_any_tokens"], 3);
    assert_eq!(row["turns_without_input_tokens"], 2);
    assert_eq!(row["turns_without_output_tokens"], 2);
    assert_eq!(row["turns_without_cached_tokens"], 3);
    assert_eq!(row["turns_without_cache_write_tokens"], 2);
    assert_eq!(answer["totals"]["input_tokens"], 15.0);
    assert_eq!(answer["totals"]["output_tokens"], 6.0);
    assert_eq!(answer["totals"]["cached_tokens"], 3.0);
    assert_eq!(answer["totals"]["cache_write_tokens"], 3.0);
    assert_eq!(answer["totals"]["turns_without_any_tokens"], 3.0);
}

#[test]
fn query_answers_which_tools_fail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    upsert_thread(
        &connection,
        &ThreadRow {
            id: "a".to_owned(),
            title: Some("Thread a".to_owned()),
            project: Some("/repo/a".to_owned()),
            cwd: Some("/repo/a".to_owned()),
            harness: Some("codex".to_owned()),
            model: None,
            effort: None,
            parent_id: None,
            parent_title: None,
            created_at: None,
            updated_at: None,
        },
    )
    .unwrap();
    let mut first = turn("a", "2026-11-01T10:00:00Z", (10, 1), "completed");
    first.tool_calls = vec![
        ToolCallRow {
            tool_name: "run_command".to_owned(),
            success: Some(false),
            exit_code: Some(1),
            at: None,
            ..Default::default()
        },
        ToolCallRow {
            tool_name: "run_command".to_owned(),
            success: Some(true),
            exit_code: Some(0),
            at: None,
            ..Default::default()
        },
        ToolCallRow {
            tool_name: "replace_file_content".to_owned(),
            success: Some(false),
            exit_code: None,
            at: None,
            ..Default::default()
        },
    ];
    write_turn(&connection, &first).unwrap();
    // The question an agent asks of `query_data`, through the same guards
    // the tool uses.
    let answer = query_path(
        &path,
        "SELECT tool_name, SUM(success = 0) AS failures, COUNT(*) AS calls \
             FROM tool_calls GROUP BY tool_name HAVING failures > 0 \
             ORDER BY failures DESC, calls DESC",
        MAX_ROWS,
    )
    .unwrap();
    assert_eq!(answer.columns, ["tool_name", "failures", "calls"]);
    assert_eq!(
        answer.rows,
        [
            json!({"tool_name": "run_command", "failures": 1, "calls": 2}),
            json!({"tool_name": "replace_file_content", "failures": 1, "calls": 1}),
        ]
    );
}

#[test]
fn usage_reports_a_missing_store_and_a_bad_day() {
    let dir = tempfile::tempdir().unwrap();
    let error = usage_at(&dir.path().join(DATA_FILE), None, GroupBy::Harness, None).unwrap_err();
    assert!(error.contains("no data store yet"), "{error}");
    assert!(parse_since("last week").is_err());
    assert_eq!(parse_since("2026-11").unwrap(), "2026-11");
    assert_eq!(parse_since("2026-11-05").unwrap(), "2026-11-05");
    assert_eq!(GroupBy::parse("day"), Some(GroupBy::Day));
    assert_eq!(GroupBy::parse(""), Some(GroupBy::Harness));
    assert_eq!(GroupBy::parse("tool"), None);
}

/// A book with a settled link and then a working one (the order the app
/// appends them in), and the state beside them.
fn book_json() -> Value {
    json!({
        "links": [
            {
                "id": "a/1",
                "orchestrator": "a",
                "target": "b",
                "orchestratorTitle": "A",
                "targetTitle": "B",
                "message": "count them",
                "state": "done",
                "answer": "twelve",
                "createdAt": 1_761_000_000_000i64,
                "settledAt": 1_761_000_100_000i64,
                "notifiedAt": 1_761_000_200_000i64,
            },
            {
                "id": "a/2",
                "orchestrator": "a",
                "target": "c",
                "orchestratorTitle": "A",
                "targetTitle": "C",
                "message": "review the diff",
                "state": "working",
                "started": true,
                "harness": "codex",
                "filesChanged": 2,
                "createdAt": 1_762_000_000_000i64,
            },
        ],
        "grants": { "a": ["start", "b"] },
        "counters": { "a": 2 },
        "fanOuts": [{
            "id": "fan-1",
            "orchestrator": "a",
            "title": "Both ways",
            "prompt": "do it",
            "links": ["a/2"],
            "createdAt": 1_762_000_000_000i64,
        }],
    })
}

fn engine_link(id: &str, orchestrator: &str, state: &str) -> Value {
    json!({
        "id": id,
        "orchestrator": orchestrator,
        "target": "delegate",
        "state": state,
        "engine": true,
        "createdAt": 1_762_000_000_000i64,
    })
}

#[test]
fn account_grant_survives_store_reopen_without_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let key = json!(["delegate-account", "local", "codex", "work"]).to_string();
    let book = json!({"links": [], "grants": {"parent": [key]}, "counters": {}, "fanOuts": []});
    save_link_book_at(&path, &book).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["book"]["grants"], book["grants"]);
    assert!(loaded["book"]["grants"]["other"].is_null());
}

#[test]
fn an_engine_grant_is_added_once_beside_the_apps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let app = json!(["delegate-account", "local", "codex", "work"]).to_string();
    let book = json!({"links": [], "grants": {"parent": [app]}, "counters": {}, "fanOuts": []});
    save_link_book_at(&path, &book).unwrap();
    let engine = json!(["delegate-account", "local", "claude-code", "personal-1"]).to_string();
    add_grant_at(&path, "parent", &engine).unwrap();
    add_grant_at(&path, "parent", &engine).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["book"]["grants"]["parent"], json!([app, engine]));
}

#[test]
fn the_apps_save_cannot_undo_a_link_the_engine_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let caller = "caller-live-link";
    let opened = open_link_at(&path, caller, |n| {
        engine_link(&format!("{caller}/{n}"), caller, "working")
    })
    .unwrap();
    let id = opened["id"].as_str().unwrap().to_owned();
    assert_eq!(id, format!("{caller}/1"));
    engine_link_started(&id);

    // An app that attached mid-turn interrupts the link in its copy and
    // saves a book that never saw the counter move.
    let mut mine = engine_link(&id, caller, "interrupted");
    mine["answer"] = json!("stale");
    save_link_book_at(
        &path,
        &json!({ "links": [mine], "grants": {}, "counters": {}, "fanOuts": [] }),
    )
    .unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["book"]["links"][0]["state"], json!("working"));
    assert!(loaded["book"]["links"][0].get("answer").is_none());
    assert_eq!(loaded["book"]["counters"][caller], json!(1));

    // A book saved before the engine opened the link keeps it too.
    save_link_book_at(&path, &book_json()).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    let ids: Vec<&str> = loaded["book"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|link| link["id"].as_str())
        .collect();
    assert!(ids.contains(&id.as_str()), "{ids:?}");

    // The turn ends: the engine settles the link, and the app's older
    // copy no longer takes it back.
    assert!(
        change_link_at(&path, &id, |mut link| {
            link["state"] = json!("done");
            link["answer"] = json!("finished");
            link
        })
        .unwrap()
    );
    engine_link_ended(&id);
    save_link_book_at(
        &path,
        &json!({
            "links": [engine_link(&id, caller, "interrupted")],
            "grants": {}, "counters": {}, "fanOuts": [],
        }),
    )
    .unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["book"]["links"][0]["state"], json!("done"));
    assert_eq!(loaded["book"]["links"][0]["answer"], json!("finished"));
}

#[test]
fn a_link_the_engine_lost_to_a_restart_is_the_apps_to_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let caller = "caller-dead-engine";
    open_link_at(&path, caller, |n| {
        engine_link(&format!("{caller}/{n}"), caller, "working")
    })
    .unwrap();
    // Never marked live in this process: its engine is gone.
    save_link_book_at(
        &path,
        &json!({
            "links": [engine_link(&format!("{caller}/1"), caller, "interrupted")],
            "grants": {}, "counters": {}, "fanOuts": [],
        }),
    )
    .unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["book"]["links"][0]["state"], json!("interrupted"));
}

#[test]
fn a_link_whose_engine_died_is_interrupted_on_the_next_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let dead = "caller-dead-read";
    // Stamped long ago and never live in this process: its engine is gone.
    open_link_at(&path, dead, |n| {
        engine_link(&format!("{dead}/{n}"), dead, "working")
    })
    .unwrap();
    let live_caller = "caller-live-read";
    let live = open_link_at(&path, live_caller, |n| {
        engine_link(&format!("{live_caller}/{n}"), live_caller, "working")
    })
    .unwrap();
    engine_link_started(live["id"].as_str().unwrap());
    // One a running engine stamped a moment ago is not orphaned either.
    let fresh = open_link_at(&path, "caller-fresh-read", |n| {
        let mut link = engine_link(
            &format!("caller-fresh-read/{n}"),
            "caller-fresh-read",
            "working",
        );
        link["engineBeat"] = json!(now_ms());
        link
    })
    .unwrap();

    let loaded = load_link_book_at(&path).unwrap();
    let state = |id: &str| {
        loaded["book"]["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|link| link["id"] == json!(id))
            .map(|link| link["state"].clone())
    };
    assert_eq!(state(&format!("{dead}/1")), Some(json!("interrupted")));
    assert_eq!(state(live["id"].as_str().unwrap()), Some(json!("working")));
    assert_eq!(state(fresh["id"].as_str().unwrap()), Some(json!("working")));
    engine_link_ended(live["id"].as_str().unwrap());
}

/// A link opened by another engine process, stamped `age_ms` ago.
fn other_engine_link(path: &Path, caller: &str, age_ms: i64) -> String {
    let opened = open_link_at(path, caller, |n| {
        let mut link = engine_link(&format!("{caller}/{n}"), caller, "working");
        link["engineOwner"] = json!("another-process");
        link["engineBeat"] = json!(now_ms() - age_ms);
        link
    })
    .unwrap();
    opened["id"].as_str().unwrap().to_owned()
}

fn state_of(book: &Value, id: &str) -> Value {
    book["book"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["id"] == json!(id))
        .map(|link| link["state"].clone())
        .unwrap_or(Value::Null)
}

#[test]
fn a_link_another_process_is_stamping_is_not_this_ones_to_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    // The background server runs this link and stamped it 5 s ago; this
    // process (the app's engine) has nothing of it in its live set.
    let running = other_engine_link(&path, "caller-server-live", 5_000);
    let dead = other_engine_link(&path, "caller-server-dead", 120_000);

    // Its sweep leaves the stamped link alone and settles the dead one.
    assert_eq!(sweep_orphaned_links(&path, now_ms()).unwrap(), 1);
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(state_of(&loaded, &running), json!("working"));
    assert_eq!(state_of(&loaded, &dead), json!("interrupted"));
    // The app is told it runs, so `syncLinkBook` does not interrupt it.
    let live: Vec<&str> = loaded["engineLive"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(live.contains(&running.as_str()), "{live:?}");
    assert!(!live.contains(&dead.as_str()), "{live:?}");

    // The app saves its copy, where it had interrupted the link: the
    // other engine's link stays as that engine has it.
    let mut mine = engine_link(&running, "caller-server-live", "interrupted");
    mine["engineOwner"] = json!("another-process");
    save_link_book_at(
        &path,
        &json!({ "links": [mine], "grants": {}, "counters": {}, "fanOuts": [] }),
    )
    .unwrap();
    assert_eq!(
        state_of(&load_link_book_at(&path).unwrap(), &running),
        json!("working")
    );
}

#[test]
fn a_wake_from_sleep_holds_the_sweep_until_engines_have_stamped_again() {
    let mut clock = ClockWatch::new();
    let start = Instant::now();
    // Steady: a second of either clock is no jump.
    assert!(!clock.observe(Duration::from_secs(1), 1_000, start));
    // A laptop slept for two minutes: the wall clock moved, the monotonic one did not.
    assert!(clock.observe(Duration::from_secs(1), 121_000, start));
    // Still quiet a beat later, and quiet ends once every engine had its turn.
    assert!(clock.observe(
        Duration::from_secs(10),
        10_000,
        start + Duration::from_secs(10)
    ));
    assert!(!clock.observe(
        Duration::from_secs(10),
        10_000,
        start + AFTER_JUMP_QUIET + Duration::from_secs(1)
    ));
    // A clock set back counts as a jump too.
    assert!(clock.observe(
        Duration::from_secs(1),
        -60_000,
        start + AFTER_JUMP_QUIET + Duration::from_secs(2)
    ));
}

#[test]
fn a_sleep_does_not_get_a_running_link_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    // Stamped 90 s ago by an engine that slept with the machine: stale by
    // the clock, running in fact.
    let sleepy = other_engine_link(&path, "caller-slept", 90_000);
    // A sweep that finds the clocks jumped changes nothing...
    let mut clock = ClockWatch::new();
    let now = Instant::now();
    assert!(clock.observe(Duration::from_secs(1), 120_000, now));
    // ...so the engine's next stamp lands before anyone takes the link.
    change_link_at(&path, &sleepy, |mut link| {
        link["engineBeat"] = json!(now_ms());
        link
    })
    .unwrap();
    assert_eq!(sweep_orphaned_links(&path, now_ms()).unwrap(), 0);
    assert_eq!(
        state_of(&load_link_book_at(&path).unwrap(), &sleepy),
        json!("working")
    );
}

/// This process's own live links are never swept, however old the stamp.
#[test]
fn a_link_this_process_runs_is_never_swept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let caller = "caller-own-live";
    let opened = open_link_at(&path, caller, |n| {
        let mut link = engine_link(&format!("{caller}/{n}"), caller, "working");
        link["engineOwner"] = json!(engine_owner());
        link["engineBeat"] = json!(now_ms() - 600_000);
        link
    })
    .unwrap();
    let id = opened["id"].as_str().unwrap();
    engine_link_started(id);
    assert_eq!(sweep_orphaned_links(&path, now_ms()).unwrap(), 0);
    engine_link_ended(id);
    assert_eq!(sweep_orphaned_links(&path, now_ms()).unwrap(), 1);
}

#[test]
fn a_settled_engine_link_survives_an_app_copy_that_never_saw_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let caller = "caller-settled-kept";
    let opened = open_link_at(&path, caller, |n| {
        engine_link(&format!("{caller}/{n}"), caller, "done")
    })
    .unwrap();
    // A second window's book from before the link existed.
    save_link_book_at(&path, &book_json()).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    let kept = loaded["book"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["id"] == opened["id"]);
    assert!(kept, "{loaded}");
}

#[test]
fn two_openers_never_share_a_link_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let first = open_link_at(&path, "same", |n| {
        engine_link(&format!("same/{n}"), "same", "done")
    })
    .unwrap();
    // The app saves a counter that is behind; the next id still moves on.
    save_link_book_at(
        &path,
        &json!({ "links": [], "grants": {}, "counters": { "same": 0 }, "fanOuts": [] }),
    )
    .unwrap();
    let second = open_link_at(&path, "same", |n| {
        engine_link(&format!("same/{n}"), "same", "done")
    })
    .unwrap();
    assert_ne!(first["id"], second["id"]);
}

#[test]
fn the_link_book_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    // Nothing saved yet: the app must know to migrate its own copy in.
    let none = load_link_book_at(&path).unwrap();
    assert_eq!(none["stored"], false);
    assert_eq!(none["book"]["links"], json!([]));

    let book = book_json();
    save_link_book_at(&path, &book).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["stored"], true);
    assert_eq!(loaded["book"], book, "{loaded}");

    // Denormalised columns answer a query without opening a body.
    let connection = open(&path).unwrap();
    let summary: (String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT state, harness, files_changed FROM thread_links WHERE id = 'a/2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        summary,
        ("working".to_owned(), Some("codex".to_owned()), Some(2))
    );
    // The book is replaced, not appended to.
    save_link_book_at(
        &path,
        &json!({ "links": [], "grants": {}, "counters": {}, "fanOuts": [] }),
    )
    .unwrap();
    let empty = load_link_book_at(&path).unwrap();
    assert_eq!(empty["book"]["links"], json!([]));
    assert_eq!(empty["stored"], true, "the state rows are still there");
}

#[test]
fn the_link_book_tolerates_fields_it_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let mut book = book_json();
    book["future"] = json!({ "kept": false });
    book["links"][1]["effort"] = json!("high");
    book["links"][1]["laterField"] = json!([1, 2, 3]);
    save_link_book_at(&path, &book).unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    // A link's own fields travel in `body`, which is the whole record.
    assert_eq!(loaded["book"]["links"][1]["laterField"], json!([1, 2, 3]));
    assert_eq!(loaded["book"]["links"][1]["effort"], json!("high"));
    // A top-level key this release has no column for is dropped, never an error.
    assert!(loaded["book"].get("future").is_none());
    assert_eq!(loaded["book"]["grants"], json!({ "a": ["start", "b"] }));
}

#[test]
fn the_link_book_is_bounded_like_the_app_prunes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let mut links: Vec<Value> = (0..60)
        .map(|n| {
            json!({
                "id": format!("a/{n}"),
                "orchestrator": "a",
                "target": format!("t{n}"),
                "state": "done",
                "createdAt": 1_000 + n,
            })
        })
        .collect();
    // An active link is kept whatever its age.
    links.push(json!({
        "id": "a/old",
        "orchestrator": "a",
        "target": "old",
        "state": "working",
        "createdAt": 1,
    }));
    // Every fan-out names the active link, so all of them still have one.
    let fan_outs: Vec<Value> = (0..25)
        .map(|n| json!({ "id": format!("fan-{n}"), "orchestrator": "a", "links": ["a/old"] }))
        .collect();
    save_link_book_at(
        &path,
        &json!({ "links": links, "grants": {}, "counters": {}, "fanOuts": fan_outs }),
    )
    .unwrap();
    let loaded = load_link_book_at(&path).unwrap();
    let kept = loaded["book"]["links"].as_array().unwrap();
    assert_eq!(kept.len(), KEPT_FINISHED_LINKS + 1);
    assert!(kept.iter().any(|link| link["id"] == "a/old"));
    assert!(
        !kept.iter().any(|link| link["id"] == "a/0"),
        "the oldest finished link goes"
    );
    assert_eq!(
        loaded["book"]["fanOuts"].as_array().unwrap().len(),
        KEPT_FAN_OUTS
    );
}

#[test]
fn a_store_from_the_previous_version_gets_the_link_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    // A store as release 1 left it: migration 1 recorded, nothing else.
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            )
            .unwrap();
        let (id, name, sql) = MIGRATIONS[0];
        connection.execute_batch(sql).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                params![id, name],
            )
            .unwrap();
    }
    let loaded = load_link_book_at(&path).unwrap();
    assert_eq!(loaded["stored"], false);
    save_link_book_at(&path, &book_json()).unwrap();
    let again = load_link_book_at(&path).unwrap();
    assert_eq!(again["book"]["links"].as_array().unwrap().len(), 2);
}

#[test]
fn migrations_apply_once_and_a_renamed_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let first = open(&path).unwrap();
    let columns: Vec<String> = first
        .prepare("PRAGMA table_info(tool_calls)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        columns,
        [
            "id",
            "thread_id",
            "turn_id",
            "tool_name",
            "success",
            "exit_code",
            "at",
            "started_at",
            "duration_ms",
            "output_bytes",
            "truncated",
            "mcp_server",
            "command_name",
            "error_kind",
            "harness_tool"
        ]
    );
    drop(first);
    open(&path).expect("a second open does not migrate again");

    let connection = open(&path).unwrap();
    connection
        .execute(
            "UPDATE schema_migrations SET name = 'something else' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(migrate(&connection).unwrap_err().contains("recorded as"));
}

#[test]
fn a_turn_is_updated_in_place_and_its_calls_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let connection = open(&dir.path().join(DATA_FILE)).unwrap();
    let started = "2026-11-04T10:00:00.000Z";
    write_turn(&connection, &turn("t1", started, (100, 20), "completed")).unwrap();
    write_turn(&connection, &turn("t1", started, (140, 30), "cancelled")).unwrap();

    let (turns, status, input, calls): (i64, String, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM turns), status, input_tokens,
                        (SELECT COUNT(*) FROM tool_calls)
                 FROM turns",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (turns, status.as_str(), input, calls),
        (1, "cancelled", 140, 1)
    );

    let per_day: (String, i64, i64) = connection
        .query_row(
            "SELECT day, input_tokens, turns FROM daily_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(per_day, ("2026-11-04".to_owned(), 140, 1));
}

#[test]
fn session_totals_become_per_turn_deltas() {
    let before = totals(&json!({
        "_meta": {"rustcode": {"inputTokens": 1000, "outputTokens": 100, "cachedTokens": 500}},
    }))
    .unwrap();
    assert_eq!(
        before,
        TokenTotals {
            input: Some(1000),
            output: Some(100),
            cached: Some(500),
            cache_write: None,
        }
    );
    let now = totals(&json!({
        "_meta": {"rustcode": {"inputTokens": 1600, "outputTokens": 180, "cachedTokens": 900}},
    }))
    .unwrap();
    assert_eq!(
        delta(now, Some(before)),
        TokenTotals {
            input: Some(600),
            output: Some(80),
            cached: Some(400),
            cache_write: None,
        }
    );
    assert_eq!(
        delta(now, None),
        now,
        "first sight of a counter is its own turn"
    );
    // A harness that restarted its own session: no negative numbers.
    assert_eq!(
        delta(
            TokenTotals {
                input: Some(5),
                output: None,
                cached: None,
                cache_write: None,
            },
            Some(before)
        ),
        TokenTotals {
            input: None,
            output: None,
            cached: None,
            cache_write: None,
        }
    );
    assert!(
        totals(&json!({"used": 10, "size": 100})).is_none(),
        "a context window alone is not a token total"
    );
}

fn fake_config(dir: &Path) {
    let thread = |id: &str, harness: Option<&str>, usage: Option<Value>, native: bool| {
        let folder = dir.join("threads/2026/11/04").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("history.json"), "[]").unwrap();
        std::fs::write(folder.join("title.txt"), format!("title {id}")).unwrap();
        std::fs::write(
            folder.join("workspace.json"),
            r#"{"cwd":"/repo","additional_directories":[]}"#,
        )
        .unwrap();
        if let Some(harness) = harness {
            let mut record = json!({
                "harness": harness,
                "cwd": "/repo",
                "title": format!("harness title {id}"),
                "model": "gpt-6-luna",
                "created_at": "2026-11-04T09:00:00+00:00",
                "updated_at": "2026-11-04T10:00:00+00:00",
            });
            if let Some(usage) = usage {
                record["usage"] = usage;
            }
            std::fs::write(
                folder.join("harness.json"),
                serde_json::to_string(&record).unwrap(),
            )
            .unwrap();
        }
        if native {
            std::fs::write(
                    folder.join("metadata.json"),
                    r#"{"schema_version":1,"id":"x","created_at_ms":1793784000000,
                        "parent":{"id":"01a10000-0000-0000-0000-000000000001","title":"Parent thread"}}"#,
                )
                .unwrap();
        }
    };
    thread(
        "01a10000-0000-0000-0000-000000000001",
        Some("codex"),
        Some(json!({"used": 10, "size": 1000, "_meta": {"rustcode": {
                "inputTokens": 700, "outputTokens": 70, "cachedTokens": 300}}})),
        false,
    );
    thread("01a10000-0000-0000-0000-000000000002", None, None, true);
}

#[test]
fn backfill_is_idempotent_and_one_summary_turn_per_external_thread() {
    let config = tempfile::tempdir().unwrap();
    fake_config(config.path());
    let connection = open(&config.path().join(DATA_FILE)).unwrap();

    let first = backfill(&connection, config.path()).unwrap();
    assert_eq!(
        first,
        BackfillReport {
            threads: 2,
            turns: 1,
            errors: 0
        }
    );
    // A later pass rewrites the same summary row rather than adding a
    // second one, which is what lets v2 refresh a v1 backfill.
    let second = backfill(&connection, config.path()).unwrap();
    assert_eq!(second.threads, 2);
    assert_eq!(second.errors, 0);

    let (threads, turns, status, input, title): (i64, i64, String, i64, String) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM threads), (SELECT COUNT(*) FROM turns),
                        t.status, t.input_tokens, th.title
                 FROM turns t JOIN threads th ON th.id = t.thread_id",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!((threads, turns), (2, 1));
    assert_eq!(status, "backfill");
    assert_eq!(input, 700);
    assert_eq!(title, "harness title 01a10000-0000-0000-0000-000000000001");

    let native: (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT harness, title, created_at, parent_id, parent_title FROM threads
                 WHERE id = '01a10000-0000-0000-0000-000000000002'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(native.0, "rustcode");
    assert!(native.1.ends_with("02"));
    assert!(native.2.is_some(), "metadata.json supplies created_at");
    assert_eq!(
        native.3.as_deref(),
        Some("01a10000-0000-0000-0000-000000000001")
    );
    assert_eq!(native.4.as_deref(), Some("Parent thread"));
}

#[test]
fn cost_is_a_delta_and_only_usd_is_kept() {
    let usd = json!({"used": 10, "size": 100, "cost": {"amount": 24.5, "currency": "USD"}});
    assert_eq!(cost_usd(&usd), Some(24.5));
    let eur = json!({"cost": {"amount": 24.5, "currency": "EUR"}});
    assert_eq!(cost_usd(&eur), None, "another currency is not converted");
    assert_eq!(cost_usd(&json!({"used": 10})), None);

    let usage = |cost: f64| SessionUsage {
        tokens: TokenTotals::default(),
        cost_usd: Some(cost),
        at: None,
    };
    assert_eq!(
        advance(&usage(0.75), Some(&usage(0.5))).0.cost_usd,
        Some(0.25)
    );
    assert_eq!(
        advance(&usage(0.2), Some(&usage(0.5))).0.cost_usd,
        Some(0.2),
        "a restarted session reports its whole cost again"
    );
    assert_eq!(advance(&usage(0.2), None).0.cost_usd, Some(0.2));
}

#[test]
fn the_first_turn_of_a_new_thread_is_recorded() {
    let config = tempfile::tempdir().unwrap();
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let thread = "01a10000-0000-0000-0000-00000000000f";
    let turn = TurnRow {
        thread_id: thread.to_owned(),
        started_at: "2026-11-04T11:00:00.000Z".to_owned(),
        finished_at: Some("2026-11-04T11:00:05.000Z".to_owned()),
        harness: Some("pi".to_owned()),
        status: "completed".to_owned(),
        ..TurnRow::default()
    };
    // No row yet, so the baseline has nothing to sum.
    write_cumulative_turn(
        &connection,
        &turn,
        SessionUsage {
            tokens: TokenTotals {
                input: Some(50),
                ..TokenTotals::default()
            },
            ..SessionUsage::default()
        },
        Some("session-a"),
    )
    .unwrap();
    let (turns, input, threads): (i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), SUM(input_tokens),
                        (SELECT COUNT(*) FROM threads WHERE id = ?1)
                 FROM turns WHERE thread_id = ?1",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((turns, input, threads), (1, 50, 1));
}

#[test]
fn a_restarted_harness_counts_each_turn_once() {
    let config = tempfile::tempdir().unwrap();
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let thread = "01a10000-0000-0000-0000-000000000010";
    // Claude Code reports session totals: cost, and one token counter per
    // kind, all of which restart together when its process does.
    let cumulative = |cost: f64, input: i64| SessionUsage {
        tokens: TokenTotals {
            input: Some(input),
            output: Some(input / 10),
            cached: Some(input * 5),
            cache_write: Some(input / 2),
        },
        cost_usd: Some(cost),
        at: None,
    };
    let turn = |started: &str, status: &str| TurnRow {
        thread_id: thread.to_owned(),
        started_at: started.to_owned(),
        finished_at: Some(started.to_owned()),
        harness: Some("claude-code".to_owned()),
        status: status.to_owned(),
        ..TurnRow::default()
    };
    let record = |started: &str, status: &str, cost: f64, input: i64| {
        write_cumulative_turn(
            &connection,
            &turn(started, status),
            cumulative(cost, input),
            Some("session-a"),
        )
        .unwrap();
    };
    record("2026-11-04T11:00:00.000Z", "completed", 10.0, 1000);
    record("2026-11-04T11:05:00.000Z", "completed", 15.0, 1500);
    // The harness restarted: its session counter starts again at 2.
    record("2026-11-05T09:00:00.000Z", "completed", 2.0, 200);
    record("2026-11-05T09:05:00.000Z", "completed", 4.0, 400);
    // A failed turn reports the previous total again: no change, no row value.
    record("2026-11-05T09:06:00.000Z", "failed", 4.0, 400);

    let rows: Vec<(
        String,
        Option<f64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    )> = {
        let mut statement = connection
            .prepare(
                "SELECT status, cost_usd, input_tokens, output_tokens,
                                cached_tokens, cache_write_tokens
                         FROM turns WHERE thread_id = ?1 ORDER BY started_at",
            )
            .unwrap();
        statement
            .query_map([thread], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    let spent: Vec<(String, Option<f64>, Option<i64>)> = rows
        .iter()
        .map(|(status, cost, input, _, _, _)| (status.clone(), *cost, *input))
        .collect();
    assert_eq!(
        spent,
        [
            ("completed".to_owned(), Some(10.0), Some(1000)),
            ("completed".to_owned(), Some(5.0), Some(500)),
            ("completed".to_owned(), Some(2.0), Some(200)),
            ("completed".to_owned(), Some(2.0), Some(200)),
            ("failed".to_owned(), Some(0.0), Some(0)),
        ]
    );
    // Each token counter grows the same way, and the summed rows are what
    // the harness actually spent: 19 dollars, 1900 input tokens.
    let (output, cached, cache_write): (i64, i64, i64) = connection
        .query_row(
            "SELECT SUM(output_tokens), SUM(cached_tokens), SUM(cache_write_tokens)
                 FROM turns WHERE thread_id = ?1",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((output, cached, cache_write), (190, 9500, 950));
    let total: f64 = connection
        .query_row(
            "SELECT SUM(cost_usd) FROM turns WHERE thread_id = ?1",
            [thread],
            |row| row.get(0),
        )
        .unwrap();
    assert!((total - 19.0).abs() < 1e-9, "{total} is 10+5+2+2+0");
    // The cursor holds the last total, not the sum of the rows.
    let cursor: (f64, i64) = connection
        .query_row(
            "SELECT cost_usd, input_tokens FROM usage_cursors
                 WHERE thread_id = ?1 AND harness = 'claude-code' AND series = 'session-a'",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!((cursor.0 - 4.0).abs() < 1e-9);
    assert_eq!(cursor.1, 400);

    // A thread that switches harness starts its new harness at zero.
    write_cumulative_turn(
        &connection,
        &TurnRow {
            harness: Some("codex".to_owned()),
            ..turn("2026-11-05T10:00:00.000Z", "completed")
        },
        cumulative(3.0, 30),
        Some("session-a"),
    )
    .unwrap();
    let switched: (Option<f64>, Option<i64>) = connection
        .query_row(
            "SELECT cost_usd, input_tokens FROM turns
                 WHERE thread_id = ?1 AND harness = 'codex'",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(switched, (Some(3.0), Some(30)));
}

#[test]
fn repeated_claude_usage_view_flags_candidates_without_changing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir.path().join(DATA_FILE)).unwrap();
    db.execute_batch("INSERT INTO turns (thread_id, started_at, harness, output_tokens, cost_usd, tool_calls, status) VALUES
            ('t', '1', 'claude-code', 100, 2.0, 3, 'completed'),
            ('t', '2', 'claude-code', 100, 2.0, 0, 'failed'),
            ('t', '3', 'claude-code', 100, 2.0, 1, 'completed'),
            ('t', '4', 'claude-code', 101, 2.0, 0, 'cancelled');").unwrap();
    let flagged: Vec<String> = db
        .prepare("SELECT started_at FROM suspected_repeated_claude_turn_usage")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(flagged, ["2"]);
    let cost: f64 = db
        .query_row(
            "SELECT cost_usd FROM turns WHERE started_at = '2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cost, 2.0);
}

#[test]
fn returning_to_an_earlier_session_counts_only_its_own_growth() {
    let config = tempfile::tempdir().unwrap();
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let thread = "01a10000-0000-0000-0000-000000000012";
    let cumulative = |cost: f64| SessionUsage {
        tokens: TokenTotals {
            input: Some(cost as i64 * 100),
            ..TokenTotals::default()
        },
        cost_usd: Some(cost),
        at: None,
    };
    let record = |started: &str, cost: f64, series: &str| {
        write_cumulative_turn(
            &connection,
            &TurnRow {
                thread_id: thread.to_owned(),
                started_at: started.to_owned(),
                finished_at: Some(started.to_owned()),
                harness: Some("claude-code".to_owned()),
                status: "completed".to_owned(),
                ..TurnRow::default()
            },
            cumulative(cost),
            Some(series),
        )
        .unwrap();
    };
    record("2026-11-04T11:00:00.000Z", 10.0, "session-a");
    record("2026-11-04T11:05:00.000Z", 15.0, "session-a");
    // Resume in another session: its own total, not 2 minus 15.
    record("2026-11-04T12:00:00.000Z", 2.0, "session-b");
    // Back in the first session: the difference against *its* last total,
    // never against the other session's 16.
    record("2026-11-04T13:00:00.000Z", 16.0, "session-a");

    let rows: Vec<Option<f64>> = {
        let mut statement = connection
            .prepare("SELECT cost_usd FROM turns WHERE thread_id = ?1 ORDER BY started_at")
            .unwrap();
        statement
            .query_map([thread], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    assert_eq!(
        rows,
        [Some(10.0), Some(5.0), Some(2.0), Some(1.0)],
        "each series is measured against its own last total"
    );
    let cursors: Vec<(String, f64)> = {
        let mut statement = connection
            .prepare(
                "SELECT series, cost_usd FROM usage_cursors
                     WHERE thread_id = ?1 ORDER BY series",
            )
            .unwrap();
        statement
            .query_map([thread], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    assert_eq!(
        cursors,
        [
            ("session-a".to_owned(), 16.0),
            ("session-b".to_owned(), 2.0)
        ]
    );
}

#[test]
fn a_counter_that_dips_alone_records_no_growth() {
    let config = tempfile::tempdir().unwrap();
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let thread = "01a10000-0000-0000-0000-000000000013";
    let record = |started: &str, cost: f64, input: i64, output: i64, cached: i64| {
        write_cumulative_turn(
            &connection,
            &TurnRow {
                thread_id: thread.to_owned(),
                started_at: started.to_owned(),
                finished_at: Some(started.to_owned()),
                harness: Some("claude-code".to_owned()),
                status: "completed".to_owned(),
                ..TurnRow::default()
            },
            SessionUsage {
                tokens: TokenTotals {
                    input: Some(input),
                    output: Some(output),
                    cached: Some(cached),
                    cache_write: Some(input / 2),
                },
                cost_usd: Some(cost),
                at: None,
            },
            Some("session-a"),
        )
        .unwrap();
    };
    record("2026-11-04T11:00:00.000Z", 10.0, 1000, 100, 500);
    // A rotated or truncated log, or an account directory that went
    // missing: one counter goes back while the others keep rising. That is
    // not a restart, so the dipped counter records nothing instead of a
    // whole session, and its cursor keeps the higher total.
    record("2026-11-04T11:05:00.000Z", 11.0, 400, 130, 600);
    // Once the log is whole again, growth resumes from the kept total.
    record("2026-11-04T11:10:00.000Z", 11.5, 1100, 150, 700);

    let rows: Vec<(Option<f64>, Option<i64>, Option<i64>, Option<i64>)> = {
        let mut statement = connection
            .prepare(
                "SELECT cost_usd, input_tokens, output_tokens, cached_tokens
                     FROM turns WHERE thread_id = ?1 ORDER BY started_at",
            )
            .unwrap();
        statement
            .query_map([thread], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    assert_eq!(
        rows,
        [
            (Some(10.0), Some(1000), Some(100), Some(500)),
            (Some(1.0), Some(0), Some(30), Some(100)),
            (Some(0.5), Some(100), Some(20), Some(100)),
        ]
    );
    let cursor: (Option<i64>, Option<i64>) = connection
        .query_row(
            "SELECT input_tokens, cached_tokens FROM usage_cursors
                 WHERE thread_id = ?1 AND series = 'session-a'",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cursor, (Some(1100), Some(700)));
}

#[test]
fn a_cumulative_turn_survives_a_commit_from_another_writer() {
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    // Another writer holds the write lock and commits while this turn is
    // between its reads and its write. A DEFERRED transaction would answer
    // SQLITE_BUSY_SNAPSHOT there and lose the turn; IMMEDIATE takes the
    // lock first and waits for its turn.
    let other_path = path.clone();
    let (locked, ready) = std::sync::mpsc::channel();
    let other = std::thread::spawn(move || {
        let other = open(&other_path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        other
            .execute("INSERT INTO threads (id) VALUES ('another-writer')", [])
            .unwrap();
        locked.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        other.execute_batch("COMMIT").unwrap();
    });
    ready.recv().unwrap();
    let thread = "01a10000-0000-0000-0000-000000000014";
    write_cumulative_turn(
        &connection,
        &TurnRow {
            thread_id: thread.to_owned(),
            started_at: "2026-11-04T11:00:00.000Z".to_owned(),
            finished_at: Some("2026-11-04T11:00:01.000Z".to_owned()),
            harness: Some("claude-code".to_owned()),
            status: "completed".to_owned(),
            ..TurnRow::default()
        },
        SessionUsage {
            tokens: TokenTotals {
                input: Some(50),
                ..TokenTotals::default()
            },
            ..SessionUsage::default()
        },
        Some("session-a"),
    )
    .expect("the turn waits for the other writer instead of failing");
    other.join().unwrap();
    let (turns, input): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), SUM(input_tokens) FROM turns WHERE thread_id = ?1",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((turns, input), (1, 50));
}

#[test]
fn a_thread_recorded_before_cursors_does_not_add_an_inflated_total() {
    let config = tempfile::tempdir().unwrap();
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let thread = "01a10000-0000-0000-0000-000000000011";
    // An older release recorded this thread's rows, and the sum it was
    // measured against is inflated (830: 15 turns of a runaway total).
    write_turn(
        &connection,
        &TurnRow {
            thread_id: thread.to_owned(),
            started_at: "2026-11-04T11:00:00.000Z".to_owned(),
            harness: Some("claude-code".to_owned()),
            input_tokens: Some(830),
            cost_usd: Some(830.0),
            status: "completed".to_owned(),
            ..TurnRow::default()
        },
    )
    .unwrap();
    let cumulative = |cost: f64| SessionUsage {
        tokens: TokenTotals {
            input: Some(cost as i64),
            ..TokenTotals::default()
        },
        cost_usd: Some(cost),
        at: None,
    };
    let live = |started: &str| TurnRow {
        thread_id: thread.to_owned(),
        started_at: started.to_owned(),
        finished_at: Some(started.to_owned()),
        harness: Some("claude-code".to_owned()),
        status: "completed".to_owned(),
        ..TurnRow::default()
    };
    // The next total is below the sum, so the sum is the inflated side:
    // the lower of the two is the baseline. This turn counts nothing — the
    // rows already hold at least this total — and the cursor starts at the
    // reading, so the turns after it are exact.
    write_cumulative_turn(
        &connection,
        &live("2026-11-04T11:05:00.000Z"),
        cumulative(10.0),
        Some("session-a"),
    )
    .unwrap();
    write_cumulative_turn(
        &connection,
        &live("2026-11-04T11:10:00.000Z"),
        cumulative(12.0),
        Some("session-a"),
    )
    .unwrap();
    write_cumulative_turn(
        &connection,
        &live("2026-11-04T11:15:00.000Z"),
        cumulative(15.0),
        Some("session-a"),
    )
    .unwrap();
    let rows: Vec<Option<f64>> = {
        let mut statement = connection
            .prepare("SELECT cost_usd FROM turns WHERE thread_id = ?1 ORDER BY started_at")
            .unwrap();
        statement
            .query_map([thread], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    assert_eq!(
        rows,
        [Some(830.0), Some(0.0), Some(2.0), Some(3.0)],
        "the old rows stay, and no total already in them is added again"
    );
}

#[test]
fn migration_removes_only_backfills_with_live_turns() {
    let config = tempfile::tempdir().unwrap();
    fake_config(config.path());
    let path = config.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    backfill(&connection, config.path()).unwrap();
    let thread = "01a10000-0000-0000-0000-000000000001";
    connection.execute(
            "INSERT INTO turns (thread_id, started_at, status) VALUES (?1, '2026-11-04T11:00:00Z', 'completed')",
            [thread],
        ).unwrap();
    connection
        .execute("DELETE FROM schema_migrations WHERE id = 14", [])
        .unwrap();
    drop(connection);
    let connection = open(&path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM turns WHERE thread_id = ?1 AND status = 'backfill'",
            [thread],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_live_turn_after_backfill_counts_only_the_growth() {
    let config = tempfile::tempdir().unwrap();
    fake_config(config.path());
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    backfill(&connection, config.path()).unwrap();
    drop(connection);

    let thread = "01a10000-0000-0000-0000-000000000001";
    let cumulative = |input: i64, cost: f64| SessionUsage {
        tokens: TokenTotals {
            input: Some(input),
            output: Some(input / 10),
            cached: None,
            cache_write: None,
        },
        cost_usd: Some(cost),
        at: None,
    };
    let live = |started: &str| TurnRow {
        thread_id: thread.to_owned(),
        started_at: started.to_owned(),
        finished_at: Some(started.to_owned()),
        harness: Some("codex".to_owned()),
        model: Some("gpt-6-luna".to_owned()),
        status: "completed".to_owned(),
        ..TurnRow::default()
    };
    // The summary already holds 700 in / 70 out and no cost.
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    write_cumulative_turn(
        &connection,
        &live("2026-11-04T11:00:00.000Z"),
        cumulative(700, 0.4),
        Some("session-a"),
    )
    .unwrap();
    write_cumulative_turn(
        &connection,
        &live("2026-11-04T12:00:00.000Z"),
        cumulative(900, 0.6),
        Some("session-a"),
    )
    .unwrap();
    let (turns, input, output, cost): (i64, i64, i64, f64) = connection
        .query_row(
            "SELECT COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(cost_usd)
                 FROM turns WHERE thread_id = ?1",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(turns, 2, "only live rows remain");
    assert_eq!(input, 900);
    assert_eq!(output, 90);
    assert!((cost - 0.6).abs() < 1e-9, "cost adds up to the last total");
    backfill(&connection, config.path()).unwrap();
    let summaries: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM turns WHERE thread_id = ?1 AND status = 'backfill'",
            [thread],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(summaries, 0);
}

#[test]
fn backfill_reads_a_claude_log_for_tokens_and_the_harness_cost_for_usd() {
    let config = tempfile::tempdir().unwrap();
    let claude = tempfile::tempdir().unwrap();
    let thread = "01a10000-0000-0000-0000-000000000009";
    let folder = config.path().join("threads/2026/11/04").join(thread);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("history.json"), "[]").unwrap();
    std::fs::write(folder.join("title.txt"), "claude thread").unwrap();
    std::fs::write(
        folder.join("workspace.json"),
        r#"{"cwd":"/tmp/claude-project","additional_directories":[]}"#,
    )
    .unwrap();
    std::fs::write(
        folder.join("harness.json"),
        serde_json::to_string(&json!({
            "harness": "claude-code",
            "native_session_id": "session-1",
            "cwd": "/tmp/claude-project",
            "model": "claude-opus-5-5",
            "usage": {"used": 10, "size": 1000,
                      "cost": {"amount": 1.25, "currency": "USD"}},
            "created_at": "2026-11-04T09:00:00+00:00",
            "updated_at": "2026-11-04T10:00:00+00:00",
        }))
        .unwrap(),
    )
    .unwrap();
    // The log sits where Claude Code puts it: projects/<slug>/<id>.jsonl,
    // with the same message twice as it spans two content blocks.
    let projects = claude.path().join("projects/-tmp-claude-project");
    std::fs::create_dir_all(&projects).unwrap();
    let line = r#"{"type":"assistant","timestamp":"2026-11-04T09:30:00.000Z","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#;
    std::fs::write(
        projects.join("session-1.jsonl"),
        format!("{line}\n{line}\n"),
    )
    .unwrap();
    std::fs::write(
        projects.join("other.jsonl"),
        r#"{"type":"assistant","message":{"id":"x","usage":{"input_tokens":9999}}}"#,
    )
    .unwrap();

    let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
    // Only this test reads `CLAUDE_CONFIG_DIR`; nothing else in the crate
    // looks at the Claude folders.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", claude.path()) };
    let connection = open(&config.path().join(DATA_FILE)).unwrap();
    let report = backfill(&connection, config.path()).unwrap();
    match previous {
        Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
        None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
    }
    assert_eq!(report.errors, 0);

    let (input, output, cached, cache_write, cost, finished): (i64, i64, i64, i64, f64, String) =
        connection
            .query_row(
                "SELECT input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                        cost_usd, finished_at
                 FROM turns WHERE thread_id = ?1",
                [thread],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
    assert_eq!((input, output, cached, cache_write), (10, 100, 1000, 50));
    assert!((cost - 1.25).abs() < 1e-9);
    assert_eq!(finished, "2026-11-04T09:30:00.000Z", "the log's last stamp");
}

#[test]
fn a_write_attach_second_statement_and_another_file_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let other = dir.path().join("other.sqlite");
    let connection = open(&path).unwrap();
    write_turn(
        &connection,
        &turn("t1", "2026-11-04T10:00:00.000Z", (5, 5), "completed"),
    )
    .unwrap();
    open(&other).unwrap();
    drop(connection);

    let result = query_path(&path, "SELECT COUNT(*) AS n FROM turns", 10).unwrap();
    assert_eq!(result.rows[0]["n"], 1);

    for sql in [
        "INSERT INTO threads (id) VALUES ('x')",
        "DELETE FROM turns",
        "DROP TABLE turns",
    ] {
        assert!(query_path(&path, sql, 10).is_err(), "{sql} must be refused");
    }
    let attach = format!("ATTACH DATABASE '{}' AS other", other.display());
    assert!(query_path(&path, &attach, 10).is_err(), "ATTACH is refused");
    assert!(
        query_path(&path, "SELECT * FROM other.turns", 10).is_err(),
        "another database cannot be reached"
    );
    assert!(
        query_path(&path, "SELECT 1; SELECT 2", 10).is_err(),
        "one statement only"
    );
    assert!(
        query_path(&path, "SELECT ';' AS semi;", 10).is_ok(),
        "a semicolon inside a string is not a separator"
    );
    assert!(
        query_path(&path, "SELECT 1 -- ;\n", 10).is_ok(),
        "a semicolon inside a comment is not a separator"
    );
}

#[test]
fn rows_are_capped_and_an_endless_query_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    open(&path).unwrap();

    let many = query_path(
        &path,
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 1200)
             SELECT x FROM c",
        5000,
    )
    .unwrap();
    assert_eq!(many.rows.len(), MAX_ROWS);
    assert!(many.truncated);

    let started = Instant::now();
    let error = query_path(
        &path,
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c)
             SELECT count(*) FROM c",
        10,
    )
    .unwrap_err();
    assert!(error.contains("stopped"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_missing_store_says_how_to_create_it() {
    let dir = tempfile::tempdir().unwrap();
    let error = query_path(&dir.path().join(DATA_FILE), "SELECT 1", 10).unwrap_err();
    assert!(error.contains("no data store yet"), "{error}");
}

// ------------------------------------------------------ request telemetry

fn days_ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn minutes_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn count_rows(path: &Path, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) AS n FROM {table}");
    query_path(path, &sql, 10).unwrap().rows[0]["n"]
        .as_i64()
        .unwrap()
}

fn one_row(path: &Path, sql: &str) -> Value {
    let rows = query_path(path, sql, 10).unwrap().rows;
    assert_eq!(rows.len(), 1, "{sql}: {rows:?}");
    rows.into_iter().next().unwrap()
}

fn provider_row(thread: &str, started_at: &str, payload: Option<&str>) -> ProviderRequestRow {
    ProviderRequestRow {
        thread_id: thread.to_owned(),
        turn_id: Some("turn-1".to_owned()),
        round: Some(2),
        provider: Some("openai".to_owned()),
        model: Some("gpt-6-luna".to_owned()),
        endpoint_host: Some("api.openai.com".to_owned()),
        endpoint_path: Some("/v1/chat/completions".to_owned()),
        method: Some("POST".to_owned()),
        attempt: Some(1),
        started_at: started_at.to_owned(),
        headers_at: Some(started_at.to_owned()),
        finished_at: Some(started_at.to_owned()),
        latency_ms: Some(412),
        status_code: Some(200),
        request_bytes: Some(1024),
        response_bytes: Some(2048),
        stream_event_count: Some(37),
        input_tokens: Some(100),
        output_tokens: Some(20),
        cached_tokens: None,
        error_kind: None,
        request_payload_json: payload.map(str::to_owned),
        response_summary_json: None,
        payload_truncated: false,
    }
}

fn rpc_row(thread: &str, at: &str, payload: Option<&str>) -> RpcMessageRow {
    RpcMessageRow {
        thread_id: thread.to_owned(),
        turn_id: Some("turn-1".to_owned()),
        harness: Some("codex".to_owned()),
        protocol: Some("acp".to_owned()),
        direction: DIRECTION_RUSTCODE_TO_HARNESS.to_owned(),
        message_kind: KIND_REQUEST.to_owned(),
        rpc_id: Some("7".to_owned()),
        method: Some("session/prompt".to_owned()),
        at: at.to_owned(),
        latency_ms: Some(30),
        byte_size: Some(512),
        payload_json: payload.map(str::to_owned),
        payload_truncated: false,
    }
}

fn event_row(thread: &str, at: &str, fields: Option<&str>) -> OperationalEventRow {
    OperationalEventRow {
        thread_id: thread.to_owned(),
        turn_id: Some("turn-1".to_owned()),
        event_type: "turn.summary".to_owned(),
        at: at.to_owned(),
        fields_json: fields.map(str::to_owned),
        original_bytes: fields.map(|fields| fields.len() as i64),
        captured_bytes: None,
        payload_truncated: false,
    }
}

fn compaction_row(thread: &str, at: &str, trigger: &str) -> CompactionRow {
    CompactionRow {
        thread_id: thread.to_owned(),
        turn_id: None,
        harness: Some("claude-code".to_owned()),
        model: None,
        at: at.to_owned(),
        duration_ms: Some(2_400),
        trigger: trigger.to_owned(),
        mode: Some("harness_summary".to_owned()),
        tokens_before: Some(120_000),
        tokens_after: Some(18_000),
        messages_before: None,
        messages_after: None,
        idle_secs_before: None,
        count: Some(1),
        success: true,
        error_kind: None,
        external_id: Some("compact-1".to_owned()),
        source: Some("claude-code".to_owned()),
    }
}

#[test]
fn migration_adds_turn_metrics_and_the_compactions_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let columns = |table: &str| -> Vec<String> {
        connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    let turns = columns("turns");
    for column in [
        "duration_ms",
        "time_to_first_token_ms",
        "model_time_ms",
        "tool_time_ms",
        "context_tokens",
        "context_window",
        "stop_reason",
    ] {
        assert!(turns.iter().any(|name| name == column), "{column}");
    }
    let compactions = columns("compactions");
    for column in [
        "thread_id",
        "harness",
        "trigger",
        "mode",
        "tokens_before",
        "tokens_after",
        "success",
        "external_id",
        "source",
    ] {
        assert!(compactions.iter().any(|name| name == column), "{column}");
    }
    let views: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'view'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    for view in [
        "daily_usage",
        "daily_compactions",
        "tool_failure_rates",
        "turn_latency_daily",
    ] {
        assert!(views.iter().any(|name| name == view), "{view}");
    }
}

#[test]
fn stats_count_rows_and_a_tables_indexes_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    write_turn(
        &connection,
        &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
    )
    .unwrap();
    let index_bytes: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name = 'turns_day'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);

    let stats = stats_at(&path).unwrap();
    let object = |name: &str| {
        stats["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|object| object["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name}"))
    };
    assert_eq!(object("turns")["rows"], 1);
    assert_eq!(object("compactions")["rows"], 0);
    assert_eq!(object("daily_usage")["rows"], 1, "a view reports its rows");
    let bytes = object("turns")["bytes"].as_i64().expect("dbstat sizes");
    assert!(
        bytes > index_bytes,
        "the table's pages and its indexes both count: {bytes} <= {index_bytes}"
    );
}

#[test]
fn a_compaction_round_trips_and_a_repeated_update_keeps_one_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    upsert_thread(
        &connection,
        &ThreadRow {
            id: "t".to_owned(),
            harness: Some("claude-code".to_owned()),
            model: Some("sonnet".to_owned()),
            ..ThreadRow::default()
        },
    )
    .unwrap();
    let at = "2026-11-01T10:00:00Z";
    record_compaction_at(&path, &compaction_row("t", at, "manual")).unwrap();
    let row = one_row(&path, "SELECT * FROM compactions");
    assert_eq!(row["thread_id"], "t");
    assert_eq!(row["harness"], "claude-code");
    assert_eq!(row["model"], "sonnet", "filled from the thread row");
    assert_eq!(row["trigger"], "manual");
    assert_eq!(row["mode"], "harness_summary");
    assert_eq!(row["tokens_before"], 120_000);
    assert_eq!(row["tokens_after"], 18_000);
    assert_eq!(row["duration_ms"], 2_400);
    assert_eq!(row["success"], 1);
    assert_eq!(row["at"], "2026-11-01T10:00:00.000Z");

    // The same harness event again (a duplicate terminal update) updates
    // its row instead of adding a second one.
    let mut again = compaction_row("t", "2026-11-01T10:00:01Z", "manual");
    again.tokens_after = Some(12_000);
    record_compaction_at(&path, &again).unwrap();
    assert_eq!(count_rows(&path, "compactions"), 1);
    let row = one_row(&path, "SELECT tokens_after FROM compactions");
    assert_eq!(row["tokens_after"], 12_000);

    // The one-SELECT acceptance question, through the same guards as
    // `query_data`.
    let daily = one_row(
        &path,
        "SELECT compactions, reclaimed_tokens FROM daily_compactions",
    );
    assert_eq!(daily["compactions"], 1);
    assert_eq!(daily["reclaimed_tokens"], 108_000);
}

#[test]
fn turn_and_tool_metrics_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let mut row = turn("t", "2026-11-01T10:00:00Z", (100, 20), "completed");
    row.duration_ms = Some(9_500);
    row.time_to_first_token_ms = Some(700);
    row.model_time_ms = Some(7_000);
    row.tool_time_ms = Some(1_500);
    row.context_tokens = Some(48_000);
    row.context_window = Some(200_000);
    row.stop_reason = Some("end_turn".to_owned());
    row.tool_calls[0].duration_ms = Some(250);
    row.tool_calls[0].truncated = Some(true);
    row.tool_calls[0].command_name = Some("cargo".to_owned());
    row.tool_calls[0].error_kind = Some("exit_code".to_owned());
    row.tool_calls[0].output_bytes = Some(512);
    row.tool_calls.push(ToolCallRow {
        tool_name: "mcp__rustcode__thread_usage".to_owned(),
        ..Default::default()
    });
    write_turn(&connection, &row).unwrap();

    let stored = one_row(&path, "SELECT * FROM turns");
    assert_eq!(stored["duration_ms"], 9_500);
    assert_eq!(stored["time_to_first_token_ms"], 700);
    assert_eq!(stored["model_time_ms"], 7_000);
    assert_eq!(stored["tool_time_ms"], 1_500);
    assert_eq!(stored["context_tokens"], 48_000);
    assert_eq!(stored["context_window"], 200_000);
    assert_eq!(stored["stop_reason"], "end_turn");
    let tools = query_path(
        &path,
        "SELECT tool_name, mcp_server, command_name, duration_ms, truncated, error_kind \
             FROM tool_calls ORDER BY id",
        10,
    )
    .unwrap()
    .rows;
    let command_time = one_row(
        &path,
        "SELECT command_name, calls, timed_calls, total_duration_ms FROM tool_command_time_daily WHERE command_name = 'cargo'",
    );
    assert_eq!(command_time["command_name"], "cargo");
    assert_eq!(command_time["calls"], 1);
    assert_eq!(command_time["timed_calls"], 1);
    assert_eq!(command_time["total_duration_ms"], 250);
    assert_eq!(tools[0]["command_name"], "cargo");
    assert_eq!(tools[0]["duration_ms"], 250);
    assert_eq!(tools[0]["truncated"], 1);
    assert_eq!(tools[0]["error_kind"], "exit_code");
    assert!(tools[0]["mcp_server"].is_null());
    assert_eq!(tools[1]["mcp_server"], "rustcode");
    let latency = one_row(
        &path,
        "SELECT turns, mean_duration_ms FROM turn_latency_daily",
    );
    assert_eq!(latency["turns"], 1);
    assert_eq!(latency["mean_duration_ms"], 9_500.0);
}

#[test]
fn harness_tools_and_failure_kinds_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let mut row = turn("t", "2026-11-01T10:00:00Z", (100, 20), "failed");
    row.error_kind = Some(turn_failure_kind("unknown ACP session: 01a1"));
    row.tool_calls[0].tool_name = "other".to_owned();
    row.tool_calls[0].harness_tool = Some("mcp__rustcode__wait_session".to_owned());
    write_turn(&connection, &row).unwrap();

    let stored = one_row(&path, "SELECT error_kind FROM turns");
    assert_eq!(stored["error_kind"], "unknown_session");
    let tool = one_row(
        &path,
        "SELECT tool_name, harness_tool, mcp_server FROM tool_calls",
    );
    assert_eq!(tool["tool_name"], "other");
    assert_eq!(tool["harness_tool"], "mcp__rustcode__wait_session");
    assert_eq!(tool["mcp_server"], "rustcode");
}

#[test]
fn turn_failure_kinds_read_the_error_without_keeping_it() {
    assert_eq!(
        turn_failure_kind("HTTP 429 Too Many Requests"),
        "rate_limited"
    );
    assert_eq!(
        turn_failure_kind("OpenAI token request failed (HTTP 400)"),
        "auth"
    );
    assert_eq!(turn_failure_kind("status 503 overloaded"), "provider_error");
    assert_eq!(
        turn_failure_kind("the harness process exited"),
        "harness_exited"
    );
    assert_eq!(turn_failure_kind("TLS UnknownIssuer"), "network");
    assert_eq!(turn_failure_kind("something odd"), "other");
    // Digits inside an id are not a status.
    assert_eq!(
        turn_failure_kind("thread 01a1d4291-4030 went away"),
        "other"
    );
}

#[test]
fn captured_air_compactions_are_backfilled_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "s", "update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "c1",
            "_meta": {"jetbrains": {"air": {"contextCompaction": {
                "version": 1, "trigger": "manual", "preTokens": 200_000,
                "postTokens": 15_000, "durationMs": 30_000
            }}}}
        }}
    });
    connection
        .execute(
            "INSERT INTO harness_rpc_messages (thread_id, harness, direction, message_kind,
                        method, at, payload_json)
                 VALUES ('t', 'claude-code', 'harness_to_rustcode', 'notification',
                        'session/update', '2026-11-14T22:24:13.712Z', ?1)",
            [payload.to_string()],
        )
        .unwrap();
    connection
        .execute("DELETE FROM schema_migrations WHERE id = 13", [])
        .unwrap();
    migrate(&connection).unwrap();
    // Running it again (a second open) adds nothing.
    connection
        .execute("DELETE FROM schema_migrations WHERE id = 13", [])
        .unwrap();
    migrate(&connection).unwrap();

    let rows = query_path(
            &path,
            "SELECT trigger, tokens_before, tokens_after, duration_ms, external_id, success              FROM compactions",
            10,
        )
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["trigger"], "manual");
    assert_eq!(rows[0]["tokens_before"], 200_000);
    assert_eq!(rows[0]["tokens_after"], 15_000);
    assert_eq!(rows[0]["duration_ms"], 30_000);
    assert_eq!(rows[0]["external_id"], "c1");
    assert_eq!(rows[0]["success"], 1);
}

/// A thread, a turn and a tool call written with plain SQL against only
/// the columns migrations 1 to 8 know, so a test can build an old store.
const OLD_ROWS: &str = "INSERT INTO threads (id, title) VALUES ('t', 'kept');
        INSERT INTO turns (id, thread_id, started_at, input_tokens, output_tokens, status)
            VALUES (1, 't', '2026-11-01T10:00:00Z', 10, 2, 'completed');
        INSERT INTO tool_calls (thread_id, turn_id, tool_name, success) VALUES ('t', 1, 'x', 1);";

#[test]
fn the_telemetry_tables_migrate_again_without_losing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    write_turn(
        &connection,
        &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
    )
    .unwrap();
    record_provider_request_at(&path, &provider_row("t", "2026-11-01T10:00:00Z", None)).unwrap();

    // Opening again runs `migrate` on a store that already has the tables,
    // which must find its own work done rather than fail on CREATE TABLE.
    let connection = open(&path).unwrap();
    migrate(&connection).unwrap();
    let recorded: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(recorded, MIGRATIONS.len() as i64);
    assert_eq!(count_rows(&path, "turns"), 1, "the old rows survive");
    assert_eq!(count_rows(&path, "provider_requests"), 1);
    assert_eq!(count_rows(&path, "harness_rpc_messages"), 0);
    assert_eq!(count_rows(&path, "usage_cursors"), 0);
    assert_eq!(count_rows(&path, "operational_events"), 0);
    let kept = one_row(&path, "SELECT status FROM turns");
    assert_eq!(kept["status"], "completed");
}

#[test]
fn a_store_at_migration_seven_gets_the_tool_call_turn_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let indexes = |path: &Path| -> Vec<String> {
        let connection = Connection::open(path).unwrap();
        connection
            .prepare("PRAGMA index_list(tool_calls)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    // A store as migration 7 left it: every earlier step recorded, rows
    // written, and no index on `tool_calls.turn_id`.
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            )
            .unwrap();
        for (id, name, sql) in MIGRATIONS.iter().filter(|(id, _, _)| *id <= 7) {
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                    params![id, name],
                )
                .unwrap();
        }
        connection.execute_batch(OLD_ROWS).unwrap();
    }
    assert!(
        !indexes(&path).iter().any(|name| name == "tool_calls_turn"),
        "the migration-7 store has no such index"
    );

    // Opening runs migration 8 on top of 7, and the index it adds is the
    // one an upsert's `DELETE FROM tool_calls WHERE turn_id = ?` needs.
    let connection = open(&path).unwrap();
    assert!(indexes(&path).iter().any(|name| name == "tool_calls_turn"));
    let recorded: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(recorded, MIGRATIONS.len() as i64);
    assert_eq!(count_rows(&path, "turns"), 1, "the old rows survive");
    assert_eq!(count_rows(&path, "tool_calls"), 1, "the old rows survive");
    // `IF NOT EXISTS` keeps the step harmless on a store that already has
    // the index some other way.
    let last = MIGRATIONS.iter().find(|(id, _, _)| *id == 8).unwrap().2;
    connection.execute_batch(last).unwrap();
}

#[test]
fn migration_nine_only_adds_and_keeps_every_existing_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    // A store as migration 8 left it, holding a thread, a turn and a call.
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            )
            .unwrap();
        for (id, name, sql) in MIGRATIONS.iter().filter(|(id, _, _)| *id <= 8) {
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                    params![id, name],
                )
                .unwrap();
        }
        connection.execute_batch(OLD_ROWS).unwrap();
    }
    let columns = |table: &str| -> Vec<String> {
        let connection = Connection::open(&path).unwrap();
        connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    let threads_before = columns("threads");
    let turns_before = columns("turns");

    let connection = open(&path).unwrap();
    let recorded: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(recorded, MIGRATIONS.len() as i64);
    // Every old column is still there, in order, with new ones after it.
    assert_eq!(columns("threads")[..threads_before.len()], threads_before);
    assert_eq!(columns("turns")[..turns_before.len()], turns_before);
    for column in [
        "branch",
        "final_commit",
        "pull_request",
        "settled_at",
        "machine",
    ] {
        assert!(
            columns("threads").iter().any(|name| name == column),
            "{column}"
        );
    }
    for column in [
        "effort",
        "account",
        "diff_files",
        "diff_added",
        "diff_removed",
    ] {
        assert!(
            columns("turns").iter().any(|name| name == column),
            "{column}"
        );
    }
    // The rows survive, the new columns are NULL, and the view still reads.
    assert_eq!(count_rows(&path, "turns"), 1);
    assert_eq!(count_rows(&path, "tool_calls"), 1);
    assert_eq!(count_rows(&path, "approvals"), 0);
    let kept = one_row(
        &path,
        "SELECT th.title, tu.input_tokens, tu.effort, th.branch
             FROM turns tu JOIN threads th ON th.id = tu.thread_id",
    );
    assert_eq!(kept["title"], "kept");
    assert_eq!(kept["input_tokens"], 10);
    assert!(
        kept["effort"].is_null() && kept["branch"].is_null(),
        "{kept}"
    );
    assert_eq!(count_rows(&path, "daily_usage"), 1);
}

#[test]
fn a_turn_keeps_its_effort_account_machine_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let mut row = turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed");
    row.effort = Some("high".to_owned());
    row.account = Some("paral-2".to_owned());
    row.diff_files = Some(3);
    row.diff_added = Some(40);
    row.diff_removed = Some(7);
    write_turn(&connection, &row).unwrap();
    let stored = one_row(
        &path,
        "SELECT effort, account, rustcode_version, diff_files, diff_added, diff_removed
             FROM turns",
    );
    assert_eq!(stored["effort"], "high");
    assert_eq!(stored["account"], "paral-2");
    assert_eq!(stored["rustcode_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(stored["diff_files"], 3);
    assert_eq!(stored["diff_added"], 40);
    assert_eq!(stored["diff_removed"], 7);
    let thread = one_row(&path, "SELECT account, created_with_version FROM threads");
    assert_eq!(thread["account"], "paral-2");
    assert_eq!(thread["created_with_version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn numstat_lines_sum_into_files_and_lines() {
    let text = "10\t2\tsrc/a.rs\n-\t-\tlogo.png\n3\t0\tdocs/b.md\n";
    assert_eq!(parse_numstat(text), (3, 13, 2));
    assert_eq!(parse_numstat(""), (0, 0, 0));
}

#[test]
fn a_threads_outcome_pull_request_settling_and_approvals_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    {
        let connection = open(&path).unwrap();
        write_turn(
            &connection,
            &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
        )
        .unwrap();
    }
    record_thread_outcome_at(&path, "t", "feat/x", "abc123").unwrap();
    record_thread_pull_request_at(
        &path,
        "t",
        "https://example.test/pull/9",
        "merged",
        "def456",
    )
    .unwrap();
    record_thread_settled_at(&path, "t", true).unwrap();
    let thread = one_row(
        &path,
        "SELECT branch, final_commit, pull_request, pull_request_state, settled_at,
                    title FROM threads WHERE id = 't'",
    );
    assert_eq!(thread["branch"], "feat/x");
    assert_eq!(thread["final_commit"], "def456", "the merged head wins");
    assert_eq!(thread["pull_request"], "https://example.test/pull/9");
    assert_eq!(thread["pull_request_state"], "merged");
    assert!(thread["settled_at"].is_string(), "{thread}");
    // Settling keeps the turns and tool calls it already had.
    assert_eq!(count_rows(&path, "turns"), 1);
    assert_eq!(count_rows(&path, "tool_calls"), 1);
    record_thread_settled_at(&path, "t", false).unwrap();
    let reopened = one_row(&path, "SELECT settled_at FROM threads WHERE id = 't'");
    assert!(reopened["settled_at"].is_null(), "{reopened}");

    // An open pull request does not replace the final commit.
    record_thread_pull_request_at(&path, "t", "https://example.test/pull/9", "open", "zzz")
        .unwrap();
    let after = one_row(&path, "SELECT final_commit FROM threads WHERE id = 't'");
    assert_eq!(after["final_commit"], "def456");

    record_approval_at(
        &path,
        &ApprovalRow {
            thread_id: "t".to_owned(),
            source: "builtin".to_owned(),
            tool_name: Some("run_command".to_owned()),
            requested_at: "2026-11-01T10:00:00.000Z".to_owned(),
            decided_at: Some("2026-11-01T10:00:04.000Z".to_owned()),
            wait_ms: Some(4000),
            decision: "allow_once".to_owned(),
        },
    )
    .unwrap();
    let approval = one_row(&path, "SELECT * FROM approvals");
    assert_eq!(approval["decision"], "allow_once");
    assert_eq!(approval["wait_ms"], 4000);
    assert_eq!(approval["tool_name"], "run_command");
}

fn pull(number: u64, state: &str, head: &str, failed: bool) -> PullRequestOutcome {
    PullRequestOutcome {
        url: format!("https://github.com/acme/app/pull/{number}"),
        number,
        title: format!("feat: change {number}"),
        state: state.to_owned(),
        head: head.to_owned(),
        checks_failed: failed,
        ..Default::default()
    }
}

#[test]
fn a_pull_requests_ci_failures_fix_rounds_merge_and_revert_are_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-17T10:00:00.000Z";
    // Head a fails twice (one failure, read twice), a fix pushes b, which
    // fails too, then c goes green and merges.
    for read in [
        pull(7, "open", "a", true),
        pull(7, "open", "a", true),
        pull(7, "open", "b", true),
        pull(7, "open", "c", false),
        PullRequestOutcome {
            merged_at: Some("2026-11-17T11:00:00Z".to_owned()),
            ..pull(7, "merged", "c", false)
        },
    ] {
        record_pull_request_at(&path, "t", &read, at).unwrap();
    }
    let row = one_row(&path, "SELECT * FROM thread_outcomes");
    assert_eq!(row["repo"], "acme/app");
    assert_eq!(row["pr_number"], 7);
    assert_eq!(row["state"], "merged");
    assert_eq!(row["ci_failures"], 2, "{row}");
    assert_eq!(row["fix_rounds"], 2, "{row}");
    assert_eq!(row["merged_at"], "2026-11-17T11:00:00Z");
    assert_eq!(row["closed_unmerged"], 0);
    assert_eq!(row["reverted"], 0);
    let thread = one_row(
        &path,
        "SELECT pull_request_state, final_commit FROM threads WHERE id = 't'",
    );
    assert_eq!(thread["pull_request_state"], "merged");
    assert_eq!(thread["final_commit"], "c");

    // A merged revert of #7 marks it reverted.
    let revert = PullRequestOutcome {
        reverts_number: Some(7),
        ..pull(8, "merged", "r", false)
    };
    record_pull_request_at(&path, "u", &revert, at).unwrap();
    let reverted = one_row(
        &path,
        "SELECT reverted, reverted_by FROM thread_outcomes WHERE pr_number = 7",
    );
    assert_eq!(reverted["reverted"], 1);
    assert_eq!(reverted["reverted_by"], 8);

    // Closed without merging.
    record_pull_request_at(&path, "v", &pull(9, "closed", "x", false), at).unwrap();
    let closed = one_row(
        &path,
        "SELECT closed_unmerged, closed_at, merged_at FROM thread_outcomes WHERE pr_number = 9",
    );
    assert_eq!(closed["closed_unmerged"], 1);
    assert!(closed["closed_at"].is_string(), "{closed}");
    assert!(closed["merged_at"].is_null(), "{closed}");
}

#[test]
fn pr_outcomes_by_model_reports_cost_and_tokens_per_merged_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    {
        let connection = open(&path).unwrap();
        for (thread, cost) in [("m", 3.0), ("c", 1.0)] {
            write_turn(
                &connection,
                &TurnRow {
                    harness: Some("claude".to_owned()),
                    model: Some("opus".to_owned()),
                    cached_tokens: Some(100),
                    cache_write_tokens: Some(0),
                    cost_usd: Some(cost),
                    ..turn(thread, "2026-11-17T10:00:00Z", (1000, 200), "completed")
                },
            )
            .unwrap();
        }
        // A thread without a pull request is not counted.
        write_turn(
            &connection,
            &TurnRow {
                harness: Some("claude".to_owned()),
                model: Some("opus".to_owned()),
                cost_usd: Some(50.0),
                ..turn("none", "2026-11-17T10:00:00Z", (9, 9), "completed")
            },
        )
        .unwrap();
    }
    let at = "2026-11-17T10:00:00.000Z";
    record_pull_request_at(&path, "m", &pull(1, "open", "a", true), at).unwrap();
    record_pull_request_at(&path, "m", &pull(1, "merged", "b", false), at).unwrap();
    record_pull_request_at(&path, "c", &pull(2, "closed", "z", false), at).unwrap();
    let row = one_row(
        &path,
        "SELECT * FROM pr_outcomes_by_model WHERE harness = 'claude'",
    );
    assert_eq!(row["model"], "opus");
    assert_eq!(row["threads"], 2);
    assert_eq!(row["pull_requests"], 2);
    assert_eq!(row["merged"], 1);
    assert_eq!(row["closed_unmerged"], 1);
    assert_eq!(row["merge_share"], 0.5);
    assert_eq!(row["avg_ci_failures"], 0.5);
    // Every linked thread's spend, merged or not, over the merged count.
    assert_eq!(row["cost_per_merged_pr"], 4.0);
    assert_eq!(row["tokens_per_merged_pr"], 2600);
    let report = query_path(&path, PULL_REQUEST_OUTCOMES_SQL, 10).unwrap();
    assert_eq!(report.rows.len(), 1, "{:?}", report.rows);
}

#[test]
fn pull_request_repository_reads_owner_and_name() {
    assert_eq!(
        pull_request_repository("https://github.com/acme/app/pull/12").as_deref(),
        Some("acme/app")
    );
    assert_eq!(pull_request_repository("https://example.test/pull/9"), None);
}

#[test]
fn by_default_old_and_large_payloads_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let payload = format!("{{\"pad\":\"{}\"}}", "p".repeat(1024));
    for days in [400, 40, 8, 0] {
        write_provider_request(
            &connection,
            &provider_row("t", &days_ago(days), Some(&payload)),
        )
        .unwrap();
    }
    // A one-byte budget would clear everything if size mattered.
    let report = prune_with(&connection, 1, None).unwrap();
    assert_eq!(report.payloads_cleared, 0, "{report:?}");
    assert_eq!(report.rows_deleted, 0, "{report:?}");
    assert_eq!(count_rows(&path, "provider_requests"), 4);
    let kept = query_path(
        &path,
        "SELECT request_payload_json FROM provider_requests",
        10,
    )
    .unwrap()
    .rows;
    assert!(
        kept.iter()
            .all(|row| row["request_payload_json"] == payload)
    );
}

#[test]
fn telemetry_rows_round_trip_without_a_payload_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-01T10:00:00Z";
    record_provider_request_at(&path, &provider_row("t", at, None)).unwrap();
    let written = record_rpc_messages_at(&path, &[&rpc_row("t", at, None)]).unwrap();
    assert_eq!(written, 1);
    let sequence = record_operational_event_at(&path, &event_row("t", at, None)).unwrap();
    assert_eq!(sequence, 1);

    let request = one_row(&path, "SELECT * FROM provider_requests");
    assert_eq!(request["thread_id"], "t");
    assert_eq!(request["turn_id"], "turn-1");
    assert_eq!(request["round"], 2);
    assert_eq!(request["provider"], "openai");
    assert_eq!(request["endpoint_host"], "api.openai.com");
    assert_eq!(request["endpoint_path"], "/v1/chat/completions");
    assert_eq!(request["attempt"], 1);
    assert_eq!(request["latency_ms"], 412);
    assert_eq!(request["status_code"], 200);
    assert_eq!(request["stream_event_count"], 37);
    assert_eq!(request["input_tokens"], 100);
    // Timestamps are normalised, so string order is time order.
    assert_eq!(request["started_at"], "2026-11-01T10:00:00.000Z");
    // Metadata by default: no payload, no redaction to record.
    assert!(request["request_payload_json"].is_null(), "{request}");
    assert_eq!(request["payload_truncated"], 0);
    assert_eq!(request["redaction_version"], 0);

    let rpc = one_row(&path, "SELECT * FROM harness_rpc_messages");
    assert_eq!(rpc["direction"], DIRECTION_RUSTCODE_TO_HARNESS);
    assert_eq!(rpc["message_kind"], KIND_REQUEST);
    assert_eq!(rpc["rpc_id"], "7");
    assert_eq!(rpc["method"], "session/prompt");
    assert_eq!(rpc["byte_size"], 512);
    assert!(rpc["payload_json"].is_null(), "{rpc}");

    let event = one_row(&path, "SELECT * FROM operational_events");
    assert_eq!(event["event_type"], "turn.summary");
    assert_eq!(event["sequence"], 1);
    assert!(event["fields_json"].is_null(), "{event}");
    assert_eq!(event["redaction_version"], 0);
}

#[test]
fn a_captured_payload_is_redacted_and_capped_before_it_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-01T10:00:00Z";
    let payload = json!({
        "model": "gpt-6-luna",
        "headers": {"authorization": "Bearer sk-live-123", "accept": "text/event-stream"},
        "pad": "z".repeat(4000),
    });
    let (text, truncated) = redact::prepare_payload(&payload, 512);
    assert!(truncated, "a 4 KiB pad is clipped");
    let mut row = provider_row("t", at, Some(&text));
    row.payload_truncated = truncated;
    record_provider_request_at(&path, &row).unwrap();

    let stored = one_row(&path, "SELECT * FROM provider_requests");
    let kept = stored["request_payload_json"].as_str().unwrap();
    assert!(
        kept.len() <= MAX_PROVIDER_PAYLOAD_BYTES,
        "{} bytes",
        kept.len()
    );
    assert!(!kept.contains("sk-live-123"), "{kept}");
    assert_eq!(stored["payload_truncated"], 1);
    assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
    let parsed: Value = serde_json::from_str(kept).expect("the stored payload is valid JSON");
    assert_eq!(parsed["model"], "gpt-6-luna");
    assert_eq!(parsed["headers"]["authorization"], redact::REDACTED);

    // A caller that skipped the cap is still capped, and told so.
    let long = format!(
        "{{\"pad\":\"{}\"}}",
        "z".repeat(MAX_PROVIDER_PAYLOAD_BYTES + 200)
    );
    record_provider_request_at(&path, &provider_row("u", at, Some(&long))).unwrap();
    let stored = one_row(
        &path,
        "SELECT * FROM provider_requests WHERE thread_id = 'u'",
    );
    let kept = stored["request_payload_json"].as_str().unwrap();
    assert!(
        kept.len() <= MAX_PROVIDER_PAYLOAD_BYTES,
        "{} bytes",
        kept.len()
    );
    assert_eq!(stored["payload_truncated"], 1);
    assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");

    // The insert redacts again, so a caller that skipped it cannot write
    // a secret.
    let raw = r#"{"headers": {"Authorization": "Bearer sk-raw-1"}, "max_tokens": 64}"#;
    record_provider_request_at(&path, &provider_row("v", at, Some(raw))).unwrap();
    let stored = one_row(
        &path,
        "SELECT * FROM provider_requests WHERE thread_id = 'v'",
    );
    let kept = stored["request_payload_json"].as_str().unwrap();
    assert!(!kept.contains("sk-raw-1"), "{kept}");
    assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
    let parsed: Value = serde_json::from_str(kept).unwrap();
    assert_eq!(parsed["headers"]["Authorization"], redact::REDACTED);
    // A token limit is a number, not a secret, so it is kept.
    assert_eq!(parsed["max_tokens"], 64);
}

#[test]
fn every_telemetry_payload_is_held_to_its_own_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-01T10:00:00Z";
    let long = |field: &str| format!("{{\"{field}\":\"{}\"}}", "z".repeat(32 * 1024));

    record_rpc_messages_at(&path, &[&rpc_row("t", at, Some(&long("params")))]).unwrap();
    let stored = one_row(&path, "SELECT * FROM harness_rpc_messages");
    let kept = stored["payload_json"].as_str().unwrap();
    assert!(kept.len() <= MAX_RPC_PAYLOAD_BYTES, "{} bytes", kept.len());
    assert_eq!(stored["payload_truncated"], 1);
    assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
    assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");

    record_operational_event_at(&path, &event_row("t", at, Some(&long("fields")))).unwrap();
    let stored = one_row(&path, "SELECT * FROM operational_events");
    let kept = stored["fields_json"].as_str().unwrap();
    assert!(kept.len() <= MAX_EVENT_FIELDS_BYTES, "{} bytes", kept.len());
    assert_eq!(stored["payload_truncated"], 1);
    assert_eq!(stored["captured_bytes"], kept.len() as i64);
    assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");
}

#[test]
fn operational_events_number_themselves_per_thread() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-01T10:00:00Z";
    for expected in 1..=3 {
        let sequence =
            record_operational_event_at(&path, &event_row("a", at, Some("{\"n\":1}"))).unwrap();
        assert_eq!(sequence, expected);
    }
    let other = record_operational_event_at(&path, &event_row("b", at, None)).unwrap();
    assert_eq!(other, 1, "each thread has its own sequence");
    // A scratch id is not a thread, so nothing is stored for it.
    assert_eq!(
        record_operational_event_at(&path, &event_row("session-title-1700000000000", at, None))
            .unwrap(),
        0,
        "a title request is not a thread"
    );
    let sequences = query_path(
        &path,
        "SELECT thread_id, sequence FROM operational_events ORDER BY thread_id, sequence",
        10,
    )
    .unwrap()
    .rows;
    assert_eq!(sequences.len(), 4, "{sequences:?}");
}

#[test]
fn pruning_drops_old_raw_payloads_and_keeps_structured_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let payload = "{\"a\":1}";
    write_provider_request(&connection, &provider_row("t", &days_ago(8), Some(payload))).unwrap();
    write_rpc_messages(&connection, &[&rpc_row("t", &days_ago(8), Some(payload))]).unwrap();
    write_operational_event(&connection, &event_row("t", &days_ago(8), Some(payload))).unwrap();
    write_turn(
        &connection,
        &turn("t", &days_ago(400), (10, 2), "completed"),
    )
    .unwrap();
    // Older than the payload window.
    write_provider_request(
        &connection,
        &provider_row("t", &days_ago(40), Some(payload)),
    )
    .unwrap();
    // Fresh: untouched.
    write_provider_request(&connection, &provider_row("t", &days_ago(0), Some(payload))).unwrap();

    let report = prune_with(&connection, TELEMETRY_MAX_BYTES, Some(10_000)).unwrap();
    assert!(report.payloads_cleared >= 2, "{report:?}");
    assert_eq!(
        report.rows_deleted, 0,
        "forever deletes no rows: {report:?}"
    );

    let requests = query_path(
        &path,
        "SELECT started_at, request_payload_json FROM provider_requests ORDER BY started_at",
        10,
    )
    .unwrap()
    .rows;
    assert_eq!(requests.len(), 3, "every raw row stays: {requests:?}");
    assert!(
        requests[0]["request_payload_json"].is_null(),
        "the 40-day payload is gone: {requests:?}"
    );
    assert!(
        requests[1]["request_payload_json"].is_null(),
        "the 8-day payload is gone: {requests:?}"
    );
    assert_eq!(
        requests[2]["request_payload_json"], payload,
        "today keeps its payload"
    );

    let rpc = one_row(&path, "SELECT payload_json FROM harness_rpc_messages");
    assert!(rpc["payload_json"].is_null(), "{rpc}");
    // Operational events are structured rows: their fields are history.
    let event = one_row(
        &path,
        "SELECT fields_json, captured_bytes FROM operational_events",
    );
    assert_eq!(event["fields_json"], payload, "{event}");
    // A 400-day-old turn is still queryable.
    assert_eq!(count_rows(&path, "turns"), 1);
}

#[test]
fn a_folded_streaming_row_keeps_its_metadata_and_a_captured_tool_call_its_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let coalesced = "{\"coalesced\":42,\"update\":\"command_output\",\"bytes\":900}";
    let raw_body = format!("{{\"delta\":\"{}\"}}", "x".repeat(900));
    let chunk_text = format!(
        "{{\"params\":{{\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":\"{}\"}}}}}}",
        "y".repeat(900)
    );
    let captured_tool_call = format!(
        "{{\"params\":{{\"update\":{{\"sessionUpdate\":\"tool_call_update\",\"rawOutput\":{{\"text\":\"{}\"}}}}}}}}",
        "z".repeat(900)
    );
    let row = |method: &str, payload: &str| {
        let mut row = rpc_row("t", &days_ago(0), Some(payload));
        row.method = Some(method.to_owned());
        row
    };
    write_rpc_messages(
        &connection,
        &[
            &row("item/commandExecution/outputDelta", coalesced),
            &row("item/commandExecution/outputDelta", &raw_body),
            &row("session/update", &chunk_text),
            &row("session/update", &captured_tool_call),
        ],
    )
    .unwrap();

    let report = prune_with(&connection, TELEMETRY_MAX_BYTES, Some(10_000)).unwrap();
    assert_eq!(
        report.payloads_cleared, 2,
        "the raw bodies only: {report:?}"
    );
    let payloads: Vec<Value> = query_path(
        &path,
        "SELECT payload_json FROM harness_rpc_messages ORDER BY id",
        10,
    )
    .unwrap()
    .rows
    .into_iter()
    .map(|row| row["payload_json"].clone())
    .collect();
    assert_eq!(
        payloads[0], coalesced,
        "a folded chunk keeps its counts: {payloads:?}"
    );
    assert!(payloads[1].is_null(), "the raw delta body is gone");
    assert!(payloads[2].is_null(), "the chunk text is gone");
    assert_eq!(
        payloads[3], captured_tool_call,
        "a captured tool call is not noise: {payloads:?}"
    );
    assert_eq!(count_rows(&path, "harness_rpc_messages"), 4);
}

#[test]
fn a_configured_retention_deletes_only_raw_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    write_provider_request(&connection, &provider_row("t", &days_ago(40), None)).unwrap();
    write_rpc_messages(&connection, &[&rpc_row("t", &days_ago(40), None)]).unwrap();
    write_operational_event(&connection, &event_row("t", &days_ago(40), None)).unwrap();
    write_turn(&connection, &turn("t", &days_ago(40), (10, 2), "completed")).unwrap();

    let report = prune_with(&connection, TELEMETRY_MAX_BYTES, Some(30)).unwrap();
    assert_eq!(report.rows_deleted, 2, "only the raw rows: {report:?}");
    assert_eq!(count_rows(&path, "provider_requests"), 0);
    assert_eq!(count_rows(&path, "harness_rpc_messages"), 0);
    assert_eq!(count_rows(&path, "operational_events"), 1);
    assert_eq!(count_rows(&path, "turns"), 1);
}

#[test]
fn telemetry_retention_parses_forever_and_days() {
    assert_eq!(super::parse_retention_days("forever"), None);
    assert_eq!(super::parse_retention_days(""), None);
    assert_eq!(super::parse_retention_days(" 30 "), Some(30));
    assert_eq!(super::parse_retention_days("0"), None);
    assert_eq!(super::parse_retention_days("later"), None);
}

#[test]
fn the_byte_budget_drops_the_oldest_payloads_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    let payload = format!("{{\"pad\":\"{}\"}}", "p".repeat(1024));
    // 300 rows of ~1 KiB, one minute apart so none is old enough for the
    // age policy, newest last.
    for minutes in (0..300).rev() {
        write_provider_request(
            &connection,
            &provider_row("t", &minutes_ago(minutes), Some(&payload)),
        )
        .unwrap();
    }
    let budget = 100 * 1024;
    let report = prune_with(&connection, budget, Some(10_000)).unwrap();
    assert!(report.bytes_before > budget, "{report:?}");
    assert!(report.bytes_after <= budget, "{report:?}");
    assert!(report.payloads_cleared > 0, "{report:?}");
    assert_eq!(
        report.rows_deleted, 0,
        "payloads go before rows: {report:?}"
    );
    assert_eq!(count_rows(&path, "provider_requests"), 300);

    let newest = one_row(
        &path,
        "SELECT request_payload_json FROM provider_requests ORDER BY started_at DESC LIMIT 1",
    );
    assert_eq!(
        newest["request_payload_json"], payload,
        "the newest payload stays"
    );
    let oldest = one_row(
        &path,
        "SELECT request_payload_json FROM provider_requests ORDER BY started_at LIMIT 1",
    );
    assert!(
        oldest["request_payload_json"].is_null(),
        "the oldest payload goes"
    );
}

// -------------------------------------------------------------- activity

#[test]
fn newest_activity_pages_include_provider_failures_without_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    mixed_activity(&path, "t", 4);
    let mut items = Vec::new();
    let mut after = None;
    loop {
        let page = activity_at_ordered(&path, "t", after.as_deref(), 2, true).unwrap();
        items.extend(page.items);
        after = page.next;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(items.len(), 12);
    let keys: Vec<_> = items
        .iter()
        .map(|i| (i["at"].as_str().unwrap(), i["source"].as_str().unwrap()))
        .collect();
    assert!(keys.windows(2).all(|w| w[0] > w[1]));
    assert!(items.iter().any(|i| i["error_kind"] == "http_500"));
    assert!(items.iter().any(|i| i["status_code"] == 500));
    assert_eq!(
        last_provider_error_at(&path, "t").unwrap().unwrap()["error_kind"],
        "http_500"
    );
}

/// One row in each telemetry table per minute, oldest first, so a page
/// boundary can fall inside, between and across sources.
fn mixed_activity(path: &Path, thread: &str, minutes: usize) {
    for minute in (0..minutes).rev() {
        let at = minutes_ago(minute as i64);
        let mut request = provider_row(thread, &at, None);
        request.error_kind = (minute % 3 == 0).then(|| "http_500".to_owned());
        if request.error_kind.is_some() {
            request.status_code = Some(500);
        }
        write_provider_request(&open(path).unwrap(), &request).unwrap();
        record_rpc_messages_at(path, &[&rpc_row(thread, &at, None)]).unwrap();
        record_operational_event_at(
            path,
            &OperationalEventRow {
                event_type: "turn.start".to_owned(),
                ..event_row(thread, &at, Some("{\"tool_rounds\":1}"))
            },
        )
        .unwrap();
    }
}

#[test]
fn activity_pages_have_no_gaps_or_duplicates_across_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    mixed_activity(&path, "t", 4);
    mixed_activity(&path, "other", 2);

    let mut seen: Vec<(String, String)> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = activity_at(&path, "t", after.as_deref(), 5).unwrap();
        assert!(page.items.len() <= 5);
        for item in &page.items {
            let at = item["at"].as_str().unwrap().to_owned();
            let source = item["source"].as_str().unwrap().to_owned();
            seen.push((at, source));
        }
        if let Some(next) = page.next {
            after = Some(next);
        }
        if !page.has_more {
            break;
        }
    }
    // 4 minutes x 3 sources, this thread only.
    assert_eq!(seen.len(), 12, "{seen:?}");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a page repeated an item");
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(sorted, seen, "pages are in time order");
    assert!(
        seen.iter().all(|(_, source)| source != ""),
        "every item names its source"
    );
}

#[test]
fn activity_limits_are_clamped_and_a_bad_cursor_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    mixed_activity(&path, "t", 3);

    assert_eq!(activity_at(&path, "t", None, 0).unwrap().items.len(), 1);
    // More rows than one capped page, so the cap is visible and a page says
    // there is more instead of silently stopping at 200.
    mixed_activity(&path, "t", 80);
    let capped = activity_at(&path, "t", None, MAX_ACTIVITY_ITEMS + 1).unwrap();
    assert_eq!(capped.items.len(), MAX_ACTIVITY_ITEMS);
    assert!(capped.has_more);

    let error = activity_at(&path, "t", Some("nonsense"), 5).unwrap_err();
    assert!(error.contains("next token"), "{error}");
    let error = activity_at(&path, "t", Some("2026-11-01T00:00:00Z|nope|1"), 5).unwrap_err();
    assert!(error.contains("next token"), "{error}");
    assert!(activity_at(&path, "  ", None, 5).is_err());
}

#[test]
fn activity_returns_summaries_and_never_a_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let at = "2026-11-01T10:00:00Z";
    record_provider_request_at(
        &path,
        &provider_row("t", at, Some("{\"messages\":\"SECRET-PROMPT\"}")),
    )
    .unwrap();
    record_rpc_messages_at(
        &path,
        &[&rpc_row("t", at, Some("{\"params\":\"SECRET-RPC\"}"))],
    )
    .unwrap();
    record_operational_event_at(&path, &event_row("t", at, Some("{\"n\":1}"))).unwrap();

    let page = activity_at(&path, "t", None, 10).unwrap();
    assert_eq!(page.items.len(), 3);
    let rendered = serde_json::to_string(&page.items).unwrap();
    assert!(!rendered.contains("SECRET"), "{rendered}");
    let request = page
        .items
        .iter()
        .find(|item| item["source"] == "provider_request")
        .unwrap();
    assert_eq!(request["provider"], "openai");
    assert_eq!(request["model"], "gpt-6-luna");
    assert_eq!(request["status_code"], 200);
    assert_eq!(request["latency_ms"], 412);
    assert_eq!(request["input_tokens"], 100);
    assert!(request.get("request_payload_json").is_none());
    let rpc = page
        .items
        .iter()
        .find(|item| item["source"] == "rpc_message")
        .unwrap();
    assert_eq!(rpc["direction"], DIRECTION_RUSTCODE_TO_HARNESS);
    assert_eq!(rpc["kind"], KIND_REQUEST);
    assert_eq!(rpc["method"], "session/prompt");
    assert_eq!(rpc["byte_size"], 512);
    assert!(rpc.get("payload_json").is_none());
    let event = page
        .items
        .iter()
        .find(|item| item["source"] == "event")
        .unwrap();
    assert_eq!(event["type"], "turn.summary");
    assert_eq!(event["fields"]["n"], 1);

    // A thread nothing was recorded for is an empty page, and so is a
    // store that does not exist yet.
    assert_eq!(
        activity_at(&path, "nobody", None, 10).unwrap().items.len(),
        0
    );
    assert_eq!(
        activity_at(&dir.path().join("missing.sqlite"), "t", None, 10)
            .unwrap()
            .items
            .len(),
        0
    );
}

fn receipt(caller: &str, key: &str, action: &str, hash: &str) -> ReceiptRow {
    ReceiptRow {
        caller: caller.to_owned(),
        client_request_id: key.to_owned(),
        action: action.to_owned(),
        args_hash: hash.to_owned(),
        result_json: json!({ "session_id": "child", "link_id": "a/1" }).to_string(),
    }
}

#[test]
fn a_receipt_round_trips_and_a_changed_call_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);

    assert_eq!(
        check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap(),
        Receipt::None
    );
    record_receipt_at(&path, &receipt("a", "k1", "start_session", "h1")).unwrap();

    // The same call again: the recorded answer, and nothing stored but the
    // action, the hash and the result.
    let again = check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap();
    let Receipt::Replay { result, .. } = again else {
        panic!("expected a replay, got {again:?}");
    };
    assert_eq!(result["session_id"], "child");
    assert_eq!(result["link_id"], "a/1");
    let row = one_row(
        &path,
        "SELECT action, args_hash, result_json FROM command_receipts",
    );
    assert_eq!(row["action"], "start_session");
    assert_eq!(row["args_hash"], "h1");

    // The same key for other arguments or another action is refused.
    assert_eq!(
        check_receipt_at(&path, "a", "k1", "start_session", "h2").unwrap(),
        Receipt::Conflict
    );
    assert_eq!(
        check_receipt_at(&path, "a", "k1", "fan_out", "h1").unwrap(),
        Receipt::Conflict
    );
    // Another caller, or no key, is another call.
    assert_eq!(
        check_receipt_at(&path, "b", "k1", "start_session", "h1").unwrap(),
        Receipt::None
    );
    assert_eq!(
        check_receipt_at(&path, "a", "k2", "start_session", "h1").unwrap(),
        Receipt::None
    );

    // A second write under a key keeps the first receipt.
    let mut second = receipt("a", "k1", "start_session", "h1");
    second.result_json = json!({ "session_id": "other" }).to_string();
    record_receipt_at(&path, &second).unwrap();
    let Receipt::Replay { result, .. } =
        check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap()
    else {
        panic!("the first receipt must stay");
    };
    assert_eq!(result["session_id"], "child");

    // A receipt that carries no caller or key is refused, never written.
    assert!(record_receipt_at(&path, &receipt("", "k1", "start_session", "h1")).is_err());
    assert_eq!(count_rows(&path, "command_receipts"), 1);
}

#[test]
fn receipts_age_out_of_the_retry_window_and_stay_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    let connection = open(&path).unwrap();
    // One inside the window, one past it, inserted with its own timestamp.
    for (key, created) in [("fresh", days_ago(1)), ("old", days_ago(9))] {
        connection
            .execute(
                "INSERT INTO command_receipts
                        (caller, client_request_id, action, args_hash, result_json, created_at)
                     VALUES ('a', ?1, 'message_session', 'h1', '{}', ?2)",
                params![key, created],
            )
            .unwrap();
    }

    let report = prune_with(&connection, TELEMETRY_MAX_BYTES, Some(10_000)).unwrap();
    assert_eq!(report.rows_deleted, 1, "{report:?}");
    assert_eq!(
        check_receipt_at(&path, "a", "old", "message_session", "h1").unwrap(),
        Receipt::None,
        "past the window a call is new again"
    );
    assert!(matches!(
        check_receipt_at(&path, "a", "fresh", "message_session", "h1").unwrap(),
        Receipt::Replay { .. }
    ));

    // The row cap drops the oldest keepers of the window, not the newest.
    for key in ["k1", "k2", "k3"] {
        record_receipt_at(&path, &receipt("a", key, "fan_out", "h1")).unwrap();
    }
    let deleted = prune_receipts(&connection, COMMAND_RECEIPT_RETENTION_DAYS, 2).unwrap();
    assert_eq!(deleted, 2, "four receipts, two kept");
    assert_eq!(count_rows(&path, "command_receipts"), 2);
    let mut keys: Vec<String> =
        query_path(&path, "SELECT client_request_id FROM command_receipts", 10)
            .unwrap()
            .rows
            .iter()
            .map(|row| {
                row["client_request_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
    keys.sort();
    assert_eq!(keys, ["k2", "k3"], "the newest two stay");
}

fn outside_call(client: &str, tool: &str, outcome: &str) -> OutsideCallRow {
    OutsideCallRow {
        client_id: client.to_owned(),
        client_name: "Cursor".to_owned(),
        platform: Some("cursor".to_owned()),
        scope: "read_only".to_owned(),
        tool: tool.to_owned(),
        outcome: outcome.to_owned(),
        duration_ms: Some(12),
        detail: Some("x".repeat(500)),
    }
}

#[test]
fn outside_agent_calls_are_recorded_with_who_what_and_outcome_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    record_outside_call_at(&path, &outside_call("c1", "list_sessions", "ok")).unwrap();
    record_outside_call_at(&path, &outside_call("c2", "start_session", "denied")).unwrap();
    assert!(record_outside_call_at(&path, &outside_call("", "list_sessions", "ok")).is_err());

    let all = recent_outside_calls_at(&path, None, 10).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["tool"], "start_session", "newest first");
    assert_eq!(all[0]["outcome"], "denied");
    assert_eq!(
        all[0]["detail"].as_str().unwrap().chars().count(),
        MAX_OUTSIDE_DETAIL_CHARS,
        "a reason is cut, never stored whole"
    );
    let one = recent_outside_calls_at(&path, Some("c1"), 10).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0]["clientName"], "Cursor");
    // The table has no column that could hold arguments or an answer.
    let connection = open(&path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('outside_agent_calls')")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        columns
            .iter()
            .all(|name| !["args", "arguments", "result", "answer"]
                .iter()
                .any(|bad| name.contains(bad))),
        "{columns:?}"
    );
}

#[test]
fn outside_agent_calls_past_the_retention_window_are_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let connection = open(&path).unwrap();
    record_outside_call_at(&path, &outside_call("c1", "list_sessions", "ok")).unwrap();
    connection
            .execute(
                "INSERT INTO outside_agent_calls (at, client_id, client_name, scope, tool, outcome)
                 VALUES ('2020-01-01T00:00:00.000Z', 'old', 'Old', 'read_only', 'list_sessions', 'ok')",
                [],
            )
            .unwrap();
    assert_eq!(prune_outside_calls(&connection).unwrap(), 1);
    assert_eq!(count_rows(&path, "outside_agent_calls"), 1);
}

#[test]
fn a_harness_last_account_is_the_newest_turn_that_named_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATA_FILE);
    assert_eq!(last_account_at(&path, "claude-code").unwrap(), None);
    let connection = open(&path).unwrap();
    let on = |thread: &str, started: &str, harness: &str, account: Option<&str>| TurnRow {
        harness: Some(harness.to_owned()),
        account: account.map(str::to_owned),
        ..turn(thread, started, (1, 1), "completed")
    };
    for row in [
        on(
            "a",
            "2026-11-18T10:00:00Z",
            "claude-code",
            Some("personal-1"),
        ),
        on("b", "2026-11-19T09:00:00Z", "claude-code", Some("paral-2")),
        // A scheduled run on the default login, and another harness's account,
        // both newer: neither is what a new Claude Code thread runs on.
        on("c", "2026-11-19T10:00:00Z", "claude-code", None),
        on("d", "2026-11-19T11:00:00Z", "codex", Some("work")),
    ] {
        write_turn(&connection, &row).unwrap();
    }
    assert_eq!(
        last_account_at(&path, "claude-code").unwrap().as_deref(),
        Some("paral-2")
    );
    assert_eq!(
        last_account_at(&path, "codex").unwrap().as_deref(),
        Some("work")
    );
    assert_eq!(last_account_at(&path, "gemini").unwrap(), None);
}
