//! SSH tunnel: reach a database through a bastion host. [`SshTunnel::open`] connects and
//! authenticates to the bastion, binds a listener on a random localhost port, and forwards
//! every TCP connection made to it through a `direct-tcpip` channel to the real database
//! host. The backend driver then simply connects to `127.0.0.1:{local_port}`.

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::error::{CoreError, Result};
use crate::model::ConnectionConfig;

/// Accepts whatever host key the bastion presents, like the DB backends' non-verify SSL
/// modes. Checking against `known_hosts` is a possible follow-up; the tunnel's purpose
/// here is reachability and the DB connection inside it carries its own TLS policy.
struct AcceptingHandler;

impl russh::client::Handler for AcceptingHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
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
        if cfg.ssh_host.trim().is_empty() || cfg.ssh_user.trim().is_empty() {
            return Err(CoreError::InvalidConfig(
                "SSH tunnel needs a host and a user".into(),
            ));
        }

        let config = Arc::new(russh::client::Config::default());
        let mut session = russh::client::connect(
            config,
            (cfg.ssh_host.trim().to_string(), cfg.ssh_port),
            AcceptingHandler,
        )
        .await
        .map_err(ssh_err)?;

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

        let tunnel = SshTunnel::open(&cfg, Some("hunter2"))
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
    }

    #[tokio::test]
    async fn tunnel_rejects_bad_credentials_and_config() {
        let echo_port = spawn_echo().await;
        let ssh_port = spawn_bastion().await;
        let cfg = tunnel_cfg(echo_port, ssh_port);

        let err = SshTunnel::open(&cfg, Some("wrong-password")).await;
        assert!(matches!(err, Err(CoreError::Ssh(_))), "bad password must fail");

        let mut blank = cfg.clone();
        blank.ssh_host.clear();
        let err = SshTunnel::open(&blank, Some("hunter2")).await;
        assert!(matches!(err, Err(CoreError::InvalidConfig(_))));
    }
}
