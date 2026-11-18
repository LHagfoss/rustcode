//! The schema migrations, applied in order by `migrate`.

pub(super) const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "threads_turns_and_tool_calls",
        "CREATE TABLE threads (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        project TEXT,
        cwd TEXT,
        harness TEXT,
        model TEXT,
        effort TEXT,
        created_at TEXT,
        updated_at TEXT
    );
    CREATE TABLE turns (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT,
        harness TEXT,
        model TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        cache_write_tokens INTEGER,
        cost_usd REAL,
        tool_calls INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL,
        UNIQUE(thread_id, started_at)
    );
    CREATE INDEX turns_day ON turns(started_at);
    CREATE INDEX turns_thread ON turns(thread_id);
    CREATE TABLE tool_calls (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id INTEGER,
        tool_name TEXT NOT NULL,
        success INTEGER,
        exit_code INTEGER,
        at TEXT
    );
    CREATE INDEX tool_calls_thread ON tool_calls(thread_id);
    CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE VIEW daily_usage AS
        SELECT date(started_at) AS day, harness, model,
               SUM(input_tokens) AS input_tokens,
               SUM(output_tokens) AS output_tokens,
               SUM(cached_tokens) AS cached_tokens,
               SUM(cache_write_tokens) AS cache_write_tokens,
               SUM(cost_usd) AS cost_usd,
               COUNT(*) AS turns
        FROM turns
        GROUP BY day, harness, model;",
    ),
    (
        2,
        "thread_links_and_orchestration_state",
        "CREATE TABLE thread_links (
        id TEXT PRIMARY KEY,
        orchestrator TEXT,
        target TEXT,
        state TEXT,
        started_at INTEGER,
        settled_at INTEGER,
        harness TEXT,
        files_changed INTEGER,
        body TEXT NOT NULL
    );
    CREATE INDEX thread_links_target ON thread_links(target);
    CREATE TABLE orchestration_state (key TEXT PRIMARY KEY, body TEXT NOT NULL);",
    ),
    (
        3,
        "thread_parents",
        "ALTER TABLE threads ADD COLUMN parent_id TEXT;
    ALTER TABLE threads ADD COLUMN parent_title TEXT;
    CREATE INDEX threads_parent ON threads(parent_id);",
    ),
    (
        4,
        "request_and_protocol_telemetry",
        "CREATE TABLE provider_requests (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        round INTEGER,
        provider TEXT,
        model TEXT,
        endpoint_host TEXT,
        endpoint_path TEXT,
        method TEXT,
        attempt INTEGER,
        started_at TEXT NOT NULL,
        headers_at TEXT,
        finished_at TEXT,
        latency_ms INTEGER,
        status_code INTEGER,
        request_bytes INTEGER,
        response_bytes INTEGER,
        stream_event_count INTEGER,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        error_kind TEXT,
        request_payload_json TEXT,
        response_summary_json TEXT,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX provider_requests_thread ON provider_requests(thread_id, started_at);
    CREATE INDEX provider_requests_started ON provider_requests(started_at);
    CREATE TABLE harness_rpc_messages (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        harness TEXT,
        protocol TEXT,
        direction TEXT NOT NULL,
        message_kind TEXT NOT NULL,
        rpc_id TEXT,
        method TEXT,
        at TEXT NOT NULL,
        latency_ms INTEGER,
        byte_size INTEGER,
        payload_json TEXT,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX harness_rpc_messages_thread ON harness_rpc_messages(thread_id, at);
    CREATE INDEX harness_rpc_messages_at ON harness_rpc_messages(at);
    CREATE TABLE operational_events (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        sequence INTEGER NOT NULL,
        event_type TEXT NOT NULL,
        at TEXT NOT NULL,
        fields_json TEXT,
        original_bytes INTEGER,
        captured_bytes INTEGER,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0,
        UNIQUE(thread_id, sequence)
    );
    CREATE INDEX operational_events_thread ON operational_events(thread_id, at);
    CREATE INDEX operational_events_at ON operational_events(at);",
    ),
    (
        5,
        "command_receipts",
        "CREATE TABLE command_receipts (
        caller TEXT NOT NULL,
        client_request_id TEXT NOT NULL,
        action TEXT NOT NULL,
        args_hash TEXT NOT NULL,
        result_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (caller, client_request_id)
    );
    CREATE INDEX command_receipts_created ON command_receipts(created_at);",
    ),
    (
        6,
        "usage_cursors",
        "CREATE TABLE usage_cursors (
        thread_id TEXT NOT NULL,
        harness TEXT NOT NULL,
        series TEXT NOT NULL DEFAULT '',
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        cache_write_tokens INTEGER,
        cost_usd REAL,
        updated_at TEXT,
        PRIMARY KEY (thread_id, harness, series)
    );",
    ),
    (
        7,
        "durable_telemetry",
        "ALTER TABLE turns ADD COLUMN duration_ms INTEGER;
    ALTER TABLE turns ADD COLUMN time_to_first_token_ms INTEGER;
    ALTER TABLE turns ADD COLUMN model_time_ms INTEGER;
    ALTER TABLE turns ADD COLUMN tool_time_ms INTEGER;
    ALTER TABLE turns ADD COLUMN context_tokens INTEGER;
    ALTER TABLE turns ADD COLUMN context_window INTEGER;
    ALTER TABLE turns ADD COLUMN stop_reason TEXT;
    ALTER TABLE tool_calls ADD COLUMN started_at TEXT;
    ALTER TABLE tool_calls ADD COLUMN duration_ms INTEGER;
    ALTER TABLE tool_calls ADD COLUMN output_bytes INTEGER;
    ALTER TABLE tool_calls ADD COLUMN truncated INTEGER;
    ALTER TABLE tool_calls ADD COLUMN mcp_server TEXT;
    ALTER TABLE tool_calls ADD COLUMN command_name TEXT;
    ALTER TABLE tool_calls ADD COLUMN error_kind TEXT;
    CREATE INDEX tool_calls_name ON tool_calls(tool_name);
    CREATE INDEX harness_rpc_messages_method ON harness_rpc_messages(method);
    CREATE TABLE compactions (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        harness TEXT,
        model TEXT,
        at TEXT NOT NULL,
        duration_ms INTEGER,
        trigger TEXT NOT NULL,
        mode TEXT,
        tokens_before INTEGER,
        tokens_after INTEGER,
        messages_before INTEGER,
        messages_after INTEGER,
        idle_secs_before INTEGER,
        count INTEGER,
        success INTEGER NOT NULL DEFAULT 1,
        error_kind TEXT,
        external_id TEXT,
        source TEXT,
        UNIQUE(thread_id, external_id)
    );
    CREATE INDEX compactions_thread ON compactions(thread_id, at);
    CREATE INDEX compactions_at ON compactions(at);
    CREATE VIEW daily_compactions AS
        SELECT date(at) AS day, harness, trigger,
               COUNT(*) AS compactions,
               SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failed,
               SUM(tokens_before) AS tokens_before,
               SUM(tokens_after) AS tokens_after,
               SUM(CASE WHEN tokens_before IS NOT NULL AND tokens_after IS NOT NULL
                        THEN tokens_before - tokens_after END) AS reclaimed_tokens,
               AVG(duration_ms) AS mean_duration_ms
        FROM compactions
        GROUP BY day, harness, trigger;
    CREATE VIEW tool_failure_rates AS
        SELECT date(at) AS day, tool_name,
               COUNT(*) AS calls,
               SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failures,
               CASE WHEN COUNT(*) > 0
                    THEN 1.0 * SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) / COUNT(*)
               END AS failure_rate,
               AVG(duration_ms) AS mean_duration_ms,
               SUM(CASE WHEN truncated = 1 THEN 1 ELSE 0 END) AS truncated
        FROM tool_calls
        WHERE at IS NOT NULL
        GROUP BY day, tool_name;
    CREATE VIEW turn_latency_daily AS
        SELECT date(started_at) AS day, harness, model,
               COUNT(*) AS turns,
               AVG(duration_ms) AS mean_duration_ms,
               AVG(time_to_first_token_ms) AS mean_time_to_first_token_ms,
               AVG(model_time_ms) AS mean_model_time_ms,
               AVG(tool_time_ms) AS mean_tool_time_ms,
               AVG(context_tokens) AS mean_context_tokens
        FROM turns
        WHERE duration_ms IS NOT NULL
        GROUP BY day, harness, model;",
    ),
    (
        8,
        "tool_calls_turn_index",
        "CREATE INDEX IF NOT EXISTS tool_calls_turn ON tool_calls(turn_id);",
    ),
    // Additive only: new nullable columns and a new table. No existing column,
    // row or view is changed, so a store at migration 8 keeps every row.
    (
        9,
        "keep_more_thread_data",
        "ALTER TABLE threads ADD COLUMN account TEXT;
         ALTER TABLE threads ADD COLUMN machine TEXT;
         ALTER TABLE threads ADD COLUMN created_with_version TEXT;
         ALTER TABLE threads ADD COLUMN branch TEXT;
         ALTER TABLE threads ADD COLUMN final_commit TEXT;
         ALTER TABLE threads ADD COLUMN pull_request TEXT;
         ALTER TABLE threads ADD COLUMN pull_request_state TEXT;
         ALTER TABLE threads ADD COLUMN settled_at TEXT;
         ALTER TABLE turns ADD COLUMN effort TEXT;
         ALTER TABLE turns ADD COLUMN account TEXT;
         ALTER TABLE turns ADD COLUMN machine TEXT;
         ALTER TABLE turns ADD COLUMN rustcode_version TEXT;
         ALTER TABLE turns ADD COLUMN diff_files INTEGER;
         ALTER TABLE turns ADD COLUMN diff_added INTEGER;
         ALTER TABLE turns ADD COLUMN diff_removed INTEGER;
         CREATE TABLE IF NOT EXISTS approvals (
            id INTEGER PRIMARY KEY,
            thread_id TEXT NOT NULL,
            source TEXT NOT NULL,
            tool_name TEXT,
            requested_at TEXT NOT NULL,
            decided_at TEXT,
            wait_ms INTEGER,
            decision TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS approvals_thread ON approvals(thread_id, requested_at);",
    ),
    (
        10,
        "outside_agent_calls",
        "CREATE TABLE outside_agent_calls (
        id INTEGER PRIMARY KEY,
        at TEXT NOT NULL,
        client_id TEXT NOT NULL,
        client_name TEXT NOT NULL,
        platform TEXT,
        scope TEXT NOT NULL,
        tool TEXT NOT NULL,
        outcome TEXT NOT NULL,
        duration_ms INTEGER,
        detail TEXT
    );
    CREATE INDEX outside_agent_calls_at ON outside_agent_calls(at);
    CREATE INDEX outside_agent_calls_client ON outside_agent_calls(client_id, at);",
    ),
    (
        11,
        "tool_command_time_daily",
        "CREATE VIEW tool_command_time_daily AS
         SELECT substr(at, 1, 10) AS day, tool_name, command_name,
                COUNT(*) AS calls, COUNT(duration_ms) AS timed_calls,
                SUM(duration_ms) AS total_duration_ms,
                AVG(duration_ms) AS mean_duration_ms
         FROM tool_calls
         GROUP BY substr(at, 1, 10), tool_name, command_name;",
    ),
    // Additive: why a turn failed, and an external harness's own tool name
    // next to the ACP kind in `tool_name`.
    (
        12,
        "failure_kinds_and_harness_tools",
        "ALTER TABLE turns ADD COLUMN error_kind TEXT;
         ALTER TABLE tool_calls ADD COLUMN harness_tool TEXT;",
    ),
    // Claude Code compactions the detector missed: the adapter moved their
    // facts to `_meta.jetbrains.air.contextCompaction`. Where payload capture
    // kept the update, it is the row; an existing row is left alone.
    (
        13,
        "backfill_claude_compactions",
        "INSERT INTO compactions (thread_id, harness, model, at, duration_ms, trigger, mode,
                tokens_before, tokens_after, success, external_id, source)
         SELECT m.thread_id, m.harness,
                (SELECT model FROM threads WHERE id = m.thread_id),
                m.at,
                json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.durationMs'),
                CASE json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.trigger')
                    WHEN 'manual' THEN 'manual'
                    WHEN 'idle' THEN 'cache_expiry'
                    ELSE 'harness_auto' END,
                'harness_summary',
                json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.preTokens'),
                json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.postTokens'),
                1,
                json_extract(m.payload_json, '$.params.update.toolCallId'),
                m.harness
         FROM harness_rpc_messages m
         WHERE m.thread_id IS NOT NULL
           AND m.method = 'session/update'
           AND m.payload_json LIKE '%contextCompaction%'
           AND json_valid(m.payload_json)
           AND json_extract(m.payload_json, '$.params.update.toolCallId') IS NOT NULL
           AND (json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.trigger') IS NOT NULL
                OR json_extract(m.payload_json, '$.params.update._meta.jetbrains.air.contextCompaction.preTokens') IS NOT NULL)
         ON CONFLICT(thread_id, external_id) DO NOTHING;",
    ),
    (
        14,
        "remove_duplicate_backfill_turns",
        "DELETE FROM turns WHERE status = 'backfill' AND EXISTS (
            SELECT 1 FROM turns live WHERE live.thread_id = turns.thread_id
              AND live.status != 'backfill'
         );",
    ),
    // Equal usage is only evidence of a possible duplicate, not proof. Keep
    // the original rows intact and expose candidates for review.
    (
        15,
        "flag_repeated_claude_turn_usage",
        "CREATE VIEW suspected_repeated_claude_turn_usage AS
         SELECT current.id AS turn_id, previous.id AS previous_turn_id,
                current.thread_id, current.started_at
         FROM turns current JOIN turns previous ON previous.id = (
             SELECT p.id FROM turns p
             WHERE p.thread_id = current.thread_id AND p.started_at < current.started_at
             ORDER BY p.started_at DESC LIMIT 1
         )
         WHERE current.harness = 'claude-code' AND previous.harness = 'claude-code'
           AND current.status <> 'backfill' AND previous.status <> 'backfill'
           AND current.tool_calls = 0 AND current.output_tokens > 0
           AND current.output_tokens = previous.output_tokens
           AND current.cost_usd IS NOT NULL AND current.cost_usd = previous.cost_usd;",
    ),
    // What each thread's pull request came to (#3907), from the pull request
    // watch. Rows for pull requests recorded before this migration only know
    // the state: their counters stay NULL rather than a guessed 0.
    (
        16,
        "thread_pull_request_outcomes",
        "CREATE TABLE IF NOT EXISTS thread_outcomes (
            thread_id TEXT NOT NULL,
            repo TEXT NOT NULL,
            pr_number INTEGER NOT NULL,
            url TEXT,
            title TEXT,
            state TEXT NOT NULL,
            first_seen_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            merged_at TEXT,
            closed_at TEXT,
            closed_unmerged INTEGER NOT NULL DEFAULT 0,
            ci_failures INTEGER,
            fix_rounds INTEGER,
            reverted INTEGER NOT NULL DEFAULT 0,
            reverted_by INTEGER,
            head TEXT,
            failed_head TEXT,
            PRIMARY KEY (thread_id, repo, pr_number)
         );
         CREATE INDEX IF NOT EXISTS thread_outcomes_pull ON thread_outcomes(repo, pr_number);
         INSERT INTO thread_outcomes (thread_id, repo, pr_number, url, state, first_seen_at,
                updated_at, closed_unmerged)
         SELECT id,
                substr(pull_request, instr(pull_request, '://') + 3,
                       instr(pull_request, '/pull/') - instr(pull_request, '://') - 3),
                CAST(substr(pull_request, instr(pull_request, '/pull/') + 6) AS INTEGER),
                pull_request, pull_request_state,
                COALESCE(updated_at, created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                COALESCE(updated_at, created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                pull_request_state = 'closed'
         FROM threads
         WHERE pull_request LIKE '%://%/pull/%' AND pull_request_state IS NOT NULL
         ON CONFLICT DO NOTHING;
         UPDATE thread_outcomes
         SET repo = substr(repo, instr(repo, '/') + 1)
         WHERE instr(repo, '/') > 0 AND repo LIKE '%/%/%';
         CREATE VIEW pr_outcomes_by_model AS
         WITH spend AS (
            SELECT thread_id, COALESCE(harness, 'unknown') AS harness,
                   COALESCE(model, '') AS model,
                   SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens,
                   SUM(cached_tokens) AS cached_tokens,
                   SUM(cache_write_tokens) AS cache_write_tokens,
                   SUM(cost_usd) AS cost_usd, COUNT(*) AS turns,
                   SUM(CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END) AS turns_without_cost
            FROM turns
            WHERE thread_id IN (SELECT thread_id FROM thread_outcomes)
            GROUP BY 1, 2, 3
         ), pulls AS (
            SELECT s.harness, s.model, o.repo, o.pr_number,
                   MAX(o.state = 'merged') AS merged, MAX(o.state = 'closed') AS closed,
                   MAX(o.state = 'open') AS open, MAX(o.ci_failures) AS ci_failures,
                   MAX(o.fix_rounds) AS fix_rounds, MAX(o.reverted) AS reverted
            FROM spend s JOIN thread_outcomes o ON o.thread_id = s.thread_id
            GROUP BY 1, 2, 3, 4
         ), outcome AS (
            SELECT harness, model, COUNT(*) AS pull_requests, SUM(merged) AS merged,
                   SUM(closed) - SUM(merged AND closed) AS closed_unmerged,
                   SUM(open AND NOT merged AND NOT closed) AS still_open,
                   SUM(reverted) AS reverted,
                   AVG(ci_failures) AS avg_ci_failures, AVG(fix_rounds) AS avg_fix_rounds,
                   COUNT(ci_failures) AS counted_pull_requests
            FROM pulls GROUP BY 1, 2
         ), cost AS (
            SELECT harness, model, COUNT(DISTINCT thread_id) AS threads,
                   SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens,
                   SUM(cached_tokens) AS cached_tokens,
                   SUM(cache_write_tokens) AS cache_write_tokens,
                   SUM(cost_usd) AS cost_usd, SUM(turns) AS turns,
                   SUM(turns_without_cost) AS turns_without_cost
            FROM spend GROUP BY 1, 2
         )
         SELECT c.harness, c.model, c.threads, o.pull_requests, o.merged, o.closed_unmerged,
                o.still_open,
                CASE WHEN o.merged + o.closed_unmerged > 0
                     THEN CAST(o.merged AS REAL) / (o.merged + o.closed_unmerged) END
                    AS merge_share,
                o.avg_ci_failures, o.avg_fix_rounds, o.counted_pull_requests, o.reverted,
                c.input_tokens, c.output_tokens, c.cached_tokens, c.cache_write_tokens,
                c.cost_usd, c.turns, c.turns_without_cost,
                CASE WHEN o.merged > 0 THEN c.cost_usd / o.merged END AS cost_per_merged_pr,
                CASE WHEN o.merged > 0 THEN c.input_tokens / o.merged END
                    AS input_tokens_per_merged_pr,
                CASE WHEN o.merged > 0 THEN c.output_tokens / o.merged END
                    AS output_tokens_per_merged_pr,
                CASE WHEN o.merged > 0 AND (c.input_tokens IS NOT NULL OR c.output_tokens IS NOT NULL)
                     THEN (COALESCE(c.input_tokens, 0) + COALESCE(c.output_tokens, 0)
                           + CASE WHEN c.harness = 'codex' THEN 0
                                  ELSE COALESCE(c.cached_tokens, 0)
                                       + COALESCE(c.cache_write_tokens, 0) END)
                          / o.merged END AS tokens_per_merged_pr
         FROM cost c JOIN outcome o ON o.harness = c.harness AND o.model = c.model;",
    ),
];
