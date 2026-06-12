//! ER diagram state and layout — no egui drawing here; rendering is in panels.rs.
//!
//! The diagram is a snapshot built from an introspected [`SchemaTree`]: one node per
//! table (its columns, with PK/FK markers) and one edge per foreign key. Node positions
//! come from a small force-directed layout run once at build time; the user can then
//! drag nodes freely. Nothing here persists — closing the diagram discards the layout.

use std::collections::BTreeMap;

use dbcore::SchemaTree;
use egui::{pos2, vec2, Pos2, Rect, Vec2};

/// One column row inside a node.
pub struct ErdColumn {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
    /// Participates in at least one outgoing foreign key.
    pub foreign_key: bool,
}

/// One table box on the canvas.
pub struct ErdNode {
    /// Display name: schema-qualified only when the database spans several schemas.
    pub title: String,
    pub columns: Vec<ErdColumn>,
    /// Top-left corner, in scene coordinates.
    pub pos: Pos2,
    /// Measured at first render from real font metrics; `Vec2::ZERO` until then.
    /// The layout uses [`ErdNode::estimated_size`] instead, which needs no fonts.
    pub size: Vec2,
}

impl ErdNode {
    /// Font-free size estimate used by the layout (and until the first frame measures
    /// the real galleys): a fixed-ish width plus one row of height per column.
    pub fn estimated_size(&self) -> Vec2 {
        let chars = self
            .columns
            .iter()
            .map(|c| c.name.len() + c.data_type.len())
            .chain(std::iter::once(self.title.len()))
            .max()
            .unwrap_or(10) as f32;
        let width = (chars * 7.5 + 48.0).clamp(170.0, 320.0);
        let height = HEADER_H + self.columns.len() as f32 * ROW_H + 8.0;
        vec2(width, height)
    }

    /// The box rect at its current position, preferring the measured size.
    pub fn rect(&self) -> Rect {
        let size = if self.size == Vec2::ZERO {
            self.estimated_size()
        } else {
            self.size
        };
        Rect::from_min_size(self.pos, size)
    }
}

/// Node header band height, in scene points.
pub const HEADER_H: f32 = 28.0;
/// Height of one column row, in scene points.
pub const ROW_H: f32 = 19.0;

/// One foreign key, drawn as a curve from the referencing column row to the
/// referenced table, with crow's-foot cardinality marks at both ends.
pub struct ErdEdge {
    pub from: usize,
    /// Row index (into `nodes[from].columns`) of the FK's first column.
    pub from_row: usize,
    pub to: usize,
    /// Row of the first referenced column in the target, when it resolved.
    pub to_row: Option<usize>,
    /// Many rows may share one parent (crow's foot at the FK end). `false` when the FK
    /// columns carry a unique constraint, which makes the relation one-to-one.
    pub many: bool,
    /// Any FK column is nullable, so a row may reference no parent at all
    /// (zero-or-one at the referenced end instead of exactly-one).
    pub optional: bool,
    /// Tooltip text: constraint, column pairs, actions, and cardinality.
    pub detail: String,
}

pub struct ErDiagram {
    /// Saved-connection id the snapshot was built from (shown alongside the database).
    pub conn_id: String,
    pub database: String,
    pub nodes: Vec<ErdNode>,
    pub edges: Vec<ErdEdge>,
    /// Pan/zoom state for `egui::Scene`. An empty rect makes the scene auto-fit the
    /// content on the next frame, so this doubles as the "zoom to fit" request.
    pub scene_rect: Rect,
    /// Node whose edges are highlighted (clicked). `None` = no highlight.
    pub selected: Option<usize>,
}

impl ErDiagram {
    /// Snapshot `schema` into nodes and edges and run the initial layout.
    pub fn build(conn_id: &str, schema: &SchemaTree) -> Self {
        // Qualify titles only when tables span more than one schema/namespace.
        let mut namespaces: Vec<&str> = schema
            .tables
            .iter()
            .map(|t| t.schema.as_deref().unwrap_or(""))
            .collect();
        namespaces.sort_unstable();
        namespaces.dedup();
        let qualify = namespaces.len() > 1;

        let title_of = |schema_name: Option<&str>, table: &str| -> String {
            match schema_name {
                Some(s) if qualify => format!("{s}.{table}"),
                _ => table.to_string(),
            }
        };

        // Tables keyed by (schema, name) — and by bare name for backends/FKs that don't
        // qualify the referenced table — so edge targets resolve in one lookup.
        let mut by_key: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
        for (i, t) in schema.tables.iter().enumerate() {
            by_key.insert(
                (t.schema.clone().unwrap_or_default(), t.name.clone()),
                i,
            );
            by_name.entry(t.name.clone()).or_insert(i);
        }

        let nodes: Vec<ErdNode> = schema
            .tables
            .iter()
            .map(|t| ErdNode {
                title: title_of(t.schema.as_deref(), &t.name),
                columns: t
                    .columns
                    .iter()
                    .map(|c| ErdColumn {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                        primary_key: c.primary_key,
                        foreign_key: t
                            .foreign_keys
                            .iter()
                            .any(|fk| fk.columns.iter().any(|fc| fc == &c.name)),
                    })
                    .collect(),
                pos: Pos2::ZERO,
                size: Vec2::ZERO,
            })
            .collect();

        let mut edges = Vec::new();
        for (i, t) in schema.tables.iter().enumerate() {
            for fk in &t.foreign_keys {
                // Prefer the schema-qualified target (same schema when unqualified);
                // fall back to the first table with that bare name.
                let target_schema = fk
                    .ref_schema
                    .clone()
                    .or_else(|| t.schema.clone())
                    .unwrap_or_default();
                let Some(&to) = by_key
                    .get(&(target_schema, fk.ref_table.clone()))
                    .or_else(|| by_name.get(&fk.ref_table))
                else {
                    continue; // referenced table not in the snapshot (filtered/system)
                };
                let from_row = fk
                    .columns
                    .first()
                    .and_then(|c| t.columns.iter().position(|tc| &tc.name == c))
                    .unwrap_or(0);
                let to_row = fk.ref_columns.first().and_then(|c| {
                    schema.tables[to].columns.iter().position(|tc| &tc.name == c)
                });

                // Cardinality, derived from the referencing side's constraints: a unique
                // FK column set means at most one child per parent (1:1), and a nullable
                // FK column means the parent is optional (0..1 instead of exactly 1).
                let mut fk_cols: Vec<&str> = fk.columns.iter().map(|s| s.as_str()).collect();
                fk_cols.sort_unstable();
                let mut pk_cols: Vec<&str> = t
                    .columns
                    .iter()
                    .filter(|c| c.primary_key)
                    .map(|c| c.name.as_str())
                    .collect();
                pk_cols.sort_unstable();
                let unique = (!pk_cols.is_empty() && pk_cols == fk_cols)
                    || t.indexes.iter().any(|ix| {
                        let mut cols: Vec<&str> =
                            ix.columns.iter().map(|s| s.as_str()).collect();
                        cols.sort_unstable();
                        ix.unique && cols == fk_cols
                    });
                let optional = fk.columns.iter().any(|fc| {
                    t.columns.iter().any(|tc| &tc.name == fc && tc.nullable)
                });

                let name = if fk.name.is_empty() {
                    String::new()
                } else {
                    format!("{} · ", fk.name)
                };
                edges.push(ErdEdge {
                    from: i,
                    from_row,
                    to,
                    to_row,
                    many: !unique,
                    optional,
                    detail: format!(
                        "{name}{} · on delete {} · on update {} · {}",
                        fk.display(),
                        fk.on_delete,
                        fk.on_update,
                        match (unique, optional) {
                            (true, true) => "one-to-one (optional)",
                            (true, false) => "one-to-one",
                            (false, true) => "many-to-one (optional)",
                            (false, false) => "many-to-one",
                        }
                    ),
                });
            }
        }

        let mut diagram = Self {
            conn_id: conn_id.to_string(),
            database: schema.database_name.clone(),
            nodes,
            edges,
            scene_rect: Rect::NOTHING,
            selected: None,
        };
        diagram.layout();
        diagram
    }

    /// Ask the scene to zoom-to-fit the content on its next frame.
    pub fn request_fit(&mut self) {
        self.scene_rect = Rect::NOTHING;
    }

    /// (Re)compute node positions: a deterministic grid seed refined by a few hundred
    /// Fruchterman–Reingold iterations — repulsion between all boxes, attraction along
    /// edges — then normalized so the content starts at the origin.
    pub fn layout(&mut self) {
        let n = self.nodes.len();
        if n == 0 {
            return;
        }

        let sizes: Vec<Vec2> = self.nodes.iter().map(|nd| nd.estimated_size()).collect();

        // Seed: row-major grid, deterministic (no RNG anywhere in the layout).
        let cols = (n as f32).sqrt().ceil() as usize;
        let cell = sizes
            .iter()
            .fold(Vec2::ZERO, |acc, s| acc.max(*s))
            + vec2(120.0, 100.0);
        let mut centers: Vec<Pos2> = (0..n)
            .map(|i| {
                pos2(
                    (i % cols) as f32 * cell.x,
                    (i / cols) as f32 * cell.y,
                )
            })
            .collect();

        // Ideal edge length scales with box size so big tables get breathing room.
        let k = (cell.x.max(cell.y)) * 0.9;
        let mut temperature = cell.x * (cols as f32) * 0.25;
        const ITERATIONS: usize = 250;

        for _ in 0..ITERATIONS {
            let mut disp = vec![Vec2::ZERO; n];

            for i in 0..n {
                for j in (i + 1)..n {
                    let mut delta = centers[i] - centers[j];
                    if delta == Vec2::ZERO {
                        // Coincident centers (same grid cell can't happen, but identical
                        // drag positions can): nudge apart deterministically.
                        delta = vec2(0.01 * (i as f32 + 1.0), 0.01);
                    }
                    let dist = delta.length().max(1.0);
                    let repulse = (k * k) / dist;
                    let push = delta / dist * repulse;
                    disp[i] += push;
                    disp[j] -= push;
                }
            }

            for e in &self.edges {
                if e.from == e.to {
                    continue; // self-references don't pull
                }
                let delta = centers[e.from] - centers[e.to];
                let dist = delta.length().max(1.0);
                let attract = (dist * dist) / k;
                let pull = delta / dist * attract;
                disp[e.from] -= pull;
                disp[e.to] += pull;
            }

            for i in 0..n {
                let d = disp[i];
                let len = d.length();
                if len > 0.0 {
                    centers[i] += d / len * len.min(temperature);
                }
            }
            temperature *= 0.96;
        }

        // Convert centers to top-left positions and shift everything to start at the
        // origin (the scene auto-fits, but keeping coordinates small avoids drift).
        let mut min = pos2(f32::INFINITY, f32::INFINITY);
        for (c, s) in centers.iter().zip(&sizes) {
            min = min.min(*c - *s / 2.0);
        }
        for ((node, c), s) in self.nodes.iter_mut().zip(&centers).zip(&sizes) {
            node.pos = (*c - *s / 2.0 - min.to_vec2() + vec2(40.0, 40.0)).round();
        }

        self.request_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ColumnInfo, ForeignKeyInfo, SchemaTree, TableInfo};

    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "INTEGER".into(),
            nullable: !pk,
            primary_key: pk,
        }
    }

    fn fk(cols: &[&str], ref_table: &str, ref_cols: &[&str]) -> ForeignKeyInfo {
        ForeignKeyInfo {
            name: format!("fk_{ref_table}"),
            columns: cols.iter().map(|s| s.to_string()).collect(),
            ref_schema: None,
            ref_table: ref_table.into(),
            ref_columns: ref_cols.iter().map(|s| s.to_string()).collect(),
            on_delete: "CASCADE".into(),
            on_update: "NO ACTION".into(),
        }
    }

    fn table(name: &str, columns: Vec<ColumnInfo>, fks: Vec<ForeignKeyInfo>) -> TableInfo {
        TableInfo {
            schema: None,
            name: name.into(),
            columns,
            indexes: Vec::new(),
            foreign_keys: fks,
        }
    }

    fn sample_schema() -> SchemaTree {
        SchemaTree {
            database_name: "shop".into(),
            tables: vec![
                table("users", vec![col("id", true), col("email", false)], vec![]),
                table(
                    "orders",
                    vec![col("id", true), col("user_id", false)],
                    vec![fk(&["user_id"], "users", &["id"])],
                ),
                table(
                    "employees",
                    vec![col("id", true), col("manager_id", false)],
                    vec![fk(&["manager_id"], "employees", &["id"])],
                ),
            ],
        }
    }

    #[test]
    fn build_resolves_edges_and_markers() {
        let d = ErDiagram::build("c1", &sample_schema());
        assert_eq!(d.nodes.len(), 3);
        assert_eq!(d.edges.len(), 2);
        assert_eq!(d.database, "shop");

        // orders.user_id → users.id, anchored at the right rows. user_id is nullable
        // and not unique, so the relation is an optional many-to-one.
        let e = d.edges.iter().find(|e| d.nodes[e.from].title == "orders").unwrap();
        assert_eq!(d.nodes[e.to].title, "users");
        assert_eq!(e.from_row, 1); // user_id is the second column
        assert_eq!(e.to_row, Some(0)); // users.id is the first
        assert!(e.detail.contains("user_id → users(id)"));
        assert!(e.many && e.optional);
        assert!(e.detail.contains("many-to-one (optional)"));

        // The FK column is marked; the self-reference resolves to its own node.
        let orders = d.nodes.iter().find(|n| n.title == "orders").unwrap();
        assert!(orders.columns[1].foreign_key);
        assert!(!orders.columns[0].foreign_key);
        let self_edge = d.edges.iter().find(|e| e.from == e.to).unwrap();
        assert_eq!(d.nodes[self_edge.from].title, "employees");
    }

    #[test]
    fn unique_fk_columns_make_the_edge_one_to_one() {
        let mut schema = sample_schema();
        // profiles.user_id → users.id with a unique index on user_id: 1:1.
        let mut user_id = col("user_id", false);
        user_id.nullable = false;
        schema.tables.push(TableInfo {
            schema: None,
            name: "profiles".into(),
            columns: vec![col("id", true), user_id],
            indexes: vec![dbcore::IndexInfo {
                name: "uq_profiles_user".into(),
                unique: true,
                columns: vec!["user_id".into()],
            }],
            foreign_keys: vec![fk(&["user_id"], "users", &["id"])],
        });
        // children.parent_pk → users.id where the FK *is* the primary key: also 1:1.
        schema.tables.push(TableInfo {
            schema: None,
            name: "children".into(),
            columns: vec![col("parent_pk", true)],
            indexes: Vec::new(),
            foreign_keys: vec![fk(&["parent_pk"], "users", &["id"])],
        });

        let d = ErDiagram::build("c1", &schema);
        let profiles = d.edges.iter().find(|e| d.nodes[e.from].title == "profiles").unwrap();
        assert!(!profiles.many && !profiles.optional);
        assert!(profiles.detail.ends_with("one-to-one"));
        let children = d.edges.iter().find(|e| d.nodes[e.from].title == "children").unwrap();
        assert!(!children.many, "a PK that is also the FK caps the child at one");
    }

    #[test]
    fn unresolvable_targets_are_skipped() {
        let mut schema = sample_schema();
        schema.tables[1].foreign_keys[0].ref_table = "missing".into();
        let d = ErDiagram::build("c1", &schema);
        assert_eq!(d.edges.len(), 1); // only the employees self-reference survives
    }

    #[test]
    fn layout_is_finite_separated_and_deterministic() {
        let a = ErDiagram::build("c1", &sample_schema());
        let b = ErDiagram::build("c1", &sample_schema());
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert!(na.pos.x.is_finite() && na.pos.y.is_finite());
            assert_eq!(na.pos, nb.pos, "layout must be deterministic");
        }
        // No two boxes on top of each other.
        for i in 0..a.nodes.len() {
            for j in (i + 1)..a.nodes.len() {
                let (ri, rj) = (a.nodes[i].rect(), a.nodes[j].rect());
                assert!(
                    !ri.shrink(4.0).intersects(rj.shrink(4.0)),
                    "nodes {i} and {j} overlap: {ri:?} vs {rj:?}"
                );
            }
        }
    }

    #[test]
    fn schema_qualified_titles_only_when_needed() {
        let mut schema = sample_schema();
        let d = ErDiagram::build("c1", &schema);
        assert_eq!(d.nodes[0].title, "users");

        schema.tables[0].schema = Some("auth".into());
        schema.tables[1].schema = Some("sales".into());
        schema.tables[2].schema = Some("sales".into());
        let d = ErDiagram::build("c1", &schema);
        assert!(d.nodes.iter().any(|n| n.title == "auth.users"));
        assert!(d.nodes.iter().any(|n| n.title == "sales.orders"));
    }
}
