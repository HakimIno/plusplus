//! End-to-end DuckDB workflows exercised through the public core API.

use plusplus_core::{connect, ConnectionConfig, DbKind, Value};

#[tokio::test]
async fn connects_queries_mutates_and_introspects_an_embedded_database() {
    let mut cfg = ConnectionConfig::new(DbKind::DuckDb);
    cfg.duckdb_path = ":memory:".into();
    let db = connect(&cfg, None, None).await.expect("connect DuckDB");

    db.execute("CREATE TABLE plusplus_ci_smoke (id INTEGER PRIMARY KEY, label VARCHAR NOT NULL)")
        .await
        .expect("create smoke table");
    db.execute("INSERT INTO plusplus_ci_smoke VALUES (1, 'hello')")
        .await
        .expect("insert smoke row");

    let result = db
        .execute("SELECT id, label FROM plusplus_ci_smoke")
        .await
        .expect("read smoke row");
    assert_eq!(
        result.rows,
        vec![vec![Value::Int(1), Value::Text("hello".into())]]
    );

    db.execute("UPDATE plusplus_ci_smoke SET label = 'changed' WHERE id = 1")
        .await
        .expect("update smoke row");
    let schema = db.introspect().await.expect("introspect DuckDB");
    assert!(
        schema
            .tables
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("plusplus_ci_smoke")),
        "smoke table missing from schema: {schema:?}"
    );
}
