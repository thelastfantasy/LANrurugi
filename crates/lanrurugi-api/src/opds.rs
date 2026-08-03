//! `opds` endpoint group: OPDS 1.2 catalog with PSE 1.1 (page-streaming) compatibility, XML shapes
//! verified against `~/LANraragi/tools/openapi.yaml`'s examples.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use lanrurugi_core::entities::Archive;
use lanrurugi_search::engine::{search, SearchParams};
use serde::Deserialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/opds", get(opds_catalog))
        .route("/opds/{id}", get(opds_item))
        .route("/opds/{id}/pse", get(opds_page))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn first_tag_value(tags: &str, namespace: &str) -> String {
    tags.split(',')
        .find_map(|t| t.trim().strip_prefix(&format!("{namespace}:")))
        .unwrap_or("")
        .to_string()
}

fn entry_xml(archive: &Archive) -> String {
    let title = xml_escape(&archive.title);
    let id = &archive.id;
    let author = xml_escape(&first_tag_value(&archive.tags, "artist"));
    let publisher = xml_escape(&first_tag_value(&archive.tags, "group"));
    let category = if archive.isnew {
        "New Archive"
    } else {
        "Archive"
    };
    let summary = xml_escape(&archive.tags);
    let extension = archive.extension();

    format!(
        r#"<entry>
    <title>{title}</title>
    <id>urn:lrr:{id}</id>
    <updated>1970-01-01T00:00:00Z</updated>
    <published>1970-01-01T00:00:00Z</published>
    <author><name>{author}</name></author>
    <rights></rights>
    <dcterms:language></dcterms:language>
    <dcterms:publisher>{publisher}</dcterms:publisher>
    <dcterms:issued></dcterms:issued>
    <category term="{category}" />
    <summary>{summary}</summary>
    <link rel="alternate" href="/api/opds/{id}" type="application/atom+xml;type=entry;profile=opds-catalog" />
    <link rel="http://opds-spec.org/image" href="/api/archives/{id}/thumbnail" type="image/jpeg" />
    <link rel="http://opds-spec.org/image/thumbnail" href="/api/archives/{id}/thumbnail" type="image/jpeg" />
    <link rel="http://opds-spec.org/acquisition" href="/api/archives/{id}/download" title="Download/Read" type="application/x-{extension}" />
    <link rel="http://vaemendis.net/opds-pse/stream" type="image/jpeg" href="/api/opds/{id}/pse?page={{pageNumber}}" pse:count="{pagecount}" />
    <link type="text/html" rel="alternate" title="Open in LANrurugi" href="/reader?id={id}" />
</entry>"#,
        pagecount = archive.pagecount,
    )
}

#[derive(Debug, Deserialize, Default)]
pub struct OpdsQuery {
    category: Option<String>,
}

async fn opds_catalog(State(state): State<AppState>, Query(q): Query<OpdsQuery>) -> Response {
    let category = match &q.category {
        Some(id) => state
            .repos
            .categories
            .get(&lanrurugi_core::ids::CategoryId(id.clone()))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let params = SearchParams {
        category,
        groupby_tanks: true,
        ..Default::default()
    };
    let result = match search(&state.redis.archive, &state.redis.search, &params).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("search error: {e}"),
            )
                .into_response()
        }
    };

    let mut entries = String::new();
    for id in &result.ids {
        if id.starts_with("TANK") {
            continue;
        }
        if let Ok(Some(a)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
        {
            entries.push_str(&entry_xml(&a));
            entries.push('\n');
        }
    }

    let categories = state.repos.categories.list_all().await.unwrap_or_default();
    let mut facets = String::new();
    facets.push_str(
        r#"<link rel="http://opds-spec.org/facet" href="/api/opds" title="All Archives" opds:facetGroup="Categories" opds:activeFacet="true" />"#,
    );
    for c in &categories {
        facets.push_str(&format!(
            r#"<link rel="http://opds-spec.org/facet" href="/api/opds?category={}" title="{}" opds:facetGroup="Categories" />"#,
            c.catid,
            xml_escape(&c.name)
        ));
    }

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:pse="http://vaemendis.net/opds-pse/ns">
<id>urn:lrr:0</id>
<link rel="self" href="/api/opds" type="application/atom+xml;profile=opds-catalog;kind=acquisition" />
<link rel="start" href="/api/opds" type="application/atom+xml;profile=opds-catalog;kind=acquisition" />
<title>LANrurugi</title>
<updated>1970-01-01T00:00:00Z</updated>
<subtitle>Welcome to this Library running LANrurugi!</subtitle>
<author><name>LANrurugi</name></author>
{facets}
{entries}
</feed>"#
    );

    ([(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

async fn opds_item(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    match state.repos.archives.get(&id).await {
        Ok(Some(a)) => {
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<entry xmlns="http://www.w3.org/2005/Atom" xmlns:thr="http://purl.org/syndication/thread/1.0" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:pse="http://vaemendis.net/opds-pse/ns">
<link rel="start" href="/api/opds" type="application/atom+xml;profile=opds-catalog;kind=navigation" />
<link rel="self" href="/api/opds/{id}" type="application/atom+xml;type=entry;profile=opds-catalog" />
{entry}
</entry>"#,
                entry = entry_xml(&a),
            );
            ([(header::CONTENT_TYPE, "application/xml")], xml).into_response()
        }
        Ok(None) => (StatusCode::BAD_REQUEST, "No archive ID specified.").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct PseQuery {
    page: Option<u32>,
}

/// Serves the raw image for `page` (1-indexed), matching the `pse:stream` link's contract.
async fn opds_page(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(q): Query<PseQuery>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::BAD_REQUEST, "No archive ID specified.").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let page = q.page.unwrap_or(1).max(1);
    let pages =
        match lanrurugi_scanner::archive_format::list_pages(std::path::Path::new(&archive.file)) {
            Ok(p) => p,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
    let Some(entry) = pages.get((page - 1) as usize) else {
        return (StatusCode::BAD_REQUEST, "Page out of range.").into_response();
    };
    match lanrurugi_scanner::archive_format::read_entry(std::path::Path::new(&archive.file), entry)
    {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
