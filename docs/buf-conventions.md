# Buf conventions — OpenAPI and Proto linting

This document describes how [Buf](https://buf.build) is used in this repository
to lint the OpenAPI specification and (in future) Protobuf definitions, and what
rules contributors must follow.

---

## Table of contents

- [Why Buf?](#why-buf)
- [Repository layout](#repository-layout)
- [Running buf locally](#running-buf-locally)
- [Lint rules](#lint-rules)
- [Breaking-change rules](#breaking-change-rules)
- [OpenAPI conventions](#openapi-conventions)
- [Future proto conventions](#future-proto-conventions)
- [Pre-commit integration](#pre-commit-integration)
- [CI integration](#ci-integration)
- [Adding new endpoints or fields](#adding-new-endpoints-or-fields)
- [FAQ](#faq)

---

## Why Buf?

Buf provides:

1. **Deterministic linting** — the same rules run locally (pre-commit) and in CI
   so "it passes on my machine" is always true before opening a PR.
2. **Breaking-change detection** — `buf breaking` compares the current branch
   against `main` and fails if any wire-incompatible change is introduced, e.g.
   removing a path, renaming a required field, or changing a type.
3. **Plugin ecosystem** — when proto files are added, buf handles code generation
   (Go stubs, TypeScript client, gRPC-Gateway) from a single `buf.gen.yaml`.

---

## Repository layout

```
.
├── buf.yaml           # Workspace config: modules, lint rules, breaking rules
├── buf.gen.yaml       # Code-gen plugins (OpenAPI lint + future proto)
├── openapi/
│   └── openapi.yaml   # OpenAPI 3.1 spec for the Stellabill REST API
└── proto/             # (future) Protobuf service definitions
    └── .gitkeep
```

---

## Running buf locally

### Install buf

```bash
# macOS (Homebrew)
brew install bufbuild/buf/buf

# Linux / Windows — download the binary
# https://github.com/bufbuild/buf/releases/tag/v1.47.2
curl -sSL \
  https://github.com/bufbuild/buf/releases/download/v1.47.2/buf-Linux-x86_64 \
  -o /usr/local/bin/buf && chmod +x /usr/local/bin/buf
```

### Lint the OpenAPI spec

```bash
buf lint
```

Exit 0 means no violations. All errors are printed to stdout with file, line,
and column, e.g.:

```
openapi/openapi.yaml:42:5:Field "foo" must be camelCase.
```

### Check for breaking changes against main

```bash
buf breaking --against '.git#branch=main'
```

This is safe to run locally at any time. It exits 0 on the `main` branch
(no divergence) and exits non-zero only when a breaking change is detected on
a feature branch.

### Validate buf config

```bash
buf config ls-lint-rules   # list active lint rules
buf build                  # build all modules (validates YAML structure)
```

---

## Lint rules

`buf.yaml` activates the `DEFAULT` lint group. Key rules for the OpenAPI module:

| Rule | Description |
|------|-------------|
| `FIELD_NAMES_LOWER_CAMEL_CASE` | All JSON field names must be `lowerCamelCase`. |
| `ENUM_VALUE_PREFIX` | Enum values must be prefixed with the enum name. |
| `COMMENT_ENUM`, `COMMENT_FIELD`, etc. | Public types and fields must have descriptions. |
| `PACKAGE_SAME_DIRECTORY` | (proto only) All files in a package live in one directory. |

The rule `PACKAGE_VERSION_SUFFIX` is suppressed until proto files are added
(it does not apply to OpenAPI).

---

## Breaking-change rules

`buf.yaml` activates the `FILE` breaking-change group. A PR **will be blocked**
by CI if any of the following occur:

| Change | Why it is breaking |
|--------|--------------------|
| Remove an API path (`DELETE /subscriptions/{id}`) | Callers get 404 unexpectedly. |
| Remove or rename a required request field | Clients sending the old field get 400. |
| Remove or rename a response field | Clients reading the field get `null`. |
| Change a field type (e.g. `integer` → `string`) | Deserialisation failures. |
| Remove an enum value | Clients on the old value hit unhandled cases. |
| Change an `operationId` | Generated SDK method names break. |

**Allowed** (not breaking):

- Adding new optional fields to request or response schemas.
- Adding new paths or operations.
- Adding new enum values (suppressed via `ENUM_VALUE_NO_DELETE_UNLESS_NAME_RESERVED`).
- Adding or changing description text.
- Relaxing a constraint (e.g. increasing `maximum`).

When you must introduce a breaking change, version the API path
(`/v2/subscriptions/…`) and deprecate the old path with `deprecated: true`
before removal. Update this document with a migration note.

---

## OpenAPI conventions

Follow these rules when editing `openapi/openapi.yaml`:

### Naming

- **Path parameters**: `lowerCamelCase` (`subscriptionId`, not `subscription_id`).
- **Query parameters**: `lowerCamelCase`.
- **JSON field names**: `lowerCamelCase` in all request and response schemas.
- **Schema names** (`$ref` targets): `UpperCamelCase` (`SubscriptionResponse`).
- **`operationId`**: `lowerCamelCase` verb + noun (`createSubscription`, `batchCharge`).
- **Tags**: `lowercase`, single word or hyphenated (`subscriptions`, `charges`).

### Descriptions

Every path, operation, schema, and non-obvious field must have a `description`.
Buf's `COMMENT_*` rules enforce this for proto; for OpenAPI we rely on PR review
since the buf OpenAPI plugin only checks structure.

### Error responses

Every mutating operation must declare at minimum:
`400`, `401`, `404` (if path-parameterised), and `500`.
Use `$ref: "#/components/responses/…"` to avoid duplication.

### Status codes

| Code | Meaning in Stellabill |
|------|----------------------|
| 201  | Resource created (`POST /subscriptions`). |
| 200  | Mutation accepted (all other mutations). |
| 400  | Invalid request (bad field value, missing required field). |
| 401  | Auth failure (wrong signer, missing admin key). |
| 404  | Resource not found. |
| 409  | Conflict / invalid state transition. |
| 422  | Well-formed but unprocessable (e.g. insufficient balance). |
| 500  | Unexpected server error. |

### Stellar addresses

Stellar account addresses are always validated against:

```yaml
pattern: "^[GC][A-Z2-7]{55}$"
```

Include this pattern on every `address`-typed field.

---

## Future proto conventions

When `.proto` files are added to `proto/`:

1. **Package name**: `stellabill.<service>.<version>`, e.g.
   `stellabill.billing.v1`.
2. **Version suffix**: all packages must end in `v<N>` (buf `PACKAGE_VERSION_SUFFIX` rule, currently suppressed, will be re-enabled).
3. **File layout**: one service per file, named `<service>.proto`.
4. **Field numbers**: never reuse a deleted field number; mark removed fields
   `reserved`.
5. **Deprecation**: use the `deprecated = true` option before removal, wait one
   release cycle, then remove.
6. **Imports**: use Well-Known Types (`google.protobuf.Timestamp`, etc.) in
   preference to `int64` for semantic clarity.

---

## Pre-commit integration

`buf lint` and `buf breaking` run automatically before every `git commit` if
pre-commit is installed:

```bash
pip install pre-commit
pre-commit install
```

To run all hooks manually:

```bash
pre-commit run --all-files
```

The hooks only fire on changes to `openapi/`, `proto/`, `buf.yaml`, or
`buf.gen.yaml` to keep commit latency low for Rust-only changes.

---

## CI integration

The `.github/workflows/buf-lint.yml` workflow runs on every PR and push to
`main` that touches spec or config files. It:

1. Checks out the full git history (`fetch-depth: 0`).
2. Installs buf `v1.47.2` (pinned).
3. Runs `buf lint` — any violation fails the job and blocks merge.
4. Runs `buf breaking --against '.git#branch=main'` — any breaking change
   detected on a PR branch fails the job and blocks merge.

A failing lint report is uploaded as a CI artifact for easy download.

---

## Adding new endpoints or fields

1. Edit `openapi/openapi.yaml`.
2. Run `buf lint` locally — fix any violations before pushing.
3. Run `buf breaking --against '.git#branch=main'` — confirm the change is
   non-breaking, or follow the versioning guidance above if it is.
4. Open a PR. CI will re-run both checks automatically.
5. If you are adding a new schema, add a description to every field and
   register the schema under `components/schemas` with a `$ref` reference.

---

## FAQ

**Q: buf lint passes locally but fails in CI.**  
A: Check that you are using buf `v1.47.2` (same as CI). Run `buf --version`.

**Q: I need to rename a required field.**  
A: Add the new field as optional, deprecate the old field (`deprecated: true`),
release, migrate clients, then remove the old field in a follow-up PR with a
version bump.

**Q: Can I skip buf-breaking for a hotfix?**  
A: No — the check is required on all PRs into `main`. Version the path instead.
If you believe a change is genuinely non-breaking, open an issue with evidence
and a maintainer can merge via a direct push after review.

**Q: Where do I add proto files?**  
A: Create them under `proto/<package>/v1/<service>.proto`. Uncomment the proto
module in `buf.yaml` and the generator plugins in `buf.gen.yaml`.
