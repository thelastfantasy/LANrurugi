import { useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import type { ArchiveMetadata, CategoryMetadata } from '../../api/types'
import RatingWidget from './RatingWidget'

// Namespaces treated as timestamps for display (legacy `buildTagsDiv`: `/^(date|time)/.test(key)`
// converts the tag value through a date formatter instead of printing it raw).
const TIMESTAMP_NAMESPACE = /^(date|time)/i

function displayNamespace(key: string): string {
  if (key === 'date_added') return 'Date Added'
  return key.charAt(0).toUpperCase() + key.slice(1)
}

function formatTagValue(namespace: string, value: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  const ms = Number(value) * 1000
  if (Number.isNaN(ms)) return value
  return new Date(ms).toLocaleDateString()
}

/** Mirrors legacy's `splitTagsByNamespace` + `buildTagsDiv` (`~/LANraragi/public/js/mod/common.js`)
 * — groups a flat comma-separated tag string by its `namespace:value` prefix (untagged values fall
 * under `other`), rendered as a `caption-namespace` row per namespace with each value as a
 * clickable search-link chip. */
function TagsTable({ tags }: { tags: string }) {
  if (!tags) return null
  const byNamespace = new Map<string, string[]>()
  for (const raw of tags.split(',')) {
    const tag = raw.trim()
    if (!tag) continue
    const idx = tag.indexOf(':')
    const namespace = idx === -1 ? 'other' : tag.slice(0, idx).trim()
    const value = idx === -1 ? tag : tag.slice(idx + 1).trim()
    if (namespace.toLowerCase() === 'rating') continue // shown by RatingWidget, not the table
    const list = byNamespace.get(namespace) ?? []
    list.push(value)
    byNamespace.set(namespace, list)
  }

  const namespaces = [...byNamespace.keys()].sort()
  if (namespaces.length === 0) return null

  return (
    <table className="itg" style={{ boxShadow: 'none', border: 'none', borderRadius: 0 }}>
      <tbody>
        {namespaces.map((namespace) => (
          <tr key={namespace}>
            <td className={`caption-namespace ${namespace.toLowerCase()}-tag`}>
              {displayNamespace(namespace)}:
            </td>
            <td>
              {(byNamespace.get(namespace) ?? []).map((value) => (
                <div className="gt" key={value}>
                  <a
                    href={`/?sort=0&q=${encodeURIComponent(`${namespace}:${value}$`)}&`}
                    onClick={(e) => e.stopPropagation()}
                  >
                    {formatTagValue(namespace, value)}
                  </a>
                </div>
              ))}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

// Mirrors legacy's `#archivePagesOverlay` (`updateArchiveOverlay`/`generateThumbnails` in
// `~/LANraragi/public/js/reader.js`) — thumbnail (left) + Admin Options/Categories/Rating (right)
// side by side via `.reader-thumbnail`'s `display:inline-block` (verified against
// `~/LANraragi/public/css/lrr.css`), the full per-namespace tags table below it, then a thumbnail
// grid scoped to the current chapter (or the whole archive if there's no TOC).
export default function ArchiveOverviewOverlay({
  archive,
  categories,
  loggedIn,
  onClose,
  onSelectPage,
}: {
  archive: ArchiveMetadata
  categories: CategoryMetadata[] | undefined
  loggedIn: boolean
  onClose: () => void
  onSelectPage: (page: number) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const staticCategories = (categories ?? []).filter((c) => !c.search)
  const archiveCategories = staticCategories.filter((c) => c.archives.includes(archive.arcid))

  const chapters = archive.toc.length > 0 ? archive.toc : null

  async function addToCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'PUT' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function removeFromCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'DELETE' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function deleteArchive() {
    if (
      !window.confirm(
        t('This will delete both metadata and matching files from your system! Please use with caution.') ??
          undefined,
      )
    ) {
      return
    }
    await fetch(`/api/archives/${archive.arcid}`, { method: 'DELETE' })
    navigate('/')
  }

  const pageCount = archive.pagecount

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — legacy's own JS explicitly shows it
          (`fadeTo`) when opening an overlay rather than relying on presence in the DOM, so this
          needs the same explicit override or clicking it (or even seeing it) does nothing. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: 'block', opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay">
        <h2 className="ih" style={{ textAlign: 'center' }}>
          {t('Archive Overview')}
        </h2>

        <div id="tagContainer" className="caption caption-tags caption-reader">
          <br />
          <div style={{ marginBottom: 16 }}>
            <div className="id3 nocrop reader-thumbnail">
              <img alt="" src={`/api/archives/${archive.arcid}/thumbnail`} />
            </div>

            {loggedIn && (
              <div style={{ display: 'inline-block', verticalAlign: 'middle' }}>
                <h2>{t('Admin Options')}</h2>

                <input
                  className="stdbtn"
                  type="button"
                  value={t('Edit Archive Metadata') ?? undefined}
                  onClick={() => navigate(`/edit/${archive.arcid}`)}
                />
                <input
                  className="stdbtn"
                  type="button"
                  value={t('Delete Archive') ?? undefined}
                  onClick={() => void deleteArchive()}
                />
                <br />

                <h2>{t('Categories')}</h2>
                <div style={{ display: 'inline-block' }}>
                  {archiveCategories.map((c) => (
                    <div key={c.id} className="gt" style={{ fontSize: 14, padding: 4 }}>
                      <span className="label">{c.name}</span>
                      <a
                        href="#"
                        style={{ marginLeft: 4, marginRight: 2 }}
                        onClick={(e) => {
                          e.preventDefault()
                          void removeFromCategory(c.id)
                        }}
                      >
                        ×
                      </a>
                    </div>
                  ))}
                </div>

                <br />
                <span>{t('Add to : ')}</span>
                <select
                  id="category"
                  className="favtag-btn"
                  style={{ width: 200 }}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void addToCategory(e.target.value)
                  }}
                >
                  <option value="">{t(' -- No Category -- ')}</option>
                  {staticCategories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>

                <h2>{t('Rating')}</h2>
                <RatingWidget archiveId={archive.arcid} tags={archive.tags} />
              </div>
            )}
          </div>

          <TagsTable tags={archive.tags} />
        </div>

        <br />
        <br />

        <div className="overlay-bar">
          <div className="overlay-bar-left" />
          <h2 className="ih">{chapters ? t('Chapters') : t('Pages')}</h2>
          <div className="chapter-selector">
            {chapters && (
              <select
                id="chapter-select"
                style={{ width: 200 }}
                onChange={(e) => {
                  const page = Number(e.target.value)
                  if (page > 0) onSelectPage(page)
                }}
              >
                {chapters.map((c) => (
                  <option key={c.page} value={c.page}>
                    {c.name}
                  </option>
                ))}
              </select>
            )}
          </div>
        </div>

        <div id="pages-section" style={{ textAlign: 'center' }}>
          {Array.from({ length: pageCount }, (_, i) => i + 1).map((page) => (
            <div
              key={page}
              className="id1"
              style={{ display: 'inline-block', cursor: 'pointer' }}
              onClick={() => onSelectPage(page)}
            >
              <div className="id3 quick-thumbnail">
                <span className="page-number">{page}</span>
                <img
                  loading="lazy"
                  alt={`${t('Page')} ${page}`}
                  src={`/api/archives/${archive.arcid}/thumbnail?page=${page}`}
                />
              </div>
            </div>
          ))}
        </div>
      </div>
    </>
  )
}
