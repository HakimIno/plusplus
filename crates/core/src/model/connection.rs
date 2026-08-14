//! Persisted connection configuration and its supporting value types.

use super::DbKind;
use serde::{Deserialize, Serialize};

/// User-chosen glyph for a connection in the sidebar. Persisted with the connection config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionIcon {
    #[default]
    Database,
    Table,
    #[serde(alias = "code")]
    Cloud,
    #[serde(alias = "settings")]
    Storage,
    #[serde(alias = "connect")]
    Star,
    #[serde(alias = "key")]
    Treasure,
}

impl ConnectionIcon {
    pub const ALL: [ConnectionIcon; 6] = [
        ConnectionIcon::Database,
        ConnectionIcon::Table,
        ConnectionIcon::Cloud,
        ConnectionIcon::Storage,
        ConnectionIcon::Star,
        ConnectionIcon::Treasure,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConnectionIcon::Database => "Database",
            ConnectionIcon::Table => "Table",
            ConnectionIcon::Cloud => "Cloud",
            ConnectionIcon::Storage => "Local disk",
            ConnectionIcon::Star => "Favorite",
            ConnectionIcon::Treasure => "Treasure",
        }
    }
}

/// A named safety policy for a saved connection. `Custom` is the backward-compatible
/// default: connections saved before profiles existed keep their independent production
/// and read-only flags exactly as they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SafetyProfile {
    Development,
    Staging,
    Production,
    #[default]
    Custom,
}

impl SafetyProfile {
    pub const ALL: [SafetyProfile; 4] = [
        SafetyProfile::Development,
        SafetyProfile::Staging,
        SafetyProfile::Production,
        SafetyProfile::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SafetyProfile::Development => "Development",
            SafetyProfile::Staging => "Staging",
            SafetyProfile::Production => "Production",
            SafetyProfile::Custom => "Custom",
        }
    }

    /// Concise user-facing explanation of the protections this profile applies.
    pub fn description(self) -> &'static str {
        match self {
            SafetyProfile::Development => {
                "Writes are allowed without Production Guardian confirmation."
            }
            SafetyProfile::Staging => {
                "Writes are allowed; destructive changes require Production Guardian review."
            }
            SafetyProfile::Production => {
                "All writes are blocked and the database session is read-only where supported."
            }
            SafetyProfile::Custom => {
                "Configure Production Guardian and read-only protection independently."
            }
        }
    }
}

/// How strictly a server connection should use TLS, mirroring Postgres' `sslmode`
/// vocabulary so it translates cleanly to every backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SslMode {
    /// Never use TLS; fail if the server insists on it.
    Disable,
    /// Use TLS if the server supports it, fall back to plaintext otherwise.
    /// Matches the pre-TLS-config behavior, so it's the default for old configs.
    #[default]
    Prefer,
    /// Require TLS but don't verify the server certificate.
    Require,
    /// Require TLS and verify the certificate against a trusted CA.
    VerifyCa,
    /// Require TLS, verify the CA, and check the hostname matches the certificate.
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::VerifyCa => "Verify CA",
            SslMode::VerifyFull => "Verify Full",
        }
    }

    /// Does this mode validate the server certificate?
    pub fn verifies_certificate(self) -> bool {
        matches!(self, SslMode::VerifyCa | SslMode::VerifyFull)
    }

    /// A short security caveat to show beneath the SSL picker, or `None` for the modes that
    /// verify the server's identity (and so need no warning).
    pub fn security_warning(self) -> Option<&'static str> {
        match self {
            SslMode::Disable => {
                Some("Not encrypted — only use on your own machine or a fully trusted network.")
            }
            SslMode::Prefer => Some(
                "Falls back to plaintext when the server has no TLS, and can be forced down to \
                 plaintext by an attacker. Prefer Require or higher.",
            ),
            SslMode::Require => Some(
                "Encrypted, but the server's certificate isn't verified — still open to a \
                 man-in-the-middle. Use Verify Full for production.",
            ),
            SslMode::VerifyCa | SslMode::VerifyFull => None,
        }
    }
}

/// A saved connection. Secret fields (passwords) are **never** stored here — they live in
/// the OS keychain keyed by [`ConnectionConfig::id`]. Only non-secret fields are persisted
/// to the JSON config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Stable unique id, also used as the keychain account name.
    pub id: String,
    /// User-facing name for this connection.
    pub name: String,
    pub kind: DbKind,
    // --- server backends ---
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    /// TLS policy for server backends. Ignored by SQLite.
    #[serde(default)]
    pub ssl_mode: SslMode,
    /// Path to a PEM CA certificate used by the verify modes. Empty means the
    /// system trust store.
    #[serde(default)]
    pub ssl_ca_cert: String,
    /// Path to a PEM client certificate for mutual TLS. Empty means none.
    /// Only honoured by backends where [`DbKind::supports_client_cert`] is true.
    #[serde(default)]
    pub ssl_client_cert: String,
    /// Path to the PEM private key matching `ssl_client_cert`. Empty means none.
    #[serde(default)]
    pub ssl_client_key: String,
    // --- SSH tunnel (server backends) ---
    /// Reach the database through an SSH bastion instead of connecting directly.
    /// `host`/`port` above then name the database as seen *from the bastion*.
    #[serde(default)]
    pub ssh_enabled: bool,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// Path to a private key file for the bastion. Empty means password authentication.
    /// The key passphrase / SSH password lives in the keychain, never here.
    #[serde(default)]
    pub ssh_key_path: String,
    // --- file backends ---
    #[serde(default)]
    pub sqlite_path: String,
    /// Optional user-chosen title bar color for visually marking important connections.
    #[serde(default)]
    pub title_bar_color: Option<ConnectionColor>,
    /// Sidebar icon for this connection.
    #[serde(default)]
    pub icon: ConnectionIcon,
    /// Named policy controlling the safety flags below. `Custom` preserves the flags as-is;
    /// the managed profiles enforce their policy even if a config file is hand-edited.
    #[serde(default)]
    pub safety_profile: SafetyProfile,
    /// Marks a production database: destructive queries (UPDATE/DELETE/DROP/TRUNCATE/ALTER)
    /// must be confirmed in a dialog before they run.
    #[serde(default)]
    pub production: bool,
    /// Hard read-only mode: only provably read statements run (see
    /// [`crate::safety::write_statements`]), in-grid editing and DDL are refused, and the
    /// backends additionally pin the session read-only where the engine supports it
    /// (Postgres `default_transaction_read_only`, MySQL/MariaDB `SET SESSION TRANSACTION
    /// READ ONLY`, SQLite opened read-only; SQL Server has no session-level equivalent —
    /// `ApplicationIntent=ReadOnly` is sent but only enforced by readable replicas).
    #[serde(default)]
    pub read_only: bool,
}

impl ConnectionConfig {
    /// Create a new config with a freshly generated id and sane defaults for `kind`.
    pub fn new(kind: DbKind) -> Self {
        Self {
            id: generate_id(),
            name: format!("New {}", kind.label()),
            kind,
            host: "localhost".to_string(),
            port: kind.default_port(),
            user: String::new(),
            database: String::new(),
            // New connections default to Require: encrypted, with no silent fallback to
            // plaintext (which an attacker could force). Saved configs are left untouched —
            // a file missing `ssl_mode` still deserializes to Prefer (see SslMode's Default),
            // so upgrading the app never changes an existing connection's security.
            ssl_mode: SslMode::Require,
            ssl_ca_cert: String::new(),
            ssl_client_cert: String::new(),
            ssl_client_key: String::new(),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: default_ssh_port(),
            ssh_user: String::new(),
            ssh_key_path: String::new(),
            sqlite_path: String::new(),
            title_bar_color: None,
            icon: ConnectionIcon::default(),
            safety_profile: SafetyProfile::Custom,
            production: false,
            read_only: false,
        }
    }

    /// Select a named safety policy and immediately materialize its enforced flags.
    pub fn set_safety_profile(&mut self, profile: SafetyProfile) {
        self.safety_profile = profile;
        self.apply_safety_profile();
    }

    /// Apply the managed profile to the legacy flags used throughout the execution paths.
    /// `Custom` deliberately leaves both flags untouched.
    pub fn apply_safety_profile(&mut self) {
        match self.safety_profile {
            SafetyProfile::Development => {
                self.production = false;
                self.read_only = false;
            }
            SafetyProfile::Staging => {
                self.production = true;
                self.read_only = false;
            }
            SafetyProfile::Production => {
                self.production = true;
                self.read_only = true;
            }
            SafetyProfile::Custom => {}
        }
    }

    /// Effective Production Guardian state, including an unmaterialized managed profile.
    pub fn is_production(&self) -> bool {
        match self.safety_profile {
            SafetyProfile::Development => false,
            SafetyProfile::Staging | SafetyProfile::Production => true,
            SafetyProfile::Custom => self.production,
        }
    }

    /// Effective hard read-only state, including an unmaterialized managed profile.
    pub fn is_read_only(&self) -> bool {
        match self.safety_profile {
            SafetyProfile::Production => true,
            SafetyProfile::Development | SafetyProfile::Staging => false,
            SafetyProfile::Custom => self.read_only,
        }
    }

    /// A short subtitle describing the target, shown in the connection list.
    pub fn target_summary(&self) -> String {
        match self.kind {
            DbKind::Sqlite => self.sqlite_path.clone(),
            _ => format!(
                "{}@{}:{}/{}",
                self.user, self.host, self.port, self.database
            ),
        }
    }
}

/// Stored RGB color for per-connection UI markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ConnectionColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

fn default_ssh_port() -> u16 {
    22
}

/// Generate a process-unique, time-ordered id without pulling in a uuid dependency.
fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("conn-{nanos:x}-{n:x}")
}
