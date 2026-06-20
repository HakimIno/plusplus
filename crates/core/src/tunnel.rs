//! SSH tunnel: reach a database through a bastion host. [`SshTunnel::open`] connects and
//! authenticates to the bastion, binds a listener on a random localhost port, and forwards
//! every TCP connection made to it through a `direct-tcpip` channel to the real database
//! host. The backend driver then simply connects to `127.0.0.1:{local_port}`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

use crate::error::{CoreError, Result};
use crate::model::ConnectionConfig;

/// The user's standard OpenSSH known_hosts (`~/.ssh/known_hosts`), if a home directory is
/// resolvable. Read-only — verified against, never modified — so a bastion the user already
/// trusts via the `ssh` CLI is honoured here too.
fn user_known_hosts() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

/// plusplus's own known_hosts (`<config-dir>/known_hosts`). New bastion host keys are
/// recorded here on first use, so we never touch the user's `~/.ssh/known_hosts`.
fn plusplus_known_hosts() -> Result<PathBuf> {
    Ok(crate::config::config_dir()?.join("known_hosts"))
}

/// The result of checking a presented host key against the known_hosts files.
enum HostKeyCheck {
    /// The key matches a recorded entry — trust it.
    Match,
    /// No entry exists for this host yet — first contact.
    Unknown,
    /// An entry exists for this host but the key differs — refuse (the message names the
    /// offending file/line so the user can act).
    Mismatch(String),
}

/// Check `pubkey` against each known_hosts file in `paths`, in order. A matching entry wins;
/// a *conflicting* entry (same host, different key) fails closed as a possible MITM; if no
/// file knows the host at all, it's [`HostKeyCheck::Unknown`].
fn check_host_key(
    host: &str,
    port: u16,
    pubkey: &russh::keys::PublicKey,
    paths: &[PathBuf],
) -> HostKeyCheck {
    for path in paths {
        match russh::keys::check_known_hosts_path(host, port, pubkey, path) {
            Ok(true) => return HostKeyCheck::Match,
            Ok(false) => continue, // this file doesn't know the host; try the next
            Err(russh::keys::Error::KeyChanged { line }) => {
                return HostKeyCheck::Mismatch(format!(
                    "the SSH bastion's host key does not match the key recorded in {} (line {line}). \
                     This can mean a man-in-the-middle attack — or the host key was legitimately \
                     rotated. If you trust the new key, remove that line and reconnect.",
                    path.display()
                ));
            }
            Err(e) => {
                // Any other error means we couldn't establish trust — refuse rather than
                // fall through to "accept".
                return HostKeyCheck::Mismatch(format!(
                    "could not verify the SSH bastion's host key against {}: {e}",
                    path.display()
                ));
            }
        }
    }
    HostKeyCheck::Unknown
}

/// Verifies the bastion's host key against known_hosts (trust-on-first-use). A key that
/// matches is accepted; a *changed* key is refused as a possible MITM; an unseen host is
/// recorded and then trusted, mirroring OpenSSH's `StrictHostKeyChecking=accept-new`.
struct VerifyingHandler {
    host: String,
    port: u16,
    /// known_hosts files to verify against, in order (user's first, then plusplus-managed).
    known_hosts: Vec<PathBuf>,
    /// File an unseen host key is recorded into (the plusplus-managed known_hosts).
    learn_path: PathBuf,
    /// Set to a precise reason when a key is refused, so [`SshTunnel::open`] can surface it
    /// instead of the generic disconnect russh reports for a rejected key.
    rejection: Arc<Mutex<Option<String>>>,
}

impl VerifyingHandler {
    /// Record `reason` for the caller and reject the key (`Ok(false)` tells russh to refuse).
    fn reject(&self, reason: String) -> std::result::Result<bool, russh::Error> {
        if let Ok(mut slot) = self.rejection.lock() {
            *slot = Some(reason);
        }
        Ok(false)
    }
}

impl russh::client::Handler for VerifyingHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        match check_host_key(&self.host, self.port, server_public_key, &self.known_hosts) {
            HostKeyCheck::Match => Ok(true),
            HostKeyCheck::Unknown => {
                // Trust on first use: record the key so a *later* change is caught as a
                // mismatch. If we can't persist it, refuse rather than trust unverifiably.
                match russh::keys::known_hosts::learn_known_hosts_path(
                    &self.host,
                    self.port,
                    server_public_key,
                    &self.learn_path,
                ) {
                    Ok(()) => Ok(true),
                    Err(e) => self.reject(format!(
                        "could not record the SSH bastion's host key in {}: {e}",
                        self.learn_path.display()
                    )),
                }
            }
            HostKeyCheck::Mismatch(reason) => self.reject(reason),
        }
    }
}

fn ssh_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Ssh(e.to_string())
}

/// A live tunnel. Dropping it tears everything down: the accept loop is aborted, which
/// drops the SSH session handle, closing the bastion connection and every forward in it.
pub struct SshTunnel {
    /// The localhost port the database driver should connect to instead of the real host.
    pub local_port: u16,
    accept_task: tokio::task::JoinHandle<()>,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

impl SshTunnel {
    /// Connect to the bastion in `cfg` and start forwarding to `cfg.host:cfg.port`.
    ///
    /// `secret` is the SSH credential fetched from the keychain by the caller: the
    /// passphrase when `ssh_key_path` is set (empty/None for an unencrypted key), the
    /// SSH password otherwise.
    pub async fn open(cfg: &ConnectionConfig, secret: Option<&str>) -> Result<Self> {
        // Verify the bastion's host key against the user's ~/.ssh/known_hosts and a
        // plusplus-managed known_hosts, recording an unseen host on first use (TOFU).
        let mut known_hosts = Vec::new();
        if let Some(user) = user_known_hosts() {
            known_hosts.push(user);
        }
        let learn_path = plusplus_known_hosts()?;
        known_hosts.push(learn_path.clone());
        Self::open_verified(cfg, secret, known_hosts, learn_path).await
    }

    /// Like [`SshTunnel::open`], but with explicit known_hosts files (verified in order) and
    /// the file an unseen host key is recorded into. Lets tests point at a temp known_hosts
    /// instead of the user's real one.
    async fn open_verified(
        cfg: &ConnectionConfig,
        secret: Option<&str>,
        known_hosts: Vec<PathBuf>,
        learn_path: PathBuf,
    ) -> Result<Self> {
        if cfg.ssh_host.trim().is_empty() || cfg.ssh_user.trim().is_empty() {
            return Err(CoreError::InvalidConfig(
                "SSH tunnel needs a host and a user".into(),
            ));
        }

        let config = Arc::new(russh::client::Config::default());
        let rejection = Arc::new(Mutex::new(None));
        let handler = VerifyingHandler {
            host: cfg.ssh_host.trim().to_string(),
            port: cfg.ssh_port,
            known_hosts,
            learn_path,
            rejection: rejection.clone(),
        };
        let mut session = match russh::client::connect(
            config,
            (cfg.ssh_host.trim().to_string(), cfg.ssh_port),
            handler,
        )
        .await
        {
            Ok(session) => session,
            Err(e) => {
                // Prefer the precise host-key reason the handler recorded, if any: russh
                // collapses a rejected key into a generic disconnect otherwise.
                if let Some(reason) = rejection.lock().ok().and_then(|mut r| r.take()) {
                    return Err(CoreError::Ssh(reason));
                }
                return Err(ssh_err(e));
            }
        };

        let user = cfg.ssh_user.trim().to_string();
        let key_path = cfg.ssh_key_path.trim();
        let auth = if !key_path.is_empty() {
            let passphrase = secret.filter(|s| !s.is_empty());
            let key = russh::keys::load_secret_key(key_path, passphrase)
                .map_err(|e| CoreError::Ssh(format!("could not load SSH key: {e}")))?;
            let hash_alg = session
                .best_supported_rsa_hash()
                .await
                .map_err(ssh_err)?
                .flatten();
            session
                .authenticate_publickey(
                    user,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(ssh_err)?
        } else {
            session
                .authenticate_password(user, secret.unwrap_or(""))
                .await
                .map_err(ssh_err)?
        };
        if !auth.success() {
            return Err(CoreError::Ssh(
                "SSH authentication failed (check user, password, or key)".into(),
            ));
        }

        // Port 0 lets the OS pick a free port; only this process knows it, and the
        // listener is bound to loopback so nothing remote can reach the forward.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(ssh_err)?;
        let local_port = listener.local_addr().map_err(ssh_err)?.port();

        let target_host = cfg.host.trim().to_string();
        let target_port = cfg.port;
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((mut tcp, peer)) = listener.accept().await else {
                    break;
                };
                // One SSH channel per pooled DB connection, multiplexed over the session.
                match session
                    .channel_open_direct_tcpip(
                        target_host.clone(),
                        target_port as u32,
                        peer.ip().to_string(),
                        peer.port() as u32,
                    )
                    .await
                {
                    Ok(channel) => {
                        tokio::spawn(async move {
                            let mut stream = channel.into_stream();
                            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                        });
                    }
                    // The session died (bastion dropped us); new connections can't be
                    // forwarded, so stop accepting. Live forwards fail on their own.
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            local_port,
            accept_task,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::server::{self, Auth, Msg, Session};
    use russh::Channel;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// A throwaway known_hosts path under the temp dir, unique per call, so host-key
    /// verification in tests never reads or writes the user's real `~/.ssh/known_hosts`.
    fn temp_known_hosts() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plusplus-knownhosts-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A deterministic Ed25519 public key from a one-byte seed. Seed 7 matches the host key
    /// [`spawn_bastion`] presents; any other seed stands in for a different (rogue) key.
    fn host_pubkey(seed: u8) -> russh::keys::PublicKey {
        use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
        let mut pk = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[seed; 32])),
            "test",
        )
        .unwrap()
        .public_key()
        .clone();
        // Keys presented over the wire and parsed back from known_hosts carry no comment;
        // clear it so equality compares key material only (as it does in real use).
        pk.set_comment("");
        pk
    }

    /// A miniature in-process bastion: password auth (`user` / `hunter2`) and real
    /// direct-tcpip forwarding, the two things [`SshTunnel`] relies on from an sshd.
    struct TestBastion;

    impl server::Handler for TestBastion {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> std::result::Result<Auth, Self::Error> {
            if user == "user" && password == "hunter2" {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            _session: &mut Session,
        ) -> std::result::Result<bool, Self::Error> {
            let target = (host_to_connect.to_string(), port_to_connect as u16);
            tokio::spawn(async move {
                if let Ok(mut tcp) = TcpStream::connect(target).await {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                }
            });
            Ok(true)
        }
    }

    /// Start the test bastion on a random port; a fixed-seed Ed25519 host key keeps the
    /// test free of any RNG dependency.
    async fn spawn_bastion() -> u16 {
        use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
        let host_key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[7u8; 32])),
            "test-bastion",
        )
        .unwrap();
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            ..Default::default()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, socket, TestBastion).await {
                        let _ = session.await;
                    }
                });
            }
        });
        port
    }

    /// A TCP echo server standing in for the database behind the bastion.
    async fn spawn_echo() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    fn tunnel_cfg(echo_port: u16, ssh_port: u16) -> ConnectionConfig {
        let mut cfg = ConnectionConfig::new(crate::model::DbKind::Postgres);
        cfg.host = "127.0.0.1".into();
        cfg.port = echo_port;
        cfg.ssh_enabled = true;
        cfg.ssh_host = "127.0.0.1".into();
        cfg.ssh_port = ssh_port;
        cfg.ssh_user = "user".into();
        cfg
    }

    #[tokio::test]
    async fn tunnel_forwards_bytes_end_to_end() {
        let echo_port = spawn_echo().await;
        let ssh_port = spawn_bastion().await;
        let cfg = tunnel_cfg(echo_port, ssh_port);

        // Unknown bastion → trust-on-first-use records its key into the temp known_hosts.
        let kh = temp_known_hosts();
        let tunnel = SshTunnel::open_verified(&cfg, Some("hunter2"), vec![kh.clone()], kh.clone())
            .await
            .expect("tunnel should open");

        // Two concurrent connections, mirroring a small pool: each gets its own channel.
        for round in 0..2u8 {
            let mut conn = TcpStream::connect(("127.0.0.1", tunnel.local_port))
                .await
                .unwrap();
            let msg = format!("ping {round} through the bastion");
            conn.write_all(msg.as_bytes()).await.unwrap();
            let mut buf = vec![0u8; msg.len()];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, msg.as_bytes());
        }
        let _ = std::fs::remove_file(&kh);
    }

    #[tokio::test]
    async fn tunnel_rejects_bad_credentials_and_config() {
        let echo_port = spawn_echo().await;
        let ssh_port = spawn_bastion().await;
        let cfg = tunnel_cfg(echo_port, ssh_port);

        let kh = temp_known_hosts();
        let err =
            SshTunnel::open_verified(&cfg, Some("wrong-password"), vec![kh.clone()], kh.clone())
                .await;
        assert!(matches!(err, Err(CoreError::Ssh(_))), "bad password must fail");

        let mut blank = cfg.clone();
        blank.ssh_host.clear();
        let err =
            SshTunnel::open_verified(&blank, Some("hunter2"), vec![kh.clone()], kh.clone()).await;
        assert!(matches!(err, Err(CoreError::InvalidConfig(_))));
        let _ = std::fs::remove_file(&kh);
    }

    /// A bastion whose presented host key conflicts with the one already recorded must be
    /// refused as a possible man-in-the-middle, before authentication is even attempted.
    #[tokio::test]
    async fn tunnel_rejects_changed_host_key() {
        let echo_port = spawn_echo().await;
        let ssh_port = spawn_bastion().await;
        let cfg = tunnel_cfg(echo_port, ssh_port);

        // Pre-record a *different* key for this bastion's host:port (a rogue/changed key).
        let kh = temp_known_hosts();
        russh::keys::known_hosts::learn_known_hosts_path("127.0.0.1", ssh_port, &host_pubkey(9), &kh)
            .unwrap();

        let opened =
            SshTunnel::open_verified(&cfg, Some("hunter2"), vec![kh.clone()], kh.clone()).await;
        let reason = opened.as_ref().err().map(|e| e.to_string());
        assert!(
            matches!(&opened, Err(CoreError::Ssh(msg)) if msg.contains("does not match")),
            "a changed host key must be refused, got {reason:?}"
        );
        let _ = std::fs::remove_file(&kh);
    }

    /// The known_hosts decision logic: unseen → Unknown, recorded same key → Match,
    /// recorded different key → Mismatch, different host → Unknown. No network needed.
    #[test]
    fn check_host_key_classifies_against_known_hosts() {
        let kh = temp_known_hosts();
        let paths = [kh.clone()];

        // A missing/empty known_hosts knows no host.
        assert!(matches!(
            check_host_key("bastion.example", 2222, &host_pubkey(7), &paths),
            HostKeyCheck::Unknown
        ));

        // Record the key; now the same key matches and a different one conflicts.
        russh::keys::known_hosts::learn_known_hosts_path("bastion.example", 2222, &host_pubkey(7), &kh)
            .unwrap();
        assert!(matches!(
            check_host_key("bastion.example", 2222, &host_pubkey(7), &paths),
            HostKeyCheck::Match
        ));
        assert!(matches!(
            check_host_key("bastion.example", 2222, &host_pubkey(9), &paths),
            HostKeyCheck::Mismatch(_)
        ));

        // A host we've never recorded is still Unknown, even with a key on file for another.
        assert!(matches!(
            check_host_key("other.example", 2222, &host_pubkey(7), &paths),
            HostKeyCheck::Unknown
        ));
        let _ = std::fs::remove_file(&kh);
    }
}
