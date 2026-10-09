//! Connection URLs (`postgres://user:pass@host:5432/db?sslmode=require`) turned into a
//! connection draft. Used when the OS hands the app a database link and when one is pasted
//! into the connection form.
//!
//! A URL can come from any web page, so parsing only ever produces a *draft*: nothing here
//! connects, saves, or touches the keychain.

use super::{ConnectionConfig, DbKind, SafetyProfile, SslMode};

/// URL schemes the app registers with the operating system, matched case-insensitively.
pub const CONNECTION_URL_SCHEMES: &[&str] = &[
    "postgres",
    "postgresql",
    "mysql",
    "mariadb",
    "sqlserver",
    "mssql",
];

/// Links longer than this are rejected outright; no real connection URL comes close.
const MAX_URL_LEN: usize = 4096;

/// A connection draft parsed from a URL. The password is kept apart from the config, which
/// never holds secrets.
#[derive(Debug, Clone)]
pub struct ConnectionUrl {
    pub config: ConnectionConfig,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionUrlError {
    #[error("Not a connection URL.")]
    NotAUrl,
    #[error("The URL is too long.")]
    TooLong,
    #[error("Unsupported database scheme \"{0}\".")]
    UnsupportedScheme(String),
    #[error("The URL has an invalid port.")]
    InvalidPort,
    #[error("The URL is malformed.")]
    Malformed,
}

/// Whether `text` looks like a link to one of the supported databases.
pub fn is_connection_url(text: &str) -> bool {
    scheme_kind(text.trim()).is_some()
}

fn scheme_kind(url: &str) -> Option<(DbKind, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let kind = match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => DbKind::Postgres,
        "mysql" => DbKind::MySql,
        "mariadb" => DbKind::MariaDb,
        "sqlserver" | "mssql" => DbKind::SqlServer,
        _ => return None,
    };
    Some((kind, rest))
}

/// Parse `scheme://[user[:password]@]host[:port][/database][?key=value&…]`.
///
/// Recognised query keys: `sslmode` / `ssl-mode` / `ssl_mode`, `user`, `password`, and
/// `dbname` / `database`. SQL Server's `;key=value` style (`;database=x;user=y`) is accepted
/// after the host as well. Unknown keys are ignored.
pub fn parse_connection_url(url: &str) -> Result<ConnectionUrl, ConnectionUrlError> {
    let url = url.trim();
    if url.len() > MAX_URL_LEN {
        return Err(ConnectionUrlError::TooLong);
    }
    let Some((kind, rest)) = scheme_kind(url) else {
        return match url.split_once("://") {
            Some((scheme, _)) if !scheme.is_empty() => {
                Err(ConnectionUrlError::UnsupportedScheme(scheme.to_string()))
            }
            _ => Err(ConnectionUrlError::NotAUrl),
        };
    };
    let rest = rest.split('#').next().unwrap_or_default();

    let (rest, query) = match rest.split_once('?') {
        Some((rest, query)) => (rest, query),
        None => (rest, ""),
    };
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash + 1..]),
        None => (rest, ""),
    };
    // SQL Server JDBC-style properties ride on the authority: `host:1433;database=x`.
    let mut props = authority.split(';');
    let authority = props.next().unwrap_or_default();
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
        None => (None, authority),
    };

    let mut config = ConnectionConfig::new(kind);
    config.set_safety_profile(SafetyProfile::Development);
    let mut password = String::new();
    if let Some(userinfo) = userinfo {
        let (user, pass) = match userinfo.split_once(':') {
            Some((user, pass)) => (user, Some(pass)),
            None => (userinfo, None),
        };
        config.user = decode(user)?;
        if let Some(pass) = pass {
            password = decode(pass)?;
        }
    }

    let (host, port) = split_host_port(hostport)?;
    config.host = if host.is_empty() {
        "localhost".to_string()
    } else {
        decode(host)?
    };
    if let Some(port) = port {
        config.port = port;
    }
    config.database = decode(path.trim_end_matches('/'))?;

    let mut ssl_mode = None;
    let pairs = query
        .split('&')
        .chain(props)
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")));
    for (key, value) in pairs {
        let value = decode(value)?;
        match key.to_ascii_lowercase().replace('-', "_").as_str() {
            "sslmode" | "ssl_mode" => ssl_mode = parse_ssl_mode(&value),
            "user" | "username" => config.user = value,
            "password" => password = value,
            "dbname" | "database" | "databasename" => config.database = value,
            _ => {}
        }
    }
    // An explicit mode wins. Without one, a loopback server commonly has no TLS, so let it
    // fall back to plaintext; anything remote keeps the form's encrypted default.
    config.ssl_mode = ssl_mode.unwrap_or(if is_loopback(&config.host) {
        SslMode::Prefer
    } else {
        SslMode::Require
    });

    if [&config.host, &config.user, &config.database, &password]
        .iter()
        .any(|field| field.chars().any(char::is_control))
    {
        return Err(ConnectionUrlError::Malformed);
    }
    config.name = if config.database.is_empty() {
        config.host.clone()
    } else {
        format!("{} @ {}", config.database, config.host)
    };
    Ok(ConnectionUrl { config, password })
}

/// `host`, `host:port`, `[::1]` or `[::1]:port`.
fn split_host_port(hostport: &str) -> Result<(&str, Option<u16>), ConnectionUrlError> {
    let (host, port) = if let Some(bracketed) = hostport.strip_prefix('[') {
        let (host, after) = bracketed
            .split_once(']')
            .ok_or(ConnectionUrlError::Malformed)?;
        match after {
            "" => (host, None),
            _ => (
                host,
                Some(
                    after
                        .strip_prefix(':')
                        .ok_or(ConnectionUrlError::Malformed)?,
                ),
            ),
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (hostport, None),
        }
    };
    let port = match port {
        None | Some("") => None,
        Some(port) => Some(
            port.parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or(ConnectionUrlError::InvalidPort)?,
        ),
    };
    Ok((host, port))
}

fn parse_ssl_mode(value: &str) -> Option<SslMode> {
    Some(
        match value.to_ascii_lowercase().replace('-', "_").as_str() {
            "disable" | "disabled" | "false" => SslMode::Disable,
            "allow" | "prefer" | "preferred" => SslMode::Prefer,
            "require" | "required" | "true" => SslMode::Require,
            "verify_ca" => SslMode::VerifyCa,
            "verify_full" | "verify_identity" => SslMode::VerifyFull,
            _ => return None,
        },
    )
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

/// Percent-decode a URL component as UTF-8.
fn decode(text: &str) -> Result<String, ConnectionUrlError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or(ConnectionUrlError::Malformed)?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ConnectionUrlError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_postgres_url() {
        let parsed = parse_connection_url(
            "postgres://alice:s%40cret@db.example.com:6543/shop?sslmode=verify-full",
        )
        .unwrap();
        let config = &parsed.config;
        assert_eq!(config.kind, DbKind::Postgres);
        assert_eq!(config.user, "alice");
        assert_eq!(parsed.password, "s@cret");
        assert_eq!(config.host, "db.example.com");
        assert_eq!(config.port, 6543);
        assert_eq!(config.database, "shop");
        assert_eq!(config.ssl_mode, SslMode::VerifyFull);
        assert_eq!(config.name, "shop @ db.example.com");
        assert_eq!(config.safety_profile, SafetyProfile::Development);
    }

    #[test]
    fn fills_defaults_for_a_minimal_url() {
        let parsed = parse_connection_url("postgresql://localhost").unwrap();
        assert_eq!(parsed.config.port, 5432);
        assert_eq!(parsed.config.user, "");
        assert_eq!(parsed.password, "");
        assert_eq!(parsed.config.database, "");
        assert_eq!(parsed.config.ssl_mode, SslMode::Prefer);

        let remote = parse_connection_url("POSTGRES://user@db.internal/app").unwrap();
        assert_eq!(remote.config.ssl_mode, SslMode::Require);
        assert_eq!(remote.config.host, "db.internal");
    }

    #[test]
    fn maps_each_scheme_to_its_backend() {
        for (url, kind, port) in [
            ("mysql://root@127.0.0.1/app", DbKind::MySql, 3306),
            ("mariadb://root@127.0.0.1:3307/app", DbKind::MariaDb, 3307),
            ("sqlserver://sa@sql.local/master", DbKind::SqlServer, 1433),
            ("mssql://sa@sql.local", DbKind::SqlServer, 1433),
        ] {
            let parsed = parse_connection_url(url).unwrap();
            assert_eq!(parsed.config.kind, kind, "{url}");
            assert_eq!(parsed.config.port, port, "{url}");
        }
    }

    #[test]
    fn reads_ipv6_hosts_and_query_overrides() {
        let parsed =
            parse_connection_url("postgres://[::1]:5433/?user=bob&password=p%20w&dbname=analytics")
                .unwrap();
        assert_eq!(parsed.config.host, "::1");
        assert_eq!(parsed.config.port, 5433);
        assert_eq!(parsed.config.user, "bob");
        assert_eq!(parsed.password, "p w");
        assert_eq!(parsed.config.database, "analytics");
        assert_eq!(parsed.config.ssl_mode, SslMode::Prefer);
    }

    #[test]
    fn accepts_sql_server_semicolon_properties() {
        let parsed =
            parse_connection_url("sqlserver://sql.example.com:1444;database=Sales;user=report")
                .unwrap();
        assert_eq!(parsed.config.host, "sql.example.com");
        assert_eq!(parsed.config.port, 1444);
        assert_eq!(parsed.config.database, "Sales");
        assert_eq!(parsed.config.user, "report");
    }

    #[test]
    fn password_may_contain_colons_and_at_signs() {
        let parsed = parse_connection_url("mysql://me:a:b@c@host/db").unwrap();
        assert_eq!(parsed.config.user, "me");
        assert_eq!(parsed.password, "a:b@c");
        assert_eq!(parsed.config.host, "host");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            parse_connection_url("https://example.com").unwrap_err(),
            ConnectionUrlError::UnsupportedScheme("https".into())
        );
        assert_eq!(
            parse_connection_url("localhost:5432").unwrap_err(),
            ConnectionUrlError::NotAUrl
        );
        assert_eq!(
            parse_connection_url("postgres://host:99999/db").unwrap_err(),
            ConnectionUrlError::InvalidPort
        );
        assert_eq!(
            parse_connection_url("postgres://host:0/db").unwrap_err(),
            ConnectionUrlError::InvalidPort
        );
        assert_eq!(
            parse_connection_url("postgres://host/db%zz").unwrap_err(),
            ConnectionUrlError::Malformed
        );
        assert_eq!(
            parse_connection_url("postgres://host/db%0Adrop").unwrap_err(),
            ConnectionUrlError::Malformed
        );
        let long = format!("postgres://host/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(
            parse_connection_url(&long).unwrap_err(),
            ConnectionUrlError::TooLong
        );
    }

    #[test]
    fn recognises_connection_urls() {
        assert!(is_connection_url("  postgres://localhost "));
        assert!(is_connection_url("MSSQL://x"));
        assert!(!is_connection_url("localhost"));
        assert!(!is_connection_url("http://localhost"));
    }
}
