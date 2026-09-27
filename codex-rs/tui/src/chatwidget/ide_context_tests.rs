//! Conversation-binding coverage for TUI IDE context reads.
use super::*;
use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn private_reads_use_prompt_and_status_thread_identity() {
    use std::os::unix::net::UnixListener;

    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("private.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind private socket");
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_ide_context_server(listener, requests.clone());
    let (mut chat, _rx, _op_rx) =
        crate::chatwidget::tests::helpers::make_chatwidget_manual(/*model_override*/ None).await;
    chat.ide_context.enabled = true;
    chat.ide_context.endpoint = crate::ide_context::IdeContextEndpoint::Explicit(socket_path);
    let first_thread_id = ThreadId::new();
    chat.thread_id = Some(first_thread_id);

    let mut prompt_items = vec![UserInput::Text {
        text: "first".to_string(),
        text_elements: Vec::new(),
    }];
    chat.maybe_apply_ide_context(&mut prompt_items);
    let second_thread_id = ThreadId::new();
    chat.thread_id = Some(second_thread_id);
    chat.handle_ide_command_args("status");
    chat.thread_id = None;
    let mut cleared_items = vec![UserInput::Text {
        text: "cleared".to_string(),
        text_elements: Vec::new(),
    }];
    chat.maybe_apply_ide_context(&mut cleared_items);

    server
        .join()
        .expect("IDE context server joins")
        .expect("IDE context exchange");
    let requests = requests
        .lock()
        .expect("IDE context request capture lock poisoned")
        .clone();
    assert_eq!(requests.len(), 3);
    let workspace_root = chat.config.cwd.to_string_lossy().to_string();
    assert_eq!(
        requests[0].get("params").expect("prompt request params"),
        &serde_json::json!({
            "workspaceRoot": workspace_root,
            "threadId": first_thread_id.to_string(),
        })
    );
    assert_eq!(
        requests[1].get("params").expect("status request params"),
        &serde_json::json!({
            "workspaceRoot": workspace_root,
            "threadId": second_thread_id.to_string(),
        })
    );
    assert_eq!(
        requests[2].get("params").expect("cleared request params"),
        &serde_json::json!({ "workspaceRoot": workspace_root })
    );
}

fn spawn_ide_context_server(
    listener: std::os::unix::net::UnixListener,
    requests: Arc<Mutex<Vec<Value>>>,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for _ in 0..3 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
            let mut length = [0_u8; 4];
            std::io::Read::read_exact(&mut stream, &mut length)?;
            let length = u32::from_le_bytes(length) as usize;
            if length > 65536 {
                return Err(std::io::Error::other("oversized test request"));
            }
            let mut payload = vec![0_u8; length];
            std::io::Read::read_exact(&mut stream, &mut payload)?;
            let request =
                serde_json::from_slice::<Value>(&payload).map_err(std::io::Error::other)?;
            let request_id = request
                .get("requestId")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("missing request id"))?
                .to_string();
            requests
                .lock()
                .map_err(|_| std::io::Error::other("request capture lock poisoned"))?
                .push(request);
            let response = serde_json::json!({
                "type": "response",
                "requestId": request_id,
                "resultType": "success",
                "method": "ide-context",
                "handledByClientId": "lapis-bridge",
                "result": {
                    "type": "broadcast",
                    "ideContext": {
                        "activeFile": {
                            "label": "lib.rs",
                            "path": "src/lib.rs",
                            "selection": {
                                "start": {"line": 0, "character": 0},
                                "end": {"line": 0, "character": 3},
                            },
                            "activeSelectionContent": "identity",
                            "selections": [],
                        },
                        "openTabs": [],
                    }
                }
            });
            let response_payload = serde_json::to_vec(&response).map_err(std::io::Error::other)?;
            let response_length =
                u32::try_from(response_payload.len()).map_err(std::io::Error::other)?;
            std::io::Write::write_all(&mut stream, &response_length.to_le_bytes())?;
            std::io::Write::write_all(&mut stream, &response_payload)?;
        }
        Ok(())
    })
}
