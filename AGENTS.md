# Light

Read only the code, tests, and documentation relevant to the change.

## Design

- Treat correctness, data safety, and performance as product requirements.
- Keep one owner for each mutable fact.
- Prefer direct, bounded designs with clear failure behavior.
- Do not add abstractions or hot-path complexity without a concrete need.
- Measure filesystem and process-boundary costs.

## Verification

Use the repository commands:

```sh
just build
just test
just fmt
just lint
just check
just ci-check
```

Run focused checks while developing and `just check` before completing a substantial change. State
what was run and do not ignore failures.
