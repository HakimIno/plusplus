# Row edit planning

`plusplus_core::edits::plan_edits` is the shared, GUI-independent entry point for
validating staged row changes and building a `CommitPlan`. It performs no database
I/O. The UI flushes its active cell editor, borrows the result and staging maps into
an `EditBatch`, then displays the plan's SQL.

## Performance

- Values and column names are borrowed while building SQL; text and BLOB payloads
  are not cloned into intermediate owned vectors.
- UPDATE/DELETE-only plans never scan unrelated result rows for primary keys.
- Untouched new-row slots do not allocate SQL or trigger a primary-key scan.
- INSERT duplicate checks use a borrowed, typed hash index. For N loaded rows and
  M inserted rows, key lookup is expected O(N + M), instead of O(N*M + M*M), for
  fixed-width keys. Text/binary hashing also depends on the key byte lengths.
- Rows and columns are sorted for reproducible SQL. This costs O(E log E) for E
  staged rows, plus sorting the changed columns in each row.
- Schema refresh borrows connection metadata instead of cloning the schema tree.

Run the edit-specific benchmarks with:

```sh
cargo bench -p plusplus-core --bench core_hot_paths -- edit_planning
```

The fixtures compare a single UPDATE and 1,000 INSERTs against results containing
100, 10,000 and 100,000 rows. They measure planning, excluding database execution.

## Preview validity

The UI binds each preview to the originating tab, table/PK metadata, result
revision and live database handle. Confirmation resolves that tab explicitly and
rechecks read-only policy. Closing/rebinding the tab, reconnecting or replacing
the result invalidates the preview. Replanning detects changed staged values
before execution. Invalidating a preview preserves the staged edits.

Production Guardian remains on the confirmation path. CQL previews explicitly
explain that execution is sequential and earlier writes cannot be rolled back.

SQL edit-source recognition parses the entire statement with the existing
`sqlparser` dependency. Compound queries, grouping and unsupported syntax stay
read-only even when they begin with a simple `SELECT * FROM ... WHERE ...`.
Tree-sitter is not required for this path.

## Future MCP adapter

This change provides a reusable planner, not an MCP server. A server can call the
same planner without depending on egui, but must own these responsibilities:

1. Resolve an authorized connection, table and result snapshot server-side.
2. Map primary-key row identities and column names to the snapshot's indices.
   `EditBatch` indices are internal snapshot coordinates, not stable remote IDs.
3. Return a preview and an opaque plan ID, bound to the connection and snapshot.
4. Apply read-only/production policy and verify the plan before executing it.
5. Define durable retry/idempotency behavior before exposing a commit tool.

The planner rejects malformed indices and values even when called without the UI.
Its local PK index checks loaded rows only; database constraints remain authoritative
for unloaded rows, collations and type coercions. Columns marked as database-generated
may be omitted from INSERT (including generated primary keys); after commit the normal
query refresh displays the generated value. The preview revision protects local state,
not concurrent writes from another client.

Row identity selection is automatic: it prefers a primary key and falls back to a
unique index. This keeps the normal table view uncluttered, matching standard database
clients. The database remains authoritative for legacy tables whose uniqueness is not
declared in the schema.

UPDATE statements now include the original values of changed columns in their
`WHERE` predicates, and DELETE statements include the original row values. This
is an optimistic concurrency guard: a concurrent change makes the predicate match
zero rows, so the other user's value is not overwritten. The current transaction
interface still reports statement count rather than affected-row count; adding an
explicit conflict result to the backend API remains the next step for surfacing
that zero-row match as a dedicated UI error instead of a successful no-op.

## Editing surfaces

Editing stays focused in the Data grid and the existing Details panel. Both use the same
staged state, validation, undo/redo and generated-key rules. Fill-handle drag remains
available for copying a cell down a range.

Binary columns support staging a file up to 64 MiB, including non-image BLOB/BYTEA/
VARBINARY values. The value viewer previews binary data with a bounded hex view and can save
the original bytes back to a file. Large text/CLOB values continue to use the validated text
editor and are never copied by the edit planner unless the user stages a replacement.
