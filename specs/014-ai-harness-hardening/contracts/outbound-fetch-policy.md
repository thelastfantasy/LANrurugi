# Contract: Outbound Fetch Policy

**Feature**: `014-ai-harness-hardening` | **Story**: US1 | **Requirements**: FR-001…FR-006

## Purpose

One policy gates every outbound request the AI plugin wizard makes on the model's behalf. The single
invariant it exists to guarantee:

> **A substituted credential value is never sent to a host outside the permitted set, and no request
> is ever sent to a private, loopback, link-local or non-HTTP destination — regardless of what the
> model asks for.**

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

The permitted set is computed once per generation from the first three rows only. Nothing in the last
two rows can add to it.

## Permitted-set rule

- A host is permitted if it equals a member of the set, or is a proper subdomain of one, compared at
  DNS-label boundaries (`img.example.com` permitted when `example.com` is; `evil-example.com` never
  permitted).
- Scheme must be `http` or `https`. Every other scheme is denied.
- Port is not constrained.

## Decision procedure

Evaluated in this order; the first denial wins.

1. **Parse** the URL. Unparseable → `deny / scheme_not_allowed`.
2. **Scheme** is not `http`/`https` → `deny / scheme_not_allowed`.
3. **Host** not in the permitted set → `credentialed ? deny / host_not_permitted : deny / host_not_permitted`.
   (Host permission is required for every request, credentialed or not — a model-composed destination
   outside the user's own site is not something this feature fetches.)
4. **Resolve** the host. Resolution failure → `deny / unresolvable_host`.
5. **Address class** — any resolved address in loopback, private, link-local, unique-local,
   unspecified or multicast space → `deny / private_address`. Resolved addresses are pinned for the
   request so the connection cannot land elsewhere after the check.
6. **Credential references** — every `{{field}}` in the headers must name a field in
   `credential_fields`. An unresolvable reference → `deny / unknown_credential_reference`.
7. Otherwise → `allow`.

## Redirect hops

Each hop is evaluated by steps 3–5 independently.

- Hop host still permitted → continue, credentials retained.
- Hop host leaves the set → continue **without** credential headers, record
  `withheld_at_hop`, and disclose it in the result. Verdict for the overall request becomes
  `allow_without_credentials`.
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

## Non-goals

- Not a general SSRF proxy for plugins. The Deno plugin sandbox's own `declared_permissions.net`
  model is unchanged and remains the control for plugin-originated traffic (Principle IV).
- Not a robots/ToS or rate-limit policy. Per-domain fetch concurrency is out of scope here.
- Not a defence against a compromised server process; the policy defends against a misled *model*.

## Verifiable guarantees (mapped to spec)

| Guarantee | Spec |
| --- | --- |
| Evaluation strictly precedes any traffic | FR-001, SC-001 |
| Credential never leaves its site | FR-002, SC-001 |
| Every hop re-evaluated | FR-003 |
| Private/loopback/link-local denied | FR-004 |
| Denials are legible, not silent empties | FR-005 |
| Unknown credential reference denied, not emptied | FR-006 |
| Same-site requests behave exactly as today | scenario 1 of US1, SC-008 |
