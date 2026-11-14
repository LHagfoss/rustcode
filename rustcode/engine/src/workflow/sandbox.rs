//! The JavaScript a workflow script runs in (`docs/workflows.md`).
//!
//! QuickJS through `rquickjs`, with nothing added that reaches outside it: no
//! module loader (so no `import` of a file, a package or a URL), none of the
//! `std`/`os` helpers, no timers, no `fetch`, and no clock, because the
//! context is built without the `Date` and `Performance` intrinsics and
//! `Math.random` is deleted. A script that cannot see the time or draw a
//! random number does the same thing every time it runs, which is what lets a
//! restarted run replay its cached `agent()` calls and arrive at the same next
//! call. The only ways out are the four functions [`Host`] answers:
//! `agent()`, `parallel()` (plain JavaScript over promises), `phase()` and
//! `log()`.
//!
//! The runtime has a memory cap and a stack cap, and an interrupt handler that
//! ends a script still running at its deadline, so a script that loops without
//! awaiting cannot hold a worker thread forever.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use rquickjs::context::intrinsic;
use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Exception, Function, Promise};
use serde_json::Value;

/// A script's memory, at most.
pub(super) const MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// A script's own call stack, at most.
pub(super) const STACK_BYTES: usize = 512 * 1024;

/// What an `agent()` call settles with, as JSON: `{"value": …}` or
/// `{"error": "…"}`.
pub(super) type Reply = Pin<Box<dyn Future<Output = String> + Send>>;

/// What the sandbox asks of the run it belongs to.
pub(super) trait Host: Send + Sync + 'static {
    fn phase(&self, title: &str);
    fn log(&self, line: &str);
    /// One `agent(prompt, options)` call; `options` is its JSON text. The
    /// call's place in the script's call order is taken before this returns,
    /// not when the future first runs.
    fn agent(&self, prompt: String, options: String) -> Reply;
}

/// How far a script may go.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub deadline: Instant,
}

impl Bounds {
    pub(super) fn until(deadline: Instant) -> Self {
        Self {
            memory_bytes: MEMORY_BYTES,
            stack_bytes: STACK_BYTES,
            deadline,
        }
    }
}

/// `agent`, `parallel`, `phase` and `log`, over the three host functions the
/// sandbox installs (and then removes from the globals, so a script reaches
/// them only through these).
const PRELUDE: &str = r#"
(() => {
  const hostAgent = globalThis.__host_agent;
  const hostPhase = globalThis.__host_phase;
  const hostLog = globalThis.__host_log;
  delete globalThis.__host_agent;
  delete globalThis.__host_phase;
  delete globalThis.__host_log;
  // No clock and no randomness: a script that reads neither runs the same way
  // every time, which a restarted run relies on.
  delete Math.random;

  globalThis.agent = async (prompt, options) => {
    if (typeof prompt !== "string" || prompt.length === 0) {
      throw new TypeError("agent(prompt, options): prompt must be a non-empty string");
    }
    if (options !== undefined && (options === null || typeof options !== "object" || Array.isArray(options))) {
      throw new TypeError("agent(prompt, options): options must be an object");
    }
    const reply = JSON.parse(await hostAgent(prompt, JSON.stringify(options === undefined ? {} : options)));
    if (reply.error !== undefined) {
      throw new Error(reply.error);
    }
    return reply.value;
  };

  // Functions are called here, in order, before anything is awaited, so the
  // calls they make happen in the order they are listed.
  globalThis.parallel = async (items) => {
    if (!Array.isArray(items)) {
      throw new TypeError("parallel(items): items must be an array of functions or promises");
    }
    return Promise.all(items.map((item) => (typeof item === "function" ? item() : item)));
  };

  globalThis.phase = (title) => {
    hostPhase(String(title));
  };

  globalThis.log = (...parts) => {
    hostLog(
      parts
        .map((part) => {
          if (typeof part === "string") return part;
          try {
            const text = JSON.stringify(part);
            return text === undefined ? String(part) : text;
          } catch (_) {
            return String(part);
          }
        })
        .join(" "),
    );
  };
})();
"#;

/// Run `source` as the body of an async function (so it may `await` and
/// `return`) with `args` as a global, and give back what it returns as JSON.
pub(super) async fn run(
    source: &str,
    args: &Value,
    host: Arc<dyn Host>,
    bounds: Bounds,
) -> Result<Value, String> {
    let sandbox = |error: rquickjs::Error| format!("the script sandbox failed: {error}");
    let runtime = AsyncRuntime::new().map_err(sandbox)?;
    runtime.set_memory_limit(bounds.memory_bytes).await;
    runtime.set_max_stack_size(bounds.stack_bytes).await;
    let deadline = bounds.deadline;
    runtime
        .set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)))
        .await;
    // Only the language: no Date, no Performance, no Proxy, no typed arrays.
    let context = AsyncContext::builder()
        .with::<intrinsic::Eval>()
        .with::<intrinsic::Json>()
        .with::<intrinsic::Promise>()
        .with::<intrinsic::RegExpCompiler>()
        .with::<intrinsic::RegExp>()
        .with::<intrinsic::MapSet>()
        .build_async(&runtime)
        .await
        .map_err(sandbox)?;
    let program = format!(
        "(async () => {{\n{source}\n}})().then((value) => JSON.stringify(value === undefined ? null : value) ?? \"null\")"
    );
    let args = args.to_string();
    context
        .async_with(async move |ctx| execute(ctx, host, args, program).await)
        .await
}

async fn execute<'js>(
    ctx: Ctx<'js>,
    host: Arc<dyn Host>,
    args: String,
    program: String,
) -> Result<Value, String> {
    install(&ctx, &host).map_err(|error| describe(&ctx, error))?;
    let parsed = ctx
        .json_parse(args)
        .map_err(|error| describe(&ctx, error))?;
    ctx.globals()
        .set("args", parsed)
        .map_err(|error| describe(&ctx, error))?;
    let promise: Promise = ctx.eval(program).map_err(|error| describe(&ctx, error))?;
    let text: String = promise
        .into_future::<String>()
        .await
        .map_err(|error| describe(&ctx, error))?;
    serde_json::from_str(&text).map_err(|error| format!("the script's result is not JSON: {error}"))
}

fn install<'js>(ctx: &Ctx<'js>, host: &Arc<dyn Host>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let agent_host = Arc::clone(host);
    globals.set(
        "__host_agent",
        Function::new(
            ctx.clone(),
            Async(move |prompt: String, options: String| agent_host.agent(prompt, options)),
        )?,
    )?;
    let phase_host = Arc::clone(host);
    globals.set(
        "__host_phase",
        Function::new(ctx.clone(), move |title: String| phase_host.phase(&title))?,
    )?;
    let log_host = Arc::clone(host);
    globals.set(
        "__host_log",
        Function::new(ctx.clone(), move |line: String| log_host.log(&line))?,
    )?;
    ctx.eval::<(), _>(PRELUDE)?;
    Ok(())
}

/// What a script threw, as the sentence the run records.
fn describe(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    let rquickjs::Error::Exception = error else {
        return error.to_string();
    };
    let caught = ctx.catch();
    let message = match caught.as_object().cloned().and_then(Exception::from_object) {
        Some(exception) => exception
            .message()
            .unwrap_or_else(|| "the script threw an error".to_owned()),
        None => match caught.as_string().map(|text| text.to_string()) {
            Some(Ok(text)) => text,
            _ => "the script threw something that is not an error".to_owned(),
        },
    };
    // QuickJS ends a script the interrupt handler stopped with this message.
    if message.contains("interrupted") {
        return "the script ran past its time limit".to_owned();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Answers `agent()` with `"<prompt> done"` after a short wait, and keeps
    /// what the script told it.
    #[derive(Default)]
    struct Echo {
        phases: Mutex<Vec<String>>,
        logs: Mutex<Vec<String>>,
        prompts: Mutex<Vec<String>>,
    }

    impl Host for Echo {
        fn phase(&self, title: &str) {
            self.phases.lock().unwrap().push(title.to_owned());
        }

        fn log(&self, line: &str) {
            self.logs.lock().unwrap().push(line.to_owned());
        }

        fn agent(&self, prompt: String, options: String) -> Reply {
            self.prompts.lock().unwrap().push(prompt.clone());
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                if prompt == "fail" {
                    return json!({"error": "the worker failed"}).to_string();
                }
                let options: Value = serde_json::from_str(&options).unwrap();
                json!({"value": {"prompt": format!("{prompt} done"), "options": options}})
                    .to_string()
            })
        }
    }

    async fn run_script(source: &str) -> (Result<Value, String>, Arc<Echo>) {
        let host = Arc::new(Echo::default());
        let result = run(
            source,
            &json!({"files": ["a.rs", "b.rs"]}),
            host.clone(),
            Bounds::until(Instant::now() + Duration::from_secs(20)),
        )
        .await;
        (result, host)
    }

    #[tokio::test]
    async fn a_script_returns_json_and_reads_its_args() {
        let (result, _) = run_script("return { count: args.files.length, ok: true };").await;
        assert_eq!(result, Ok(json!({"count": 2, "ok": true})));
        let (result, _) = run_script("const x = 1;").await;
        assert_eq!(result, Ok(Value::Null));
    }

    #[tokio::test]
    async fn the_sandbox_has_no_filesystem_network_or_clock() {
        let (result, _) = run_script(
            r#"
            const names = [
              "require", "module", "exports", "process", "Deno", "Bun", "std", "os",
              "fetch", "XMLHttpRequest", "WebSocket", "EventSource",
              "setTimeout", "setInterval", "setImmediate",
              "Date", "performance",
            ];
            const present = names.filter((name) => typeof globalThis[name] !== "undefined");
            let random = "refused";
            try { Math.random(); random = "leaked"; } catch (_) {}
            let imported = "refused";
            try { await import("std"); imported = "leaked"; } catch (_) {}
            let viaFunction = new Function("return typeof process + typeof fetch + typeof Date")();
            return { present, random, imported, viaFunction };
            "#,
        )
        .await;
        assert_eq!(
            result,
            Ok(json!({
                "present": [],
                "random": "refused",
                "imported": "refused",
                "viaFunction": "undefinedundefinedundefined",
            }))
        );
    }

    #[tokio::test]
    async fn the_host_functions_are_not_left_in_the_globals() {
        let (result, _) = run_script(
            r#"return ["__host_agent", "__host_phase", "__host_log"].filter((n) => typeof globalThis[n] !== "undefined");"#,
        )
        .await;
        assert_eq!(result, Ok(json!([])));
    }

    #[tokio::test]
    async fn agent_phase_and_log_reach_the_host() {
        let (result, host) = run_script(
            r#"
            phase("review");
            log("starting", { n: 1 });
            const reply = await agent("look at a.rs", { harness: "codex" });
            phase("verify");
            return reply;
            "#,
        )
        .await;
        assert_eq!(
            result,
            Ok(json!({"prompt": "look at a.rs done", "options": {"harness": "codex"}}))
        );
        assert_eq!(*host.phases.lock().unwrap(), ["review", "verify"]);
        assert_eq!(*host.logs.lock().unwrap(), [r#"starting {"n":1}"#]);
    }

    #[tokio::test]
    async fn parallel_starts_calls_in_order_and_returns_results_in_order() {
        let (result, host) = run_script(
            r#"
            const replies = await parallel(args.files.map((file) => () => agent("review " + file)));
            const again = await parallel([agent("x"), agent("y")]);
            return [replies.map((r) => r.prompt), again.map((r) => r.prompt)];
            "#,
        )
        .await;
        assert_eq!(
            result,
            Ok(json!([
                ["review a.rs done", "review b.rs done"],
                ["x done", "y done"]
            ]))
        );
        assert_eq!(
            *host.prompts.lock().unwrap(),
            ["review a.rs", "review b.rs", "x", "y"]
        );
    }

    #[tokio::test]
    async fn a_failed_call_is_an_error_the_script_can_catch() {
        let (result, _) = run_script(
            r#"
            try { await agent("fail"); return "no error"; }
            catch (error) { return error.message; }
            "#,
        )
        .await;
        assert_eq!(result, Ok(json!("the worker failed")));
        let (result, _) = run_script(r#"await agent("fail");"#).await;
        assert_eq!(result, Err("the worker failed".to_owned()));
    }

    #[tokio::test]
    async fn bad_calls_are_refused_before_the_host_hears_of_them() {
        let (result, host) = run_script(
            r#"
            const problems = [];
            for (const call of [() => agent(""), () => agent(5), () => agent("x", []), () => agent("x", null), () => parallel(3)]) {
              try { await call(); problems.push("accepted"); } catch (error) { problems.push(error.name); }
            }
            return problems;
            "#,
        )
        .await;
        assert_eq!(
            result,
            Ok(json!([
                "TypeError",
                "TypeError",
                "TypeError",
                "TypeError",
                "TypeError"
            ]))
        );
        assert!(host.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_syntax_error_and_a_thrown_value_are_errors_not_panics() {
        let (result, _) = run_script("return (;").await;
        assert!(result.is_err(), "{result:?}");
        let (result, _) = run_script(r#"throw "plain text";"#).await;
        assert_eq!(result, Err("plain text".to_owned()));
        let (result, _) = run_script("throw new RangeError('too far');").await;
        assert_eq!(result, Err("too far".to_owned()));
    }

    #[tokio::test]
    async fn a_script_that_never_awaits_is_stopped_at_its_deadline() {
        let host = Arc::new(Echo::default());
        let started = Instant::now();
        let result = run(
            "while (true) {}",
            &json!({}),
            host,
            Bounds::until(Instant::now() + Duration::from_millis(300)),
        )
        .await;
        assert_eq!(result, Err("the script ran past its time limit".to_owned()));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_script_that_eats_memory_is_stopped() {
        let host = Arc::new(Echo::default());
        let result = run(
            r#"const hold = []; while (true) { hold.push("x".repeat(1000000)); }"#,
            &json!({}),
            host,
            Bounds {
                memory_bytes: 16 * 1024 * 1024,
                stack_bytes: STACK_BYTES,
                deadline: Instant::now() + Duration::from_secs(20),
            },
        )
        .await;
        assert!(result.is_err(), "{result:?}");
    }
}
