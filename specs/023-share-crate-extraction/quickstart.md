# Quickstart: Share Crate Extraction

## 0. Baseline

Run these before touching anything and keep the outputs; every later step diffs against them.

```sh
cargo tree -p lanrurugi-api > /tmp/tree-before.txt
mise run docs-api -- --check           # route table snapshot
scripts/share-network-matrix.sh        # matrix report
```

## 1. The crate exists and does not see the API

```sh
cargo tree -p lanrurugi-share | grep -c lanrurugi-api    # must be 0
grep -rn "AppState\|lanrurugi_api" crates/lanrurugi-share/src | wc -l   # must be 0
mise run check-crate -- lanrurugi-share
```

## 2. Sharing tests no longer need an API state

```sh
mise run test-crate -- lanrurugi-share
```

The sharing tests construct the engine with the in-memory host; a test that still builds `AppState` fails this check by
inspection (`grep -n AppState crates/lanrurugi-share/tests`).

## 3. Nothing observable changed

```sh
mise run docs-api -- --check            # same route table as step 0
mise run check-crate -- lanrurugi-api   # handlers still compile
mise run test-crate -- lanrurugi-api
scripts/share-network-matrix.sh         # same topologies, same results
```

Wire compatibility: import a share between two builds across the change in both directions (old → new, new → old); the
records published by one must parse and connect in the other.

## 4. Background tasks and start order

```sh
# start the server with RUST_LOG=info and diff the task-start log lines
```

The set of started tasks and their order match step 0. A task that moved to the engine but is spawned by the API, or the
reverse, is a failure.

## 5. Review the diff, not the files

```sh
git log --oneline --stat   # four commits: types, engine, adapter, tests
```

Each commit builds; the engine commit is a move plus boundary substitution, with no logic rewrite. Any behaviour change
found during the move gets its own commit and is reported, not smuggled in.

## 6. Rollback

Revert the four commits in reverse order. Because no key, format or path changed, reverting is a plain code revert with
no data migration.
