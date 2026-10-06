//! Link graph of a project: effective documents as nodes, links and includes as
//! edges, a deterministic force-directed layout, and SVG rendering.
//!
//! Targets resolve the way readers follow them: an unqualified link against the
//! viewing project, an unqualified include against the project that owns the
//! including document. A target in the viewing project or one of its ancestors
//! resolves to the effective document at that path, or to a `missing` node when
//! there is none. Targets in other projects become `external` nodes; their
//! existence is not checked.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

use serde::Serialize;

use crate::links::LinkTarget;
use crate::markdown::analyze;
use crate::path::{DocPath, Slug};
use crate::sync::Variant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Link,
    Include,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// A document owned by the viewing project.
    Document,
    /// A document inherited from an ancestor project.
    Inherited,
    /// A target in the viewing project's tree with no document.
    Missing,
    /// A target in a project outside the viewing project's inheritance chain.
    External,
}

/// One outgoing reference of a document variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub variant: Variant,
    pub kind: EdgeKind,
    pub target: LinkTarget,
}

/// Links and top-level includes of one variant. `base` is the document's own path.
pub fn references(md: &str, base: &DocPath, variant: Variant) -> Vec<Reference> {
    let a = analyze(md, Some(base));
    let links = a.links.into_iter().map(|target| (EdgeKind::Link, target));
    let includes = a.includes.into_iter().map(|target| (EdgeKind::Include, target));
    links
        .chain(includes)
        .map(|(kind, target)| Reference { variant, kind, target })
        .collect()
}

/// An effective document of the viewing project.
#[derive(Debug, Clone)]
pub struct GraphDoc {
    pub path: DocPath,
    pub title: Option<String>,
    /// Project owning the document; differs from the viewing project when inherited.
    pub owner: Slug,
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    /// Document path, or `project:path` for external nodes.
    pub id: String,
    /// Owning project; the target project for external nodes, the viewing project
    /// for missing ones.
    pub project: String,
    pub path: String,
    pub title: Option<String>,
    pub kind: NodeKind,
    /// Number of distinct documents with an edge to this node.
    pub inbound: usize,
    /// Number of distinct nodes this node has an edge to.
    pub outbound: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    /// Index into [`Graph::nodes`].
    pub source: usize,
    /// Index into [`Graph::nodes`].
    pub target: usize,
    pub kind: EdgeKind,
    /// Variants containing the reference.
    pub variants: Vec<Variant>,
    /// Target anchors, sorted; empty when only whole-document references exist.
    pub anchors: Vec<String>,
}

/// Nodes sorted by id; edges sorted by source, target and kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Builds the graph of `docs`, the effective documents of `project`. `chain` is
/// `project` followed by its ancestors, nearest first.
pub fn build(chain: &[Slug], docs: &[GraphDoc]) -> Graph {
    let Some(project) = chain.first() else {
        return Graph::default();
    };
    let depth = |s: &Slug| chain.iter().position(|c| c == s);
    let by_path: HashMap<&str, &GraphDoc> = docs.iter().map(|d| (d.path.as_str(), d)).collect();

    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for d in docs {
        let kind = if &d.owner == project {
            NodeKind::Document
        } else {
            NodeKind::Inherited
        };
        nodes.insert(
            d.path.to_string(),
            node(
                d.path.to_string(),
                d.owner.as_str(),
                d.path.as_str(),
                d.title.clone(),
                kind,
            ),
        );
    }

    #[derive(Default)]
    struct Merged {
        variants: BTreeSet<Variant>,
        anchors: BTreeSet<String>,
    }
    let mut edges: BTreeMap<(String, String, EdgeKind), Merged> = BTreeMap::new();
    for d in docs {
        for r in &d.references {
            let Some(path) = &r.target.path else { continue };
            let context = match r.kind {
                EdgeKind::Link => project,
                EdgeKind::Include => &d.owner,
            };
            let target_project = r.target.project.as_ref().unwrap_or(context);
            let id = match depth(target_project) {
                Some(level) => {
                    // The effective document must be visible from the target project:
                    // owned by it or by one of its ancestors.
                    match by_path.get(path.as_str()) {
                        Some(t) if depth(&t.owner).is_some_and(|o| o >= level) => path.to_string(),
                        Some(_) => external(&mut nodes, target_project, path),
                        None => nodes
                            .entry(path.to_string())
                            .or_insert_with(|| {
                                node(
                                    path.to_string(),
                                    project.as_str(),
                                    path.as_str(),
                                    None,
                                    NodeKind::Missing,
                                )
                            })
                            .id
                            .clone(),
                    }
                }
                None => external(&mut nodes, target_project, path),
            };
            if id == d.path.as_str() {
                continue;
            }
            let e = edges.entry((d.path.to_string(), id, r.kind)).or_default();
            e.variants.insert(r.variant);
            e.anchors.extend(r.target.anchor.clone());
        }
    }

    let index: HashMap<String, usize> = nodes.keys().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    let mut nodes: Vec<Node> = nodes.into_values().collect();
    let edges: Vec<Edge> = edges
        .into_iter()
        .map(|((source, target, kind), m)| Edge {
            source: index[&source],
            target: index[&target],
            kind,
            variants: m.variants.into_iter().collect(),
            anchors: m.anchors.into_iter().collect(),
        })
        .collect();
    let pairs: BTreeSet<(usize, usize)> = edges.iter().map(|e| (e.source, e.target)).collect();
    for (s, t) in pairs {
        nodes[s].outbound += 1;
        nodes[t].inbound += 1;
    }
    Graph { nodes, edges }
}

fn node(id: String, project: &str, path: &str, title: Option<String>, kind: NodeKind) -> Node {
    Node {
        id,
        project: project.to_owned(),
        path: path.to_owned(),
        title,
        kind,
        inbound: 0,
        outbound: 0,
    }
}

fn external(nodes: &mut BTreeMap<String, Node>, project: &Slug, path: &DocPath) -> String {
    let id = format!("{project}:{path}");
    nodes
        .entry(id.clone())
        .or_insert_with(|| node(id.clone(), project.as_str(), path.as_str(), None, NodeKind::External))
        .id
        .clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Node positions, indexed like [`Graph::nodes`], within a `width` × `height` box
/// that also holds the node labels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub points: Vec<Point>,
}

const MARGIN: f64 = 24.0;
const ITERATIONS: usize = 300;
/// Ideal edge length.
const K: f64 = 70.0;
/// Nodes farther apart than this do not repel each other.
const REPULSION_RANGE: f64 = 3.0 * K;
const GRAVITY: f64 = 0.04;
/// Horizontal stretch applied after the layout, since labels run horizontally.
const STRETCH_X: f64 = 1.7;
const NODE_RADIUS: f64 = 7.0;
const LABEL_GAP: f64 = 3.0;
/// Estimated label advance per character at the SVG's 11px font.
const LABEL_CHAR: f64 = 6.4;

/// Fruchterman–Reingold layout with bounded-range repulsion and gravity toward the
/// origin, so that disconnected components settle near the rest instead of
/// drifting away. The result is stretched horizontally and translated into a box
/// fitted to the nodes and their labels. Deterministic: nodes start on a
/// golden-angle spiral in index order.
pub fn layout(g: &Graph) -> Layout {
    let n = g.nodes.len();
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let mut pts: Vec<Point> = (0..n)
        .map(|i| {
            let r = K * 0.6 * (i as f64 + 0.5).sqrt();
            let a = i as f64 * golden;
            Point {
                x: r * a.cos(),
                y: r * a.sin(),
            }
        })
        .collect();
    let springs: BTreeSet<(usize, usize)> = g
        .edges
        .iter()
        .filter(|e| e.source != e.target)
        .map(|e| (e.source.min(e.target), e.source.max(e.target)))
        .collect();

    let start = K * (1.0 + (n as f64).sqrt() / 2.0);
    let mut disp = vec![Point { x: 0.0, y: 0.0 }; n];
    for iter in 0..ITERATIONS {
        let temp = start * (1.0 - iter as f64 / ITERATIONS as f64) + 0.5;
        disp.fill(Point { x: 0.0, y: 0.0 });
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (pts[i].x - pts[j].x, pts[i].y - pts[j].y);
                let d = dx.hypot(dy).max(0.01);
                if d > REPULSION_RANGE {
                    continue;
                }
                let f = K * K / d / d;
                disp[i].x += dx * f;
                disp[i].y += dy * f;
                disp[j].x -= dx * f;
                disp[j].y -= dy * f;
            }
        }
        for &(s, t) in &springs {
            let (dx, dy) = (pts[s].x - pts[t].x, pts[s].y - pts[t].y);
            let d = dx.hypot(dy).max(0.01);
            let f = d / K;
            disp[s].x -= dx * f;
            disp[s].y -= dy * f;
            disp[t].x += dx * f;
            disp[t].y += dy * f;
        }
        for (p, v) in pts.iter_mut().zip(&mut disp) {
            v.x -= p.x * GRAVITY * K / 10.0;
            v.y -= p.y * GRAVITY * K / 10.0;
            let len = v.x.hypot(v.y);
            if len > 0.0 {
                let step = len.min(temp);
                p.x += v.x / len * step;
                p.y += v.y / len * step;
            }
        }
    }

    let (mut left, mut right, mut top, mut bottom) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (p, node) in pts.iter_mut().zip(&g.nodes) {
        p.x *= STRETCH_X;
        left = left.min(p.x - NODE_RADIUS);
        right = right.max(p.x + label_extent(node));
        top = top.min(p.y - NODE_RADIUS);
        bottom = bottom.max(p.y + NODE_RADIUS);
    }
    if n == 0 {
        (left, right, top, bottom) = (0.0, 0.0, 0.0, 0.0);
    }
    for p in &mut pts {
        p.x += MARGIN - left;
        p.y += MARGIN - top;
    }
    Layout {
        width: right - left + 2.0 * MARGIN,
        height: bottom - top + 2.0 * MARGIN,
        points: pts,
    }
}

/// Distance from a node's centre to the right end of its label.
fn label_extent(node: &Node) -> f64 {
    NODE_RADIUS + LABEL_GAP + node.id.chars().count() as f64 * LABEL_CHAR
}

/// Renders `g` at `layout` as a standalone SVG document. Colours use the page's CSS
/// custom properties when embedded, with light-theme fallbacks. `href` maps a node
/// to its URL; nodes without one are not links.
pub fn svg(g: &Graph, layout: &Layout, href: &dyn Fn(&Node) -> Option<String>) -> String {
    let mut s = String::new();
    let (w, h) = (layout.width, layout.height);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="link-graph" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" role="img" aria-label="Document link graph">"#
    );
    s.push_str(concat!(
        "<style>",
        ".link-graph{font:11px system-ui,sans-serif}",
        ".link-graph .e{stroke:var(--muted,#6b7280);stroke-opacity:.55;stroke-width:1.2;fill:none}",
        ".link-graph .e.include{stroke-dasharray:5 3;stroke:var(--accent,#2f5bd3)}",
        ".link-graph .arrow{fill:var(--muted,#6b7280)}",
        ".link-graph .arrow.include{fill:var(--accent,#2f5bd3)}",
        ".link-graph circle{stroke:var(--bg,#fff);stroke-width:1.5}",
        ".link-graph .document circle{fill:var(--accent,#2f5bd3)}",
        ".link-graph .inherited circle{fill:var(--muted,#6b7280)}",
        ".link-graph .missing circle{fill:var(--del-fg,#a3261c)}",
        ".link-graph .external circle{fill:var(--bg,#fff);stroke:var(--muted,#6b7280);stroke-dasharray:2 2}",
        ".link-graph text{fill:var(--fg,#1c1e21);paint-order:stroke;stroke:var(--bg,#fff);stroke-width:3px;stroke-linejoin:round}",
        ".link-graph .missing text{fill:var(--del-fg,#a3261c)}",
        ".link-graph a:hover text{text-decoration:underline}",
        "</style>",
        "<defs>",
        r#"<marker id="lg-arrow" class="arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0L10,5L0,10z"/></marker>"#,
        r#"<marker id="lg-arrow-include" class="arrow include" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0L10,5L0,10z"/></marker>"#,
        "</defs>",
    ));

    s.push_str(r#"<g class="edges">"#);
    for e in &g.edges {
        let (a, b) = (layout.points[e.source], layout.points[e.target]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let d = dx.hypot(dy);
        if d <= 2.0 * NODE_RADIUS {
            continue;
        }
        // Stop at the circles' edges so the arrowhead stays visible.
        let (ux, uy) = (dx / d, dy / d);
        let (x1, y1) = (a.x + ux * NODE_RADIUS, a.y + uy * NODE_RADIUS);
        let (x2, y2) = (b.x - ux * (NODE_RADIUS + 1.0), b.y - uy * (NODE_RADIUS + 1.0));
        let (class, marker) = match e.kind {
            EdgeKind::Link => ("e link", "lg-arrow"),
            EdgeKind::Include => ("e include", "lg-arrow-include"),
        };
        let mut label = format!(
            "{} {} {}",
            g.nodes[e.source].id,
            match e.kind {
                EdgeKind::Link => "links to",
                EdgeKind::Include => "includes",
            },
            g.nodes[e.target].id
        );
        if !e.anchors.is_empty() {
            let _ = write!(label, " #{}", e.anchors.join(", #"));
        }
        let _ = write!(
            s,
            r#"<line class="{class}" x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" marker-end="url(#{marker})"><title>{}</title></line>"#,
            esc(&label)
        );
    }
    s.push_str("</g>");

    s.push_str(r#"<g class="nodes">"#);
    for (node, p) in g.nodes.iter().zip(&layout.points) {
        let class = match node.kind {
            NodeKind::Document => "document",
            NodeKind::Inherited => "inherited",
            NodeKind::Missing => "missing",
            NodeKind::External => "external",
        };
        let mut tip = node.id.clone();
        if let Some(t) = &node.title {
            let _ = write!(tip, " — {t}");
        }
        match node.kind {
            NodeKind::Inherited => {
                let _ = write!(tip, " (from {})", node.project);
            }
            NodeKind::Missing => tip.push_str(" (missing)"),
            NodeKind::External => tip.push_str(" (other project)"),
            NodeKind::Document => {}
        }
        let _ = write!(tip, "\n{} in, {} out", node.inbound, node.outbound);
        let link = href(node);
        if let Some(url) = &link {
            let _ = write!(s, r#"<a href="{}">"#, esc(url));
        }
        let tx = p.x + NODE_RADIUS + LABEL_GAP;
        let _ = write!(
            s,
            r#"<g class="{class}"><title>{}</title><circle cx="{:.1}" cy="{:.1}" r="{NODE_RADIUS}"/><text x="{tx:.1}" y="{:.1}">{}</text></g>"#,
            esc(&tip),
            p.x,
            p.y,
            p.y + 4.0,
            esc(&node.id)
        );
        if link.is_some() {
            s.push_str("</a>");
        }
    }
    s.push_str("</g></svg>");
    s
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(s: &str) -> Slug {
        Slug::parse(s).unwrap()
    }

    fn doc(owner: &str, path: &str, human: &str, ai: Option<&str>) -> GraphDoc {
        let p = DocPath::parse(path).unwrap();
        let mut references = references(human, &p, Variant::Human);
        if let Some(ai) = ai {
            references.extend(super::references(ai, &p, Variant::Ai));
        }
        GraphDoc {
            path: p,
            title: Some(path.to_uppercase()),
            owner: slug(owner),
            references,
        }
    }

    fn ids(g: &Graph) -> Vec<(&str, NodeKind)> {
        g.nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect()
    }

    fn edges(g: &Graph) -> Vec<(&str, &str, EdgeKind)> {
        g.edges
            .iter()
            .map(|e| (g.nodes[e.source].id.as_str(), g.nodes[e.target].id.as_str(), e.kind))
            .collect()
    }

    #[test]
    fn resolves_links_includes_missing_and_external() {
        let chain = [slug("app"), slug("shared")];
        let docs = [
            doc(
                "app",
                "design/auth",
                "# Auth\n\nSee [sessions](sessions.md#ttl), [[readme]], [[other:x]], [self](#a) and [[design/auth]].\n\n{{include shared:rules/style#tone}}\n",
                Some("# Auth\n\n[[design/sessions#expiry]] [[gone]]\n"),
            ),
            doc("app", "design/sessions", "# Sessions\n", None),
            doc("app", "readme", "# Readme\n\n[[design/auth]]\n", None),
            doc(
                "shared",
                "rules/style",
                "# Style\n\n[[readme]]\n\n{{include glossary}}\n",
                None,
            ),
        ];
        let g = build(&chain, &docs);
        assert_eq!(
            ids(&g),
            [
                ("design/auth", NodeKind::Document),
                ("design/sessions", NodeKind::Document),
                ("glossary", NodeKind::Missing),
                ("gone", NodeKind::Missing),
                ("other:x", NodeKind::External),
                ("readme", NodeKind::Document),
                ("rules/style", NodeKind::Inherited),
            ]
        );
        assert_eq!(
            edges(&g),
            [
                ("design/auth", "design/sessions", EdgeKind::Link),
                ("design/auth", "gone", EdgeKind::Link),
                ("design/auth", "other:x", EdgeKind::Link),
                ("design/auth", "readme", EdgeKind::Link),
                ("design/auth", "rules/style", EdgeKind::Include),
                ("readme", "design/auth", EdgeKind::Link),
                // Inherited documents link against the viewing project.
                ("rules/style", "glossary", EdgeKind::Include),
                ("rules/style", "readme", EdgeKind::Link),
            ]
        );
        let sessions = &g.edges[0];
        assert_eq!(sessions.variants, [Variant::Human, Variant::Ai]);
        assert_eq!(sessions.anchors, ["expiry", "ttl"]);
        let auth = &g.nodes[0];
        assert_eq!((auth.inbound, auth.outbound), (1, 5));
    }

    #[test]
    fn overridden_ancestor_target_is_external() {
        // `shared:readme` names the parent's document, which `app` overrides.
        let chain = [slug("app"), slug("shared")];
        let docs = [
            doc("app", "readme", "# Readme\n", None),
            doc("app", "guide", "[[shared:readme]] [[app:readme]]\n", None),
        ];
        let g = build(&chain, &docs);
        assert_eq!(
            edges(&g),
            [
                ("guide", "readme", EdgeKind::Link),
                ("guide", "shared:readme", EdgeKind::Link),
            ]
        );
        assert_eq!(g.nodes[2].kind, NodeKind::External);
    }

    #[test]
    fn layout_is_deterministic_and_bounded() {
        let chain = [slug("p")];
        let docs: Vec<GraphDoc> = (0..30)
            .map(|i| {
                doc(
                    "p",
                    &format!("d{i}"),
                    &format!("[[d{}]] [[d{}]]\n", (i + 1) % 30, (i * 7) % 30),
                    None,
                )
            })
            .collect();
        let g = build(&chain, &docs);
        let a = layout(&g);
        assert_eq!(a, layout(&g));
        assert_eq!(a.points.len(), 30);
        for (p, node) in a.points.iter().zip(&g.nodes) {
            assert!(p.x - NODE_RADIUS >= MARGIN - 1e-9 && p.x + label_extent(node) <= a.width - MARGIN + 1e-9);
            assert!(p.y - NODE_RADIUS >= MARGIN - 1e-9 && p.y + NODE_RADIUS <= a.height - MARGIN + 1e-9);
        }
        let min = (0..30)
            .flat_map(|i| (i + 1..30).map(move |j| (i, j)))
            .map(|(i, j)| (a.points[i].x - a.points[j].x).hypot(a.points[i].y - a.points[j].y))
            .fold(f64::MAX, f64::min);
        assert!(min > NODE_RADIUS * 2.0, "nodes overlap: {min}");
    }

    #[test]
    fn empty_graph() {
        let g = build(&[slug("p")], &[]);
        assert!(g.nodes.is_empty());
        let l = layout(&g);
        assert!(svg(&g, &l, &|_| None).ends_with("</svg>"));
    }

    #[test]
    fn svg_escapes_and_links() {
        let chain = [slug("p")];
        let docs = [GraphDoc {
            title: Some("<b>&\"".into()),
            ..doc("p", "a", "[[b]]\n", None)
        }];
        let g = build(&chain, &docs);
        let out = svg(&g, &layout(&g), &|n| {
            (n.kind != NodeKind::Missing).then(|| format!("/d/{}?x=1&y=2", n.path))
        });
        assert!(out.contains("&lt;b&gt;&amp;&quot;"));
        assert!(out.contains(r#"<a href="/d/a?x=1&amp;y=2">"#));
        assert!(!out.contains("/d/b"));
        assert!(out.contains("a links to b"));
    }
}
