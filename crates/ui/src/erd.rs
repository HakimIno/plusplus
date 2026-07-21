//! ER diagram state and layout — no egui drawing here; rendering is in panels.rs.
//!
//! The diagram is a snapshot built from an introspected [`SchemaTree`]: one node per
//! table (its columns, with PK/FK markers) and one edge per foreign key. Node positions
//! come from a layered left-to-right layout run once at build time; the user can then
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

/// Depth meaning "no hop limit": the focused diagram shows the whole schema while
/// keeping its root table highlighted (so the depth control stays available).
pub const DEPTH_ALL: usize = usize::MAX;

/// Scope of a table-focused diagram: the root table and how many foreign-key
/// hops around it (in either direction) to include ([`DEPTH_ALL`] = everything).
#[derive(Clone, PartialEq, Eq)]
pub struct ErdFocus {
    pub schema: Option<String>,
    pub table: String,
    pub depth: usize,
}

pub struct ErDiagram {
    /// Saved-connection id the snapshot was built from (shown alongside the database).
    pub conn_id: String,
    pub database: String,
    /// Connection-independent source of truth used for editing, export and forward engineering.
    pub design: dbcore::ErDesign,
    /// Live snapshots follow background introspection; imported/edited designs stay detached.
    pub tracks_schema: bool,
    pub nodes: Vec<ErdNode>,
    pub edges: Vec<ErdEdge>,
    /// Pan/zoom state for `egui::Scene`. An empty rect makes the scene auto-fit the
    /// content on the next frame, so this doubles as the "zoom to fit" request.
    pub scene_rect: Rect,
    /// Node whose edges are highlighted (clicked). `None` = no highlight.
    pub selected: Option<usize>,
    /// `Some` when the diagram shows one table's FK neighborhood instead of the
    /// whole schema (kept so refresh and depth changes rebuild the same scope).
    pub focus: Option<ErdFocus>,
}

/// Resolve FK targets without ever guessing across schemas. Backends that cannot
/// qualify an FK may use a bare table name, but only when that name is unambiguous.
struct TableLookup {
    by_key: BTreeMap<(String, String), usize>,
    by_name: BTreeMap<String, Option<usize>>,
}

impl TableLookup {
    fn new(schema: &SchemaTree) -> Self {
        let mut by_key = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        for (i, table) in schema.tables.iter().enumerate() {
            by_key.insert(
                (table.schema.clone().unwrap_or_default(), table.name.clone()),
                i,
            );
            by_name
                .entry(table.name.clone())
                .and_modify(|match_| *match_ = None)
                .or_insert(Some(i));
        }
        Self { by_key, by_name }
    }

    fn resolve(
        &self,
        table: &dbcore::TableInfo,
        fk: &dbcore::ForeignKeyInfo,
    ) -> Option<usize> {
        if let Some(ref_schema) = &fk.ref_schema {
            return self
                .by_key
                .get(&(ref_schema.clone(), fk.ref_table.clone()))
                .copied();
        }

        let same_schema = table.schema.clone().unwrap_or_default();
        self.by_key
            .get(&(same_schema, fk.ref_table.clone()))
            .copied()
            .or_else(|| self.by_name.get(&fk.ref_table).copied().flatten())
    }
}

fn schema_from_design(design: &dbcore::ErDesign) -> SchemaTree {
    SchemaTree {
        database_name: design.name.clone(),
        tables: design
            .tables
            .iter()
            .map(|table| dbcore::TableInfo {
                schema: table.schema.clone(),
                name: table.name.clone(),
                columns: table
                    .columns
                    .iter()
                    .map(|column| dbcore::ColumnInfo {
                        name: column.name.clone(),
                        data_type: column.data_type.clone(),
                        nullable: column.nullable,
                        primary_key: column.primary_key,
                    })
                    .collect(),
                indexes: table
                    .indexes
                    .iter()
                    .map(|index| dbcore::IndexInfo {
                        name: index.name.clone(),
                        unique: index.unique,
                        columns: index.columns.clone(),
                    })
                    .collect(),
                foreign_keys: table
                    .foreign_keys
                    .iter()
                    .map(|fk| dbcore::ForeignKeyInfo {
                        name: fk.name.clone(),
                        columns: fk.columns.clone(),
                        ref_schema: fk.ref_schema.clone(),
                        ref_table: fk.ref_table.clone(),
                        ref_columns: fk.ref_columns.clone(),
                        on_delete: fk.on_delete.label().to_string(),
                        on_update: "NO ACTION".to_string(),
                    })
                    .collect(),
            })
            .collect(),
        views: Vec::new(),
        routines: Vec::new(),
        triggers: Vec::new(),
    }
}

impl ErDiagram {
    /// Snapshot `schema` into nodes and edges and run the initial layout.
    pub fn build(conn_id: &str, schema: &SchemaTree) -> Self {
        let design = dbcore::ErDesign::from_schema(schema);
        Self::build_with_design(conn_id, schema, design, true)
    }

    /// Open a connection-independent design on `conn_id`. The connection determines only the
    /// target dialect when the user forward-engineers; it is never serialized into the file.
    pub fn build_design(conn_id: &str, design: dbcore::ErDesign) -> Self {
        let schema = schema_from_design(&design);
        Self::build_with_design(conn_id, &schema, design, false)
    }

    fn build_with_design(
        conn_id: &str,
        schema: &SchemaTree,
        design: dbcore::ErDesign,
        tracks_schema: bool,
    ) -> Self {
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

        let tables = TableLookup::new(schema);

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
                let Some(to) = tables.resolve(t, fk) else {
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
            design,
            tracks_schema,
            nodes,
            edges,
            scene_rect: Rect::NOTHING,
            selected: None,
            focus: None,
        };
        diagram.layout();
        for (node, table) in diagram.nodes.iter_mut().zip(&diagram.design.tables) {
            if let (Some(x), Some(y)) = (table.layout_x, table.layout_y) {
                node.pos = pos2(x, y);
            }
        }
        diagram
    }

    /// Build a diagram of just `focus.table` and every table within `focus.depth`
    /// foreign-key hops of it, following FKs both ways (parents it references and
    /// children referencing it). The root comes out selected so its relations are
    /// highlighted. Falls back to the full diagram when the root isn't in `schema`.
    pub fn build_focused(conn_id: &str, schema: &SchemaTree, focus: ErdFocus) -> Self {
        let root = schema.tables.iter().position(|t| {
            t.name == focus.table && (focus.schema.is_none() || t.schema == focus.schema)
        });
        let Some(root) = root else {
            return Self::build(conn_id, schema);
        };

        // "All": the whole schema (including tables unreachable from the root),
        // with the root still selected and the focus kept for the depth control.
        if focus.depth == DEPTH_ALL {
            let mut diagram = Self::build(conn_id, schema);
            diagram.selected = Some(root);
            diagram.focus = Some(focus);
            return diagram;
        }

        // Undirected FK adjacency over the full schema, resolving targets with the
        // same strict rules as `build`.
        let tables = TableLookup::new(schema);
        let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); schema.tables.len()];
        for (i, t) in schema.tables.iter().enumerate() {
            for fk in &t.foreign_keys {
                let Some(to) = tables.resolve(t, fk) else {
                    continue;
                };
                adjacent[i].push(to);
                adjacent[to].push(i);
            }
        }

        // BFS out to `depth` hops; `keep` stays in schema order for determinism.
        let mut dist = vec![usize::MAX; schema.tables.len()];
        dist[root] = 0;
        let mut queue = std::collections::VecDeque::from([root]);
        while let Some(i) = queue.pop_front() {
            if dist[i] == focus.depth {
                continue;
            }
            for &j in &adjacent[i] {
                if dist[j] == usize::MAX {
                    dist[j] = dist[i] + 1;
                    queue.push_back(j);
                }
            }
        }
        let keep: Vec<usize> = (0..schema.tables.len())
            .filter(|&i| dist[i] != usize::MAX)
            .collect();

        let filtered = SchemaTree {
            database_name: schema.database_name.clone(),
            tables: keep.iter().map(|&i| schema.tables[i].clone()).collect(),
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
        };
        let mut diagram = Self::build(conn_id, &filtered);
        diagram.selected = keep.iter().position(|&i| i == root);
        diagram.focus = Some(focus);
        diagram
    }

    /// Ask the scene to zoom-to-fit the content on its next frame.
    pub fn request_fit(&mut self) {
        self.scene_rect = Rect::NOTHING;
    }

    /// (Re)compute node positions with a layered ("Sugiyama-lite") arrangement:
    /// referenced tables sit in columns to the left of the tables pointing at them,
    /// so FK curves read left → right; a barycenter pass orders each column to keep
    /// related boxes near each other; disconnected components stack below one
    /// another and tables with no relations pack into a grid at the bottom.
    /// Deterministic (no RNG) and O(V + E) up to the bounded ordering sweeps.
    pub fn layout(&mut self) {
        let n = self.nodes.len();
        if n == 0 {
            return;
        }

        let sizes: Vec<Vec2> = self.nodes.iter().map(|nd| nd.estimated_size()).collect();

        const MARGIN: f32 = 40.0; // canvas origin offset
        const H_GAP: f32 = 130.0; // between columns — room for the FK curves
        const V_GAP: f32 = 36.0; // between boxes in a column
        const COMP_GAP: f32 = 110.0; // between connected components

        // Undirected adjacency; self-references don't influence placement.
        let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); n];
        for e in &self.edges {
            if e.from != e.to {
                adjacent[e.from].push(e.to);
                adjacent[e.to].push(e.from);
            }
        }

        // Longest-path layering: every table lands one column right of the tables it
        // references. Bounded relaxation so FK cycles can't spin forever — nodes on a
        // cycle just stop moving apart once the passes run out.
        let mut layer = vec![0usize; n];
        for _ in 0..n.min(32) {
            let mut changed = false;
            for e in &self.edges {
                if e.from != e.to && layer[e.from] <= layer[e.to] {
                    layer[e.from] = layer[e.to] + 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Connected components, discovered in schema order for determinism.
        let mut component = vec![usize::MAX; n];
        let mut components: Vec<Vec<usize>> = Vec::new();
        for start in 0..n {
            if component[start] != usize::MAX {
                continue;
            }
            let id = components.len();
            component[start] = id;
            let mut members = vec![start];
            let mut queue = std::collections::VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                for &j in adjacent[i].iter() {
                    if component[j] == usize::MAX {
                        component[j] = id;
                        members.push(j);
                        queue.push_back(j);
                    }
                }
            }
            components.push(members);
        }

        // Row index of every node within its column, shared across components (indices
        // are globally unique, so one flat vec works for all of them).
        let mut row_of = vec![0usize; n];
        let mut y_cursor = MARGIN;

        for members in components.iter().filter(|m| m.len() > 1) {
            // Bucket the component's nodes into columns, compacted to its own range.
            let first = members.iter().map(|&i| layer[i]).min().unwrap_or(0);
            let last = members.iter().map(|&i| layer[i]).max().unwrap_or(0);
            let mut columns: Vec<Vec<usize>> = vec![Vec::new(); last - first + 1];
            for &i in members {
                columns[layer[i] - first].push(i);
            }
            for col in &columns {
                for (r, &i) in col.iter().enumerate() {
                    row_of[i] = r;
                }
            }

            // Crossing reduction: reorder each column by the mean row of its neighbors
            // in the column the sweep just left; nodes with no neighbors there keep
            // their spot. Forward, backward, forward — enough to settle small schemas
            // and cheap enough for big ones.
            for sweep in 0..3 {
                let order: Vec<usize> = if sweep % 2 == 0 {
                    (1..columns.len()).collect()
                } else {
                    (0..columns.len().saturating_sub(1)).rev().collect()
                };
                let neighbor_col = |c: usize| if sweep % 2 == 0 { c - 1 } else { c + 1 };
                for c in order {
                    let against = first + neighbor_col(c);
                    let mut keyed: Vec<(f32, usize, usize)> = columns[c]
                        .iter()
                        .map(|&i| {
                            let rows: Vec<f32> = adjacent[i]
                                .iter()
                                .filter(|&&j| layer[j] == against)
                                .map(|&j| row_of[j] as f32)
                                .collect();
                            let key = if rows.is_empty() {
                                row_of[i] as f32
                            } else {
                                rows.iter().sum::<f32>() / rows.len() as f32
                            };
                            (key, row_of[i], i)
                        })
                        .collect();
                    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    columns[c] = keyed.iter().map(|&(_, _, i)| i).collect();
                    for (r, &i) in columns[c].iter().enumerate() {
                        row_of[i] = r;
                    }
                }
            }

            // Place the component: columns advance rightward by their widest box, and
            // each column centers vertically on the component's tallest column.
            let col_width = |col: &[usize]| {
                col.iter().map(|&i| sizes[i].x).fold(0.0_f32, f32::max)
            };
            let col_height = |col: &[usize]| {
                col.iter().map(|&i| sizes[i].y).sum::<f32>()
                    + V_GAP * col.len().saturating_sub(1) as f32
            };
            let comp_height = columns.iter().map(|c| col_height(c)).fold(0.0_f32, f32::max);
            let mut x = MARGIN;
            for col in &columns {
                if col.is_empty() {
                    continue; // layer skipped by cycle-bounded layering: no gap for it
                }
                let width = col_width(col);
                let mut y = y_cursor + (comp_height - col_height(col)) * 0.5;
                for &i in col {
                    // Center each box within its column so the lane reads as one axis.
                    self.nodes[i].pos = pos2(x + (width - sizes[i].x) * 0.5, y).round();
                    y += sizes[i].y + V_GAP;
                }
                x += width + H_GAP;
            }
            y_cursor += comp_height + COMP_GAP;
        }

        // Tables with no relations: a compact grid block under the connected parts.
        let singles: Vec<usize> = components
            .iter()
            .filter(|m| m.len() == 1)
            .map(|m| m[0])
            .collect();
        if !singles.is_empty() {
            let cell = singles
                .iter()
                .fold(Vec2::ZERO, |acc, &i| acc.max(sizes[i]))
                + vec2(H_GAP * 0.5, V_GAP);
            let per_row = (singles.len() as f32).sqrt().ceil() as usize;
            for (slot, &i) in singles.iter().enumerate() {
                self.nodes[i].pos = pos2(
                    MARGIN + (slot % per_row) as f32 * cell.x,
                    y_cursor + (slot / per_row) as f32 * cell.y,
                )
                .round();
            }
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
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
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
    fn qualified_fk_never_falls_back_to_a_same_named_table() {
        let mut schema = sample_schema();
        schema.tables[0].schema = Some("public".into());
        schema.tables[1].schema = Some("sales".into());
        schema.tables[1].foreign_keys[0].ref_schema = Some("private".into());

        let d = ErDiagram::build("c1", &schema);
        assert_eq!(
            d.edges.len(),
            1,
            "private.users is absent, so the FK must not point at public.users"
        );

        let focused = ErDiagram::build_focused(
            "c1",
            &schema,
            ErdFocus {
                schema: Some("sales".into()),
                table: "orders".into(),
                depth: 1,
            },
        );
        assert_eq!(
            focused.nodes.len(),
            1,
            "the missing target is not a neighbor"
        );
        assert!(focused.edges.is_empty());
    }

    #[test]
    fn unqualified_fk_falls_back_only_when_the_table_name_is_unique() {
        let mut schema = sample_schema();
        schema.tables[0].schema = Some("auth".into());
        schema.tables[1].schema = Some("sales".into());
        schema.tables.push(TableInfo {
            schema: Some("archive".into()),
            name: "users".into(),
            columns: vec![col("id", true)],
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        });

        let d = ErDiagram::build("c1", &schema);
        assert_eq!(
            d.edges.len(),
            1,
            "an ambiguous bare users reference must be skipped"
        );
    }

    #[test]
    fn focused_build_keeps_only_the_fk_neighborhood() {
        let mut schema = sample_schema();
        schema.tables.push(table(
            "order_items",
            vec![col("id", true), col("order_id", false)],
            vec![fk(&["order_id"], "orders", &["id"])],
        ));
        let focus = |depth| ErdFocus {
            schema: None,
            table: "users".into(),
            depth,
        };

        let d1 = ErDiagram::build_focused("c1", &schema, focus(1));
        let titles: Vec<&str> = d1.nodes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, vec!["users", "orders"]);
        assert_eq!(d1.edges.len(), 1);
        assert_eq!(d1.selected, Some(0), "the root comes out highlighted");
        assert_eq!(d1.focus.as_ref().map(|f| f.depth), Some(1));

        // Depth 2 reaches the child's children; unconnected employees never appears.
        let d2 = ErDiagram::build_focused("c1", &schema, focus(2));
        let titles: Vec<&str> = d2.nodes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, vec!["users", "orders", "order_items"]);
    }

    #[test]
    fn focused_build_follows_fks_in_both_directions() {
        // Focusing the child must pull in the parent it references…
        let d = ErDiagram::build_focused(
            "c1",
            &sample_schema(),
            ErdFocus {
                schema: None,
                table: "orders".into(),
                depth: 1,
            },
        );
        let titles: Vec<&str> = d.nodes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, vec!["users", "orders"]);
        assert_eq!(d.selected, Some(1));
        // …and focusing the parent must pull in the children referencing it.
        let d = ErDiagram::build_focused(
            "c1",
            &sample_schema(),
            ErdFocus {
                schema: None,
                table: "users".into(),
                depth: 1,
            },
        );
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.selected, Some(0));
    }

    #[test]
    fn layout_layers_parents_left_of_children() {
        let mut schema = sample_schema();
        schema.tables.push(table(
            "order_items",
            vec![col("id", true), col("order_id", false)],
            vec![fk(&["order_id"], "orders", &["id"])],
        ));
        let d = ErDiagram::build("c1", &schema);
        let x = |name: &str| d.nodes.iter().find(|n| n.title == name).unwrap().pos.x;
        // The FK chain order_items → orders → users lays out as three columns,
        // referenced tables leftmost.
        assert!(x("users") < x("orders"), "referenced table sits left");
        assert!(x("orders") < x("order_items"), "chains keep flowing right");
        // employees is unrelated (only a self-reference): parked below, not inline.
        let users_y = d.nodes.iter().find(|n| n.title == "users").unwrap().pos.y;
        let employees = d.nodes.iter().find(|n| n.title == "employees").unwrap();
        assert!(employees.pos.y > users_y, "isolated tables go under the graph");
    }

    #[test]
    fn focused_build_falls_back_to_full_when_root_missing() {
        let d = ErDiagram::build_focused(
            "c1",
            &sample_schema(),
            ErdFocus {
                schema: None,
                table: "missing".into(),
                depth: 1,
            },
        );
        assert_eq!(d.nodes.len(), 3);
        assert!(d.focus.is_none(), "fallback is the plain full diagram");
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
    fn imported_design_is_detached_and_restores_saved_layout() {
        let live = ErDiagram::build("c1", &sample_schema());
        assert!(live.tracks_schema);
        let mut design = live.design.clone();
        design.tables[0].layout_x = Some(321.0);
        design.tables[0].layout_y = Some(123.0);

        let imported = ErDiagram::build_design("c2", design);
        assert!(!imported.tracks_schema);
        assert_eq!(imported.nodes[0].pos, pos2(321.0, 123.0));
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
