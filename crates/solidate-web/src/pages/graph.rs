//! Project link graph.

use solidate_app::LinkGraph;
use solidate_app::core::Variant;
use solidate_app::core::graph::{EdgeKind, Node, NodeKind, layout, svg};
use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::{HeaderMap, HeaderValue, header, page, path_param_segment, route};
use topcoat::view::{View, view};

use crate::auth::{app, tenant_ctx};
use crate::error::OrHttp;
use crate::ui::{Trusted, doc_url, project_url, tenant_url};

/// The graph as SVG, with nodes linking to their documents.
pub(crate) fn graph_svg(t: &str, g: &LinkGraph) -> String {
    let p = g.project.slug.as_str();
    let href = |n: &Node| match n.kind {
        NodeKind::Document | NodeKind::Inherited => Some(doc_url(t, p, &n.path, Variant::Human)),
        NodeKind::External => Some(doc_url(t, &n.project, &n.path, Variant::Human)),
        NodeKind::Missing => None,
    };
    svg(&g.graph, &layout(&g.graph), &href)
}

#[page("/t/{tenant}/p/{project}/graph")]
async fn graph(cx: &Cx) -> Result<impl View> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let g = app(cx)
        .link_graph(&ctx, path_param_segment(cx, "project"))
        .await
        .or_http()?;
    let (t, p) = (ctx.tenant.slug.clone(), g.project.slug.clone());
    let nodes = &g.graph.nodes;
    let is_doc = |n: &&Node| matches!(n.kind, NodeKind::Document | NodeKind::Inherited);
    let doc_count = nodes.iter().filter(is_doc).count();
    let orphans: Vec<String> = nodes
        .iter()
        .filter(is_doc)
        .filter(|n| n.inbound == 0)
        .map(|n| n.path.clone())
        .collect();
    let missing: Vec<(String, String)> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Missing)
        .map(|n| (n.path.clone(), referrers(&g, n)))
        .collect();
    let includes = g.graph.edges.iter().filter(|e| e.kind == EdgeKind::Include).count();
    let links = g.graph.edges.len() - includes;
    let picture = graph_svg(&t, &g);
    let name = g.project.name;

    Ok(view! {
        <nav class="crumbs">
            <a href=(tenant_url(&t))>(ctx.tenant.name.as_str())</a>
            <a href=(project_url(&t, &p))>(name)</a>
            <span>"Link graph"</span>
        </nav>
        <div class="head">
            <h1>"Link graph"</h1>
            <div class="actions">
                <a class="button secondary" href=(format!("{}/graph.svg", project_url(&t, &p))) download=(format!("{p}-links.svg"))>"Download SVG"</a>
            </div>
        </div>
        <p class="muted">
            (format!(
                "{doc_count} documents, {links} links, {includes} includes. "
            ))
            <span class="graph-key document">"own"</span>
            <span class="graph-key inherited">"inherited"</span>
            <span class="graph-key missing">"missing"</span>
            <span class="graph-key external">"other project"</span>
            <span class="graph-key include">"include"</span>
        </p>
        if doc_count == 0 {
            <p class="muted">"No documents yet."</p>
        } else {
            <div class="graph">(Trusted(picture))</div>
        }
        if !missing.is_empty() {
            <h2>"Missing targets"</h2>
            <ul class="plain">
                for (path, from) in &missing {
                    <li>
                        <code>(path.as_str())</code>
                        <span class="muted small">(format!(" linked from {from}"))</span>
                    </li>
                }
            </ul>
        }
        if !orphans.is_empty() {
            <h2>"Not linked from any document"</h2>
            <ul class="plain">
                for path in &orphans {
                    <li><a href=(doc_url(&t, &p, path, Variant::Human))><code>(path.as_str())</code></a></li>
                }
            </ul>
        }
    })
}

/// Ids of the nodes with an edge to `n`.
fn referrers(g: &LinkGraph, n: &Node) -> String {
    let mut ids: Vec<&str> = g
        .graph
        .edges
        .iter()
        .filter(|e| g.graph.nodes[e.target].id == n.id)
        .map(|e| g.graph.nodes[e.source].id.as_str())
        .collect();
    ids.dedup();
    ids.join(", ")
}

#[route(GET "/t/{tenant}/p/{project}/graph.svg")]
async fn graph_download(cx: &Cx) -> Result<(HeaderMap, String)> {
    let ctx = tenant_ctx(cx, path_param_segment(cx, "tenant")).await?;
    let g = app(cx)
        .link_graph(&ctx, path_param_segment(cx, "project"))
        .await
        .or_http()?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    Ok((headers, graph_svg(&ctx.tenant.slug, &g)))
}
