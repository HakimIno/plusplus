-- App login: plusplus / plusplus (see README)
CREATE ROLE IF NOT EXISTS plusplus WITH PASSWORD = 'plusplus' AND LOGIN = true AND SUPERUSER = true;

-- Keyspace and sample data for testing plusplus with ScyllaDB.
-- Replication factor 1 is fine for local Docker.

CREATE KEYSPACE IF NOT EXISTS plusplus_demo
WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};

USE plusplus_demo;

CREATE TABLE IF NOT EXISTS users (
    id int PRIMARY KEY,
    name text,
    email text,
    active boolean,
    created timestamp,
    tags list<text>
);

CREATE INDEX IF NOT EXISTS users_email ON users (email);

INSERT INTO users (id, name, email, active, created, tags)
VALUES (1, 'Alice Chen', 'alice@example.com', true, '2026-01-15T10:00:00Z', ['admin', 'beta']);

INSERT INTO users (id, name, email, active, created, tags)
VALUES (2, 'Bob Martinez', 'bob@example.com', true, '2026-02-03T14:30:00Z', ['editor']);

INSERT INTO users (id, name, email, active, created, tags)
VALUES (3, 'Carol Nguyen', 'carol@example.com', false, '2026-03-20T09:15:00Z', ['viewer']);

CREATE TABLE IF NOT EXISTS orders (
    order_id uuid PRIMARY KEY,
    user_id int,
    product text,
    quantity int,
    total bigint,
    ordered_at timestamp
);

CREATE INDEX IF NOT EXISTS orders_user_id ON orders (user_id);

INSERT INTO orders (order_id, user_id, product, quantity, total, ordered_at)
VALUES (11111111-1111-1111-1111-111111111111, 1, 'Keyboard', 1, 8900, '2026-07-01T08:00:00Z');

INSERT INTO orders (order_id, user_id, product, quantity, total, ordered_at)
VALUES (22222222-2222-2222-2222-222222222222, 1, 'Monitor', 2, 15900, '2026-07-10T11:45:00Z');

INSERT INTO orders (order_id, user_id, product, quantity, total, ordered_at)
VALUES (33333333-3333-3333-3333-333333333333, 2, 'Mouse', 1, 2500, '2026-07-18T16:20:00Z');
