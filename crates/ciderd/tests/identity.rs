use ciderd::identity::{enroll, Session};

#[test]
fn enrollment_generation_and_lock_survive_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let identity = dir.path().join("node.json");
    let state = dir.path().join("state");
    assert!(Session::open(&identity, &state).is_err());
    let node = enroll(&identity, None).unwrap();
    assert!(enroll(&identity, None).is_err());
    let first = Session::open(&identity, &state).unwrap();
    assert_eq!(first.node_id, node);
    assert_eq!(first.generation, 1);
    assert!(Session::open(&identity, &state).is_err());
    let session = first.session_id.clone();
    drop(first);
    let second = Session::open(&identity, &state).unwrap();
    assert_eq!(second.generation, 2);
    assert_ne!(second.session_id, session);
    drop(second);
    std::fs::write(&identity, b"{}").unwrap();
    assert!(Session::open(&identity, &state).is_err());
}

#[test]
fn identity_filename_ending_in_lock_keeps_a_distinct_lock_inode() {
    let directory = tempfile::tempdir().unwrap();
    let identity = directory.path().join("node.lock");
    let state = directory.path().join("state");
    enroll(&identity, None).unwrap();
    let first = Session::open(&identity, &state).unwrap();
    assert!(Session::open(&identity, &state).is_err());
    drop(first);
    assert_eq!(Session::open(&identity, &state).unwrap().generation, 2);
}
