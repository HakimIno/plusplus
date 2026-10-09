//! Database links handed to the app by the operating system (`postgres://…` clicked in a
//! browser, or passed on the command line).
//!
//! The OS can deliver a link before the first frame, when no egui context exists yet, so links
//! wait in a process-wide inbox that every frame drains.
use super::*;
use std::sync::{Mutex, OnceLock};

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

/// Queue a connection URL for the app to open. Safe to call from any thread, before or after
/// the window exists.
pub fn open_connection_url(url: String) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.push(url);
    }
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

impl DbGuiApp {
    pub(super) fn drain_connection_urls(&mut self, ctx: &egui::Context) {
        let _ = CONTEXT.set(ctx.clone());
        let urls = PENDING
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        for url in urls {
            self.apply_action(Action::OpenConnectionUrl(url));
        }
    }

    /// Open a link as a connection draft. A page can link to any host, so nothing connects
    /// until the user confirms in the form; a link to a connection that is already saved
    /// simply opens that connection.
    pub(super) fn open_connection_url(&mut self, url: &str) {
        let parsed = match dbcore::parse_connection_url(url) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error = Some(format!("Could not open the link: {error}"));
                return;
            }
        };
        if self.show_welcome {
            self.show_welcome = false;
            self.persist_settings();
        }
        self.settings_open = false;
        if let Some(index) = self.saved_connection_matching(&parsed.config) {
            self.editor = None;
            self.apply_action(Action::Connect(index));
            return;
        }
        self.editor = Some(ConnEditor {
            config: parsed.config,
            password: parsed.password,
            ssh_password: String::new(),
            is_new: true,
            edit_index: None,
            test_state: ConnTestState::Untested,
            selecting_provider: false,
            show_advanced: false,
        });
        self.status_msg = "Review the connection, then connect".to_string();
    }

    pub(super) fn saved_connection_matching(&self, config: &ConnectionConfig) -> Option<usize> {
        let family = |kind: DbKind| match kind {
            DbKind::MariaDb => DbKind::MySql,
            kind => kind,
        };
        self.connections.iter().position(|saved| {
            family(saved.kind) == family(config.kind)
                && saved.host.eq_ignore_ascii_case(&config.host)
                && saved.port == config.port
                && saved.user == config.user
                && saved.database == config.database
        })
    }
}

impl ConnEditor {
    /// Pasting a `postgres://…` link into the Host field fills the form from it. The draft's
    /// identity, safety settings and look stay; only the target and credentials change.
    pub(super) fn apply_connection_url(&mut self, parsed: dbcore::ConnectionUrl) {
        let url = parsed.config;
        if self.config.name == format!("New {}", self.config.kind.label()) {
            self.config.name = url.name;
        }
        self.config.kind = url.kind;
        self.config.host = url.host;
        self.config.port = url.port;
        self.config.user = url.user;
        self.config.database = url.database;
        self.config.ssl_mode = url.ssl_mode;
        if !parsed.password.is_empty() {
            self.password = parsed.password;
        }
    }
}
