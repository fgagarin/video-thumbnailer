<!-- SYNC IMPACT REPORT
Version change: N/A (initial template) → 1.0.0
Modified principles: N/A — initial population from template placeholders
Added sections:
  - Core Principles (I. Code Quality, II. Testing Standards, III. UX Consistency, IV. Performance)
  - Development Workflow
  - Quality Gates
  - Governance
Removed sections: None
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ (Constitution Check gates filled at plan-time; no template change required)
  - .specify/templates/spec-template.md ✅ (no updates required)
  - .specify/templates/tasks-template.md ✅ (no updates required)
Follow-up TODOs: None — all placeholders resolved
-->

# Video Thumbnailer Constitution

## Core Principles

### I. Code Quality (NON-NEGOTIABLE)

Every line of code submitted to this project MUST be clean, readable, and maintainable.

- Code MUST follow consistent style and formatting enforced by automated linting tools (no lint
  errors allowed to merge).
- Functions and modules MUST have a single, clear responsibility; files exceeding 300 lines MUST
  be refactored unless justified with a documented rationale.
- Magic numbers, hard-coded paths, and undocumented constants are PROHIBITED; all configuration
  MUST be externalised.
- Public interfaces MUST include docstrings or inline comments explaining intent, parameters, and
  return values.
- Dead code, commented-out blocks, and unused imports MUST be removed before merge.

**Rationale**: Consistent code quality reduces onboarding time, lowers defect rate, and keeps the
codebase approachable as the project grows.

### II. Testing Standards (NON-NEGOTIABLE)

No feature or bug-fix is done until tests demonstrate it works correctly and does not regress
existing behaviour.

- Unit tests MUST cover all pure functions and non-trivial logic; minimum branch coverage target is
  80%, enforced in CI.
- Integration tests MUST cover end-to-end thumbnail generation pipelines (input video → output
  image file) for each supported format.
- Tests MUST be written before or alongside implementation (TDD/BDD preferred); no feature is
  merged with zero tests.
- All tests MUST pass in CI before a pull request can be merged; flaky tests MUST be fixed or
  quarantined within one sprint.
- Test files MUST be colocated under `tests/` mirroring the `src/` structure and named
  `test_<module>.py`.

**Rationale**: Rigorous testing prevents regressions in media-processing pipelines where silent
failures (e.g., corrupt thumbnails, wrong timestamps) are hard to detect manually.

### III. User Experience Consistency

Every user-facing interface — CLI flags, output filenames, error messages, and logging — MUST
behave predictably and uniformly across all commands and entry points.

- CLI commands MUST follow a consistent verb-noun pattern (e.g., `generate`, `extract`, `list`)
  and MUST support `--help` with meaningful descriptions for every option.
- Error messages MUST be human-readable, include the offending input, and suggest a corrective
  action where possible.
- Output file naming MUST follow a documented, deterministic scheme (default:
  `<source_stem>_<timestamp_ms>.<ext>`); deviations require explicit user opt-in via flags.
- Exit codes MUST be consistent: `0` success, `1` user/input error, `2` internal/unexpected error.
- Breaking changes to CLI flags or output formats MUST be announced via a deprecation warning for
  at least one minor release before removal.

**Rationale**: Predictable UX allows users to build reliable automation pipelines around this tool
without unexpected breakage.

### IV. Performance Requirements

Thumbnail generation MUST meet defined latency and resource budgets to remain practical for
batch and real-time workflows.

- Single-image thumbnail extraction MUST complete in ≤ 500 ms wall-clock time for video files up
  to 2 GB on reference hardware (4-core CPU, 8 GB RAM).
- Batch processing throughput MUST reach ≥ 10 thumbnails/second for standard-definition video
  (≤ 1080p) under the same reference hardware.
- Peak memory usage per worker process MUST NOT exceed 512 MB; violations require profiling and
  an approved optimisation plan before merge.
- Performance-sensitive code paths MUST include benchmark tests (under `tests/benchmarks/`) that
  are run in CI on every release branch.
- Any change that degrades a benchmark by > 10% compared to the baseline MUST include a
  documented justification and approval before merging.

**Rationale**: Video processing is CPU- and memory-intensive; without explicit budgets, unchecked
regressions make the tool unusable in production pipelines.

## Development Workflow

All development MUST follow the trunk-based flow described below.

- Feature work MUST be done on short-lived branches named `<NNN>-<kebab-description>` branched
  from `main`.
- Every pull request MUST reference a spec or issue and include a self-review checklist covering
  the four Core Principles.
- CI MUST run lint, unit tests, integration tests, and benchmark gate checks on every PR; a red
  CI status blocks merge.
- Releases MUST be tagged with semantic versions (`MAJOR.MINOR.PATCH`); MAJOR bumps require a
  migration guide.
- Dependency updates MUST be reviewed for licence compatibility and pinned to exact versions in
  the lockfile.

## Quality Gates

The following gates MUST be satisfied before any code is merged to `main`.

| Gate | Tool / Check | Threshold |
|------|-------------|-----------|
| Lint | flake8 / ruff | Zero errors |
| Type checking | mypy (strict) | Zero errors |
| Unit test coverage | pytest-cov | ≥ 80% branch |
| Integration tests | pytest | All pass |
| Benchmark regression | pytest-benchmark | ≤ 10% degradation |
| Dependency audit | pip-audit | Zero high/critical CVEs |

Exceptions to any gate MUST be documented in the PR description and require explicit maintainer
approval.

## Governance

This constitution supersedes all other coding guidelines and informal practices in this repository.
Amendments follow the process below.

- Any contributor may propose an amendment via a pull request that modifies this file.
- Amendment PRs MUST include: (a) the rationale, (b) impact on existing features, and (c) a
  migration plan if behaviour changes.
- Amendments are ratified after review and approval by at least one maintainer.
- Version MUST be bumped according to semantic versioning rules defined in the
  `CONSTITUTION_VERSION` policy above.
- All pull request reviews MUST verify compliance with the four Core Principles as part of the
  standard review checklist.

**Version**: 1.0.0 | **Ratified**: 2026-04-10 | **Last Amended**: 2026-04-10
