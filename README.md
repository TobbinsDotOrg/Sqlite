# Tobbins SQLite Pebble

Official SQLite package: `Tobbins.Database.Sqlite`, version 0.1.0.
Requires Tobbins `>=0.2.0 <0.3.0` with native ABI 1.1 (value-tree support).
SQLite is bundled inside the native library; installation needs no SQLite,
Rust, Cargo, Visual Studio, or other development tools.

```text
tob.exe add Tobbins.Database.Sqlite
```

```csharp
using Tobbins.Database.Sqlite;
var db = new SqliteConnection("application.db");
try {
    db.Execute("CREATE TABLE IF NOT EXISTS users(id INTEGER PRIMARY KEY, name TEXT)");
    db.ExecuteWith("INSERT INTO users(name) VALUES(@name)", ["name":"David"]);
    var users = db.Query("SELECT * FROM users");
    var i = 0;
    while (i < users.length) {
        var user = (Map<string,mixed>)users[i];
        console.writeLine((string)user["name"]);
        i = i + 1;
    }
} finally {
    db.Close();
}
```

This uses current Tobbins syntax. Parameterized methods have a `With` suffix
because the language does not currently support optional method parameters or
overloading. Anonymous objects can be explicitly converted: `(array){name:"David"}`.

## API

`SqliteConnection(string path)` opens a file, creates it when absent, or opens
an isolated memory database for `":memory:"`. Errors are recoverable Tobbins
`Exception`s with SQLite's message and extended error code where available.

| Method | Result and behavior |
| --- | --- |
| `Execute(string sql)` / `ExecuteWith(string sql, mixed parameters)` | `int` affected rows; one non-query statement |
| `Query(string sql)` / `QueryWith(string sql, mixed parameters)` | `Array<mixed>`; each row is `Map<string,mixed>`; empty array when absent |
| `QueryOne(string sql)` / `QueryOneWith(string sql, mixed parameters)` | `mixed` row map or NULL; first row only |
| `ExecuteScalar(string sql)` / `ExecuteScalarWith(string sql, mixed parameters)` | `mixed` first column of first row, or NULL |
| `Prepare(string sql)` | Reusable `SqliteStatement` |
| `BeginTransaction()` | `SqliteTransaction`, using `BEGIN IMMEDIATE` |
| `LastInsertRowId()` | `mixed` integer using the conversion rules below |
| `Changes()` | `int` most recent affected row count |
| `Close()` / `dispose()` | Deterministically close; repeated calls are harmless |

`SqliteStatement` has the same eight execution/query methods with the SQL
argument omitted, plus `Close()` and `dispose()`. Preparation is validated at
creation and uses rusqlite's connection-local prepared statement cache. Each
execution resets bindings; a missing parameter never reuses an old binding.
Closing a statement releases its SQL and flushes cached native statements.

`SqliteTransaction` has the same eight execution/query methods as a connection,
plus `Commit()`, `Rollback()`, and `dispose()` (rollback). Commit and rollback
are idempotent after successful completion. Prefer `try`/`finally` with explicit
rollback/dispose. Native release also rolls back an abandoned active transaction,
but garbage collection is not a deterministic cleanup mechanism. Nested
transactions are rejected. A transaction belongs to its connection; operations
through that connection participate in the same transaction. Do not share that
connection between independent logical transactions.

```csharp
var insert = db.Prepare("INSERT INTO users(name) VALUES(?)");
try {
    insert.ExecuteWith(["Alice"]);
    insert.ExecuteWith(["Bob"]);
} finally { insert.Close(); }
var tx = db.BeginTransaction();
try {
    tx.ExecuteWith("UPDATE users SET name=? WHERE id=?", ["Updated",1]);
    tx.Commit();
} finally { tx.Rollback(); }
```

Named parameters accept maps with keys matching `@name`, `:name`, or `$name`;
the prefix may be omitted when unique. Positional `?`/`?1` parameters accept
indexed arrays in parameter-index order. Every parameter must be supplied
exactly once. Unknown, ambiguous, duplicate, or missing bindings raise an
Exception. All values use SQLite prepared binding, never string substitution.
Use contiguous parameter indices; sparse `?NNN` placeholders are unsupported.

| SQLite value | Tobbins result / parameter |
| --- | --- |
| NULL | `null`; test a mixed result with `IsNull(value)` |
| INTEGER in 0..4294967295 | `int` |
| Negative or larger INTEGER | Exact signed decimal `string`; bind with `Integer("-42")` |
| REAL | Round-trip decimal `string`; bind with `Real("1.25")` |
| TEXT | UTF-8 `string`, preserving embedded NUL and Unicode |
| BLOB | `Buffer`, preserving every byte |

Tobbins currently has u32 arithmetic. Decimal strings preserve i64/f64 values
without truncation or new numeric language features. Ordinary string parameters
always bind as TEXT. `Integer` and `Real` are input helpers, represented by normal
Tobbins maps with reserved `$integer`/`$real` keys. Results remain ordinary values.
Use SQL `typeof(column)` when the distinction between decimal text and a numeric
result matters. `IsNull` returns 1 for NULL and 0 otherwise. Missing map keys
raise an Exception, distinguishing a missing column from a present SQL NULL.
Invalid UTF-8 TEXT is rejected; use SQL `CAST(value AS BLOB)` to obtain its bytes.
Duplicate column names are rejected for row queries; use `AS` aliases.

Connections and cached statements are owned behind opaque handles and mutexes;
they can move between scheduler threads, but never execute concurrently on one
connection. Native calls are synchronous and serialized by Tobbins per library.
The busy timeout is zero: locks fail promptly with an Exception. Queries
materialize at most 100,000 rows / approximately 32 MiB; use SQL limits for large
datasets. Each query has a ten-million SQLite VM instruction budget; runaway
recursive queries raise an Exception. A value/row is limited to 32 MiB and SQL
text to 1 MiB. Closing a connection invalidates its statements and transactions.
Extension loading is disabled. SQL is trusted program input; parameter values
are untrusted data and must be bound.

## Build and test independently

Author tools: Rust 1.88+, Cargo, a C compiler, Python 3, and LLVM tools
(`llvm-nm`, `llvm-objcopy`, `llvm-ar`, `llvm-strip`). Native Windows builds use
the MSVC tools. Consumers use only the precompiled artifacts.

```text
cargo test --locked --target x86_64-pc-windows-msvc --lib
python scripts/build.py --target windows-amd64
python scripts/verify.py --compiler C:/path/to/tob.exe
```

The verifier serves the real package registry protocol on loopback, installs
with `tob add`, exercises `install` and `update`, executes the test `.tob` in
cold/warm runs, links a standalone application, and runs it with only the system
PATH. It also rejects incompatible locks on build and execution. `--public`
uses the public registry and the ordinary global Pebble cache. Tests execute
only on Windows AMD64. See `tests/results.json` and `tests/public-results.json`
for the latest evidence.

| Package target | Rust target | Precompiled artifacts | Execution validation |
| --- | --- | --- | --- |
| windows-amd64 | x86_64-pc-windows-msvc | DLL, DLL import `.dll.lib`, static `.lib` | Windows AMD64 JIT + standalone |
| linux-amd64 | x86_64-unknown-linux-gnu | `.so`, static `.a` | Built, unexecuted; glibc >=2.28 |
| darwin-arm64 | aarch64-apple-darwin | `.dylib`, static `.a` | Built, unexecuted; macOS >=11 |

Run `scripts/build.py --target linux-amd64` or `--target darwin-arm64` on the
corresponding author host. `--llvm-bin` overrides the LLVM directory. From
Windows, `--zig C:/path/to/zig.exe` supports the two Unix cross builds. Other
Tobbins targets are not included in this release. The static library is used
for standalone applications, while the import library is provided for C tools;
an import-only library is not a substitute for the static archive in metadata.
The build script isolates private Rust/SQLite symbols in static archives so
independent Rust runtimes do not interpose allocation or panic state.

Dependencies: rusqlite **0.40.2**, `bundled`, `hooks`, and `limits` features; libsqlite3-sys **0.38.2**.
All transitive versions are pinned by the committed `Cargo.lock`. The copied
`abi/` files are Tobbins' public C ABI 1.1 and dependency-free value codec. Rust
types, allocations, and layouts stay private. Each returned allocation has a
matching native release function. Panics are caught at every callable boundary.

## Publish

Build all three targets, run the Windows tests, commit the source, lockfile,
bindings, and `native/` binaries to the public repository's default branch, then:

```text
python scripts/publish.py
```

This uses the existing `https://tobbins.org/api/v1/publish` endpoint. The registry
snapshots the public default branch and creates the normal immutable package
ZIP; no install-time build scripts are used. Version 0.1.0 cannot be overwritten
after publication. Future releases must update the manifest, native descriptor,
entry symbol, and symbol-isolation prefix together. Compatibility is checked by
the registry at publication and by the compiler for all direct/transitive locked
dependencies during installation, updates, builds, and source execution.
