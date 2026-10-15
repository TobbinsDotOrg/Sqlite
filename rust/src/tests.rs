use super::*;
fn empty() -> Tree {
    Tree::Array(vec![])
}
fn mem() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    db
}
fn scalar(db: &Connection, sql: &str) -> Tree {
    run(db, sql, &empty(), 3).unwrap()
}
#[test]
fn crud_named_positional_and_injection() {
    let db = mem();
    run(
        &db,
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT UNIQUE)",
        &empty(),
        0,
    )
    .unwrap();
    assert_eq!(
        run(
            &db,
            "INSERT INTO t(v) VALUES(@v)",
            &Tree::Map(vec![(
                "v".into(),
                Tree::Text("O'Brien; DROP TABLE t; --".into())
            )]),
            0
        )
        .unwrap(),
        Tree::Number(1)
    );
    assert_eq!(db.last_insert_rowid(), 1);
    assert_eq!(db.changes(), 1);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM t"), Tree::Number(1));
    run(
        &db,
        "UPDATE t SET v=? WHERE id=?",
        &Tree::Array(vec![Tree::Text("世界 héllo \0 😀".into()), Tree::Number(1)]),
        0,
    )
    .unwrap();
    assert_eq!(
        scalar(&db, "SELECT v FROM t"),
        Tree::Text("世界 héllo \0 😀".into())
    );
    run(&db, "DELETE FROM t", &empty(), 0).unwrap();
    assert_eq!(
        run(&db, "SELECT * FROM t", &empty(), 1).unwrap(),
        Tree::Array(vec![])
    );
}
#[test]
fn null_integer_real_blob_roundtrips() {
    let db = mem();
    let values = [
        Tree::Null,
        Tree::Number(u32::MAX),
        Tree::Text("é世界\0".into()),
        Tree::Bytes(vec![0, 255, 128, 1]),
    ];
    for value in values {
        assert_eq!(
            run(&db, "SELECT ?", &Tree::Array(vec![value.clone()]), 3).unwrap(),
            value
        );
    }
    for (tag, s) in [
        ("$integer", "-9223372036854775808"),
        ("$integer", "9223372036854775807"),
        ("$real", "-12.75"),
    ] {
        assert_eq!(
            run(
                &db,
                "SELECT ?",
                &Tree::Array(vec![Tree::Map(vec![(tag.into(), Tree::Text(s.into()))])]),
                3
            )
            .unwrap(),
            Tree::Text(s.into())
        );
    }
    assert_eq!(
        run(&db, "SELECT 1 WHERE 0", &empty(), 2).unwrap(),
        Tree::Null
    );
    assert_eq!(scalar(&db, "SELECT NULL"), Tree::Null);
    assert!(sql_value(&Tree::Map(vec![("$real".into(), Tree::Text("NaN".into()))])).is_err());
}
#[test]
fn strict_bindings_constraints_and_columns() {
    let db = mem();
    db.execute_batch("CREATE TABLE t(v UNIQUE);INSERT INTO t VALUES(1)")
        .unwrap();
    assert!(
        run(&db, "INSERT INTO t VALUES(1)", &empty(), 0)
            .unwrap_err()
            .contains("SQLite error 2067")
    );
    for params in [
        empty(),
        Tree::Array(vec![Tree::Number(1), Tree::Number(2)]),
        Tree::Map(vec![("wrong".into(), Tree::Number(1))]),
    ] {
        assert!(run(&db, "SELECT @v", &params, 3).is_err());
    }
    assert!(run(&db, "SELECT 1 AS x,2 AS x", &empty(), 1).is_err());
    assert!(run(&db, "SELECT 1;SELECT 2", &empty(), 1).is_err());
}
#[test]
fn prepared_cache_reuse_resets_parameters_after_error() {
    let db = mem();
    db.execute_batch("CREATE TABLE t(v UNIQUE)").unwrap();
    for n in 0..5 {
        run(
            &db,
            "INSERT INTO t VALUES(?)",
            &Tree::Array(vec![Tree::Number(n)]),
            0,
        )
        .unwrap();
    }
    assert!(
        run(
            &db,
            "INSERT INTO t VALUES(?)",
            &Tree::Array(vec![Tree::Number(1)]),
            0
        )
        .is_err()
    );
    run(
        &db,
        "INSERT INTO t VALUES(?)",
        &Tree::Array(vec![Tree::Number(99)]),
        0,
    )
    .unwrap();
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM t"), Tree::Number(6));
}
#[test]
fn transactions_commit_rollback_cleanup_and_stale_owners() {
    let shared = Arc::new(Mutex::new(State {
        db: Some(mem()),
        transaction: 1,
        next: 1,
    }));
    {
        let state = shared.lock().unwrap();
        state
            .db
            .as_ref()
            .unwrap()
            .execute_batch("CREATE TABLE t(v);BEGIN;INSERT INTO t VALUES(1)")
            .unwrap();
    }
    let mut tx = Transaction {
        db: shared.clone(),
        id: 1,
        finished: false,
    };
    tx.finish(true).unwrap();
    tx.finish(true).unwrap();
    {
        let mut state = shared.lock().unwrap();
        state.transaction = 2;
        state
            .db
            .as_ref()
            .unwrap()
            .execute_batch("BEGIN;INSERT INTO t VALUES(2)")
            .unwrap();
    }
    {
        let _tx = Transaction {
            db: shared.clone(),
            id: 2,
            finished: false,
        };
    }
    let state = shared.lock().unwrap();
    assert_eq!(
        scalar(state.db.as_ref().unwrap(), "SELECT COUNT(*) FROM t"),
        Tree::Number(1)
    );
}
#[test]
fn persisted_reopen_multiple_connections() {
    let path = std::env::temp_dir().join(format!(
        "tobbins-sqlite-{}-{:?}.db",
        std::process::id(),
        std::thread::current().id()
    ));
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE t(v);INSERT INTO t VALUES('persistent')")
            .unwrap();
        let other = Connection::open(&path).unwrap();
        assert_eq!(
            scalar(&other, "SELECT v FROM t"),
            Tree::Text("persistent".into())
        );
    }
    {
        let db = Connection::open(&path).unwrap();
        assert_eq!(scalar(&db, "SELECT COUNT(*) FROM t"), Tree::Number(1));
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn value_wire_format_limits_and_validation() {
    let v = Tree::Map(vec![
        ("null".into(), Tree::Null),
        ("blob".into(), Tree::Bytes(vec![0, 255])),
        ("array".into(), Tree::Array(vec![Tree::Number(7)])),
    ]);
    let bytes = v.encode().unwrap();
    assert_eq!(Tree::decode(&bytes).unwrap(), v);
    for n in 0..bytes.len() {
        assert!(Tree::decode(&bytes[..n]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Tree::decode(&trailing).is_err());
    assert!(Tree::decode(&[255]).is_err());
}
#[test]
fn ffi_errors_ownership_and_explicit_resource_cleanup() {
    let path = ":memory:";
    let mut input = Value::scalar(UTF8, 0);
    input.data = path.as_ptr();
    input.len = path.len();
    let mut db = dispatch(0, &[input]).unwrap();
    assert_eq!(db.reserved, 1);
    let mut out = Value::scalar(VOID, 0);
    assert_eq!(invoke(0, std::ptr::null(), 1, &mut out), 1);
    assert_eq!(out.tag, UTF8);
    unsafe {
        out.release.unwrap()(out.owner);
    }
    dispatch(1, &[db]).unwrap();
    dispatch(1, &[db]).unwrap();
    assert!(dispatch(3, &[db]).err().unwrap().contains("closed"));
    unsafe {
        db.release.take().unwrap()(db.owner);
    }
}

#[test]
fn runaway_queries_are_interrupted_and_connection_remains_usable() {
    let db = mem();
    let error=run(&db,"WITH RECURSIVE infinite(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM infinite) SELECT sum(n) FROM infinite",&empty(),3).unwrap_err();
    assert!(error.contains("interrupted"), "{error}");
    assert_eq!(scalar(&db, "SELECT 42"), Tree::Number(42));
}

#[test]
fn sqlite_automatic_rollback_does_not_leave_a_stale_transaction() {
    let mut state = State {
        db: Some(mem()),
        transaction: 1,
        next: 1,
    };
    state
        .db
        .as_ref()
        .unwrap()
        .execute_batch(
            "CREATE TABLE t(v UNIQUE ON CONFLICT ROLLBACK);INSERT INTO t VALUES(1);BEGIN",
        )
        .unwrap();
    assert!(run_state(&mut state, "INSERT INTO t VALUES(1)", &empty(), 0).is_err());
    assert_eq!(state.transaction, 0);
    let shared = Arc::new(Mutex::new(state));
    let mut old = Transaction {
        db: shared.clone(),
        id: 1,
        finished: false,
    };
    old.finish(false).unwrap();
    shared
        .lock()
        .unwrap()
        .db
        .as_ref()
        .unwrap()
        .execute_batch("BEGIN;ROLLBACK")
        .unwrap();
}
