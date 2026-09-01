//! The debug agent proves the [`Agent`] port: a turn streams canonical events,
//! and context-insert records by source, with no model.

use clatch_core::{Agent, AppId, DebugAgent, Event, Input, Outcome, Source};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn run_streams_canonical_events() {
    let agent = DebugAgent::new();
    let stream = agent
        .run(Input::new(Source::Human, "hello"), CancellationToken::new())
        .await
        .expect("run");
    let events: Vec<Event> = stream.collect().await;

    assert!(matches!(&events[0], Event::Text(t) if t.contains("hello")));
    assert!(matches!(events.last(), Some(Event::Done(Outcome::Ok))));
}

#[tokio::test]
async fn insert_records_context_by_source() {
    let agent = DebugAgent::new();
    agent
        .insert(Input::new(Source::Clatch, "you have testapp"))
        .await
        .expect("insert");
    agent
        .insert(Input::new(
            Source::App(AppId::new("com.arfium.arfchess")),
            "move e4",
        ))
        .await
        .expect("insert");

    assert_eq!(agent.context().len(), 2);
}
