//! `opds` endpoint group: OPDS 1.2 catalog with PSE 1.1 (page-streaming) compatibility, XML shapes
//! verified against `~/LANraragi/tools/openapi.yaml`'s examples.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use lanrurugi_core::entities::Archive;
use lanrurugi_search::engine::{search, SearchParams};
use serde::Deserialize;

use crate::archives::{desired_thumbnail_format, PageParams};
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

/// The image types this *request's* client will actually be served, so the feed's own `type=`
/// attributes describe what the linked endpoints return rather than a hardcoded guess. Both are
/// negotiated per request (capability cookie/`Accept`), which is what a feed fetched by that same
/// client can be honest about — see [`negotiated_data_types`].
#[derive(Clone, Copy)]
struct DataTypes {
    thumbnail: &'static str,
    page: &'static str,
}

/// Resolves the two declared types for a feed request.
///
/// The thumbnails endpoint picks from the library's configured `thumbnail_format` filtered through
/// what the client can decode, so the answer here has to consult the same settings; the page stream
/// re-encodes into whatever [`ClientImageSupport::from_request`] resolves for this request, whose
/// own fallback for a client with no capability signal at all is WebP — so a real OPDS reader, which
/// sends `Accept: image/*` and no cookie, is declared `image/webp` and served WebP.
///
/// `image/jpeg` is therefore only reached by a client that explicitly asked for `source` bytes
/// (the capability cookie's own `source` value, set by the web frontend when it can decode neither
/// modern codec): those responses carry the archive's real per-page type in their own
/// `Content-Type`, which is as honest as a single feed-level declaration can be.
async fn negotiated_data_types(
    state: &AppState,
    support: crate::archives::ClientImageSupport,
) -> DataTypes {
    let configured = match state.redis.config.get().await {
        Ok(mut conn) => {
            lanrurugi_scanner::thumbnail::read_settings(&mut conn)
                .await
                .format
        }
        Err(_) => lanrurugi_scanner::thumbnail::ThumbFormat::Webp,
    };
    DataTypes {
        thumbnail: desired_thumbnail_format(support, configured).content_type(),
        page: if support.jxl {
            "image/jxl"
        } else if support.webp {
            "image/webp"
        } else {
            "image/jpeg"
        },
    }
}

fn entry_xml(archive: &Archive, types: DataTypes) -> String {
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
    <link rel="http://opds-spec.org/image" href="/api/archives/{id}/thumbnail" type="{thumbnail_type}" />
    <link rel="http://opds-spec.org/image/thumbnail" href="/api/archives/{id}/thumbnail" type="{thumbnail_type}" />
    <link rel="http://opds-spec.org/acquisition" href="/api/archives/{id}/download" title="Download/Read" type="application/x-{extension}" />
    <link rel="http://vaemendis.net/opds-pse/stream" type="{page_type}" href="/api/opds/{id}/pse?page={{pageNumber}}" pse:count="{pagecount}" />
    <link type="text/html" rel="alternate" title="Open in LANrurugi" href="/reader?id={id}" />
</entry>"#,
        pagecount = archive.pagecount,
        thumbnail_type = types.thumbnail,
        page_type = types.page,
    )
}

#[derive(Debug, Deserialize, Default)]
pub struct OpdsQuery {
    category: Option<String>,
}

async fn opds_catalog(
    State(state): State<AppState>,
    Query(q): Query<OpdsQuery>,
    headers: HeaderMap,
) -> Response {
    let types = negotiated_data_types(
        &state,
        crate::archives::ClientImageSupport::from_request(&headers, None),
    )
    .await;
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
    let result = match search(
        &state.redis.archive,
        &state.redis.search,
        &state.equivalence,
        &state.bookmarks,
        &params,
    )
    .await
    {
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
            entries.push_str(&entry_xml(&a, types));
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
    headers: HeaderMap,
) -> Response {
    let types = negotiated_data_types(
        &state,
        crate::archives::ClientImageSupport::from_request(&headers, None),
    )
    .await;
    match state.repos.archives.get(&id).await {
        Ok(Some(a)) => {
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<entry xmlns="http://www.w3.org/2005/Atom" xmlns:thr="http://purl.org/syndication/thread/1.0" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:pse="http://vaemendis.net/opds-pse/ns">
<link rel="start" href="/api/opds" type="application/atom+xml;profile=opds-catalog;kind=navigation" />
<link rel="self" href="/api/opds/{id}" type="application/atom+xml;type=entry;profile=opds-catalog" />
{entry}
</entry>"#,
                entry = entry_xml(&a, types),
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

/// Serves `page` (1-indexed) for a page-streaming reader, through the *same* pipeline the web
/// reader uses — legacy parity (`~/LANraragi/lib/LANraragi/Model/Opds.pm::render_archive_page` ends
/// its own comment with "Use the same code as /api/page to serve the file"), and the only way a
/// third-party reader gets a codec it can actually decode, a real `Content-Type`, and the resize
/// cache the web reader already pays for.
///
/// This used to stream the entry's raw bytes as `application/octet-stream`: an AVIF/JXL page reached
/// a reader that could not decode it, nothing was resized, and a save from the stream had no name.
async fn opds_page(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(q): Query<PseQuery>,
    headers: HeaderMap,
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
    crate::archives::serve_page(
        &state,
        auth.as_deref(),
        &id,
        PageParams::optimized(entry.clone()),
        &headers,
    )
    .await
}
