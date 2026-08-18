use std::hint::black_box;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use plusplus_core::backends::duckdb::DuckDb;
use plusplus_core::import::preview;
use plusplus_core::model::{parse_page_window, with_keyset_page, with_page_window};
use plusplus_core::safety::{dangerous_statements, write_statements};
use plusplus_core::{ConnectionConfig, Database, DbKind, ImportFormat, Value};

fn deterministic_sql_batch(statements: usize) -> String {
    (0..statements)
        .map(|index| match index % 4 {
            0 => format!("SELECT id, name FROM users WHERE tenant_id = {index};"),
            1 => format!("UPDATE users SET active = false WHERE id = {index};"),
            2 => format!("DELETE FROM sessions WHERE user_id = {index};"),
            _ => format!("SELECT count(*) FROM orders WHERE customer_id = {index};"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn deterministic_json_fixture(rows: usize) -> (PathBuf, String) {
    let path = std::env::temp_dir().join(format!(
        "plusplus-benchmark-{}-{rows}.json",
        std::process::id()
    ));
    let mut json = String::with_capacity(rows.saturating_mul(64));
    json.push('[');
    for index in 0..rows {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "{{\"id\":{index},\"name\":\"customer-{index:08}\",\"active\":true}}"
        ));
    }
    json.push(']');
    std::fs::write(&path, json.as_bytes()).expect("write benchmark fixture");
    (path, json)
}

fn bench_safety_analysis(c: &mut Criterion) {
    let mut group = c.benchmark_group("safety_analysis");
    for statements in [1usize, 10, 100] {
        let sql = deterministic_sql_batch(statements);
        group.throughput(Throughput::Elements(statements as u64));
        group.bench_with_input(
            BenchmarkId::new("dangerous_statements", statements),
            &sql,
            |b, sql| {
                b.iter(|| dangerous_statements(DbKind::Postgres, black_box(sql)));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("read_only_policy", statements),
            &sql,
            |b, sql| {
                b.iter(|| write_statements(black_box(sql)));
            },
        );
    }
    group.finish();
}

fn bench_page_rewrite(c: &mut Criterion) {
    let sql = "SELECT * FROM analytics.orders WHERE status = 'paid' ORDER BY id LIMIT 512 OFFSET 1000000;";
    let mut group = c.benchmark_group("page_sql");
    for kind in [
        DbKind::Postgres,
        DbKind::MySql,
        DbKind::SqlServer,
        DbKind::Sqlite,
        DbKind::DuckDb,
    ] {
        group.bench_with_input(
            BenchmarkId::new("parse_and_rewrite", kind.label()),
            &kind,
            |b, kind| {
                b.iter(|| {
                    let page = parse_page_window(black_box(sql)).expect("simple table query");
                    with_page_window(
                        *kind,
                        black_box(sql),
                        page.limit.unwrap_or(512),
                        page.offset,
                    )
                });
            },
        );
    }
    let key_columns = vec!["tenant_id".to_string(), "id".to_string()];
    let cursor = vec![Value::Int(42), Value::Int(1_000_000)];
    group.bench_function("composite_keyset_cursor", |b| {
        b.iter(|| {
            with_keyset_page(
                DbKind::Postgres,
                black_box("SELECT * FROM analytics.orders WHERE status = 'paid' LIMIT 1000000"),
                black_box(&key_columns),
                Some(black_box(&cursor)),
                512,
            )
        });
    });
    group.finish();
}

fn bench_duckdb_analytics(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("benchmark runtime");
    let db = DuckDb::connect(&ConnectionConfig::new(DbKind::DuckDb)).expect("in-memory DuckDB");
    runtime
        .block_on(db.execute_capped(
            "CREATE TABLE facts AS \
             SELECT i AS id, i % 1000 AS customer_id, i * 0.01 AS amount \
             FROM range(1000000) AS source(i)",
            1,
        ))
        .expect("build deterministic fact table");
    let parquet_path = std::env::temp_dir().join(format!(
        "plusplus-benchmark-duckdb-{}.parquet",
        std::process::id()
    ));
    let sql_path = parquet_path.to_string_lossy().replace('\'', "''");
    runtime
        .block_on(db.execute_capped(&format!("COPY facts TO '{sql_path}' (FORMAT PARQUET)"), 1))
        .expect("write deterministic Parquet fixture");
    let mut group = c.benchmark_group("duckdb_analytics");
    group.throughput(Throughput::Elements(1_000_000));
    group.bench_function("aggregate_1m_in_memory", |b| {
        b.iter(|| {
            runtime
                .block_on(db.execute_capped(
                    black_box("SELECT customer_id, sum(amount) FROM facts GROUP BY customer_id"),
                    1_000,
                ))
                .expect("DuckDB aggregate")
        });
    });
    let parquet_sql = format!(
        "SELECT customer_id, sum(amount) FROM read_parquet('{sql_path}') GROUP BY customer_id"
    );
    group.bench_function("aggregate_1m_parquet", |b| {
        b.iter(|| {
            runtime
                .block_on(db.execute_capped(black_box(&parquet_sql), 1_000))
                .expect("DuckDB Parquet aggregate")
        });
    });
    group.finish();
    std::fs::remove_file(parquet_path).ok();
}

fn bench_value_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_processing");
    for rows in [1_000usize, 10_000, 100_000] {
        let values = (0..rows)
            .rev()
            .map(|index| Value::Text(format!("row-{index:08}")))
            .collect::<Vec<_>>();
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("sort_text", rows), &values, |b, values| {
            b.iter_batched(
                || values.clone(),
                |mut values| values.sort_by(Value::sort_cmp),
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_json_preview(c: &mut Criterion) {
    let (path, fixture) = deterministic_json_fixture(100_000);
    let bytes = fixture.len() as u64;
    let mut group = c.benchmark_group("json_import");
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function("preview_50_of_100k", |b| {
        b.iter(|| preview(black_box(&path), ImportFormat::Json, true, 50));
    });
    group.finish();
    std::fs::remove_file(path).ok();
}

criterion_group!(
    benches,
    bench_safety_analysis,
    bench_page_rewrite,
    bench_duckdb_analytics,
    bench_value_sort,
    bench_json_preview
);
criterion_main!(benches);
