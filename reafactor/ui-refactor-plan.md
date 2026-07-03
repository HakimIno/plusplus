# UI Refactor Plan — Component Library + Code Cleanup

เป้าหมาย: รวม UI ทุกชิ้น (button, input, dialog, tab, badge ฯลฯ) ให้เป็น **component กลาง**
ที่ใช้ซ้ำได้ทั้งแอป — แก้ที่เดียว เปลี่ยนทั้งแอป — พร้อมลด comment ให้เหลือเท่าที่จำเป็น

---

## 1. สภาพปัจจุบัน (audit)

โค้ด UI อยู่ที่ `crates/ui/src/` (~19,300 บรรทัด) ตอนนี้ component กระจายอยู่ **3 ที่** และมีสไตล์เขียนปนกัน 3 แบบ:

| ที่อยู่ปัจจุบัน | มีอะไร | ปัญหา |
|---|---|---|
| `style.rs` (709 บรรทัด) | palette tokens, `text_input`, `accent_checkbox`, `accent_radio`, `segmented`, `dialog_*`, `empty_state`, `type_badge` ฯลฯ | ปน design tokens กับ widgets ในไฟล์เดียว |
| `icons.rs` (315 บรรทัด) | icon set **แต่มีปุ่มปนอยู่**: `button`, `primary_button`, `toggle_button`, `icon_button`, `db_kind_combo` | ปุ่มไม่ควรอยู่ในโมดูล icons — หาไม่เจอ, ownership ไม่ชัด |
| `app/widgets.rs` (712 บรรทัด) | `toolbar_icon_button`, `layout_toggle`, `query_tab_item`, `connection_tab_item`, `beautify_button`, `toolbar_sep` | เป็น `pub(super)` → ไฟล์นอก `app/` (schema.rs, grid.rs, filter.rs) **ใช้ไม่ได้** ต้องเขียนเองซ้ำ |

ปัญหาที่ตามมา:

- **ปุ่มมี 3 สายพันธุ์**: `ui.button("…")` เปล่าๆ (~30 จุด), `icons::button/primary_button`,
  และปุ่มวาดมือด้วย `allocate_exact_size` + painter — หน้าตา/ความสูง/hover ไม่เท่ากัน
- **inline styling ซ้ำ**: `RichText::new(x).size(11.0).color(palette::…)` กระจาย ~75 จุด
  (panels.rs 63, grid.rs 11) — เปลี่ยน type scale ทีต้องไล่แก้ทุกจุด
- **if-chain สี hover/selected ซ้ำ**: pattern `if selected {…} else if hovered {…} else {…}`
  สำหรับ fill/stroke/text ถูก copy ซ้ำใน widgets.rs, title_bar.rs, panels.rs
- **magic numbers**: corner radius มีทั้ง 3/4/5/6/8/10, icon size 12/13/14/16, padding จิปาถะ
- **ไฟล์ยักษ์**: `app/panels.rs` 5,253 บรรทัด, `app.rs` 4,864 บรรทัด
- **comment หนาเกิน**: app.rs 605 บรรทัด comment (~12%), edit.rs ~20%, grid.rs ~18% —
  ส่วนใหญ่เป็นย่อหน้าเล่าเรื่องยาวๆ

---

## 2. โครงสร้างเป้าหมาย

```
crates/ui/src/
├── style.rs            # เหลือเฉพาะ design tokens: palette, spacing, radius, type scale, apply()
├── icons.rs            # เหลือเฉพาะ icon assets + show_* helpers (ปุ่มย้ายออกหมด)
├── components/         # ★ ใหม่ — component library ใช้ได้ทั้ง crate
│   ├── mod.rs          # re-export ทุกอย่าง → call site เขียนแค่ `components::…`
│   ├── button.rs       # Button builder ตัวเดียว ครอบทุก variant
│   ├── input.rs        # text_input, icon_text_input, accent_checkbox, accent_radio, segmented
│   ├── dialog.rs       # dialog_window, dialog_frame, dialog_footer, confirm dialog helper
│   ├── tabs.rs         # query_tab_item, connection_tab_item (+ tab chip painter ร่วม)
│   ├── toolbar.rs      # toolbar_icon_button, layout_toggle, toolbar_sep, beautify (split button)
│   ├── badge.rs        # type_badge, status_dot, section_header, truncated_label
│   └── feedback.rs     # spinner, loading_state, empty_state, empty_illustration
└── app/
    └── (เหลือเฉพาะ logic + layout ของแต่ละ panel, ไม่มี widget painting)
```

### หัวใจ: Button เดียว จบทุกเคส

แทนที่ปุ่ม 3 สายพันธุ์ด้วย builder ตัวเดียวใน `components/button.rs`:

```rust
pub enum ButtonVariant {
    Primary,   // accent fill — action หลักของหน้าจอ (Run, Save)
    Default,   // surface fill + hairline — action ทั่วไป
    Ghost,     // โปร่ง, hover ค่อยมีพื้น — toolbar / icon-only
    Danger,    // โทน DANGER — Delete, Drop
}

pub struct Btn<'a> { /* label, icon, variant, enabled, active, min_width, tooltip */ }

impl<'a> Btn<'a> {
    pub fn new(label: impl Into<String>) -> Self;      // Default variant
    pub fn primary(label: …) -> Self;
    pub fn ghost_icon(src: ImageSource<'static>) -> Self;
    pub fn danger(label: …) -> Self;
    pub fn icon(self, src: ImageSource<'static>) -> Self;
    pub fn enabled(self, on: bool) -> Self;
    pub fn active(self, on: bool) -> Self;              // สำหรับ toggle
    pub fn tooltip(self, text: &'a str) -> Self;
    pub fn show(self, ui: &mut egui::Ui) -> egui::Response;
}
```

การใช้งาน — จุดเดียวกำหนดหน้าตา ทุก call site สั้นลง:

```rust
// ก่อน
if icons::primary_button(ui, icons::connect(), "Run", can_act).clicked() { … }
ui.add_enabled(false, egui::Button::new("Downloading…"));

// หลัง
if Btn::primary("Run").icon(icons::connect()).enabled(can_act).show(ui).clicked() { … }
Btn::new("Downloading…").enabled(false).show(ui);
```

### helper ฆ่า if-chain ที่ซ้ำ

เพิ่มใน `components/mod.rs` ตัวเดียว แล้วให้ tab/chip/row ทุกตัวเรียกใช้:

```rust
pub struct InteractionColors { pub fill: Color32, pub stroke: Stroke, pub text: Color32 }
pub fn interaction_colors(resp: &Response, selected: bool, dragging: bool) -> InteractionColors
```

### design tokens เพิ่มใน `style.rs`

```rust
pub mod radius { pub const SM: u8 = 4; pub const MD: u8 = 6; pub const LG: u8 = 8; pub const WINDOW: u8 = 10; }
pub mod space  { pub const XS: f32 = 2.0; pub const SM: f32 = 4.0; pub const MD: f32 = 8.0; pub const LG: f32 = 12.0; }
pub mod font   { pub const CAPTION: f32 = 10.5; pub const BODY: f32 = 12.5; pub const TITLE: f32 = 14.5; }
```

ไล่แทน magic numbers ตอน migrate call site (ไม่ต้องทำเป็น pass แยก)

---

## 3. นโยบาย comment (ทำไปพร้อมทุก phase)

- **doc comment (`///`)**: public item ละ **1 บรรทัด** บอกว่า "คืออะไร/ใช้เมื่อไหร่" — พอ
- **inline comment (`//`)**: เก็บไว้เฉพาะข้อจำกัดที่อ่านโค้ดแล้วไม่รู้ เช่น workaround ของ egui
  (เคส `Area` vs `scope_builder` ใน drag-reorder, debug overlay ใน style.rs) — ตัดให้เหลือ 1–2 บรรทัด
- **ลบทิ้ง**: comment ที่เล่าว่าโค้ดบรรทัดถัดไปทำอะไร, เล่าประวัติว่าทำไมถึงแก้, ย่อหน้าอธิบายดีไซน์ยาวๆ
- เป้า: ลด comment รวมจาก ~1,500 → **ต่ำกว่า 500 บรรทัด** ทั้ง crate

---

## 4. ลำดับงาน (แต่ละ phase จบแล้ว build + test ผ่าน, commit แยก)

### Phase 1 — ตั้งโครง `components/` (ย้ายของเดิม ยังไม่เปลี่ยนหน้าตา)
1. สร้าง `components/` ตามโครงด้านบน
2. ย้าย widgets จาก `style.rs` → `components/{input,dialog,badge,feedback}.rs`
   (ใน `style.rs` เหลือ tokens + `apply()` + `visuals()`)
3. ย้ายปุ่มทั้งหมดออกจาก `icons.rs` → `components/button.rs` (ของเดิมคงพฤติกรรม)
4. ย้าย `app/widgets.rs` → `components/{tabs,toolbar}.rs` เปลี่ยน `pub(super)` → `pub(crate)`
5. ใส่ re-export ใน `components/mod.rs` + แก้ import ทุก call site
6. ตัด comment ตามนโยบายในไฟล์ที่ย้าย

**เช็ค:** `cargo build -p plusplus-ui && cargo test -p plusplus-ui` — UI ต้องหน้าตาเหมือนเดิม 100%

### Phase 2 — สร้าง `Btn` builder + tokens แล้วยุบปุ่มเก่า
1. เขียน `Btn` (Primary/Default/Ghost/Danger + icon/active/enabled/tooltip) ให้ครอบพฤติกรรมของ
   `icons::button`, `primary_button`, `toggle_button`, `icon_button`, `update_outline_button`, `toolbar_icon_button`
2. เพิ่ม `radius`/`space`/`font` tokens + `interaction_colors()`
3. migrate ปุ่มที่มี helper อยู่แล้วมาใช้ `Btn` แล้วลบ helper เก่าทิ้ง (ไม่เก็บ 2 ทาง)

### Phase 3 — กวาด call site ที่เขียนเอง (ไฟล์ละ commit)
ลำดับจากง่ายไปยาก:
1. `update.rs`, `title_bar.rs`, `filter.rs` — ปุ่ม/ชิปเล็กน้อย
2. `grid.rs`, `edit.rs`, `schema.rs` — RichText inline + ปุ่มใน context menu
3. `app/panels.rs` — จุดใหญ่สุด: dialogs (`ui.button("Browse…")`, Clear, Close ฯลฯ),
   welcome page, pager, status bar → ใช้ `Btn` + tokens ทั้งหมด
4. `app.rs` — ที่เหลือ
- ระหว่างกวาด: `RichText::new(x).size(…)` ที่ซ้ำบ่อย → เพิ่ม helper `components::text::{caption, weak, danger}(…)` แล้วใช้แทน

### Phase 4 — ผ่าไฟล์ยักษ์ (ทำหลัง component นิ่งแล้ว)
- `app/panels.rs` (5,253) → แตกเป็น `app/{dialogs,editor_panel,sidebar,status_bar,welcome}.rs`
  ตามกลุ่ม fn ที่มีอยู่แล้ว (ดู fn list: dialogs อยู่ช่วง 2280–3500, schema editor 3406–ท้ายไฟล์)
- `app.rs` (4,864) → แยก state/Action handling ออกจาก layout ถ้ายังใหญ่
- เป้า: ไม่มีไฟล์ UI เกิน ~1,500 บรรทัด

### Phase 5 — เก็บกวาดรอบสุดท้าย
- ไล่ comment ทั้ง crate ให้เข้านโยบาย (ไฟล์ที่ยังไม่โดนแตะใน phase ก่อน: autocomplete.rs, erd.rs, ghost.rs, highlight.rs ฯลฯ)
- `cargo clippy -p plusplus-ui -- -D warnings` + ลบ `#[allow(dead_code)]` ที่ไม่จำเป็นแล้ว
- อัปเดต `crates/ui/src/lib.rs` doc ให้สะท้อนโครงใหม่

---

## 5. การตรวจสอบแต่ละ phase

1. `cargo build -p plusplus-ui` และ `cargo test --workspace`
2. โปรเจกต์มี `egui_kittest` (snapshot PNG) อยู่แล้ว — รัน snapshot test เทียบภาพก่อน/หลัง
   ทุก phase ที่ตั้งใจ **ไม่เปลี่ยนหน้าตา** (Phase 1–4) ภาพต้องตรงเดิม
3. เปิดแอปจริงไล่ดู: title bar, tabs, connection dialog, settings, danger confirm, welcome page

## 6. กติกาถาวรหลัง refactor (กันถอยหลัง)

- ห้ามเรียก `egui::Button::new` / `ui.button` ตรงๆ ใน `app/`, `panels`, `schema`, `grid` — ใช้ `components::Btn` เท่านั้น
- ห้าม hard-code สี/radius/font size — ใช้ `palette` / `radius` / `space` / `font` tokens
- widget ใหม่ที่ใช้เกิน 1 ที่ → ต้องอยู่ใน `components/`
- comment: public item 1 บรรทัด, inline เฉพาะ workaround
- เพิ่มกติกานี้ลง `CLAUDE.md` ของ repo เพื่อให้ทุก session ทำตาม
