//! Independent SQLite Pebble. All exported functions have ABI 1.1 C layouts.
#[path = "../../abi/abi.rs"]
pub mod abi;
#[path = "../../abi/value.rs"]
pub mod value;
use abi::*;
use rusqlite::{
    Connection, Statement,
    types::{Value as SqlValue, ValueRef},
};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};
use value::Tree;
type Result<T> = std::result::Result<T, String>;
struct State {
    db: Option<Connection>,
    transaction: u64,
    next: u64,
}
type Shared = Arc<Mutex<State>>;
struct Db(Shared);
struct Prepared {
    db: Shared,
    sql: Option<String>,
}
struct Transaction {
    db: Shared,
    id: u64,
    finished: bool,
}
impl Drop for Transaction {
    fn drop(&mut self) {
        let _ = self.finish(false);
    }
}
impl Transaction {
    fn finish(&mut self, commit: bool) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        let mut state = self
            .db
            .lock()
            .map_err(|_| "SQLite connection lock poisoned")?;
        if state.transaction != self.id {
            self.finished = true;
            return if commit {
                Err("SQLite transaction is no longer active".into())
            } else {
                Ok(())
            };
        }
        let db = state.db.as_ref().ok_or("SQLite connection is closed")?;
        db.execute_batch(if commit { "COMMIT" } else { "ROLLBACK" })
            .map_err(sql_error)?;
        state.transaction = 0;
        self.finished = true;
        Ok(())
    }
}
fn sql_error(e: rusqlite::Error) -> String {
    match &e {
        rusqlite::Error::SqliteFailure(code, _) => {
            format!("SQLite error {}: {e}", code.extended_code)
        }
        _ => format!("SQLite: {e}"),
    }
}
fn number(n: i64) -> Tree {
    u32::try_from(n)
        .map(Tree::Number)
        .unwrap_or_else(|_| Tree::Text(n.to_string()))
}
fn sql_value(v: &Tree) -> Result<SqlValue> {
    Ok(match v {
        Tree::Null => SqlValue::Null,
        Tree::Number(n) => SqlValue::Integer(*n as i64),
        Tree::Text(s) => SqlValue::Text(s.clone()),
        Tree::Bytes(b) => SqlValue::Blob(b.clone()),
        Tree::Map(m) if m.len() == 1 => {
            let (tag, value) = &m[0];
            let Tree::Text(text) = value else {
                return Err("SQLite numeric parameter requires decimal text".into());
            };
            match tag.as_str() {
                "$integer" => SqlValue::Integer(
                    text.parse()
                        .map_err(|_| "SQLite integer must be a signed 64-bit decimal")?,
                ),
                "$real" => {
                    let real: f64 = text
                        .parse()
                        .map_err(|_| "SQLite real must be a finite decimal")?;
                    if !real.is_finite() {
                        return Err("SQLite real must be finite".into());
                    }
                    SqlValue::Real(real)
                }
                _ => {
                    return Err(
                        "SQLite parameter must be NULL, int, text, Buffer, Integer() or Real()"
                            .into(),
                    );
                }
            }
        }
        _ => {
            return Err(
                "SQLite parameter must be NULL, int, text, Buffer, Integer() or Real()".into(),
            );
        }
    })
}
fn read_value(v: ValueRef<'_>) -> Result<Tree> {
    Ok(match v {
        ValueRef::Null => Tree::Null,
        ValueRef::Integer(n) => number(n),
        ValueRef::Real(n) => Tree::Text(n.to_string()),
        ValueRef::Text(bytes) => Tree::Text(
            std::str::from_utf8(bytes)
                .map_err(|_| "SQLite TEXT contains invalid UTF-8; CAST AS BLOB to read its bytes")?
                .to_owned(),
        ),
        ValueRef::Blob(bytes) => Tree::Bytes(bytes.to_vec()),
    })
}
fn bind(stmt: &mut Statement<'_>, params: &Tree) -> Result<()> {
    stmt.clear_bindings();
    let count = stmt.parameter_count();
    match params {
        Tree::Array(values) => {
            if values.len() != count {
                return Err(format!(
                    "SQLite expected {count} positional parameters, received {}",
                    values.len()
                ));
            }
            for (i, v) in values.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, sql_value(v)?)
                    .map_err(sql_error)?;
            }
        }
        Tree::Map(values) => {
            let mut bound = std::collections::BTreeSet::new();
            for (key, v) in values {
                let mut matches = Vec::new();
                for i in 1..=count {
                    if let Some(name) = stmt.parameter_name(i) {
                        if name == key
                            || (matches!(name.as_bytes().first(), Some(b'@' | b':' | b'$'))
                                && &name[1..] == key)
                        {
                            matches.push(i);
                        }
                    }
                }
                if matches.len() != 1 {
                    return Err(format!(
                        "SQLite named parameter '{key}' is unknown or ambiguous; include its prefix"
                    ));
                }
                let i = matches[0];
                if !bound.insert(i) {
                    return Err(format!("SQLite parameter '{key}' was supplied twice"));
                }
                stmt.raw_bind_parameter(i, sql_value(v)?)
                    .map_err(sql_error)?;
            }
            if bound.len() != count {
                return Err(format!(
                    "SQLite expected {count} parameters, received {}",
                    bound.len()
                ));
            }
        }
        _ => return Err("SQLite parameters require an indexed array or a named map".into()),
    }
    Ok(())
}
fn run_state(state: &mut State, sql: &str, params: &Tree, mode: u32) -> Result<Tree> {
    let db = state.db.as_ref().ok_or("SQLite connection is closed")?;
    let result = run(db, sql, params, mode);
    if db.is_autocommit() {
        state.transaction = 0;
    }
    result
}
fn run(db: &Connection, sql: &str, params: &Tree, mode: u32) -> Result<Tree> {
    let mut ticks = 0u32;
    db.progress_handler(
        1000,
        Some(move || {
            ticks += 1;
            ticks > 10_000
        }),
    )
    .map_err(sql_error)?;
    let mut stmt = db.prepare_cached(sql).map_err(sql_error)?;
    // prepare_cached rejects trailing statements via rusqlite's extra_check.
    bind(&mut stmt, params)?;
    if mode == 0 {
        return stmt
            .raw_execute()
            .map(|n| {
                u32::try_from(n)
                    .map(Tree::Number)
                    .map_err(|_| "SQLite affected rows exceed Tobbins int".into())
            })
            .map_err(sql_error)?;
    }
    if mode > 3 {
        return Err("SQLite invalid operation".into());
    }
    let names = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err("SQLite query requires result columns".into());
    }
    if mode != 3
        && names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != names.len()
    {
        return Err("SQLite duplicate result column names; use AS aliases".into());
    }
    let mut rows = stmt.raw_query();
    let mut result = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().map_err(sql_error)? {
        if mode == 3 {
            return read_value(row.get_ref(0).map_err(sql_error)?);
        }
        let mut fields = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let value = read_value(row.get_ref(i).map_err(sql_error)?)?;
            bytes = bytes
                .checked_add(
                    name.len()
                        + match &value {
                            Tree::Text(s) => s.len(),
                            Tree::Bytes(b) => b.len(),
                            _ => 8,
                        },
                )
                .ok_or("SQLite result too large")?;
            if bytes > 32 * 1024 * 1024 {
                return Err("SQLite query result exceeds 32 MiB; limit the query".into());
            }
            fields.push((name.clone(), value));
        }
        let row = Tree::Map(fields);
        if mode == 2 {
            return Ok(row);
        }
        result.push(row);
        if result.len() > 100_000 {
            return Err("SQLite query exceeds 100000 rows; limit the query".into());
        }
    }
    Ok(if mode == 1 {
        Tree::Array(result)
    } else {
        Tree::Null
    })
}
unsafe extern "C" fn release_bytes(owner: *mut c_void) {
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner.cast::<Vec<u8>>()) });
    }
}
fn bytes(tag: u32, data: Vec<u8>) -> Value {
    let owner = Box::new(data);
    let mut value = Value::scalar(tag, 0);
    value.data = owner.as_ptr();
    value.len = owner.len();
    value.owner = Box::into_raw(owner).cast();
    value.release = Some(release_bytes);
    value
}
unsafe extern "C" fn release_db(p: *mut c_void) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p.cast::<Db>()) });
    }
}
unsafe extern "C" fn release_stmt(p: *mut c_void) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p.cast::<Prepared>()) });
    }
}
unsafe extern "C" fn release_tx(p: *mut c_void) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p.cast::<Transaction>()) });
    }
}
fn object<T>(value: T, id: u32, release: Release) -> Value {
    let mut out = Value::scalar(OBJECT, 0);
    out.reserved = id;
    out.len = std::mem::size_of::<T>();
    out.owner = Box::into_raw(Box::new(value)).cast();
    out.release = Some(release);
    out
}
unsafe fn text(v: &Value) -> Result<&str> {
    std::str::from_utf8(unsafe { span(v)? }).map_err(|e| e.to_string())
}
unsafe fn span(v: &Value) -> Result<&[u8]> {
    if v.len > value::LIMIT || (v.len > 0 && v.data.is_null()) {
        return Err("SQLite invalid byte span".into());
    }
    Ok(if v.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(v.data, v.len) }
    })
}
unsafe fn tree(v: &Value) -> Result<Tree> {
    Tree::decode(unsafe { span(v)? })
}
unsafe fn resource<T>(v: &Value, id: u32) -> Result<&mut T> {
    if v.tag != OBJECT || v.reserved != id || v.owner.is_null() {
        return Err("SQLite invalid native resource".into());
    }
    Ok(unsafe { &mut *v.owner.cast::<T>() })
}
fn tree_output(t: Tree) -> Result<Value> {
    Ok(bytes(TREE, t.encode()?))
}
// Host serializes all calls/releases; each shared connection also has a Mutex,
// allowing safe movement between scheduler threads. SQLite never calls back into Tobbins.
fn dispatch(op: usize, args: &[Value]) -> Result<Value> {
    unsafe {
        match op {
            0 => {
                let db = Connection::open(text(&args[0])?).map_err(sql_error)?;
                db.busy_timeout(std::time::Duration::ZERO)
                    .map_err(sql_error)?;
                db.set_limit(
                    rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
                    32 * 1024 * 1024,
                )
                .map_err(sql_error)?;
                db.set_limit(
                    rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH,
                    1024 * 1024,
                )
                .map_err(sql_error)?;
                Ok(object(
                    Db(Arc::new(Mutex::new(State {
                        db: Some(db),
                        transaction: 0,
                        next: 0,
                    }))),
                    1,
                    release_db,
                ))
            }
            1 => {
                let mut state = resource::<Db>(&args[0], 1)?
                    .0
                    .lock()
                    .map_err(|_| "SQLite lock poisoned")?;
                if let Some(db) = state.db.take() {
                    if let Err((db, e)) = db.close() {
                        state.db = Some(db);
                        return Err(sql_error(e));
                    }
                }
                state.transaction = 0;
                Ok(Value::scalar(U32, 0))
            }
            2 | 3 => {
                let db = &resource::<Db>(&args[0], 1)?.0;
                let mut state = db.lock().map_err(|_| "SQLite lock poisoned")?;
                if op == 2 {
                    tree_output(run_state(
                        &mut state,
                        text(&args[1])?,
                        &tree(&args[2])?,
                        args[3].scalar as u32,
                    )?)
                } else {
                    tree_output(number(
                        state
                            .db
                            .as_ref()
                            .ok_or("SQLite connection is closed")?
                            .last_insert_rowid(),
                    ))
                }
            }
            4 => {
                let shared = resource::<Db>(&args[0], 1)?.0.clone();
                let sql = text(&args[1])?.to_string();
                {
                    let state = shared.lock().map_err(|_| "SQLite lock poisoned")?;
                    let db = state.db.as_ref().ok_or("SQLite connection is closed")?;
                    db.prepare_cached(&sql).map_err(sql_error)?;
                }
                Ok(object(
                    Prepared {
                        db: shared,
                        sql: Some(sql),
                    },
                    2,
                    release_stmt,
                ))
            }
            5 => {
                let stmt = resource::<Prepared>(&args[0], 2)?;
                let sql = stmt.sql.as_ref().ok_or("SQLite statement is closed")?;
                let mut state = stmt.db.lock().map_err(|_| "SQLite lock poisoned")?;
                tree_output(run_state(
                    &mut state,
                    sql,
                    &tree(&args[1])?,
                    args[2].scalar as u32,
                )?)
            }
            6 => {
                let stmt = resource::<Prepared>(&args[0], 2)?;
                stmt.sql = None;
                let state = stmt.db.lock().map_err(|_| "SQLite lock poisoned")?;
                if let Some(db) = &state.db {
                    db.flush_prepared_statement_cache();
                }
                Ok(Value::scalar(U32, 0))
            }
            7 => {
                let shared = resource::<Db>(&args[0], 1)?.0.clone();
                let id;
                {
                    let mut state = shared.lock().map_err(|_| "SQLite lock poisoned")?;
                    if state.transaction != 0 {
                        return Err("SQLite nested transactions are unsupported".into());
                    }
                    let next = state
                        .next
                        .checked_add(1)
                        .ok_or("SQLite transaction identity exhausted")?;
                    state
                        .db
                        .as_ref()
                        .ok_or("SQLite connection is closed")?
                        .execute_batch("BEGIN IMMEDIATE")
                        .map_err(sql_error)?;
                    state.next = next;
                    state.transaction = next;
                    id = next;
                }
                Ok(object(
                    Transaction {
                        db: shared,
                        id,
                        finished: false,
                    },
                    3,
                    release_tx,
                ))
            }
            8 => {
                resource::<Transaction>(&args[0], 3)?.finish(args[1].scalar != 0)?;
                Ok(Value::scalar(U32, 0))
            }
            9 => {
                let tx = resource::<Transaction>(&args[0], 3)?;
                let mut state = tx.db.lock().map_err(|_| "SQLite lock poisoned")?;
                if tx.finished || state.transaction != tx.id {
                    return Err("SQLite transaction is no longer active".into());
                }
                tree_output(run_state(
                    &mut state,
                    text(&args[1])?,
                    &tree(&args[2])?,
                    args[3].scalar as u32,
                )?)
            }
            10 => {
                let state = resource::<Db>(&args[0], 1)?
                    .0
                    .lock()
                    .map_err(|_| "SQLite lock poisoned")?;
                let n = state
                    .db
                    .as_ref()
                    .ok_or("SQLite connection is closed")?
                    .changes();
                Ok(Value::scalar(
                    U32,
                    u32::try_from(n).map_err(|_| "SQLite changes exceed Tobbins int")? as u64,
                ))
            }
            11 => Ok(Value::scalar(
                U32,
                matches!(tree(&args[0])?, Tree::Null) as u64,
            )),
            _ => Err("SQLite unknown operation".into()),
        }
    }
}
fn invoke(op: usize, args: *const Value, len: usize, out: *mut Value) -> u32 {
    if out.is_null() {
        return 1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let expected = SIGNATURES
            .get(op)
            .ok_or("SQLite invalid operation")?
            .1
            .len();
        if len != expected || (len != 0 && args.is_null()) {
            return Err("SQLite argument count mismatch".into());
        }
        dispatch(
            op,
            if len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(args, len) }
            },
        )
    }));
    let (status, value) = match result {
        Ok(Ok(v)) => (0, v),
        Ok(Err(e)) => (1, bytes(UTF8, e.into_bytes())),
        Err(_) => (
            1,
            bytes(
                UTF8,
                b"SQLite internal panic contained at native boundary".to_vec(),
            ),
        ),
    };
    unsafe {
        *out = value;
    }
    status
}
macro_rules! calls {($($f:ident=$i:expr),*)=>{$(unsafe extern "C" fn $f(a:*const Value,n:usize,o:*mut Value)->u32{invoke($i,a,n,o)})*};}
calls!(
    open = 0,
    close = 1,
    execute = 2,
    last_id = 3,
    prepare = 4,
    statement_run = 5,
    statement_close = 6,
    begin = 7,
    finish = 8,
    transaction_run = 9,
    changes = 10,
    is_null = 11
);
const DB: u32 = OBJECT | (1 << 16);
const STMT: u32 = OBJECT | (2 << 16);
const TX: u32 = OBJECT | (3 << 16);
const SIGNATURES: [(&str, &[u32], u32, Call); 12] = [
    ("open", &[UTF8], DB, open),
    ("close", &[DB], U32, close),
    ("run", &[DB, UTF8, TREE, U32], TREE, execute),
    ("last_id", &[DB], TREE, last_id),
    ("prepare", &[DB, UTF8], STMT, prepare),
    ("statement_run", &[STMT, TREE, U32], TREE, statement_run),
    ("statement_close", &[STMT], U32, statement_close),
    ("begin", &[DB], TX, begin),
    ("finish", &[TX, U32], U32, finish),
    (
        "transaction_run",
        &[TX, UTF8, TREE, U32],
        TREE,
        transaction_run,
    ),
    ("changes", &[DB], U32, changes),
    ("is_null", &[TREE], U32, is_null),
];
const fn exports() -> [Export; 12] {
    let mut table = [const {
        Export {
            name: std::ptr::null(),
            name_len: 0,
            parameters: std::ptr::null(),
            parameters_len: 0,
            result: 0,
            reserved: 0,
            call: open,
        }
    }; 12];
    let mut i = 0;
    while i < 12 {
        let (name, params, result, call) = SIGNATURES[i];
        table[i] = Export {
            name: name.as_ptr(),
            name_len: name.len(),
            parameters: params.as_ptr(),
            parameters_len: params.len(),
            result,
            reserved: 0,
            call,
        };
        i += 1;
    }
    table
}
static EXPORTS: [Export; 12] = exports();
static DESCRIPTOR: Descriptor = Descriptor {
    abi: ABI,
    size: std::mem::size_of::<Descriptor>() as u32,
    name: b"Tobbins.Database.Sqlite".as_ptr(),
    name_len: b"Tobbins.Database.Sqlite".len(),
    version: b"0.1.0".as_ptr(),
    version_len: 5,
    exports: EXPORTS.as_ptr(),
    exports_len: EXPORTS.len(),
};
#[unsafe(no_mangle)]
pub extern "C" fn tobbins_sqlite_v0_1_0_entry() -> *const Descriptor {
    &DESCRIPTOR
}

#[cfg(test)]
mod tests;
