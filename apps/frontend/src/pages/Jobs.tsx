import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { useClearFinishedJobs, useClearJobs, useJobs } from '../api/hooks'
import type { JobRecord, JobRecordState } from '../api/types'
import CodeBlock from '../components/CodeBlock'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

// Background Job Console (specs/002-job-console). Surfaces the existing in-process
// `lanrurugi_core::jobs::JobRegistry` as a browsable admin UI: list every tracked job with live
// state/progress, inspect a finished/failed job's result or error, clear terminal-state entries,
// and filter/search/paginate a long list — all of US4's counts/filter/search/paging are derived
// client-side from the single `useJobs()` array (research.md §5), no extra endpoint. Styling
// matches the ported pages' legacy conventions (`.ido`/`.ih`/`.itg`/`.stdbtn` — see Logs.tsx,
// Stats.tsx). The clear interaction mirrors legacy Minion Admin: a checkbox column + a toolbar
// "Remove selected" batch action, NOT a per-row button (legacy Minion is multi-select → toolbar,
// and a per-row `.stdbtn` column overflowed the `.ido` container). Deliberately omits Workers/
// Locks stats (single-process Tokio architecture has no worker processes / distributed locks —
// spec Assumptions).

const PAGE_SIZES = [10, 20, 50, 100]

/** Render order for states in the stat bar + filter. */
const STATE_ORDER: JobRecordState[] = ['active', 'queued', 'finished', 'failed']

/** State → i18n key (bare English word, added to the locale files in T024). */
const STATE_LABEL_KEYS: Record<JobRecordState, string> = {
  queued: 'Queued',
  active: 'Active',
  finished: 'Finished',
  failed: 'Failed',
}

/** State → inline badge color, reusing the green/red the ported Settings page already uses. */
const STATE_COLOR: Record<JobRecordState, string> = {
  queued: 'rgb(66, 133, 244)',
  active: 'rgb(26, 165, 26)',
  finished: 'rgb(120, 120, 120)',
  failed: 'rgb(207, 37, 37)',
}

const isTerminal = (s: JobRecordState) => s === 'finished' || s === 'failed'

export default function Jobs() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const jobs = useJobs()
  const clearJobs = useClearJobs()
  const clearFinished = useClearFinishedJobs()
  useApplyTheme()
  useDocumentTitle(t('Background Jobs') ?? undefined)

  // Stabilize the array identity so the derived `useMemo`s below don't recompute every render when
  // `jobs.data` is referentially unchanged.
  const all: JobRecord[] = useMemo(() => jobs.data ?? [], [jobs.data])

  // US4 (T020): per-state counts over the UNFILTERED array — fixed totals that must not change as
  // the admin filters/searches (research.md §5).
  const counts = useMemo(() => {
    const c: Record<JobRecordState, number> = { queued: 0, active: 0, finished: 0, failed: 0 }
    for (const job of all) c[job.state] += 1
    return c
  }, [all])

  const [stateFilter, setStateFilter] = useState<JobRecordState | 'all'>('all')
  const [search, setSearch] = useState('')
  const [page, setPage] = useState(0)
  const [pageSize, setPageSize] = useState(50)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [status, setStatus] = useState('')

  // US4 (T021/T022): state + name filters applied to the rendered list.
  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase()
    return all.filter((job) => {
      if (stateFilter !== 'all' && job.state !== stateFilter) return false
      if (needle && !job.name.toLowerCase().includes(needle)) return false
      return true
    })
  }, [all, stateFilter, search])

  // US4 (T023): pagination applied after the filters. Clamp the page when the filtered set shrinks.
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize))
  const safePage = Math.min(page, pageCount - 1)
  const paginated = filtered.slice(safePage * pageSize, safePage * pageSize + pageSize)

  // Selection is by id (persists across filter/page changes). Prune against the live set so a job
  // that vanished (cleared/evicted) between select and action isn't counted.
  const liveIds = useMemo(() => new Set(all.map((j) => j.id)), [all])
  const liveSelected = useMemo(
    () => new Set([...selected].filter((id) => liveIds.has(id))),
    [selected, liveIds],
  )

  // Select-all operates over the terminal jobs on the current page (legacy Minion's current-view
  // semantics). Non-terminal jobs can't be cleared (FR-004), so their checkboxes stay disabled.
  const pageTerminalIds = useMemo(
    () => paginated.filter((j) => isTerminal(j.state)).map((j) => j.id),
    [paginated],
  )
  const allPageSelected =
    pageTerminalIds.length > 0 && pageTerminalIds.every((id) => liveSelected.has(id))
  const somePageSelected = pageTerminalIds.some((id) => liveSelected.has(id))

  const selectAllRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = somePageSelected && !allPageSelected
    }
  }, [somePageSelected, allPageSelected])

  function toggleAll() {
    setSelected((prev) => {
      const next = new Set(prev)
      if (pageTerminalIds.every((id) => next.has(id))) {
        pageTerminalIds.forEach((id) => next.delete(id))
      } else {
        pageTerminalIds.forEach((id) => next.add(id))
      }
      return next
    })
  }

  function toggleOne(job: JobRecord) {
    if (!isTerminal(job.state)) return
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(job.id)) next.delete(job.id)
      else next.add(job.id)
      return next
    })
  }

  async function handleRemoveSelected() {
    const ids = [...liveSelected]
    if (ids.length === 0) return
    const { succeeded, failed } = await clearJobs.mutateAsync(ids)
    setSelected(new Set())
    setStatus(
      t('Removed {{n}} jobs.', { n: succeeded }) +
        (failed > 0 ? ' ' + t('{{n}} could not be removed.', { n: failed }) : ''),
    )
  }

  async function handleClearFinished() {
    const result = await clearFinished.mutateAsync()
    setSelected(new Set())
    setStatus(t('Cleared {{n}} finished jobs.', { n: result.cleared }) ?? '')
  }

  const isEmpty = !jobs.isLoading && all.length === 0
  const removing = clearJobs.isPending

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12 }}>
      <h1 className="ih">{t('Background Jobs')}</h1>
      <p style={{ fontSize: '9pt' }}>
        {t('The background job console shows currently running and recently concluded tasks.')}
      </p>

      {/* US4 stat bar (T020) — each count is a clickable state filter (T021). */}
      <div className="control-btn-group" style={{ flexWrap: 'wrap', gap: 4 }}>
        <FilterChip
          active={stateFilter === 'all'}
          label={t('All') ?? ''}
          count={all.length}
          onClick={() => {
            setStateFilter('all')
            setPage(0)
          }}
        />
        {STATE_ORDER.map((state) => (
          <FilterChip
            key={state}
            active={stateFilter === state}
            label={t(STATE_LABEL_KEYS[state]) ?? ''}
            count={counts[state]}
            color={STATE_COLOR[state]}
            onClick={() => {
              setStateFilter(state)
              setPage(0)
            }}
          />
        ))}
      </div>

      <div className="control-btn-group" style={{ marginTop: 8, alignItems: 'center' }}>
        <input
          className="stdinput"
          type="text"
          style={{ flexGrow: 1, minWidth: 160 }}
          placeholder={t('Search by name…') ?? ''}
          value={search}
          onChange={(e) => {
            setSearch(e.target.value)
            setPage(0)
          }}
        />
        {/* Toolbar batch action over the checkbox selection — only terminal jobs are selectable, so
            every selected id is clearable. Disabled (not hidden) at zero so the control stays
            discoverable. */}
        <button
          type="button"
          className="stdbtn"
          disabled={liveSelected.size === 0 || removing}
          onClick={() => void handleRemoveSelected()}
        >
          {removing
            ? t('Removing…')
            : t('Remove selected ({{n}})', { n: liveSelected.size })}
        </button>
        {/* Unscoped nuclear option (research.md §5 / FR-004): always every finished+failed job
            server-side, regardless of the active filter or selection. */}
        <button
          type="button"
          className="stdbtn"
          disabled={clearFinished.isPending}
          onClick={() => void handleClearFinished()}
        >
          {clearFinished.isPending ? t('Clearing…') : t('Clear all finished')}
        </button>
        <input
          type="button"
          id="return"
          className="stdbtn"
          value={t('Return to Library') ?? undefined}
          onClick={() => navigate('/')}
        />
      </div>

      {status && <p style={{ fontSize: '9pt' }}>{status}</p>}

      {jobs.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {/* FR-007: explicit empty state, not a blank page or error. */}
      {isEmpty && (
        <div id="nojobs" style={{ textAlign: 'center', margin: '24px 0' }}>
          <i className="fa fa-3x fa-inbox"></i>
          <p>{t('No background jobs yet.')}</p>
          <p style={{ fontSize: '9pt' }}>
            {t(
              'Jobs will appear here as you trigger thumbnail regeneration, backups, restores, duplicate scans, and other background work.',
            )}
          </p>
        </div>
      )}

      {/* Filter/search yielded nothing (but jobs exist) — a targeted hint, distinct from the
          fresh-server empty state above (FR-007 covers all-empty; this covers all>0 && none match). */}
      {all.length > 0 && filtered.length === 0 && (
        <div style={{ textAlign: 'center', margin: '24px 0' }}>
          <i className="fa fa-3x fa-search"></i>
          <p>{t('No jobs match the current filter.')}</p>
        </div>
      )}

      {all.length > 0 && filtered.length > 0 && (
        <>
          {/* `overflow-x: auto` wrapper is the bulletproof guard against any column content (long
              unbreakable job names, the progress bar) ever escaping the `.ido` container — the
              original per-row `.stdbtn` column overflowed without it. */}
          <div style={{ overflowX: 'auto', marginTop: 16 }}>
            <table className="itg" style={{ width: '100%', minWidth: 560 }}>
              <thead>
                <tr>
                  <th style={{ width: 32, textAlign: 'center' }}>
                    <input
                      ref={selectAllRef}
                      type="checkbox"
                      aria-label={t('Select all') ?? undefined}
                      checked={allPageSelected}
                      onChange={toggleAll}
                    />
                  </th>
                  <th style={{ textAlign: 'left' }}>{t('Name')}</th>
                  <th style={{ width: 100, textAlign: 'center' }}>{t('State')}</th>
                  <th style={{ width: 180, textAlign: 'center' }}>{t('Progress')}</th>
                  <th style={{ width: 28 }}></th>
                </tr>
              </thead>
              <tbody>
                {paginated.map((job) => (
                  <JobRow
                    key={job.id}
                    job={job}
                    selected={liveSelected.has(job.id)}
                    onToggleSelect={() => toggleOne(job)}
                  />
                ))}
              </tbody>
            </table>
          </div>

          {/* US4 pagination (T023). */}
          <div
            className="control-btn-group"
            style={{ marginTop: 8, justifyContent: 'center', alignItems: 'center' }}
          >
            <button
              type="button"
              className="stdbtn"
              disabled={safePage === 0}
              onClick={() => setPage(safePage - 1)}
            >
              {t('Previous')}
            </button>
            <span style={{ fontSize: '9pt' }}>
              {t('Page {{n}} of {{total}}', { n: safePage + 1, total: pageCount })}
            </span>
            <button
              type="button"
              className="stdbtn"
              disabled={safePage >= pageCount - 1}
              onClick={() => setPage(safePage + 1)}
            >
              {t('Next')}
            </button>
            <label style={{ fontSize: '9pt' }}>
              {t('per page')}
              <select
                className="stdinput"
                style={{ marginLeft: 6, width: 'auto' }}
                value={pageSize}
                onChange={(e) => {
                  setPageSize(Number(e.target.value))
                  setPage(0)
                }}
              >
                {PAGE_SIZES.map((size) => (
                  <option key={size} value={size}>
                    {size}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </>
      )}
    </div>
  )
}

/** One job row + its expandable detail (US2/T010). The leading checkbox drives multi-select clear
 * (US3); only terminal jobs are selectable, others render a disabled checkbox so the row still
 * aligns under the select-all column. No per-row action button — clearing is via the toolbar. */
function JobRow({
  job,
  selected,
  onToggleSelect,
}: {
  job: JobRecord
  selected: boolean
  onToggleSelect: () => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const selectable = isTerminal(job.state)
  const pct = Math.round(job.progress * 100)

  return (
    <>
      <tr className={open ? 'gtr1' : undefined}>
        <td style={{ textAlign: 'center' }}>
          <input
            type="checkbox"
            checked={selected}
            disabled={!selectable}
            onChange={onToggleSelect}
            aria-label={t('Select job') ?? undefined}
          />
        </td>
        <td
          onClick={() => setOpen((o) => !o)}
          style={{ cursor: 'pointer', wordBreak: 'break-word', textAlign: 'left' }}
        >
          {job.name}
        </td>
        <td>
          <span style={{ color: STATE_COLOR[job.state], fontWeight: 'bold' }}>
            {t(STATE_LABEL_KEYS[job.state])}
          </span>
        </td>
        <td>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <div
              style={{
                flex: 1,
                height: 8,
                background: 'rgba(128,128,128,0.25)',
                borderRadius: 4,
                overflow: 'hidden',
              }}
            >
              <div style={{ width: `${pct}%`, height: '100%', background: STATE_COLOR[job.state] }} />
            </div>
            <span style={{ fontSize: '9pt', minWidth: 34, textAlign: 'right' }}>{pct}%</span>
          </div>
        </td>
        <td
          style={{ textAlign: 'center', cursor: 'pointer' }}
          onClick={() => setOpen((o) => !o)}
        >
          <i className={`fa fa-caret-${open ? 'down' : 'right'}`} aria-hidden="true"></i>
        </td>
      </tr>
      {open && (
        <tr>
          <td></td>
          <td colSpan={4}>
            <JobDetail job={job} />
          </td>
        </tr>
      )}
    </>
  )
}

/** US2 (T010): a finished job's result (syntax-highlighted JSON via `CodeBlock`), or a failed job's
 * specific error (a red-tinted panel) — the data is already in `GET /jobs`'s response, so no extra
 * fetch is needed. */
function JobDetail({ job }: { job: JobRecord }) {
  const { t } = useTranslation()
  if (job.state === 'failed') {
    return (
      <div style={{ fontSize: '9pt' }}>
        <strong style={{ color: STATE_COLOR.failed }}>{t('Error')}: </strong>
        <pre
          style={{
            margin: '4px 0 0',
            padding: '8px 12px',
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
            color: STATE_COLOR.failed,
            background: 'rgba(207, 37, 37, 0.08)',
            borderLeft: `3px solid ${STATE_COLOR.failed}`,
            borderRadius: 4,
          }}
        >
          {job.error || t('(no error message captured)')}
        </pre>
      </div>
    )
  }
  if (job.state === 'finished') {
    const code =
      job.result == null
        ? t('(no result)')
        : typeof job.result === 'string'
          ? job.result
          : JSON.stringify(job.result, null, 2)
    return (
      <div style={{ fontSize: '9pt' }}>
        <strong>{t('Result')}: </strong>
        <div style={{ marginTop: 4 }}>
          <CodeBlock code={code} language="json" />
        </div>
      </div>
    )
  }
  return (
    <div style={{ fontSize: '9pt', color: 'rgb(120,120,120)' }}>
      {t('This job is still running — no result yet.')}
    </div>
  )
}

function FilterChip({
  active,
  label,
  count,
  color,
  onClick,
}: {
  active: boolean
  label: string
  count: number
  color?: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className="stdbtn"
      onClick={onClick}
      style={{
        fontWeight: active ? 'bold' : 'normal',
        outline: active ? '2px solid currentColor' : 'none',
        color: color ?? 'inherit',
      }}
    >
      {label} ({count})
    </button>
  )
}
