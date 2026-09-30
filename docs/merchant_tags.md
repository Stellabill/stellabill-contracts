# Merchant Compliance-Category Tags

## Overview

Merchant tags are a small, admin-controlled classification system for merchant
accounts. Each merchant may carry up to `MAX_MERCHANT_TAGS` short symbolic tags
(e.g. `saas`, `media`, `nonprofit`) drawn from a global, admin-maintained
allowlist. Tags are purely descriptive metadata — they never gate charging,
withdrawals, or any other financial operation on their own — and exist to
support two off-chain workflows:

- **Indexing/reporting**: downstream indexers can group merchant activity by
  compliance category without maintaining a separate off-chain mapping.
- **Efficient compliance action**: an admin (or an automated policy reading
  on-chain tags) can identify every merchant in a given category and act on
  the whole class — e.g. via the existing [blocklist](blocklist.md) or
  [merchant pause](merchant_pause.md) mechanisms — instead of tracking
  categories manually.

Tags are entirely independent of `MerchantConfig`, the [merchant
whitelist](merchant_config.md), and merchant pause/blocklist state: setting or
clearing them never touches balances, approval status, or pause flags, and —
deliberately — is never *blocked* by those states either (see
[Interaction with blocklist/pause](#interaction-with-blocklist--pause)).

## Semantics

### Tag Allowlist

- **Storage**: `DataKey::TagAllowlist` — a single global `Vec<Symbol>`, instance storage.
- **Default**: empty (no tags valid until an admin configures the allowlist).
- **Authorization**: admin-only (`set_tag_allowlist`).
- **Validation**: the allowlist itself is not bounded by `MAX_MERCHANT_TAGS`
  (that limit is per-merchant, not on the catalog of valid tags), but it must
  be duplicate-free.
- **Replace, not append**: `set_tag_allowlist` fully replaces the previous
  allowlist. Call `get_tag_allowlist` first if you need to extend rather than
  replace.

### Per-Merchant Tags

- **Storage**: `DataKey::MerchantTags(merchant_address)` — a `Vec<Symbol>`, instance storage.
- **Default**: empty (no tags).
- **Bound**: at most `MAX_MERCHANT_TAGS` (currently `8`) tags per merchant.
- **Authorization**: admin-only (`set_merchant_tags`). Tags encode a
  compliance classification, not a merchant preference, so merchants cannot
  self-assign or self-clear them.
- **Replace, not append**: `set_merchant_tags` fully replaces a merchant's
  tag set. Passing an empty `Vec` clears all tags for that merchant.

### Validation Rules

`set_merchant_tags(admin, merchant, tags)` is rejected, with no partial
effect, if:

| Condition | Error |
|---|---|
| Caller is not the stored admin | `Error::Unauthorized` (1001) |
| `tags.len() > MAX_MERCHANT_TAGS` | `Error::MerchantTagLimitExceeded` (6021) |
| The same tag appears twice in `tags` | `Error::DuplicateMerchantTag` (7006) |
| A tag is not present in the current allowlist | `Error::UnknownMerchantTag` (7005) |

`set_tag_allowlist(admin, tags)` is rejected, with no partial effect, if:

| Condition | Error |
|---|---|
| Caller is not the stored admin | `Error::Unauthorized` (1001) |
| The same tag appears twice in `tags` | `Error::DuplicateMerchantTag` (7006) |

Unknown-tag and duplicate-tag checks are performed in memory before any
storage write, so a rejected call — in either function — never partially
applies.

### Interaction with Allowlist Changes

Shrinking the allowlist (removing a tag that's already assigned to one or
more merchants) does **not** retroactively strip that tag from merchants who
already carry it — `get_merchant_tags` keeps returning it until the next
`set_merchant_tags` call for that merchant. It only blocks *future*
`set_merchant_tags` calls from reusing the removed tag. This mirrors how
`revoke_merchant` (in the [merchant whitelist](merchant_config.md) feature)
doesn't unwind merchants who registered while approved. An admin who needs to
fully retire a tag should follow up with `set_merchant_tags` calls to clear
it from every affected merchant.

### Interaction with Blocklist / Pause

Setting or clearing a merchant's tags is **never** blocked by that merchant's
blocklist or pause state. This is deliberate: compliance metadata must remain
editable — in particular, *clearable* — precisely when it's most relevant,
i.e. on a merchant an admin has already paused or blocklisted. Neither
`set_merchant_tags` nor `set_tag_allowlist` checks the emergency-stop flag
either, for the same reason: tagging is pure bookkeeping, not a financial
operation, and admins must be able to update compliance records during an
active incident.

## Entrypoints

| Function | Signature | Auth |
|---|---|---|
| `get_tag_allowlist` | `() -> Vec<Symbol>` | none |
| `set_tag_allowlist` | `(admin: Address, tags: Vec<Symbol>) -> Result<(), Error>` | admin |
| `get_merchant_tags` | `(merchant: Address) -> Vec<Symbol>` | none |
| `set_merchant_tags` | `(admin: Address, merchant: Address, tags: Vec<Symbol>) -> Result<(), Error>` | admin |

## Events

- **`TagAllowlistUpdatedEvent`** (topic `("tag_allowlist_updated",)`) — emitted
  by `set_tag_allowlist`. Fields: `admin`, `tags`, `timestamp`, `schema_version`.
- **`MerchantTagsUpdatedEvent`** (topic `("merchant_tags_updated", merchant)`) —
  emitted by `set_merchant_tags`, including when `tags` is empty (clearing).
  Fields: `merchant`, `admin`, `tags`, `timestamp`, `schema_version`.

## Design Rationale

### Why an explicit allowlist instead of free-form tags?

Free-form strings would let an admin (or a compromised admin key) create an
unbounded number of distinct tag values over time, defeating the point of
categorization for reporting, and would give indexers no fixed vocabulary to
build dashboards or filters against. A shared, admin-curated allowlist keeps
the category space small, stable, and meaningful.

### Why admin-only rather than merchant self-service?

Compliance categorization needs to be trustworthy for reporting and
enforcement purposes; a merchant self-declaring (or un-declaring) its own
compliance category would defeat that purpose.

### Why `Symbol` rather than `String`?

`Symbol` is Soroban's native short-identifier type (≤32 characters, a
constrained charset), which is both cheaper to store than a `String` and a
natural fit for a small fixed vocabulary of category names.

### Why cap at `MAX_MERCHANT_TAGS`?

Per-merchant storage in this contract is instance storage, shared across all
merchants in a single contract instance footprint. An unbounded per-merchant
tag list — like any unbounded instance-storage growth — would let a single
merchant (or a scripting error) bloat the whole contract's instance storage
without limit. `MAX_MERCHANT_TAGS = 8` mirrors the existing
`MAX_METADATA_KEYS = 10` precedent for bounding a similar per-entity list.

## Storage Impact

- **Allowlist**: one `Vec<Symbol>` for the whole contract instance.
- **Per-merchant overhead**: one `Vec<Symbol>` (at most `MAX_MERCHANT_TAGS`
  entries) per merchant that has ever been tagged. Untagged merchants incur no
  storage cost (`get_merchant_tags` defaults to an empty vector when unset).

## Testing

See `contracts/subscription_vault/src/test_merchant_tags.rs`:

- Allowlist defaults to empty; admin can set and replace it; non-admin is rejected.
- Duplicate tag within a single `set_tag_allowlist` call is rejected, with no partial write.
- Admin can assign allowlisted tags to a merchant; non-admin is rejected.
- `set_merchant_tags` replaces (not appends to) a merchant's existing tags.
- Exactly `MAX_MERCHANT_TAGS` tags is accepted; `MAX_MERCHANT_TAGS + 1` is rejected with no partial write.
- A tag absent from the allowlist is rejected, including against a still-empty allowlist.
- A duplicate tag within a single `set_merchant_tags` call is rejected, with no partial write.
- Clearing tags (empty `Vec`) succeeds on a merchant that is currently blocklisted.
- Clearing tags succeeds on a merchant that is currently paused.
- Shrinking the allowlist doesn't retroactively clear a merchant's already-assigned tag, but blocks reassigning that same tag later.
- Tags are independent per merchant.
- Both `set_tag_allowlist` and `set_merchant_tags` emit their respective event exactly once per call.
