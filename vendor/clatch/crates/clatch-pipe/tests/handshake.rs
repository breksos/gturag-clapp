//! Faz C contract: an app registers over the control pipe, fires a declared
//! signal, answers a health ping, and shuts down, all round-trip. The launcher
//! (`Pipe`) and the reference app (`Client`) talk over a real unix socket,
//! in-process, no spawned binary and no inference.

use clatch_core::{AppId, SignalDecl, SignalType};
use clatch_ipc::Listener;
use clatch_pipe::{Client, Identity, Inbound, Pipe};
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn register_signal_ping_shutdown_round_trip() {
    let run_dir = clatch_testkit::tmp_sock();
    let identity = Identity::mint(AppId::new("com.arfium.testapp"), &run_dir);
    let declared = vec![SignalDecl {
        id: "move".to_string(),
        signal_type: SignalType::Run,
    }];

    let listener = Listener::bind(&identity.addr).expect("bind");

    // The app side: connect, fire one move, then keep the pipe healthy.
    let app_identity = identity.clone();
    let app_declared = declared.clone();
    let app = tokio::spawn(async move {
        let mut client = Client::connect(app_identity, &app_declared)
            .await
            .expect("connect");
        client
            .signal("move", json!({ "from": "e2", "to": "e4" }))
            .await
            .expect("signal");
        client.serve().await.expect("serve");
    });

    // The launcher side: accept + handshake, then drive the connection.
    let mut pipe = Pipe::accept(&listener, &identity, &declared)
        .await
        .expect("accept");
    assert_eq!(pipe.app_id().as_str(), "com.arfium.testapp");

    match pipe.recv().await {
        Some(Inbound::ToAgent(s)) => {
            assert_eq!(s.id, "move");
            assert_eq!(s.signal_type, SignalType::Run);
            assert_eq!(s.payload["to"], "e4");
        }
        other => panic!("expected a move signal, got {:?}", other.is_some()),
    }

    pipe.ping(Duration::from_secs(2)).await.expect("ping");
    pipe.shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    app.await.expect("app task");
}

#[tokio::test]
async fn a_refusal_reaches_the_app() {
    // The launcher's push_refused crosses the wire and the app observes it
    // (reference/protocol.md § Signals). The daemon's DECISION to refuse whole is
    // tested in the router; this proves the notice is delivered end to end, so
    // `app.toAgentRefused` is a real message, not a phantom.
    let run_dir = clatch_testkit::tmp_sock();
    let identity = Identity::mint(AppId::new("com.arfium.testapp"), &run_dir);
    let declared = vec![SignalDecl {
        id: "move".to_string(),
        signal_type: SignalType::Run,
    }];
    let listener = Listener::bind(&identity.addr).expect("bind");

    let app_identity = identity.clone();
    let app_declared = declared.clone();
    let (handle_tx, handle_rx) = oneshot::channel();
    let app = tokio::spawn(async move {
        let mut client = Client::connect(app_identity, &app_declared)
            .await
            .expect("connect");
        // Hand the test a shared view of what we receive, then serve until shutdown.
        let _ = handle_tx.send(client.refusals_handle());
        client.serve().await.expect("serve");
    });

    let pipe = Pipe::accept(&listener, &identity, &declared)
        .await
        .expect("accept");
    let refusals = handle_rx.await.expect("app handed its refusals handle");

    pipe.push_refused("move".into(), "research".into(), "inbox_full".into());

    // Fire-and-forget: poll the shared handle until the refusal lands.
    let mut seen = Vec::new();
    for _ in 0..100 {
        seen = refusals.lock().unwrap().clone();
        if !seen.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.len(), 1, "exactly one refusal reached the app");
    assert_eq!(seen[0].id, "move");
    assert_eq!(seen[0].agent, "research");
    assert_eq!(seen[0].reason, "inbox_full");

    pipe.shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");
    app.await.expect("app task");
}

#[tokio::test]
async fn register_rejects_a_forged_token() {
    let run_dir = clatch_testkit::tmp_sock();
    let identity = Identity::mint(AppId::new("com.arfium.testapp"), &run_dir);
    let listener = Listener::bind(&identity.addr).expect("bind");

    // An app that connected with the right address but the wrong secret.
    let mut forged = identity.clone();
    forged.token = "deadbeef".repeat(8);
    let app = tokio::spawn(async move {
        // The forged client's register is rejected, so connect returns an error.
        Client::connect(forged, &[]).await.is_err()
    });

    let err = Pipe::accept(&listener, &identity, &[]).await;
    assert!(err.is_err(), "a forged token must not register");
    assert!(
        app.await.expect("app task"),
        "the app sees its register rejected"
    );
}
