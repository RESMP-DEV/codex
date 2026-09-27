//! Explicit routes must never leak a prompt-time read to the shared editor socket.
use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

fn shared_listener(home: &Path) -> std::io::Result<UnixListener> {
    let parent = home.join("ipc");
    fs::create_dir(&parent)?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    UnixListener::bind(parent.join("ipc.sock"))
}

#[test]
fn explicit_route_reads_only_its_endpoint() {
    let root = tempfile::tempdir().expect("fixture directory");
    let home = root.path().canonicalize().expect("canonical directory");
    let shared = shared_listener(&home).expect("bind shared socket");
    let path = home.join("private.sock");
    let private = UnixListener::bind(&path).expect("bind private socket");
    let server = super::tests::spawn_ide_context_server(private, "private-selection");
    let context = fetch_ide_context(
        Path::new("/repo"),
        &home,
        &IdeContextEndpoint::Explicit(path),
        /*thread_id*/ None,
    )
    .expect("private route");
    server.join().expect("server exit");
    assert_eq!(
        context
            .active_file
            .expect("active file")
            .active_selection_content,
        "private-selection"
    );
    super::tests::assert_listener_unused(&shared);
}

#[test]
fn unavailable_or_invalid_explicit_route_never_uses_shared_listener() {
    let root = tempfile::tempdir().expect("fixture directory");
    let home = root.path().canonicalize().expect("canonical directory");
    let shared = shared_listener(&home).expect("bind shared socket");
    let file = home.join("not-a-socket");
    fs::write(&file, "not a socket").expect("fixture file");
    for path in [
        home.join("missing.sock"),
        file,
        PathBuf::new(),
        PathBuf::from("relative.sock"),
    ] {
        assert!(
            fetch_ide_context(
                Path::new("/repo"),
                &home,
                &IdeContextEndpoint::Explicit(path),
                /*thread_id*/ None,
            )
            .is_err()
        );
        super::tests::assert_listener_unused(&shared);
    }
}
