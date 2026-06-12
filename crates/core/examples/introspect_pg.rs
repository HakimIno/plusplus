//! Throwaway diagnostic: introspect a Postgres database and dump what the app sees.
//! Usage: cargo run -p plusplus-core --example introspect_pg -- <port> <database> [user]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().expect("port").parse().expect("numeric port");
    let database = args.next().expect("database");
    let user = args.next().unwrap_or_else(|| "postgres".into());

    let mut cfg = plusplus_core::ConnectionConfig::new(plusplus_core::DbKind::Postgres);
    cfg.host = "127.0.0.1".into();
    cfg.port = port;
    cfg.user = user;
    cfg.database = database;
    cfg.ssl_mode = plusplus_core::SslMode::Disable;

    let db = plusplus_core::connect(&cfg, None, None).await.expect("connect");
    let schema = db.introspect().await.expect("introspect");
    println!("database: {}", schema.database_name);
    for t in &schema.tables {
        println!(
            "  {} ({} cols, {} idx, {} fks)",
            t.name,
            t.columns.len(),
            t.indexes.len(),
            t.foreign_keys.len()
        );
        for fk in &t.foreign_keys {
            println!(
                "    fk {}: {} [delete {} / update {}]",
                fk.name,
                fk.display(),
                fk.on_delete,
                fk.on_update
            );
        }
    }
}
