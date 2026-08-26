//! Unit tests for the model facade and its feature modules.

use super::*;
use crate::value::Value;

fn target(sql: &str) -> Option<(Option<String>, String)> {
    simple_select_target(sql)
}

#[test]
fn cql_create_table_omits_not_null_and_default() {
    // A CQL table: partition key `id`, a regular nullable `name`, no NOT NULL/DEFAULT,
    // no ENGINE clause. Single-column key renders inline as PRIMARY KEY.
    let cols = vec![
        ColumnDef {
            name: "id".into(),
            data_type: "uuid".into(),
            nullable: false,
            primary_key: true,
            default: None,
        },
        ColumnDef {
            // A UI-provided default and NOT NULL must both be dropped for CQL.
            name: "name".into(),
            data_type: "text".into(),
            nullable: false,
            primary_key: false,
            default: Some("'anon'".into()),
        },
    ];
    let sql = build_create_table_sql(DbKind::ScyllaDb, Some("ks"), "users", &cols, &[]);
    assert_eq!(
        sql,
        "CREATE TABLE \"ks\".\"users\" (\n    \
         \"id\" uuid PRIMARY KEY,\n    \"name\" text\n);"
    );
    assert!(!sql.contains("NOT NULL"));
    assert!(!sql.contains("DEFAULT"));
    assert!(!sql.contains("ENGINE"));
}

#[test]
fn cql_composite_primary_key_uses_trailing_clause() {
    let cols = vec![
        ColumnDef {
            name: "pk".into(),
            data_type: "text".into(),
            nullable: false,
            primary_key: true,
            default: None,
        },
        ColumnDef {
            name: "ck".into(),
            data_type: "int".into(),
            nullable: false,
            primary_key: true,
            default: None,
        },
        ColumnDef {
            name: "val".into(),
            data_type: "text".into(),
            nullable: true,
            primary_key: false,
            default: None,
        },
    ];
    let sql = build_create_table_sql(DbKind::Cassandra, None, "t", &cols, &[]);
    assert!(sql.contains("PRIMARY KEY (\"pk\", \"ck\")"), "{sql}");
    assert!(!sql.contains("NOT NULL"));
}

#[test]
fn cql_add_column_and_index_and_drop() {
    // ADD COLUMN carries no NOT NULL/DEFAULT for CQL.
    let col = ColumnDef {
        name: "age".into(),
        data_type: "int".into(),
        nullable: false,
        primary_key: false,
        default: Some("0".into()),
    };
    assert_eq!(
        build_add_column_sql(DbKind::ScyllaDb, Some("ks"), "t", &col),
        "ALTER TABLE \"ks\".\"t\" ADD COLUMN \"age\" int;"
    );

    // Secondary index: the name is bare (no keyspace prefix) in CREATE INDEX.
    let idx = IndexDef {
        name: "by_email".into(),
        columns: vec!["email".into()],
        unique: false,
    };
    assert_eq!(
        build_create_index_sql(DbKind::Cassandra, Some("ks"), "t", &idx),
        "CREATE INDEX \"by_email\" ON \"ks\".\"t\" (\"email\");"
    );

    // DROP INDEX qualifies with the keyspace.
    assert_eq!(
        build_drop_index_sql(DbKind::Cassandra, Some("ks"), "t", "by_email"),
        "DROP INDEX \"ks\".\"by_email\";"
    );

    // TRUNCATE uses the standard TABLE form (not SQLite's DELETE fallback).
    assert_eq!(
        build_truncate_table_sql(DbKind::ScyllaDb, Some("ks"), "t"),
        "TRUNCATE TABLE \"ks\".\"t\";"
    );
}

#[test]
fn cql_rename_column_omits_column_keyword() {
    assert_eq!(
        build_rename_column_sql(DbKind::Cassandra, Some("ks"), "t", "old", "new"),
        "ALTER TABLE \"ks\".\"t\" RENAME \"old\" TO \"new\";"
    );
}

#[test]
fn cql_unsupported_ddl_is_refused_or_empty() {
    // Clone: no CREATE TABLE LIKE / INSERT SELECT in CQL — returns no statements.
    assert!(build_clone_table_sql(DbKind::Cassandra, Some("ks"), "t", "t_copy").is_empty());
    // ALTER column type: CQL dropped it — no statements.
    let col = ColumnDef {
        name: "c".into(),
        data_type: "text".into(),
        nullable: true,
        primary_key: false,
        default: None,
    };
    assert!(build_alter_column_sql(DbKind::ScyllaDb, None, "t", &col).is_empty());
    // Routines and triggers are refused with a clear message.
    let rb = RoutineBuild {
        schema: None,
        name: "f",
        kind: RoutineKind::Function,
        params: &[],
        return_type: Some("int"),
        language: "",
        body: "return 1;",
    };
    assert!(build_create_routine_sql(DbKind::Cassandra, &rb, false).is_err());
}

#[test]
fn cql_bool_literal_uses_true_false() {
    // Copy-as-INSERT path renders booleans as TRUE/FALSE for CQL, not 1/0.
    let sql = build_insert_sql(
        DbKind::ScyllaDb,
        Some("ks"),
        "t",
        &[("id", &Value::Int(1)), ("active", &Value::Bool(true))],
    )
    .unwrap();
    assert_eq!(
        sql,
        "INSERT INTO \"ks\".\"t\" (\"id\", \"active\") VALUES (1, TRUE);"
    );
}

#[test]
fn create_view_per_dialect() {
    // Postgres / MySQL redefine in place with OR REPLACE.
    assert_eq!(
        build_create_view_sql(
            DbKind::Postgres,
            Some("public"),
            "v",
            "SELECT 1",
            false,
            true
        ),
        "CREATE OR REPLACE VIEW \"public\".\"v\" AS\nSELECT 1;"
    );
    assert_eq!(
        build_create_view_sql(DbKind::MySql, None, "v", "SELECT 1;", false, true),
        "CREATE OR REPLACE VIEW `v` AS\nSELECT 1;"
    );
    // SQL Server uses OR ALTER instead.
    assert_eq!(
        build_create_view_sql(DbKind::SqlServer, Some("dbo"), "v", "SELECT 1", false, true),
        "CREATE OR ALTER VIEW \"dbo\".\"v\" AS\nSELECT 1;"
    );
    // SQLite has no replace form even when asked (caller drops first).
    assert_eq!(
        build_create_view_sql(DbKind::Sqlite, None, "v", "SELECT 1", false, false),
        "CREATE VIEW \"v\" AS\nSELECT 1;"
    );
    // Postgres materialized view: MATERIALIZED, and OR REPLACE is suppressed for it.
    assert_eq!(
        build_create_view_sql(DbKind::Postgres, None, "mv", "SELECT 1", true, true),
        "CREATE MATERIALIZED VIEW \"mv\" AS\nSELECT 1;"
    );
}

#[test]
fn drop_view_per_dialect() {
    assert_eq!(
        build_drop_view_sql(DbKind::Postgres, Some("public"), "v", false),
        "DROP VIEW \"public\".\"v\";"
    );
    assert_eq!(
        build_drop_view_sql(DbKind::Postgres, None, "mv", true),
        "DROP MATERIALIZED VIEW \"mv\";"
    );
    assert_eq!(
        build_drop_view_sql(DbKind::MySql, None, "v", false),
        "DROP VIEW `v`;"
    );
    // Only Postgres has materialized views; the keyword is dropped elsewhere.
    assert_eq!(
        build_drop_view_sql(DbKind::MySql, None, "v", true),
        "DROP VIEW `v`;"
    );
}

#[test]
fn view_replace_support_per_dialect() {
    assert!(view_supports_replace(DbKind::Postgres, false));
    assert!(view_supports_replace(DbKind::SqlServer, false));
    assert!(!view_supports_replace(DbKind::Sqlite, false));
    assert!(!view_supports_replace(DbKind::Postgres, true)); // materialized: no OR REPLACE
}

#[test]
fn create_trigger_postgres_generates_function() {
    let t = TriggerBuild {
        schema: Some("public"),
        name: "trg",
        table: "t",
        timing: TriggerTiming::Before,
        events: &[TriggerEvent::Insert, TriggerEvent::Update],
        level: TriggerLevel::Row,
        when_condition: Some("NEW.n > 0"),
        body: "BEGIN NEW.updated := now(); RETURN NEW; END;",
        pg_existing_function: false,
    };
    let sql = build_create_trigger_sql(DbKind::Postgres, &t).unwrap();
    assert_eq!(sql.len(), 2);
    assert!(sql[0].starts_with("CREATE OR REPLACE FUNCTION \"public\".\"trg_trigfn\"()"));
    assert!(sql[1].contains("CREATE TRIGGER \"trg\" BEFORE INSERT OR UPDATE ON \"public\".\"t\""));
    assert!(sql[1].contains("FOR EACH ROW"));
    assert!(sql[1].contains("WHEN (NEW.n > 0)"));
    assert!(sql[1].ends_with("EXECUTE FUNCTION \"public\".\"trg_trigfn\"();"));
}

#[test]
fn create_trigger_postgres_existing_function() {
    let t = TriggerBuild {
        schema: None,
        name: "trg",
        table: "t",
        timing: TriggerTiming::After,
        events: &[TriggerEvent::Delete],
        level: TriggerLevel::Statement,
        when_condition: None,
        body: "audit_fn",
        pg_existing_function: true,
    };
    let sql = build_create_trigger_sql(DbKind::Postgres, &t).unwrap();
    assert_eq!(sql.len(), 1);
    assert!(sql[0].contains("FOR EACH STATEMENT"));
    assert!(sql[0].ends_with("EXECUTE FUNCTION audit_fn();"));
}

#[test]
fn create_trigger_mysql_single_event_only() {
    let one = TriggerBuild {
        schema: None,
        name: "trg",
        table: "t",
        timing: TriggerTiming::Before,
        events: &[TriggerEvent::Insert],
        level: TriggerLevel::Row,
        when_condition: None,
        body: "SET NEW.created = NOW()",
        pg_existing_function: false,
    };
    assert_eq!(
        build_create_trigger_sql(DbKind::MySql, &one).unwrap(),
        vec!["CREATE TRIGGER `trg` BEFORE INSERT ON `t`\nFOR EACH ROW\nSET NEW.created = NOW();"]
    );
    let multi = TriggerBuild {
        events: &[TriggerEvent::Insert, TriggerEvent::Update],
        ..one
    };
    assert!(build_create_trigger_sql(DbKind::MySql, &multi).is_err());
}

#[test]
fn create_trigger_sqlite_wraps_begin_end() {
    let t = TriggerBuild {
        schema: None,
        name: "trg",
        table: "t",
        timing: TriggerTiming::After,
        events: &[TriggerEvent::Update],
        level: TriggerLevel::Row,
        when_condition: Some("NEW.n <> OLD.n"),
        body: "INSERT INTO audit VALUES ('u');",
        pg_existing_function: false,
    };
    let sql = build_create_trigger_sql(DbKind::Sqlite, &t).unwrap();
    assert!(sql[0].contains("CREATE TRIGGER \"trg\" AFTER UPDATE ON \"t\""));
    assert!(sql[0].contains("WHEN (NEW.n <> OLD.n)"));
    assert!(sql[0].contains("BEGIN\nINSERT INTO audit VALUES ('u');\nEND;"));
}

#[test]
fn create_trigger_sqlserver_rejects_before() {
    let after = TriggerBuild {
        schema: Some("dbo"),
        name: "trg",
        table: "t",
        timing: TriggerTiming::After,
        events: &[TriggerEvent::Insert, TriggerEvent::Delete],
        level: TriggerLevel::Statement,
        when_condition: None,
        body: "BEGIN SET NOCOUNT ON; END",
        pg_existing_function: false,
    };
    let sql = build_create_trigger_sql(DbKind::SqlServer, &after).unwrap();
    assert!(sql[0].contains("CREATE TRIGGER \"trg\" ON \"dbo\".\"t\""));
    assert!(sql[0].contains("AFTER INSERT, DELETE"));
    let before = TriggerBuild {
        timing: TriggerTiming::Before,
        ..after
    };
    assert!(build_create_trigger_sql(DbKind::SqlServer, &before).is_err());
}

#[test]
fn drop_trigger_per_dialect() {
    assert_eq!(
        build_drop_trigger_sql(DbKind::Postgres, Some("public"), "trg", "t"),
        "DROP TRIGGER \"trg\" ON \"public\".\"t\";"
    );
    assert_eq!(
        build_drop_trigger_sql(DbKind::MySql, Some("app"), "trg", "t"),
        "DROP TRIGGER `app`.`trg`;"
    );
    assert_eq!(
        build_drop_trigger_sql(DbKind::Sqlite, None, "trg", "t"),
        "DROP TRIGGER \"trg\";"
    );
    assert_eq!(
        build_drop_trigger_sql(DbKind::SqlServer, Some("dbo"), "trg", "t"),
        "DROP TRIGGER \"dbo\".\"trg\";"
    );
}

fn param(name: &str, ty: &str, mode: ParamMode, default: Option<&str>) -> RoutineParam {
    RoutineParam {
        name: name.into(),
        data_type: ty.into(),
        mode,
        default: default.map(str::to_string),
    }
}

#[test]
fn create_function_postgres() {
    let params = [
        param("a", "integer", ParamMode::In, None),
        param("b", "integer", ParamMode::In, Some("0")),
    ];
    let r = RoutineBuild {
        schema: Some("public"),
        name: "add",
        kind: RoutineKind::Function,
        params: &params,
        return_type: Some("integer"),
        language: "sql",
        body: "SELECT a + b;",
    };
    assert_eq!(
        build_create_routine_sql(DbKind::Postgres, &r, true).unwrap()[0],
        "CREATE OR REPLACE FUNCTION \"public\".\"add\"(a integer, b integer DEFAULT 0) \
         RETURNS integer\nLANGUAGE sql AS $$\nSELECT a + b;\n$$;"
    );
}

#[test]
fn create_procedure_mysql_with_modes() {
    let params = [
        param("x", "INT", ParamMode::In, None),
        param("y", "INT", ParamMode::Out, None),
    ];
    let r = RoutineBuild {
        schema: None,
        name: "p",
        kind: RoutineKind::Procedure,
        params: &params,
        return_type: None,
        language: "",
        body: "BEGIN SET y = x; END",
    };
    assert_eq!(
        build_create_routine_sql(DbKind::MySql, &r, false).unwrap()[0],
        "CREATE PROCEDURE `p`(x INT, OUT y INT)\nBEGIN SET y = x; END"
    );
}

#[test]
fn create_routine_sqlserver_function_and_procedure() {
    let fparams = [param("a", "int", ParamMode::In, None)];
    let f = RoutineBuild {
        schema: Some("dbo"),
        name: "f",
        kind: RoutineKind::Function,
        params: &fparams,
        return_type: Some("int"),
        language: "",
        body: "BEGIN RETURN @a; END",
    };
    assert_eq!(
        build_create_routine_sql(DbKind::SqlServer, &f, true).unwrap()[0],
        "CREATE OR ALTER FUNCTION \"dbo\".\"f\"(@a int)\nRETURNS int\nAS\nBEGIN RETURN @a; END;"
    );
    // Procedures list parameters without parentheses.
    let pparams = [param("id", "int", ParamMode::In, None)];
    let p = RoutineBuild {
        schema: None,
        name: "p",
        kind: RoutineKind::Procedure,
        params: &pparams,
        return_type: None,
        language: "",
        body: "BEGIN SELECT 1; END",
    };
    assert_eq!(
        build_create_routine_sql(DbKind::SqlServer, &p, false).unwrap()[0],
        "CREATE PROCEDURE \"p\" @id int\nAS\nBEGIN SELECT 1; END;"
    );
}

#[test]
fn routine_validation_and_sqlite() {
    // A function with no return type is rejected.
    let f = RoutineBuild {
        schema: None,
        name: "f",
        kind: RoutineKind::Function,
        params: &[],
        return_type: None,
        language: "sql",
        body: "SELECT 1",
    };
    assert!(build_create_routine_sql(DbKind::Postgres, &f, false).is_err());
    // SQLite has no routines at all.
    let p = RoutineBuild {
        schema: None,
        name: "p",
        kind: RoutineKind::Procedure,
        params: &[],
        return_type: None,
        language: "",
        body: "x",
    };
    assert!(build_create_routine_sql(DbKind::Sqlite, &p, false).is_err());
}

#[test]
fn drop_routine_per_dialect() {
    let params = [
        param("a", "integer", ParamMode::In, None),
        param("o", "integer", ParamMode::Out, None),
    ];
    // Postgres disambiguates by IN/INOUT arg types (OUT excluded).
    assert_eq!(
        build_drop_routine_sql(
            DbKind::Postgres,
            Some("public"),
            "f",
            RoutineKind::Function,
            &params
        ),
        "DROP FUNCTION \"public\".\"f\"(integer);"
    );
    assert_eq!(
        build_drop_routine_sql(DbKind::MySql, None, "p", RoutineKind::Procedure, &params),
        "DROP PROCEDURE `p`;"
    );
    assert_eq!(
        build_drop_routine_sql(
            DbKind::SqlServer,
            Some("dbo"),
            "f",
            RoutineKind::Function,
            &params
        ),
        "DROP FUNCTION \"dbo\".\"f\";"
    );
    assert!(routine_supports_replace(DbKind::Postgres));
    assert!(routine_supports_replace(DbKind::SqlServer));
    assert!(!routine_supports_replace(DbKind::MySql));
}

/// New connections start at Require (encrypted, no plaintext fallback). The bare
/// `SslMode::default()` stays Prefer — that's the value an old, pre-TLS config file
/// deserializes to, and it must not change underneath existing connections.
#[test]
fn new_connection_defaults_to_require_but_default_stays_prefer() {
    assert_eq!(
        ConnectionConfig::new(DbKind::Postgres).ssl_mode,
        SslMode::Require
    );
    assert_eq!(SslMode::default(), SslMode::Prefer);
    // Only the non-verifying modes carry a warning; the verifying ones don't.
    assert!(SslMode::Prefer.security_warning().is_some());
    assert!(SslMode::Require.security_warning().is_some());
    assert!(SslMode::VerifyFull.security_warning().is_none());
}

#[test]
fn duckdb_connection_defaults_to_an_in_memory_embedded_database() {
    let config = ConnectionConfig::new(DbKind::DuckDb);
    assert_eq!(config.duckdb_path, ":memory:");
    assert!(!config.kind.is_server());
    assert_eq!(config.target_summary(), ":memory:");
}

#[test]
fn safety_profiles_enforce_their_policy() {
    let mut cfg = ConnectionConfig::new(DbKind::Postgres);
    assert_eq!(cfg.safety_profile, SafetyProfile::Custom);

    cfg.set_safety_profile(SafetyProfile::Development);
    assert!(!cfg.is_production());
    assert!(!cfg.is_read_only());

    cfg.set_safety_profile(SafetyProfile::Staging);
    assert!(cfg.is_production());
    assert!(!cfg.is_read_only());

    cfg.set_safety_profile(SafetyProfile::Production);
    assert!(cfg.is_production());
    assert!(cfg.is_read_only());

    // Effective checks fail closed even before normalization if persisted legacy flags
    // are stale or the config was hand-edited.
    cfg.production = false;
    cfg.read_only = false;
    assert!(cfg.is_production());
    assert!(cfg.is_read_only());
    cfg.apply_safety_profile();
    assert!(cfg.production);
    assert!(cfg.read_only);

    let json = serde_json::to_value(&cfg).unwrap();
    assert_eq!(json["safety_profile"], "production");
    assert_eq!(json["production"], true);
    assert_eq!(json["read_only"], true);
}

#[test]
fn alter_column_postgres_splits_type_and_nullability() {
    let col = ColumnDef {
        name: "price".into(),
        data_type: "numeric(10,2)".into(),
        nullable: false,
        primary_key: false,
        default: Some("0".into()),
    };
    let sql = build_alter_column_sql(DbKind::Postgres, Some("public"), "items", &col);
    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" TYPE numeric(10,2);",
            "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" SET NOT NULL;",
            "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" SET DEFAULT 0;",
        ]
    );
}

#[test]
fn alter_column_mysql_uses_single_modify() {
    let col = ColumnDef {
        name: "name".into(),
        data_type: "varchar(255)".into(),
        nullable: true,
        primary_key: false,
        default: None,
    };
    let sql = build_alter_column_sql(DbKind::MySql, None, "users", &col);
    assert_eq!(
        sql,
        vec!["ALTER TABLE `users` MODIFY COLUMN `name` varchar(255) NULL;"]
    );
}

#[test]
fn alter_column_sqlserver_alters_then_adds_default() {
    let col = ColumnDef {
        name: "qty".into(),
        data_type: "int".into(),
        nullable: false,
        primary_key: false,
        default: Some("1".into()),
    };
    let sql = build_alter_column_sql(DbKind::SqlServer, Some("dbo"), "orders", &col);
    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"dbo\".\"orders\" ALTER COLUMN \"qty\" int NOT NULL;",
            "ALTER TABLE \"dbo\".\"orders\" ADD DEFAULT 1 FOR \"qty\";",
        ]
    );
}

#[test]
fn simple_select_target_accepts_single_table_reads() {
    assert_eq!(target("SELECT * FROM users"), Some((None, "users".into())));
    assert_eq!(
        target("select * from users limit 20000;"),
        Some((None, "users".into()))
    );
    assert_eq!(
        target("SELECT * FROM \"public\".\"users\" LIMIT 100;"),
        Some((Some("public".into()), "users".into()))
    );
    assert_eq!(
        target("SELECT * FROM `db`.`orders` WHERE total > 10 ORDER BY id LIMIT 50"),
        Some((Some("db".into()), "orders".into()))
    );
    assert_eq!(
        target("SELECT TOP 100 * FROM [dbo].[Invoices];"),
        Some((Some("dbo".into()), "Invoices".into()))
    );
    // Embedded doubled quotes un-double.
    assert_eq!(
        target(r#"SELECT * FROM "we""ird""#),
        Some((None, "we\"ird".into()))
    );
}

#[test]
fn simple_select_target_rejects_everything_else() {
    // Projections lose the guarantee that the PK columns are present.
    assert_eq!(target("SELECT id, name FROM users"), None);
    // Joins/aliases/commas: rows no longer map 1:1 to table rows.
    assert_eq!(target("SELECT * FROM a JOIN b ON a.id = b.id"), None);
    assert_eq!(target("SELECT * FROM a, b"), None);
    assert_eq!(target("SELECT * FROM users u WHERE u.id = 1"), None);
    assert_eq!(target("SELECT * FROM (SELECT * FROM users) x"), None);
    // Non-SELECT and multi-statement scripts.
    assert_eq!(target("UPDATE users SET name = 'x'"), None);
    assert_eq!(target("SELECT * FROM a; SELECT * FROM b"), None);
    assert_eq!(target("SELECT * FROM users GROUP BY id"), None);
}

#[test]
fn parse_page_window_reads_every_dialect() {
    let win = |sql: &str| parse_page_window(sql).unwrap();
    assert_eq!(
        win("SELECT * FROM t"),
        PageWindow {
            limit: None,
            offset: 0
        }
    );
    assert_eq!(
        win("SELECT * FROM t LIMIT 100;"),
        PageWindow {
            limit: Some(100),
            offset: 0
        }
    );
    assert_eq!(
        win("SELECT * FROM t LIMIT 100 OFFSET 300"),
        PageWindow {
            limit: Some(100),
            offset: 300
        }
    );
    // MySQL's comma form puts the offset first.
    assert_eq!(
        win("SELECT * FROM t LIMIT 300, 100"),
        PageWindow {
            limit: Some(100),
            offset: 300
        }
    );
    assert_eq!(
        win("SELECT TOP 50 * FROM [dbo].[t];"),
        PageWindow {
            limit: Some(50),
            offset: 0
        }
    );
    assert_eq!(
        win("SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"),
        PageWindow {
            limit: Some(100),
            offset: 200
        }
    );
    // WHERE/ORDER BY don't confuse the parser; quoted text containing keywords is inert.
    assert_eq!(
        win("SELECT * FROM t WHERE name = 'limit 5 offset 2' ORDER BY id LIMIT 10 OFFSET 20"),
        PageWindow {
            limit: Some(10),
            offset: 20
        }
    );
    // An unquoted column named `offset` in WHERE isn't a paging clause.
    assert_eq!(
        win("SELECT * FROM t WHERE offset > 5 LIMIT 10"),
        PageWindow {
            limit: Some(10),
            offset: 0
        }
    );
    // Not a simple single-table read → no window.
    assert!(parse_page_window("SELECT a, b FROM t LIMIT 5").is_none());
}

#[test]
fn with_page_window_rewrites_in_place() {
    let pg = |sql: &str, l, o| with_page_window(DbKind::Postgres, sql, l, o).unwrap();
    assert_eq!(
        pg("SELECT * FROM t LIMIT 100;", 100, 200),
        "SELECT * FROM t LIMIT 100 OFFSET 200;"
    );
    assert_eq!(
        pg("SELECT * FROM t LIMIT 100 OFFSET 200;", 100, 0),
        "SELECT * FROM t LIMIT 100;"
    );
    // WHERE and ORDER BY survive the rewrite.
    assert_eq!(
        pg(
            "SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 50",
            50,
            100
        ),
        "SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 100;"
    );
    // A query with no paging clause gains one.
    assert_eq!(pg("SELECT * FROM t", 100, 0), "SELECT * FROM t LIMIT 100;");

    let ms = |sql: &str, l, o| with_page_window(DbKind::SqlServer, sql, l, o).unwrap();
    // Page one without ORDER BY keeps the TOP form.
    assert_eq!(
        ms("SELECT TOP 100 * FROM t;", 100, 0),
        "SELECT TOP 100 * FROM t;"
    );
    // Deeper pages need OFFSET…FETCH, which needs an ORDER BY.
    assert_eq!(
        ms("SELECT TOP 100 * FROM t;", 100, 200),
        "SELECT * FROM t ORDER BY (SELECT NULL) OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"
    );
    assert_eq!(
        ms(
            "SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;",
            100,
            300
        ),
        "SELECT * FROM t ORDER BY id OFFSET 300 ROWS FETCH NEXT 100 ROWS ONLY;"
    );
    // Joins and projections are refused.
    assert!(with_page_window(DbKind::Postgres, "SELECT a FROM t", 10, 0).is_none());
}

#[test]
fn keyset_page_is_stable_composite_and_preserves_filters() {
    let keys = vec!["tenant_id".to_string(), "id".to_string()];
    let values = vec![Value::Int(7), Value::Text("a'b".into())];
    assert_eq!(
        with_keyset_page(
            DbKind::Postgres,
            "SELECT * FROM events WHERE active = TRUE LIMIT 1000;",
            &keys,
            Some(&values),
            512,
        )
        .unwrap(),
        "SELECT * FROM events WHERE (active = TRUE) AND ((\"tenant_id\" > 7) OR (\"tenant_id\" = 7 AND \"id\" > 'a''b')) ORDER BY \"tenant_id\", \"id\" LIMIT 512;"
    );
    assert_eq!(
        with_keyset_page(
            DbKind::SqlServer,
            "SELECT TOP 1000 * FROM [events];",
            &["id".to_string()],
            None,
            512,
        )
        .unwrap(),
        "SELECT TOP 512 * FROM [events] ORDER BY \"id\";"
    );
}

#[test]
fn keyset_page_falls_back_for_unsafe_query_shapes() {
    let keys = vec!["id".to_string()];
    assert!(with_keyset_page(
        DbKind::Postgres,
        "SELECT * FROM events ORDER BY created_at LIMIT 100",
        &keys,
        None,
        50,
    )
    .is_none());
    assert!(with_keyset_page(
        DbKind::Postgres,
        "SELECT * FROM events LIMIT 100 OFFSET 50",
        &keys,
        None,
        50,
    )
    .is_none());
    assert!(with_keyset_page(
        DbKind::Postgres,
        "SELECT * FROM events LIMIT 100",
        &keys,
        Some(&[Value::Null]),
        50,
    )
    .is_none());
    assert!(with_keyset_page(
        DbKind::Postgres,
        "SELECT * FROM events LIMIT 100",
        &keys,
        Some(&[Value::Float(f64::NAN)]),
        50,
    )
    .is_none());
}

#[test]
fn build_count_sql_keeps_where_drops_order_and_paging() {
    assert_eq!(
        build_count_sql("SELECT * FROM t LIMIT 100 OFFSET 200;").unwrap(),
        "SELECT COUNT(*) FROM t;"
    );
    assert_eq!(
        build_count_sql("SELECT * FROM \"s\".\"t\" WHERE a > 1 ORDER BY a LIMIT 50").unwrap(),
        "SELECT COUNT(*) FROM \"s\".\"t\" WHERE a > 1;"
    );
    assert_eq!(
        build_count_sql("SELECT TOP 100 * FROM [dbo].[t]").unwrap(),
        "SELECT COUNT(*) FROM [dbo].[t];"
    );
    assert!(build_count_sql("SELECT a, b FROM t").is_none());
}

#[test]
fn build_insert_quotes_and_escapes() {
    let name = Value::Text("O'Brien".into());
    let age = Value::Int(42);
    let cols = [("name", &name), ("age", &age)];
    assert_eq!(
        build_insert_sql(DbKind::Postgres, Some("public"), "users", &cols),
        Some(
            "INSERT INTO \"public\".\"users\" (\"name\", \"age\") VALUES ('O''Brien', 42);"
                .to_string()
        )
    );
    // MySQL uses backtick identifiers.
    assert_eq!(
        build_insert_sql(DbKind::MySql, None, "users", &cols),
        Some("INSERT INTO `users` (`name`, `age`) VALUES ('O''Brien', 42);".to_string())
    );
    // No columns ⇒ nothing to insert.
    assert_eq!(build_insert_sql(DbKind::Postgres, None, "users", &[]), None);
    // Binary has no portable literal form.
    let blob = Value::Bytes(vec![1, 2, 3]);
    assert_eq!(
        build_insert_sql(DbKind::Postgres, None, "t", &[("data", &blob)]),
        None
    );
}

#[test]
fn build_delete_targets_by_key() {
    let id = Value::Int(7);
    assert_eq!(
        build_delete_sql(DbKind::Postgres, Some("public"), "users", &[("id", &id)]),
        Some("DELETE FROM \"public\".\"users\" WHERE \"id\" = 7;".to_string())
    );
    // NULL keys compare with IS NULL, and composite keys AND together.
    let null = Value::Null;
    let tenant = Value::Int(3);
    assert_eq!(
        build_delete_sql(
            DbKind::Postgres,
            None,
            "t",
            &[("tenant", &tenant), ("note", &null)]
        ),
        Some("DELETE FROM \"t\" WHERE \"tenant\" = 3 AND \"note\" IS NULL;".to_string())
    );
    // No keys ⇒ refuse (never emit an unfiltered DELETE).
    assert_eq!(build_delete_sql(DbKind::Postgres, None, "t", &[]), None);
}

#[test]
fn build_select_where_follows_a_foreign_key() {
    // Single-column FK: filter the referenced table to the pointed-at key.
    let uid = Value::Int(7);
    assert_eq!(
        build_select_where_sql(
            DbKind::Postgres,
            Some("public"),
            "users",
            &[("id", &uid)],
            100
        ),
        Some("SELECT * FROM \"public\".\"users\" WHERE \"id\" = 7 LIMIT 100;".to_string())
    );
    // SQL Server caps with TOP, not LIMIT.
    assert_eq!(
        build_select_where_sql(DbKind::SqlServer, None, "users", &[("id", &uid)], 100),
        Some("SELECT TOP 100 * FROM \"users\" WHERE \"id\" = 7;".to_string())
    );
    // Composite FK ANDs the key columns; string values are escaped (no literal breakout).
    let tenant = Value::Text("O'Brien".into());
    let seq = Value::Int(3);
    assert_eq!(
        build_select_where_sql(
            DbKind::MySql,
            None,
            "orders",
            &[("tenant", &tenant), ("seq", &seq)],
            100
        ),
        Some(
            "SELECT * FROM `orders` WHERE `tenant` = 'O''Brien' AND `seq` = 3 LIMIT 100;"
                .to_string()
        )
    );
    // No keys ⇒ refuse (never emit an unfiltered scan); binary keys have no literal form.
    assert_eq!(
        build_select_where_sql(DbKind::Postgres, None, "t", &[], 100),
        None
    );
    let blob = Value::Bytes(vec![1, 2, 3]);
    assert_eq!(
        build_select_where_sql(DbKind::Postgres, None, "t", &[("k", &blob)], 100),
        None
    );
}

#[test]
fn fk_action_parses_backend_rule_spellings() {
    // information_schema spells it with spaces; sys.foreign_keys with underscores.
    assert_eq!(FkAction::from_rule("CASCADE"), Some(FkAction::Cascade));
    assert_eq!(FkAction::from_rule("SET NULL"), Some(FkAction::SetNull));
    assert_eq!(FkAction::from_rule("SET_NULL"), Some(FkAction::SetNull));
    assert_eq!(FkAction::from_rule("no action"), Some(FkAction::NoAction));
    assert_eq!(FkAction::from_rule("RESTRICT"), Some(FkAction::Restrict));
    assert_eq!(FkAction::from_rule("SET DEFAULT"), None);
    assert_eq!(FkAction::from_rule(""), None);
}

#[test]
fn build_fk_ddl_is_dialect_aware() {
    let fk = ForeignKeyDef {
        name: "fk_orders_user".into(),
        columns: vec!["user_id".into()],
        ref_table: "users".into(),
        ref_columns: vec!["id".into()],
        on_delete: FkAction::Cascade,
    };
    assert_eq!(
        build_add_fk_sql(DbKind::Postgres, Some("public"), "orders", &fk),
        "ALTER TABLE \"public\".\"orders\" ADD CONSTRAINT \"fk_orders_user\" \
         FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE;"
    );
    assert_eq!(
        build_drop_fk_sql(DbKind::Postgres, Some("public"), "orders", "fk_orders_user"),
        "ALTER TABLE \"public\".\"orders\" DROP CONSTRAINT \"fk_orders_user\";"
    );
    // MySQL drops via DROP FOREIGN KEY, with backtick identifiers.
    assert_eq!(
        build_drop_fk_sql(DbKind::MySql, None, "orders", "fk_orders_user"),
        "ALTER TABLE `orders` DROP FOREIGN KEY `fk_orders_user`;"
    );
}

#[test]
fn truncate_table_dialects() {
    assert_eq!(
        build_truncate_table_sql(DbKind::Postgres, Some("public"), "orders"),
        "TRUNCATE TABLE \"public\".\"orders\";"
    );
    // SQLite has no TRUNCATE — it falls back to an unfiltered DELETE.
    assert_eq!(
        build_truncate_table_sql(DbKind::Sqlite, None, "orders"),
        "DELETE FROM \"orders\";"
    );
}

#[test]
fn clone_table_dialects() {
    // Postgres preserves structure (LIKE … INCLUDING ALL), then copies rows.
    assert_eq!(
        build_clone_table_sql(DbKind::Postgres, Some("public"), "orders", "orders_copy"),
        vec![
            "CREATE TABLE \"public\".\"orders_copy\" (LIKE \"public\".\"orders\" INCLUDING ALL);"
                .to_string(),
            "INSERT INTO \"public\".\"orders_copy\" SELECT * FROM \"public\".\"orders\";"
                .to_string(),
        ]
    );
    // MySQL uses CREATE TABLE … LIKE with backtick identifiers.
    assert_eq!(
        build_clone_table_sql(DbKind::MySql, None, "orders", "orders_copy"),
        vec![
            "CREATE TABLE `orders_copy` LIKE `orders`;".to_string(),
            "INSERT INTO `orders_copy` SELECT * FROM `orders`;".to_string(),
        ]
    );
    // SQLite copies columns + data only, in a single statement.
    assert_eq!(
        build_clone_table_sql(DbKind::Sqlite, None, "orders", "orders_copy"),
        vec!["CREATE TABLE \"orders_copy\" AS SELECT * FROM \"orders\";".to_string()]
    );
}

#[test]
fn server_filter_preserves_existing_where_order_and_page() {
    assert_eq!(
        with_where_predicate(
            "SELECT * FROM trades WHERE active = TRUE ORDER BY date DESC LIMIT 100 OFFSET 200;",
            "\"symbol\" = 'HEROHONDA'",
        ),
        Some(
            "SELECT * FROM trades WHERE (active = TRUE) AND (\"symbol\" = 'HEROHONDA') \
             ORDER BY date DESC LIMIT 100 OFFSET 200;"
                .to_string()
        )
    );
    assert_eq!(
        with_where_predicate("SELECT TOP 100 * FROM trades;", "[price] > 1200"),
        Some("SELECT TOP 100 * FROM trades WHERE [price] > 1200;".to_string())
    );
}
