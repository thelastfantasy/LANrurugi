# Contract: `GET /api/llm/usage`

**Feature**: `014-ai-harness-hardening` | **Story**: US3 | **Requirements**: FR-012…FR-015

## Purpose

Report per-feature model usage over a window. Answers, without reading logs: *how many requests, how
many tokens, how much time, and how many failures did each AI feature consume?*

Today this is answerable for 1 of 9 features (translation, which has its own spend ledger with
enforcement semantics). This endpoint observes only — it never refuses a call.

## Request

```http
GET /api/llm/usage?window=day|week
```

| Parameter | Values | Default |
| --- | --- | --- |
| `window` | `day` (today, UTC), `week` (last 7 UTC days including today) | `day` |

Authorization: same session-scoped authority as the rest of `/api/*`; guests denied via the route
policy file. No token-scoped admin/guest grant.

## Response

```json
{
  "window": "week",
  "features": [
    {
      "feature": "plugin_wizard_generate",
      "status": "reported",
      "requests": 12,
      "elapsed_ms": 184300,
      "tokens": {
        "prompt": 41200,
        "completion": 9800,
        "total": 51000,
        "unreported_calls": 0
      },
      "failures": {
        "auth": 0,
        "balance": 0,
        "rate_limited": 1,
        "transient": 0,
        "timeout": 1,
        "malformed": 0,
        "rejected": 0
      }
    },
    {
      "feature": "tankoubon_rename",
      "status": "no_data"
    }
  ]
}
```

**Field semantics**

| Field | Meaning |
| --- | --- |
| `status` | `reported` when any call landed in the window; `no_data` when none did. A `no_data` row carries no figures — deliberately, so an unused feature is not displayed as costing nothing |
| `requests` | total calls, including failed ones |
| `elapsed_ms` | sum of per-call wall clock across all attempts |
| `tokens.unreported_calls` | calls in this feature whose provider response carried no usage block. Non-zero means the token figures undercount and must not be read as complete |
| `failures.*` | counts by class; their sum is ≤ `requests` (the difference being successes) |

**Never reported as zero**: token figures for a feature whose calls all lacked usage blocks are
omitted rather than shown as `0`. The client renders them as "not reported", not as "free".

## Ordering and completeness

Features are returned in a fixed order (stable across calls) so the panel does not reshuffle when a
feature becomes active. All 9 known feature ids are always present in some form — `reported` or
`no_data` — so a missing feature is visibly a bug rather than silently absent.

## Failure modes

| Condition | Response |
| --- | --- |
| Redis unavailable | `503` with `error: "llm_usage_unavailable"`; the panel shows unavailable, never zeros |
| Unknown `window` value | `400` with `error: "invalid_window"` |
| Guest / anonymous caller | denied by the route policy, before the handler |

## Retention and non-goals

- Window sums read what exists; missing day keys contribute nothing and no synthetic zero days are
  created.
- 40-day TTL per day key means a week window is always fully answerable.
- **Not** an enforcement mechanism: no feature may consult this endpoint or its storage to decide
  whether to run. Translation's existing budget enforcement stays exactly as it is, in its own
  storage, with its own semantics.
