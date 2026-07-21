//! Opt-in smoke tests against real server databases.
//!
//! Normal local test runs skip this test. CI supplies one `PLUSPLUS_LIVE_*` configuration
//! per job and invokes the ignored test explicitly.

use plusplus_core::{connect, safety, ConnectionConfig, DbKind, SslMode, Value};

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
    let attempts = std::env::var("PLUSPLUS_LIVE_CONNECT_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let mut attempt = 0;
    let db = loop {
        attempt += 1;
        match connect(&cfg, Some(password.clone()), None).await {
            Ok(db) => break db,
            Err(error) if attempt < attempts => {
                eprintln!("database not ready ({attempt}/{attempts}): {error}");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            Err(error) => panic!("connect to live database after {attempt} attempt(s): {error}"),
        }
    };

    let (drop_child_sql, drop_sql, create_sql, create_child_sql) = match cfg.kind {
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => (
            "DROP TABLE IF EXISTS plusplus_ci_smoke_child",
            "DROP TABLE IF EXISTS plusplus_ci_smoke",
            "CREATE TABLE plusplus_ci_smoke (id INTEGER PRIMARY KEY, label VARCHAR(64) NOT NULL)",
            "CREATE TABLE plusplus_ci_smoke_child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL, CONSTRAINT plusplus_ci_smoke_child_fk FOREIGN KEY (parent_id) REFERENCES plusplus_ci_smoke(id))",
        ),
        DbKind::SqlServer => (
            "IF OBJECT_ID('plusplus_ci_smoke_child', 'U') IS NOT NULL DROP TABLE plusplus_ci_smoke_child",
            "IF OBJECT_ID('plusplus_ci_smoke', 'U') IS NOT NULL DROP TABLE plusplus_ci_smoke",
            "CREATE TABLE plusplus_ci_smoke (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL)",
            "CREATE TABLE plusplus_ci_smoke_child (id INT PRIMARY KEY, parent_id INT NOT NULL, CONSTRAINT plusplus_ci_smoke_child_fk FOREIGN KEY (parent_id) REFERENCES plusplus_ci_smoke(id))",
        ),
        DbKind::Sqlite => unreachable!("SQLite has its own always-on test suite"),
    };

    db.execute(drop_child_sql)
        .await
        .expect("clean stale smoke child table");
    db.execute(drop_sql).await.expect("clean stale smoke table");
    db.execute(create_sql).await.expect("create smoke table");
    db.execute(create_child_sql)
        .await
        .expect("create smoke child table");
    db.execute("INSERT INTO plusplus_ci_smoke (id, label) VALUES (1, 'hello')")
        .await
        .expect("insert smoke row");

    let result = db
        .execute("SELECT id, label FROM plusplus_ci_smoke WHERE id = 1")
        .await
        .expect("read smoke row");
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.columns.len(), 2);

    // Production Guardian must collect evidence without executing the guarded UPDATE.
    let guarded_sql = "UPDATE plusplus_ci_smoke SET label = 'changed' WHERE id = 1";
    let statement = safety::dangerous_statements(cfg.kind, guarded_sql)
        .into_iter()
        .next()
        .expect("UPDATE must be classified as destructive");
    let preflight = db.production_preflight(&statement).await;
    assert_eq!(preflight.affected_rows, Some(1));
    match cfg.kind {
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => {
            assert!(
                preflight.plan.is_some(),
                "server EXPLAIN should produce a plan"
            );
        }
        DbKind::SqlServer => {
            assert!(preflight.plan.is_none(), "SHOWPLAN is deliberately skipped");
        }
        DbKind::Sqlite => unreachable!(),
    }
    let unchanged = db
        .execute("SELECT label FROM plusplus_ci_smoke WHERE id = 1")
        .await
        .expect("read row after guardian preflight");
    assert_eq!(unchanged.rows, vec![vec![Value::Text("hello".into())]]);

    let schema = db.introspect().await.expect("introspect live database");
    assert!(
        schema
            .tables
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("plusplus_ci_smoke")),
        "smoke table missing from schema: {schema:?}"
    );
    let child = schema
        .tables
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("plusplus_ci_smoke_child"))
        .expect("smoke child table missing from schema");
    let fk = child
        .foreign_keys
        .iter()
        .find(|fk| fk.name.eq_ignore_ascii_case("plusplus_ci_smoke_child_fk"))
        .expect("smoke child foreign key missing from schema");
    assert!(fk.ref_table.eq_ignore_ascii_case("plusplus_ci_smoke"));
    assert_eq!(fk.columns.len(), 1);
    assert!(fk.columns[0].eq_ignore_ascii_case("parent_id"));
    assert_eq!(fk.ref_columns.len(), 1);
    assert!(fk.ref_columns[0].eq_ignore_ascii_case("id"));

    db.execute(drop_child_sql).await.expect("drop smoke child table");
    db.execute(drop_sql).await.expect("drop smoke table");
}
