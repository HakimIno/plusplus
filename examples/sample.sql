-- Seed for examples/sample.sqlite — a small Thai e-commerce database, rich enough to
-- exercise every feature: foreign keys in all flavours (plain, self-referencing,
-- composite primary key, ON DELETE CASCADE / SET NULL), indexes, Thai text, dates,
-- and a few hundred rows of generated but deterministic order history.
--
-- Regenerate with:
--   rm -f examples/sample.sqlite && sqlite3 examples/sample.sqlite < examples/sample.sql

PRAGMA foreign_keys = ON;

-- ─── Tables ──────────────────────────────────────────────────────────────────

-- Product taxonomy; parent_id makes the ER diagram show a self-referencing loop.
CREATE TABLE categories (
  id        INTEGER PRIMARY KEY,
  name      TEXT NOT NULL UNIQUE,
  parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL
);

CREATE TABLE customers (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  email      TEXT NOT NULL UNIQUE,
  city       TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- A customer can have several addresses; deleting the customer removes them.
CREATE TABLE addresses (
  id          INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  label       TEXT NOT NULL DEFAULT 'บ้าน',
  street      TEXT NOT NULL,
  district    TEXT,
  province    TEXT NOT NULL,
  postal_code TEXT
);

CREATE TABLE products (
  id          INTEGER PRIMARY KEY,
  category_id INTEGER NOT NULL REFERENCES categories(id),
  sku         TEXT NOT NULL UNIQUE,
  name        TEXT NOT NULL,
  price       REAL NOT NULL,
  stock       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE orders (
  id              INTEGER PRIMARY KEY,
  customer_id     INTEGER NOT NULL REFERENCES customers(id),
  ship_address_id INTEGER REFERENCES addresses(id) ON DELETE SET NULL,
  status          TEXT NOT NULL DEFAULT 'pending',
  ordered_at      TEXT NOT NULL,
  total           REAL NOT NULL DEFAULT 0
);

-- Order lines: composite primary key, two foreign keys.
CREATE TABLE order_items (
  order_id   INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  product_id INTEGER NOT NULL REFERENCES products(id),
  qty        INTEGER NOT NULL CHECK (qty > 0),
  unit_price REAL NOT NULL,
  PRIMARY KEY (order_id, product_id)
);

CREATE INDEX idx_addresses_customer   ON addresses(customer_id);
CREATE INDEX idx_products_category    ON products(category_id);
CREATE INDEX idx_orders_customer      ON orders(customer_id);
CREATE INDEX idx_orders_ordered_at    ON orders(ordered_at);
CREATE INDEX idx_order_items_product  ON order_items(product_id);

-- ─── Reference data ──────────────────────────────────────────────────────────

INSERT INTO categories (id, name, parent_id) VALUES
  (1, 'อิเล็กทรอนิกส์', NULL),
  (2, 'เครื่องใช้ในบ้าน', NULL),
  (3, 'แฟชั่น',          NULL),
  (4, 'สมาร์ทโฟน',       1),
  (5, 'โน้ตบุ๊ก',         1),
  (6, 'หูฟัง',           1),
  (7, 'เครื่องครัว',      2),
  (8, 'เสื้อผ้าผู้ชาย',    3);

INSERT INTO customers (id, name, email, city, created_at) VALUES
  (1,  'สมชาย ใจดี',      'somchai@example.com',   'กรุงเทพฯ',   '2025-01-12 09:15:00'),
  (2,  'สมหญิง รักเรียน',  'somying@example.com',   'เชียงใหม่',   '2025-01-20 14:02:00'),
  (3,  'วิชัย พาณิชย์',     'wichai@example.com',    'ขอนแก่น',    '2025-02-03 11:40:00'),
  (4,  'มาลี ศรีสุข',      'malee@example.com',     'ภูเก็ต',      '2025-02-14 16:25:00'),
  (5,  'ประเสริฐ มั่นคง',   'prasert@example.com',   'กรุงเทพฯ',   '2025-03-01 08:55:00'),
  (6,  'นภา แสงทอง',      'napa@example.com',      'นนทบุรี',     '2025-03-18 19:30:00'),
  (7,  'อนันต์ วงศ์ใหญ่',   'anan@example.com',      'ชลบุรี',      '2025-04-02 10:05:00'),
  (8,  'รัตนา ทองแท้',     'rattana@example.com',   'เชียงราย',    '2025-04-21 13:45:00'),
  (9,  'กิตติ เกียรติยศ',   'kitti@example.com',     'กรุงเทพฯ',   '2025-05-06 09:00:00'),
  (10, 'พรทิพย์ จันทร์เพ็ญ','porntip@example.com',   'สงขลา',      '2025-05-19 17:20:00'),
  (11, 'ธนา เศรษฐี',       'tana@example.com',      'ระยอง',      '2025-06-08 12:10:00'),
  (12, 'อรุณี ฟ้าใส',      'arunee@example.com',    'อุดรธานี',    '2025-06-25 15:35:00');

INSERT INTO addresses (id, customer_id, label, street, district, province, postal_code) VALUES
  (1,  1,  'บ้าน',     '99/1 ถ.สุขุมวิท 71',        'วัฒนา',        'กรุงเทพมหานคร', '10110'),
  (2,  1,  'ที่ทำงาน',  '128 อาคารพญาไทพลาซ่า',     'ราชเทวี',      'กรุงเทพมหานคร', '10400'),
  (3,  2,  'บ้าน',     '45/8 ถ.นิมมานเหมินท์',      'เมือง',        'เชียงใหม่',     '50200'),
  (4,  3,  'บ้าน',     '212 ถ.มิตรภาพ',            'เมือง',        'ขอนแก่น',      '40000'),
  (5,  4,  'บ้าน',     '7 ถ.ราษฎร์อุทิศ 200 ปี',     'ป่าตอง',       'ภูเก็ต',        '83150'),
  (6,  4,  'คอนโด',    '88/123 ถ.เจ้าฟ้าตะวันออก',   'เมือง',        'ภูเก็ต',        '83000'),
  (7,  5,  'บ้าน',     '14 ซ.ลาดพร้าว 101',         'บางกะปิ',      'กรุงเทพมหานคร', '10240'),
  (8,  6,  'บ้าน',     '300/45 ถ.งามวงศ์วาน',       'เมือง',        'นนทบุรี',       '11000'),
  (9,  7,  'บ้าน',     '55 ถ.พัทยาเหนือ',           'บางละมุง',     'ชลบุรี',        '20150'),
  (10, 8,  'บ้าน',     '23/4 ถ.พหลโยธิน',           'เมือง',        'เชียงราย',      '57000'),
  (11, 9,  'ที่ทำงาน',  '989 อาคารสยามพิวรรธน์',     'ปทุมวัน',      'กรุงเทพมหานคร', '10330'),
  (12, 9,  'บ้าน',     '11/2 ซ.อารีย์ 5',           'พญาไท',       'กรุงเทพมหานคร', '10400'),
  (13, 10, 'บ้าน',     '64 ถ.นิพัทธ์อุทิศ 3',        'หาดใหญ่',      'สงขลา',        '90110'),
  (14, 11, 'บ้าน',     '150 ถ.สุขุมวิท',            'เมือง',        'ระยอง',        '21000'),
  (15, 12, 'บ้าน',     '39/7 ถ.โพศรี',             'เมือง',        'อุดรธานี',      '41000'),
  (16, 6,  'ที่ทำงาน',  '120 ถ.แจ้งวัฒนะ',           'ปากเกร็ด',     'นนทบุรี',       '11120');

INSERT INTO products (id, category_id, sku, name, price, stock) VALUES
  (1,  4, 'PH-001', 'สมาร์ทโฟน Galaxy A55 5G',        12990.0, 42),
  (2,  4, 'PH-002', 'iPhone 15 128GB',                 28900.0, 18),
  (3,  4, 'PH-003', 'สมาร์ทโฟน Redmi Note 13',          6990.0, 65),
  (4,  5, 'NB-001', 'โน้ตบุ๊ก ThinkPad X1 Carbon',      62900.0,  7),
  (5,  5, 'NB-002', 'MacBook Air M3 13"',              39900.0, 12),
  (6,  5, 'NB-003', 'โน้ตบุ๊ก ASUS Vivobook 15',        17990.0, 25),
  (7,  6, 'AU-001', 'หูฟังไร้สาย AirPods Pro 2',         8990.0, 33),
  (8,  6, 'AU-002', 'หูฟังครอบหู Sony WH-1000XM5',      11990.0, 14),
  (9,  6, 'AU-003', 'หูฟังเกมมิ่ง HyperX Cloud III',      3290.0, 27),
  (10, 7, 'KT-001', 'หม้อทอดไร้น้ำมัน 5.5 ลิตร',         2590.0, 48),
  (11, 7, 'KT-002', 'เครื่องปั่นอเนกประสงค์',            1290.0, 56),
  (12, 7, 'KT-003', 'กระทะเหล็กหล่อ 26 ซม.',            890.0,  73),
  (13, 7, 'KT-004', 'หม้อหุงข้าวดิจิทัล 1.8 ลิตร',        1990.0, 31),
  (14, 8, 'MW-001', 'เสื้อยืดคอกลม Cotton 100%',          290.0, 200),
  (15, 8, 'MW-002', 'กางเกงยีนส์ทรงกระบอก',             1190.0,  85),
  (16, 8, 'MW-003', 'เสื้อเชิ้ตลินินแขนยาว',              790.0,  60),
  (17, 1, 'EL-001', 'สายชาร์จ USB-C 100W 2 ม.',          390.0, 150),
  (18, 1, 'EL-002', 'พาวเวอร์แบงก์ 20000mAh',            990.0,  92),
  (19, 2, 'HM-001', 'เครื่องฟอกอากาศ 32 ตร.ม.',          4990.0,  21),
  (20, 2, 'HM-002', 'พัดลมตั้งพื้น DC 18 นิ้ว',           2290.0,  37);

-- ─── Generated order history (deterministic — no random()) ──────────────────
--
-- 120 orders spread over the first half of 2026, each assigned to a customer and
-- one of that customer's own addresses by simple modular arithmetic.

INSERT INTO orders (id, customer_id, ship_address_id, status, ordered_at, total)
WITH RECURSIVE seq(i) AS (
  SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 120
),
ord(i, cust) AS (
  SELECT i, (i * 7) % 12 + 1 FROM seq
)
SELECT
  i,
  cust,
  -- One of the customer's own addresses; alternating orders flip between them.
  CASE WHEN i % 2 = 0
    THEN (SELECT MIN(a.id) FROM addresses a WHERE a.customer_id = cust)
    ELSE (SELECT MAX(a.id) FROM addresses a WHERE a.customer_id = cust)
  END,
  CASE i % 10
    WHEN 0 THEN 'cancelled'
    WHEN 1 THEN 'pending'
    WHEN 2 THEN 'paid'
    WHEN 3 THEN 'paid'
    ELSE 'delivered'
  END,
  datetime('2026-01-01 08:00:00', '+' || ((i * 37) % 160) || ' days',
                                   '+' || ((i * 53) % 720) || ' minutes'),
  0
FROM ord;

-- 1–3 lines per order; the line set and quantities derive from the order id.
INSERT INTO order_items (order_id, product_id, qty, unit_price)
WITH RECURSIVE line(o, n) AS (
  SELECT id, 1 FROM orders
  UNION ALL
  SELECT o, n + 1 FROM line WHERE n < 1 + (o % 3)
)
SELECT
  o,
  (o * 13 + n * 7) % 20 + 1,
  1 + (o + n) % 4,
  (SELECT price FROM products WHERE id = (o * 13 + n * 7) % 20 + 1)
FROM line;

-- Totals come from the lines, like a real shop would compute them.
UPDATE orders SET total = (
  SELECT ROUND(SUM(qty * unit_price), 2) FROM order_items WHERE order_id = orders.id
);

ANALYZE;
