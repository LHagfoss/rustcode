use super::*;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<EngineEvent>>>);

impl Sink for Recorder {
    fn deliver(&self, event: EngineEvent) -> bool {
        self.0.lock().unwrap().push(event);
        true
    }
}

impl Recorder {
    fn take(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    fn lines(&self) -> Vec<Value> {
        self.take()
            .into_iter()
            .flat_map(|event| match event {
                EngineEvent::Line { line } => vec![line],
                EngineEvent::Lines { lines } => lines,
                _ => Vec::new(),
            })
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    }
}

fn open() -> EngineEvent {
    EngineEvent::Open {
        engine: "Studio (192.168.1.20:17879)".to_owned(),
    }
}

fn request(id: u64, method: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}}).to_string()
}

fn wire_id(line: &str) -> u64 {
    serde_json::from_str::<Value>(line).unwrap()["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn two_windows_share_one_connection_and_each_gets_its_own_answers() {
    let link = Link::<Recorder>::default();
    let (main, second) = (Recorder::default(), Recorder::default());
    assert!(!link.attach("main", main.clone()));
    link.send(open()).unwrap();
    assert!(!link.attach("window-2", second.clone()));
    assert!(matches!(
        second.take().as_slice(),
        [EngineEvent::Open { .. }]
    ));
    main.take();

    let from_main = link.outbound("main", &request(1, "session/list")).unwrap();
    let from_second = link
        .outbound("window-2", &request(1, "session/list"))
        .unwrap();
    assert_ne!(wire_id(&from_main), wire_id(&from_second));

    let answer = |id: u64, which: &str| {
        json!({"jsonrpc": "2.0", "id": id, "result": {"from": which}}).to_string()
    };
    link.send(EngineEvent::Lines {
        lines: vec![
            answer(wire_id(&from_second), "second"),
            answer(wire_id(&from_main), "main"),
        ],
    })
    .unwrap();
    assert_eq!(
        main.lines(),
        vec![json!({"jsonrpc": "2.0", "id": 1, "result": {"from": "main"}})]
    );
    assert_eq!(
        second.lines(),
        vec![json!({"jsonrpc": "2.0", "id": 1, "result": {"from": "second"}})]
    );
}

#[test]
fn a_window_that_joins_is_initialized_from_the_first_ones_answer() {
    let link = Link::<Recorder>::default();
    let (main, second) = (Recorder::default(), Recorder::default());
    link.attach("main", main.clone());
    link.send(open()).unwrap();
    let init = link.outbound("main", &request(1, "initialize")).unwrap();
    link.send(EngineEvent::Line {
        line: json!({"jsonrpc": "2.0", "id": wire_id(&init), "result": {"protocolVersion": 1}})
            .to_string(),
    })
    .unwrap();
    link.attach("window-2", second.clone());
    second.take();

    assert_eq!(link.outbound("window-2", &request(7, "initialize")), None);
    assert_eq!(
        second.lines(),
        vec![json!({"jsonrpc": "2.0", "id": 7, "result": {"protocolVersion": 1}})]
    );
}

#[test]
fn notifications_reach_every_window_and_a_new_connection_starts_over() {
    let link = Link::<Recorder>::default();
    let (main, second) = (Recorder::default(), Recorder::default());
    link.attach("main", main.clone());
    link.attach("window-2", second.clone());
    link.send(open()).unwrap();
    let init = link.outbound("main", &request(1, "initialize")).unwrap();
    link.send(EngineEvent::Line {
        line: json!({"jsonrpc": "2.0", "id": wire_id(&init), "result": {}}).to_string(),
    })
    .unwrap();
    main.take();
    second.take();

    let update =
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s"}});
    link.send(EngineEvent::Line {
        line: update.to_string(),
    })
    .unwrap();
    assert_eq!(main.lines(), vec![update.clone()]);
    assert_eq!(second.lines(), vec![update]);

    link.send(EngineEvent::Closed {
        reason: "gone".to_owned(),
    })
    .unwrap();
    link.send(open()).unwrap();
    assert!(link
        .outbound("window-2", &request(1, "initialize"))
        .is_some());
}

#[test]
fn the_last_window_to_leave_ends_the_link() {
    let link = Link::<Recorder>::default();
    link.attach("main", Recorder::default());
    link.attach("window-2", Recorder::default());
    assert!(link.has("window-2"));
    assert!(!link.detach("window-2"));
    assert!(!link.detach("window-2"));
    assert!(link.detach("main"));
    assert!(link.is_empty());
    assert_eq!(link.send(open()), Err(NoWindow));
}

#[test]
fn the_same_window_attaching_again_replaces_its_channel() {
    let link = Link::<Recorder>::default();
    let (old, new) = (Recorder::default(), Recorder::default());
    link.attach("main", old.clone());
    assert!(link.attach("main", new.clone()));
    link.send(open()).unwrap();
    assert!(old.take().is_empty());
    assert_eq!(new.take().len(), 1);
}

#[test]
fn a_window_that_attaches_after_the_connection_ended_waits_for_the_next_open() {
    let link = Link::<Recorder>::default();
    link.attach("main", Recorder::default());
    link.send(open()).unwrap();
    link.end();
    let late = Recorder::default();
    link.attach("window-2", late.clone());
    assert!(late.take().is_empty());
}
