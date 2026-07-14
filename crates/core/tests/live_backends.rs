//! Opt-in smoke tests against real server databases.
//!
//! Normal local test runs skip this test. CI supplies one `PLUSPLUS_LIVE_*` configuration
//! per job and invokes the ignored test explicitly.

use plusplus_core::{connect, ConnectionConfig, DbKind, SslMode};

fn live_config() -> Option<(ConnectionConfig, String)> {
    let kind = match std::env::var("PLUSPLUS_LIVE_KIND").ok()?.as_str() {
        "postgres" => DbKind::Postgres,
        "mysql" => DbKind::MySql,
        "sqlserver" => DbKind::SqlServer,
        other => panic!("unsupported PLUSPLUS_LIVE_KIND: {other}"),
    };
    let mut cfg = ConnectionConfig::new(kind);
    cfg.host = std::env::var("PLUSPLUS_LIVE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    cfg.port = std::env::var("PLUSPLUS_LIVE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| kind.default_port());
    cfg.user = std::env::var("PLUSPLUS_LIVE_USER").expect("PLUSPLUS_LIVE_USER");
    cfg.database = std::env::var("PLUSPLUS_LIVE_DATABASE").expect("PLUSPLUS_LIVE_DATABASE");
    cfg.ssl_mode = SslMode::Disable;
    let password = std::env::var("PLUSPLUS_LIVE_PASSWORD").expect("PLUSPLUS_LIVE_PASSWORD");
    Some((cfg, password))
}

#[tokio::test]
#[ignore = "requires PLUSPLUS_LIVE_* and a real database server"]
async fn connect_query_mutate_and_introspect() {
    let Some((cfg, password)) = live_config() else {
        eprintln!("skipped: PLUSPLUS_LIVE_KIND is not configured");
        return;
    };
    let db = connect(&cfg, Some(password), None)
        .await
        .expect("connect to live database");

    let (drop_sql, create_sql) = match cfg.kind {
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => (
            "DROP TABLE IF EXISTS plusplus_ci_smoke",
            "CREATE TABLE plusplus_ci_smoke (id INTEGER PRIMARY KEY, label VARCHAR(64) NOT NULL)",
        ),
        DbKind::SqlServer => (
            "IF OBJECT_ID('plusplus_ci_smoke', 'U') IS NOT NULL DROP TABLE plusplus_ci_smoke",
            "CREATE TABLE plusplus_ci_smoke (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL)",
        ),
        DbKind::Sqlite => unreachable!("SQLite has its own always-on test suite"),
    };

    db.execute(drop_sql).await.expect("clean stale smoke table");
    db.execute(create_sql).await.expect("create smoke table");
    db.execute("INSERT INTO plusplus_ci_smoke (id, label) VALUES (1, 'hello')")
        .await
        .expect("insert smoke row");

    let result = db
        .execute("SELECT id, label FROM plusplus_ci_smoke WHERE id = 1")
        .await
        .expect("read smoke row");
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.columns.len(), 2);

    let schema = db.introspect().await.expect("introspect live database");
    assert!(
        schema
            .tables
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("plusplus_ci_smoke")),
        "smoke table missing from schema: {schema:?}"
    );

    db.execute(drop_sql).await.expect("drop smoke table");
}
