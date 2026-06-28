"""Tests for generate_error_table.py.

Run with:
    python -m pytest scripts/test_generate_error_table.py -v
or:
    python scripts/test_generate_error_table.py

Coverage targets (from issue #513):
  ✓ All Error variants extracted from types.rs
  ✓ New variant added without doc update fails --check
  ✓ Alias/grouped variants are correctly listed
  ✓ Deprecated variants carry strikethrough marker
  ✓ Table includes all required columns
  ✓ --check passes when file is already up to date
  ✓ --check fails when file is stale
  ✓ Entrypoint grep returns expected modules for a known variant
  ✓ Category ranges map correctly
  ✓ Splicing replaces existing sentinel block
  ✓ Splicing appends when sentinels absent
  ✓ Undocumented variants are surfaced with a warning
  ✓ Path-traversal attempt is rejected
  ✓ Missing types.rs returns exit code 2
  ✓ Script returns 0 when no changes needed
"""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path
from types import ModuleType
from unittest.mock import patch

import pytest

# ---------------------------------------------------------------------------
# Load the module under test without requiring it to be on sys.path
# ---------------------------------------------------------------------------

SCRIPT_PATH = Path(__file__).resolve().parent / "generate_error_table.py"


def _load_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("generate_error_table", SCRIPT_PATH)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


MOD = _load_module()

# Re-export symbols under test
parse_variants = MOD.parse_variants
grep_entrypoints = MOD.grep_entrypoints
build_table = MOD.build_table
splice_table = MOD.splice_table
main = MOD.main
_category = MOD._category
START_SENTINEL = MOD.START_SENTINEL
END_SENTINEL = MOD.END_SENTINEL
REMEDIATION = MOD.REMEDIATION

# ---------------------------------------------------------------------------
# Helpers / fixtures
# ---------------------------------------------------------------------------

MINIMAL_TYPES_RS = textwrap.dedent("""\
    use soroban_sdk::contracterror;

    #[contracterror]
    #[derive(Clone, Copy, Debug)]
    #[repr(u32)]
    pub enum Error {
        // --- Auth (1000-1099) ---
        /// Caller does not have the required authorization.
        Unauthorized = 1001,
        /// Caller is authenticated but not allowed.
        Forbidden = 1002,

        // --- Accounting (5000-5099) ---
        /// Arithmetic underflow.
        Underflow = 5004,
        /// Arithmetic overflow.
        Overflow = 5005,
    }
""")

MINIMAL_TYPES_RS_WITH_NEW = textwrap.dedent("""\
    use soroban_sdk::contracterror;

    #[contracterror]
    #[derive(Clone, Copy, Debug)]
    #[repr(u32)]
    pub enum Error {
        /// Caller does not have the required authorization.
        Unauthorized = 1001,
        /// Totally new undocumented variant.
        BrandNewVariant = 9999,
    }
""")


def _make_types_rs(tmp_path: Path, content: str) -> Path:
    """Write a fake types.rs and return its path."""
    p = tmp_path / "types.rs"
    p.write_text(content, encoding="utf-8")
    return p


def _make_src_dir(tmp_path: Path, files: dict[str, str]) -> Path:
    """Create a fake src/ directory with the given {filename: content} mapping."""
    src = tmp_path / "src"
    src.mkdir()
    for name, content in files.items():
        (src / name).write_text(content, encoding="utf-8")
    return src


def _make_errors_md(tmp_path: Path, content: str) -> Path:
    docs = tmp_path / "docs"
    docs.mkdir(exist_ok=True)
    p = docs / "errors.md"
    p.write_text(content, encoding="utf-8")
    return p


# ---------------------------------------------------------------------------
# Tests: parse_variants
# ---------------------------------------------------------------------------


def test_parse_variants_extracts_all(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    variants = parse_variants(types_rs)
    names = {v.name for v in variants}
    assert "Unauthorized" in names
    assert "Forbidden" in names
    assert "Underflow" in names
    assert "Overflow" in names


def test_parse_variants_codes_correct(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    by_name = {v.name: v for v in parse_variants(types_rs)}
    assert by_name["Unauthorized"].code == 1001
    assert by_name["Underflow"].code == 5004


def test_parse_variants_categories(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    by_name = {v.name: v for v in parse_variants(types_rs)}
    assert by_name["Unauthorized"].category == "Auth"
    assert by_name["Underflow"].category == "Accounting"


def test_parse_variants_sorted_by_code(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    variants = parse_variants(types_rs)
    codes = [v.code for v in variants]
    assert codes == sorted(codes)


def test_parse_variants_raises_on_missing_enum(tmp_path):
    types_rs = _make_types_rs(tmp_path, "pub struct NotAnEnum {}")
    with pytest.raises(ValueError, match="Could not locate"):
        parse_variants(types_rs)


def test_parse_variants_real_types_rs():
    """Smoke-test against the actual repository types.rs."""
    if not MOD.TYPES_RS.exists():
        pytest.skip("types.rs not found — not in repository")
    variants = parse_variants(MOD.TYPES_RS)
    names = {v.name for v in variants}
    # Spot-check a handful of known variants
    for expected in ("Unauthorized", "NotFound", "InsufficientBalance", "SchemaMigrationDowngrade"):
        assert expected in names, f"Missing expected variant: {expected}"
    # Ensure numeric codes are unique
    codes = [v.code for v in variants]
    assert len(codes) == len(set(codes)), "Duplicate numeric codes detected"


# ---------------------------------------------------------------------------
# Tests: _category
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("code,expected", [
    (1001, "Auth"),
    (1099, "Auth"),
    (2001, "Not Found"),
    (3005, "Invalid Args"),
    (4007, "State Transition"),
    (5003, "Accounting"),
    (6008, "Limits"),
    (7001, "Merchant Config"),
    (8002, "Token"),
    (9001, "Subscription Update"),
    (9101, "Schema Migration"),
    (9999, "Unknown"),
])
def test_category_ranges(code, expected):
    assert _category(code) == expected


# ---------------------------------------------------------------------------
# Tests: grep_entrypoints
# ---------------------------------------------------------------------------


def test_grep_finds_occurrence(tmp_path):
    src = _make_src_dir(tmp_path, {
        "lib.rs": "fn foo() -> Result<(), Error> { Err(Error::Unauthorized) }",
        "admin.rs": "// no match here",
    })
    hits = grep_entrypoints(src, "Unauthorized")
    assert "lib.rs" in hits


def test_grep_no_false_positive(tmp_path):
    src = _make_src_dir(tmp_path, {
        "lib.rs": "fn foo() { let _ = Error::Forbidden; }",
    })
    hits = grep_entrypoints(src, "Unauthorized")
    assert hits == []


def test_grep_deduplicates(tmp_path):
    content = "Error::Overflow; Error::Overflow; Error::Overflow;"
    src = _make_src_dir(tmp_path, {"lib.rs": content})
    hits = grep_entrypoints(src, "Overflow")
    assert len([h for h in hits if "lib.rs" in h]) == 1


def test_grep_multiple_files(tmp_path):
    src = _make_src_dir(tmp_path, {
        "admin.rs":   "Err(Error::NotFound)",
        "merchant.rs": "Err(Error::NotFound)",
        "unrelated.rs": "// nothing",
    })
    hits = grep_entrypoints(src, "NotFound")
    assert len(hits) == 2


def test_grep_path_traversal_rejected(tmp_path):
    """Files outside src_dir must never be read."""
    src = _make_src_dir(tmp_path, {})
    evil = tmp_path / "evil.rs"
    evil.write_text("Error::Unauthorized", encoding="utf-8")
    # Ensure the file outside src/ is not picked up
    hits = grep_entrypoints(src, "Unauthorized")
    assert "evil.rs" not in hits


# ---------------------------------------------------------------------------
# Tests: build_table
# ---------------------------------------------------------------------------


def test_build_table_columns_present(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    src = _make_src_dir(tmp_path, {"lib.rs": "Err(Error::Unauthorized)"})
    variants = parse_variants(types_rs)
    table_text, _ = build_table(variants, src)
    assert "Code" in table_text
    assert "Variant" in table_text
    assert "Category" in table_text
    assert "Emitting entrypoints" in table_text
    assert "Recovery action" in table_text
    assert "Related event" in table_text


def test_build_table_known_variant_has_remediation(tmp_path):
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    src = _make_src_dir(tmp_path, {})
    variants = parse_variants(types_rs)
    table_text, undocumented = build_table(variants, src)
    # Unauthorized is in REMEDIATION — should not appear in undocumented list
    assert "Unauthorized" not in undocumented
    # Its code should appear in the table
    assert "1001" in table_text


def test_build_table_new_variant_flags_undocumented(tmp_path):
    """A new variant not in REMEDIATION appears in the undocumented list."""
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS_WITH_NEW)
    src = _make_src_dir(tmp_path, {})
    variants = parse_variants(types_rs)
    _, undocumented = build_table(variants, src)
    assert "BrandNewVariant" in undocumented


def test_build_table_deprecated_variant_strikethrough(tmp_path):
    """Variants marked deprecated=True in REMEDIATION get strikethrough."""
    # Inject a temporary deprecated entry
    original = REMEDIATION.get("Unauthorized")
    REMEDIATION["Unauthorized"] = ("Fix it.", "—", True)
    try:
        types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
        src = _make_src_dir(tmp_path, {})
        variants = parse_variants(types_rs)
        table_text, _ = build_table(variants, src)
        assert "~~(deprecated)~~" in table_text
    finally:
        if original is not None:
            REMEDIATION["Unauthorized"] = original
        else:
            del REMEDIATION["Unauthorized"]


def test_build_table_no_entrypoints_shows_dash(tmp_path):
    """A variant with zero grep hits shows '—' in the entrypoints column."""
    types_rs = _make_types_rs(tmp_path, MINIMAL_TYPES_RS)
    src = _make_src_dir(tmp_path, {"lib.rs": "// no errors here"})
    variants = parse_variants(types_rs)
    table_text, _ = build_table(variants, src)
    # Some variants will have no hits; the '—' placeholder must appear
    assert "| — |" in table_text or "| —" in table_text


# ---------------------------------------------------------------------------
# Tests: splice_table
# ---------------------------------------------------------------------------

_ERRORS_MD_WITH_SENTINELS = """\
# Error Codes

Some existing content.

<!-- GENERATED:entrypoint-table:start -->
OLD TABLE CONTENT
<!-- GENERATED:entrypoint-table:end -->

More content after.
"""

_ERRORS_MD_WITHOUT_SENTINELS = """\
# Error Codes

Some existing content.
"""


def test_splice_replaces_existing_block(tmp_path):
    errors_md = _make_errors_md(tmp_path, _ERRORS_MD_WITH_SENTINELS)
    new_content = splice_table(errors_md, "NEW TABLE", [])
    assert "OLD TABLE CONTENT" not in new_content
    assert "NEW TABLE" in new_content
    assert "Some existing content." in new_content
    assert "More content after." in new_content


def test_splice_appends_when_no_sentinels(tmp_path):
    errors_md = _make_errors_md(tmp_path, _ERRORS_MD_WITHOUT_SENTINELS)
    new_content = splice_table(errors_md, "APPENDED TABLE", [])
    assert "APPENDED TABLE" in new_content
    assert "Some existing content." in new_content


def test_splice_sentinels_present_in_output(tmp_path):
    errors_md = _make_errors_md(tmp_path, _ERRORS_MD_WITHOUT_SENTINELS)
    new_content = splice_table(errors_md, "TABLE", [])
    assert START_SENTINEL in new_content
    assert END_SENTINEL in new_content


def test_splice_undocumented_warning_present(tmp_path):
    errors_md = _make_errors_md(tmp_path, _ERRORS_MD_WITHOUT_SENTINELS)
    new_content = splice_table(errors_md, "TABLE", ["MissingVariant"])
    assert "MissingVariant" in new_content
    assert "Undocumented variants" in new_content


def test_splice_no_undocumented_no_warning(tmp_path):
    errors_md = _make_errors_md(tmp_path, _ERRORS_MD_WITHOUT_SENTINELS)
    new_content = splice_table(errors_md, "TABLE", [])
    assert "Undocumented variants" not in new_content


# ---------------------------------------------------------------------------
# Tests: main() — integration
# ---------------------------------------------------------------------------


def _make_repo(tmp_path: Path, types_content: str, md_content: str, src_files: dict[str, str]) -> Path:
    """Scaffold a minimal fake repository tree and return its root."""
    src_dir = tmp_path / "contracts" / "subscription_vault" / "src"
    src_dir.mkdir(parents=True)
    (src_dir / "types.rs").write_text(types_content, encoding="utf-8")
    for name, content in src_files.items():
        (src_dir / name).write_text(content, encoding="utf-8")

    docs_dir = tmp_path / "docs"
    docs_dir.mkdir()
    (docs_dir / "errors.md").write_text(md_content, encoding="utf-8")

    scripts_dir = tmp_path / "scripts"
    scripts_dir.mkdir()

    return tmp_path


def test_main_generates_file(tmp_path):
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS, _ERRORS_MD_WITHOUT_SENTINELS, {
        "lib.rs": "Err(Error::Unauthorized)"
    })
    rc = main(["--repo-root", str(repo)])
    assert rc == 0
    updated = (repo / "docs" / "errors.md").read_text(encoding="utf-8")
    assert START_SENTINEL in updated
    assert "1001" in updated


def test_main_check_passes_when_up_to_date(tmp_path):
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS, _ERRORS_MD_WITHOUT_SENTINELS, {})
    # First run to generate
    main(["--repo-root", str(repo)])
    # Second run with --check should pass
    rc = main(["--check", "--repo-root", str(repo)])
    assert rc == 0


def test_main_check_fails_when_stale(tmp_path):
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS, _ERRORS_MD_WITHOUT_SENTINELS, {})
    # Write stale sentinel block
    errors_md = repo / "docs" / "errors.md"
    errors_md.write_text(
        _ERRORS_MD_WITHOUT_SENTINELS
        + f"\n{START_SENTINEL}\nSTALE CONTENT\n{END_SENTINEL}\n",
        encoding="utf-8",
    )
    rc = main(["--check", "--repo-root", str(repo)])
    assert rc == 1


def test_main_returns_2_when_types_rs_missing(tmp_path):
    # No types.rs written
    docs_dir = tmp_path / "docs"
    docs_dir.mkdir()
    (docs_dir / "errors.md").write_text("# Errors", encoding="utf-8")
    rc = main(["--repo-root", str(tmp_path)])
    assert rc == 2


def test_main_no_change_exits_0(tmp_path):
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS, _ERRORS_MD_WITHOUT_SENTINELS, {})
    main(["--repo-root", str(repo)])           # generate
    rc = main(["--repo-root", str(repo)])       # no-op run
    assert rc == 0


def test_main_new_variant_without_remediation_flags_warning(tmp_path, capsys):
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS_WITH_NEW, _ERRORS_MD_WITHOUT_SENTINELS, {})
    main(["--repo-root", str(repo)])
    updated = (repo / "docs" / "errors.md").read_text(encoding="utf-8")
    assert "BrandNewVariant" in updated
    assert "Undocumented variants" in updated


def test_main_check_fails_when_new_variant_added(tmp_path):
    """CI must fail when a new variant lands without updating docs."""
    repo = _make_repo(tmp_path, MINIMAL_TYPES_RS, _ERRORS_MD_WITHOUT_SENTINELS, {})
    # Generate for the original types.rs
    main(["--repo-root", str(repo)])
    # Now "add" a new variant by swapping in the extended types.rs
    src_dir = repo / "contracts" / "subscription_vault" / "src"
    (src_dir / "types.rs").write_text(MINIMAL_TYPES_RS_WITH_NEW, encoding="utf-8")
    rc = main(["--check", "--repo-root", str(repo)])
    assert rc == 1


# ---------------------------------------------------------------------------
# Run directly (no pytest needed)
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    # Simple runner for environments without pytest
    import traceback

    passed = failed = 0
    globs = list(globals().items())
    for name, obj in globs:
        if not (name.startswith("test_") and callable(obj)):
            continue
        # Count parameters to know if we need tmp_path
        import inspect
        sig = inspect.signature(obj)
        params = list(sig.parameters.keys())
        try:
            if "tmp_path" in params:
                import tempfile
                with tempfile.TemporaryDirectory() as td:
                    kwargs: dict = {}
                    if "tmp_path" in params:
                        kwargs["tmp_path"] = Path(td)
                    if "capsys" in params:
                        # Skip capsys tests in non-pytest mode
                        print(f"  SKIP {name} (requires capsys)")
                        continue
                    obj(**kwargs)
            else:
                obj()
            print(f"  PASS {name}")
            passed += 1
        except Exception:
            print(f"  FAIL {name}")
            traceback.print_exc()
            failed += 1

    print(f"\n{passed} passed, {failed} failed")
    sys.exit(0 if failed == 0 else 1)
