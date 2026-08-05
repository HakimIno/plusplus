# ScyllaDB local test environment

Docker Compose stack for testing [plusplus](https://github.com/HakimIno/plusplus) against ScyllaDB.

## Start

```bash
cd examples/scylladb
docker compose up -d
```

The first boot can take 1–2 minutes. The `init` service loads sample data once ScyllaDB is healthy:

```bash
docker compose logs -f init
```

When you see the init container exit with code 0, the database is ready.

## Connect from plusplus

| Field | Value |
| --- | --- |
| Type | ScyllaDB |
| Host | `127.0.0.1` |
| Port | `9042` |
| User | `plusplus` |
| Password | `plusplus` |
| Keyspace | `plusplus_demo` |
| SSL | Disable |

The demo login is created by `init.cql`. Edit the role there if you want different app
credentials, then recreate the stack with `docker compose down -v && docker compose up -d`.

The init container signs in as the default superuser (`cassandra` / `cassandra`) once to create
the `plusplus` role and seed data. You can override the admin login through
`SCYLLA_ADMIN_USER` / `SCYLLA_ADMIN_PASSWORD` in your shell or a local `.env` file.

## Sample data

**Keyspace:** `plusplus_demo`

**Table `users`** — 3 rows with text, boolean, timestamp, and list columns.

**Table `orders`** — 3 rows with uuid, int, bigint, and timestamp columns.

Example queries:

```sql
SELECT * FROM users;
SELECT * FROM orders WHERE user_id = 1 ALLOW FILTERING;
```

## Run the live backend smoke test

```bash
PLUSPLUS_LIVE_KIND=scylladb \
PLUSPLUS_LIVE_HOST=127.0.0.1 \
PLUSPLUS_LIVE_PORT=9042 \
PLUSPLUS_LIVE_USER=plusplus \
PLUSPLUS_LIVE_PASSWORD=plusplus \
PLUSPLUS_LIVE_DATABASE=plusplus_demo \
PLUSPLUS_LIVE_CONNECT_ATTEMPTS=30 \
cargo test -p plusplus-core --test live_backends cql_connect_query_mutate_and_introspect -- --ignored --nocapture
```

## Stop / reset

```bash
docker compose down        # keep data volume
docker compose down -v     # wipe data and re-seed on next up
```

Works with Docker or Podman (`podman compose up`).

## Troubleshooting

### `Connect timeout elapsed` / `No connections in the pool`

ScyllaDB in Docker advertises its internal container IP (e.g. `10.x.x.x`). Clients on your Mac must connect via `127.0.0.1:9042`. plusplus handles this automatically for localhost connections; rebuild or rerun from source after pulling the fix.

Also check:

- Container is running: `podman ps --filter name=plusplus-scylla`
- **SSL** is set to **Disable** (local dev has no TLS)
- User/password: `plusplus` / `plusplus`
- Keyspace: `plusplus_demo`

### `no space left on device` (Podman on macOS)

Podman runs containers inside a Linux VM with its own disk (often 20 GB). Old images can fill it even when your Mac still has free space.

Check VM disk usage:

```bash
podman machine ssh df -h /sysroot
```

Free space by pruning unused images and volumes:

```bash
podman container prune -f
podman image prune -a -f
podman volume prune -f
```

If you still run out of room, resize the VM (requires a stop/start):

```bash
podman machine stop
podman machine set --disk-size 100 podman-machine-default
podman machine start
```
