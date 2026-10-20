# Contract: Outbound Fetch Policy

**Feature**: `014-ai-harness-hardening` | **Story**: US1 | **Requirements**: FR-001…FR-006, FR-028, FR-030

## Purpose

One policy gates every outbound request the AI plugin wizard makes on the model's behalf. The single
invariant it exists to guarantee:

> **A substituted credential value is never sent to a host outside the target site, no request is
> ever sent to a host outside the user-supplied sites, and no request is ever sent to a private,
> loopback, link-local or non-HTTP destination — regardless of what the model asks for.**

The policy is evaluated by the system before execution. The model has no way to inspect, widen or
bypass it.

## Inputs

| Input | Source | Trust |
| --- | --- | --- |
| `target_domain` | the wizard session's domain, supplied with the generation request | user |
| link hosts | hosts of `test_links` and `auxiliary_reference_urls` in the same request | user |
| `credential_fields` | the associated login plugin's declared field names | user/plugin |
| `url` | model-composed | **untrusted** |
| `headers` | model-composed, may contain `{{field}}` placeholders | **untrusted** |

Two host sets are computed once per generation from the first two rows only. Nothing in the last
two rows can add to them.

| Set | Members | Governs |
| --- | --- | --- |
| `fetch_hosts` | `target_domain` and every link host | whether a request may be sent at all |
| `credential_hosts` | `target_domain` only | whether a request may carry a substituted credential |

## Host rule

- A host is in a set if it equals a member, or is a proper subdomain of one, compared at DNS-label
  boundaries (`img.example.com` is in when `example.com` is; `evil-example.com` never is).
- Scheme must be `http` or `https`. Every other scheme is denied.
- Port is not constrained.

## Decision procedure

Evaluated in this order; the first denial wins.

1. **Parse** the URL. Unparseable → `deny / scheme_not_allowed`.
2. **Scheme** is not `http`/`https` → `deny / scheme_not_allowed`.
3. **Host** not in `fetch_hosts` → `deny / host_not_permitted`. Required for every request,
   credentialed or not.
4. **Resolve** the host. Resolution failure → `deny / unresolvable_host`.
5. **Address class** — any resolved address in loopback, private, link-local, unique-local,
   unspecified or multicast space → `deny / private_address`. Resolved addresses are pinned for the
   request so the connection cannot land elsewhere after the check.
6. **Credential references** — every `{{field}}` in the headers must name a field in
   `credential_fields`. An unresolvable reference → `deny / unknown_credential_reference`.
7. **Credential host** — a request with any credential reference whose host is not in
   `credential_hosts` → `deny / credential_host_not_permitted`.
8. Otherwise → `allow`.

## Redirect hops

Each hop is evaluated by steps 3–5 independently.

- Hop host in `credential_hosts` → continue, credentials retained.
- Hop host in `fetch_hosts` but not `credential_hosts` → continue **without** credential headers,
  record `withheld_at_hop`, and disclose it in the result. Verdict becomes
  `allow_without_credentials`.
- Hop host outside `fetch_hosts` → end the chain with `deny / redirect_left_permitted_set`; nothing is
  sent to that hop. (Continuing without credentials would still let an open redirect carry
  model-composed data — including authenticated page content the model has read — to any host.)
- Hop resolution lands in private space → terminate the chain with `deny / private_address`.
- The existing redirect cap (10) and per-fetch timeout are unchanged and still apply.

## Result shape

The tool result returned to the model carries, in addition to today's fields:

| Field | When present |
| --- | --- |
| `status: "refused"` | the request was denied |
| `refusal_reason` | with `refused`; one of the decision reasons |
| `withheld_credentials_from_hop` | when credentials were dropped mid-chain |

The refusal text MUST state, in the model's own working language, *which rule* refused it and *what
would be permitted*, so the model can retry inside the boundary instead of concluding the site is
broken. It MUST NOT echo the credential value or the resolved address.

## Trial-run egress (FR-028)

A trial run executes model-written code with the user's credentials, so the code's declared network
hosts are a second path off the target site. Before a trial run executes a draft:

1. Read the draft's declared hosts from its zero-permission `pluginInfo()` probe (already performed
   today).
2. `outside_hosts` = declared hosts not in `fetch_hosts`.
3. If `outside_hosts` is empty, or every one of them is in the request's `approved_hosts`, run as
   today. Otherwise refuse with `409 trial_hosts_need_approval` and the list, before anything runs.

`approved_hosts` is sent by the frontend only after the user confirmed those hosts in the trial-run
UI for this one run. It is never stored, never inferred from the draft, and never supplied by the
model. The host-side plugin HTTP client's existing per-hop enforcement of the declared hosts is
unchanged; this check decides only whether the draft may run.

## Untrusted content framing (FR-030)

Every fetched page's `content` reaches the model inside a fixed frame that names it as untrusted page
data from a given URL, and the generation's system prompt states that instructions inside such a
frame are never to be followed. The frame is the same for every result, so a result under the cap
differs from today's only by the frame. This is defense in depth: every guarantee above holds without
it.

## Non-goals

- Not a general SSRF proxy for plugins. The Deno plugin sandbox's own `declared_permissions.net`
  model is unchanged and remains the control for installed plugins' traffic (Principle IV); the
  trial-run check above only adds a user confirmation before a not-yet-saved draft runs.
- Not a robots/ToS or rate-limit policy. Per-domain fetch concurrency is out of scope here.
- Not a defence against a compromised server process; the policy defends against a misled *model*.

## Verifiable guarantees (mapped to spec)

| Guarantee | Spec |
| --- | --- |
| Evaluation strictly precedes any traffic | FR-001, SC-001 |
| Credential never leaves its site | FR-002, SC-001 |
| Every hop re-evaluated; a hop off the supplied sites ends the chain | FR-003 |
| Private/loopback/link-local denied | FR-004 |
| Denials are legible, not silent empties | FR-005 |
| Unknown credential reference denied, not emptied | FR-006 |
| Same-site requests behave exactly as today | scenario 1 of US1, SC-008 |
| A draft's off-site hosts need the user's approval before a trial run | FR-028, SC-001 |
| Fetched content is framed as untrusted data | FR-030 |
