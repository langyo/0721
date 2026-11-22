#!/usr/bin/env python3
"""i18n locale completeness & key-usage analyzer.

i18n locale completeness & key-usage analyzer for the 0721 webui.

Verifies translation locales under
``res/i18n/locales/<lang>/<namespace>.json``.

The scanner uses simple regexes (no external dependencies) and an
annotation comment convention for dynamic call sites.

The two-translator model
~~~~~~~~~~~~~~~~~~~~~~~~
The webui speaks to TWO translators whose key spaces are disjoint:

* the app's vue-i18n composer (``import { useI18n } from "vue-i18n"``,
  or the ``gt`` bridge in ``src/i18n``) serves the deep-merged chest
  locale tree — dot-separated keys like ``common.actions.search``;
* hikari's own i18n context (``import { useI18n } from
  "@celestia-island/hikari"``) serves a flat string map holding only
  upstream ``hikari::*`` / ``plana::*`` keys — keys literally contain
  ``::``.

Routing rule: a key containing ``::`` must only ever be passed to
hikari-scope ``t``; a key without ``::`` must only ever be passed to
app-scope ``t``.  Routing one through the other renders the raw key
(vue-i18n can never resolve a ``::``-qualified key; hikari's ``t``
freezes chest keys to their fallback or the raw key).

Four checks:

1. Missing translations  (HARD ERROR — exit 1 unless --no-fail)
   Cross-checks every locale against en/zh-Hans baselines.

2. Referenced-but-undefined keys  (HARD ERROR unless --no-fail)
   Every literal key passed to ``t()`` / ``gt()`` and every annotation-
   resolved key must exist in the baseline locale.

5. Placeholder-token parity across locales  (HARD ERROR unless --no-fail)
   Every locale value for an en key must carry the same ``{token}`` set as
   en (a dropped/renamed token renders an empty hole or the raw token at
   runtime, and key parity cannot see it). Pipe-plural values must also
   keep en's segment count when both sides use pipes; single-form plural
   avoidance (CJK/ru/ar) is legitimate and not compared. Raised 2026-10-12
   after R2 caught a ko value that dropped ``{name}`` while passing every
   pre-existing gate.

3. Unused keys  (WARN; FAIL with --strict)
   Locale keys never referenced.  Exempt by wildcard prefix (from
   ``t(`prefix.${x}`)``), annotation, or the heuristic that any
   namespace-prefixed string literal in source counts as used.

4. Wrong-resolver routing  (HARD ERROR unless --no-fail)
   Per-file import tracking resolves every ``useI18n`` import
   (``useI18n as X`` aliases included) to its translator scope, and
   ``const { t } = <hook>()`` destructures map each local ``t``-family
   binding to that scope.  The call-site regex is compiled per file
   from those bindings (base ``t``/``gt``/… names plus renamed locals
   like ``tHikari``), so a renamed binding is matched where it is
   called, not merely mapped.  Every quoted-literal (and
   non-interpolated template-literal) call site is then classified:

   * ``::``-qualified key via an app-scope binding → error (renders
     the raw key);
   * key-like key without ``::`` via a hikari-scope binding → error
     (unlocalized fallback or raw key);
   * ``::``-qualified key via a hikari-scope binding → OK — trusted
     upstream scope, never validated against the chest locale trees.

   Bindings whose scope cannot be determined (and ``gt`` / ``$t``)
   fail closed to the app scope — the app tree is the only one that
   carries chest keys.  Member-style calls (``x.t(...)``) do not
   correspond to any tracked local binding and default to app scope.
   The deliberate vue-i18n mirror of ``errors.*`` under the literal
   ``plana::errors`` namespace (see ``src/i18n/index.ts``) is the one
   exempt ``::`` prefix on the app side.  ``// i18n: ...`` annotations
   suppress this check for their line, same as the other checks.

   Historically the key extraction used ``_KEY_LIKE_RE``, whose
   letter-only namespace segments silently dropped ``::``-qualified
   literals — the blind spot that let ``hikari::theme.customThemeName``
   ship through vue-i18n (raw key rendered in the UI).  This check
   consumes those literals explicitly so the bug class cannot ship
   again.

Annotation convention
~~~~~~~~~~~~~~~~~~~~~
Dynamic call sites that cannot be statically resolved are preceded
by an annotation comment::

    // i18n: pattern1, pattern2, ...

where each pattern is one of:

   * ``exact.key``        — exact match
   * ``prefix.*``         — wildcard (keys starting with ``prefix.``)
   * ``prefix.``          — same as ``prefix.*``
   * ``/regex/``          — regex against the full key
   * ``end`` / ``off``    — clear active annotation

Once an annotation is placed, it stays *active* for all subsequent
``t()`` lines in the file until a new annotation replaces it or
``// i18n:end`` clears it.  A trailing ``// i18n: ...`` on the
same line as a ``t()`` call acts as a one-shot annotation for
that line only and does not change the active annotation.

Usage:
    python scripts/check_i18n.py                  # full check, exit 1 on hard errors
    python scripts/check_i18n.py --no-fail        # report only, never exit non-zero
    python scripts/check_i18n.py --quiet          # suppress per-key detail lines
    python scripts/check_i18n.py --strict         # also fail on unused keys
    python scripts/check_i18n.py --json           # machine-readable stdout
"""

from __future__ import annotations

import argparse
import bisect
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES_DIR = ROOT / "res" / "i18n" / "locales"
SRC_DIR = ROOT / "packages" / "webui" / "src"

BASELINE_LANGS: tuple[str, ...] = ("en", "zh-Hans")
SCAN_EXTS: tuple[str, ...] = (".ts", ".tsx")
SCAN_SKIP_DIRS: set[str] = {"node_modules", "dist", ".cache", "locales"}
DEFAULT_ALLOWLIST = ROOT / "scripts" / "i18n_unused_allowlist.txt"

# ── Regexes ────────────────────────────────────────────────────────────
#   ``(?<!\w)`` keeps multi-letter aliases like ``gt`` working while
#   rejecting ``output(...)``, ``HtmlElement(...)``, etc.
#   DOTALL so multi-line calls (``t(\n  "key",\n  fallback\n)``) work.

#  t("...") / gt('...') — quoted literal key  (DOTALL for multi-line)
#  t(`...`) — template literal  (DOTALL for multi-line)
#
# The head alternation is compiled PER FILE (see ``_t_call_regex``): the
# base t-family names plus whatever local aliases the file's useI18n
# destructures produced (``tHikari``, ``tApp``, …).  A static alternation
# would make renamed bindings invisible at their call sites — exactly the
# sites where wrong-resolver routing happens.
_T_FAMILY_BASE: tuple[str, ...] = ("t", "gt", "tc", "te", "tt", "$t")

#  quoted-literal call template — group(1)=binding, (2)=quote, (3)=key
_T_QUOTED_BODY = r"""\s*\(\s*(["'])(.+?)\2"""
#  template-literal call template — group(1)=binding, (2)=body
_T_TEMPLATE_BODY = r"""\s*\(\s*`([^`]*)`"""


def _t_call_regex(t_scopes: dict[str, str], body: str) -> re.Pattern:
    """Compile a t-family call regex covering this file's bindings.

    The head alternation is the base t-family names plus every local
    binding mapped by ``_t_binding_scopes`` (``t: tHikari`` and friends),
    longest first so multi-letter aliases win deterministically.  Group
    1 is always the matched binding name, so call-site handling stays
    identical for base names and aliases.
    """
    names = sorted(set(_T_FAMILY_BASE) | set(t_scopes), key=len, reverse=True)
    alt = "|".join(re.escape(n) for n in names)
    return re.compile(rf"(?<!\w)({alt}){body}", re.DOTALL)

# Any ``t(`` call site — used for per-line detection of dynamic args.
_T_CALL_SITE_RE = re.compile(r"""(?<!\w)(t|gt|tc|te|tt|\$t)\s*\(""")

# ``// i18n: ...`` — dedicated annotation on its own line.
_ANNOTATION_RE = re.compile(r"^\s*//\s+i18n\s*:\s*(.+)$")

# Trailing annotation on the same line as a call.
_TRAILING_ANNOTATION_RE = re.compile(r"//\s+i18n\s*:\s*(.+)$")

# Dotted locale key — must start with a letter-only namespace segment.
_KEY_LIKE_RE = re.compile(r"^[a-zA-Z][a-zA-Z]*+(?:\.[A-Za-z0-9_][A-Za-z0-9_\-]*)+")

# ``import { ... } from "<module>"`` — single- or multi-line (DOTALL).
_IMPORT_RE = re.compile(r"""\bimport\s*\{([^}]*)\}\s*from\s*(["'])([^"']+)\2""", re.DOTALL)

# ``const { ... } = <fn>()`` — single-line destructure of a hook call.
_DESTRUCTURE_RE = re.compile(r"""\bconst\s*\{([^}]*)\}\s*=\s*([A-Za-z_$][\w$]*)\s*\(\s*\)""")

# ── translator scopes (the two-translator model) ──────────────────────

_I18N_SCOPE_APP = "app"          # vue-i18n composer / gt bridge — chest tree
_I18N_SCOPE_HIKARI = "hikari"    # @celestia-island/hikari — hikari::/plana:: map

# The one legitimate ``::`` prefix on the app side: src/i18n/index.ts
# mirrors the ``errors.*`` tree under the literal ``plana::errors``
# namespace ("one source, two mounts"), so ``plana::errors.*`` keys do
# resolve through vue-i18n.  Any other ``::`` namespace through the app
# composer stays a hard error.
APP_MOUNTED_NS_PREFIXES: tuple[str, ...] = ("plana::errors.",)

# ── logging (plain columnar printer) ────────────────────────────────────
# The chest version routes through a shared scripts/utils logger; this
# repo has no such module, so the built-in fallback printer is the only
# path. Output shape is identical ([ OK ]/[INFO]/[WARN]/[FAIL] columns).
_log = None


def _emit(level: str, msg: str) -> None:
    if _log:
        _log.log(level, msg)
    else:
        token = {"OK": "[ OK ]", "INFO": "[INFO]", "WARN": "[WARN]", "ERROR": "[FAIL]", "DEBUG": "      "}.get(level, "[INFO]")
        print(f"{token} {msg}")


def _ok(msg: str) -> None: _emit("OK", msg)
def _info(msg: str) -> None: _emit("INFO", msg)
def _warn(msg: str) -> None: _emit("WARN", msg)
def _fail(msg: str) -> None: _emit("ERROR", msg)
def _dim(msg: str) -> None: _emit("DEBUG", msg)


# ── locale key loading ─────────────────────────────────────────────────


def _flatten(prefix: str, obj: object, acc: set[str]) -> None:
    if isinstance(obj, dict):
        for k, v in obj.items():
            nxt = f"{prefix}.{k}" if prefix else k
            _flatten(nxt, v, acc)
    else:
        if prefix:
            acc.add(prefix)


def load_locale_keys() -> dict[str, set[str]]:
    result: dict[str, set[str]] = {}
    if not LOCALES_DIR.is_dir():
        return result
    for lang_dir in sorted(p for p in LOCALES_DIR.iterdir() if p.is_dir()):
        keys: set[str] = set()
        for ns_file in sorted(lang_dir.glob("*.json")):
            try:
                data = json.loads(ns_file.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as e:
                _fail(f"could not parse {ns_file.relative_to(ROOT)}: {e}")
                continue
            _flatten(ns_file.stem, data, keys)
        result[lang_dir.name] = keys
    return result


def load_locale_values() -> dict[str, dict[str, str]]:
    """Flatten every locale to {dotted.key: string value} (string leaves only)."""
    result: dict[str, dict[str, str]] = {}

    def _flatten_values(prefix: str, obj: object, acc: dict[str, str]) -> None:
        if isinstance(obj, dict):
            for k, v in obj.items():
                nxt = f"{prefix}.{k}" if prefix else k
                _flatten_values(nxt, v, acc)
        elif isinstance(obj, str):
            acc[prefix] = obj

    if not LOCALES_DIR.is_dir():
        return result
    for lang_dir in sorted(p for p in LOCALES_DIR.iterdir() if p.is_dir()):
        values: dict[str, str] = {}
        for ns_file in sorted(lang_dir.glob("*.json")):
            try:
                data = json.loads(ns_file.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue  # parse failures are reported by load_locale_keys
            _flatten_values(ns_file.stem, data, values)
        result[lang_dir.name] = values
    return result


_TOKEN_RE = re.compile(r"\{[A-Za-z_][A-Za-z0-9_]*\}")


def check_token_parity(
    locale_values: dict[str, dict[str, str]],
) -> list[tuple[str, str, str]]:
    """Cross-locale placeholder contract (check 5).

    vue-i18n renders named placeholders from the message itself, so a locale
    value that drops or renames a ``{token}`` silently prints the raw token or
    an empty hole at runtime — key parity cannot see it because the KEY
    exists. Every non-baseline locale must carry the same {token} set as en
    for the same key. Pipe-plural values must additionally keep the same
    segment count as en WHEN both sides use pipes (single-form plural
    avoidance in CJK/ru/ar is legitimate and not comparable).
    """
    errors: list[tuple[str, str, str]] = []
    en = locale_values.get("en")
    if not en:
        return errors
    for lang in sorted(locale_values):
        if lang == "en":
            continue
        vals = locale_values[lang]
        for key, en_val in en.items():
            val = vals.get(key)
            if not isinstance(val, str):
                continue  # absent keys are check 1's domain
            en_toks = frozenset(_TOKEN_RE.findall(en_val))
            toks = frozenset(_TOKEN_RE.findall(val))
            if toks != en_toks:
                missing = sorted(en_toks - toks)
                extra = sorted(toks - en_toks)
                detail = []
                if missing:
                    detail.append(f"missing {missing}")
                if extra:
                    detail.append(f"extra {extra}")
                errors.append((lang, key, "; ".join(detail)))
                continue
            if "|" in en_val and "|" in val:
                if len(val.split("|")) != len(en_val.split("|")):
                    errors.append(
                        (
                            lang,
                            key,
                            f"pipe segments {len(val.split('|'))} != en {len(en_val.split('|'))}",
                        )
                    )
    return errors


# ── scanning ───────────────────────────────────────────────────────────


def _iter_source_files() -> list[Path]:
    out: list[Path] = []
    for path in SRC_DIR.rglob("*"):
        if not path.is_file() or path.suffix not in SCAN_EXTS:
            continue
        if any(part in SCAN_SKIP_DIRS for part in path.parts):
            continue
        out.append(path)
    return out


def _parse_patterns(raw: str) -> list[str]:
    """Split comma-separated annotation patterns, stripping whitespace
    and trailing `` — `` style comments."""
    result: list[str] = []
    for chunk in raw.split(","):
        p = chunk.strip()
        # Strip trailing descriptions like " — dynamic via t(...)"
        dash = p.find(" — ")
        if dash > 0:
            p = p[:dash].strip()
        if p:
            result.append(p)
    return result


def _use_i18n_scopes(text: str) -> dict[str, str]:
    """Map each local ``useI18n`` import alias to its translator scope.

    Tracks ``import { useI18n }`` / ``import { useI18n as X }`` bindings
    and classifies the source module: anything containing
    ``@celestia-island/hikari`` (or ending in ``/hikari``) is hikari's
    own i18n context; ``vue-i18n`` is the app composer.  Modules that
    are neither are not tracked here — their bindings fall through to
    the fail-closed app default at resolution time.
    """
    scopes: dict[str, str] = {}
    for m in _IMPORT_RE.finditer(text):
        module = m.group(3)
        if "vue-i18n" in module:
            scope: str = _I18N_SCOPE_APP
        elif "@celestia-island/hikari" in module or module.endswith("/hikari"):
            scope = _I18N_SCOPE_HIKARI
        else:
            continue
        for member in m.group(1).split(","):
            spec = member.strip()
            # Defensive: drop a ``type`` modifier (a type-only import can
            # never name a runtime hook).
            if spec.startswith("type "):
                spec = spec[len("type "):].strip()
            parts = [p.strip() for p in spec.split(" as ") if p.strip()]
            if parts and parts[0] == "useI18n":
                scopes[parts[-1]] = scope
    return scopes


def _t_binding_scopes(text: str, use_scopes: dict[str, str]) -> dict[str, str]:
    """Map local ``t``-family bindings to a scope from ``const { t } = <hook>()``.

    Only destructures whose callee is a known ``useI18n`` alias are
    tracked; both ``t`` and ``t: alias`` members are honored.  Bindings
    that never appear here (``gt``, ``$t``, function parameters, member
    access) resolve through the fail-closed app default.
    """
    scopes: dict[str, str] = {}
    for m in _DESTRUCTURE_RE.finditer(text):
        scope = use_scopes.get(m.group(2))
        if scope is None:
            continue
        for member in m.group(1).split(","):
            seg = member.split(":")
            if seg[0].strip() != "t":
                continue
            local = seg[1].strip() if len(seg) > 1 else "t"
            local = local.split("=")[0].strip()  # drop a default value, if any
            if local:
                scopes[local] = scope
    return scopes


def _resolve_t_scope(binding: str, t_scopes: dict[str, str], member_call: bool) -> str:
    """Translator scope for one t-family call site.

    Member-style calls (``x.t(...)``) never correspond to a tracked
    local binding; everything unmapped fails closed to the app scope —
    the app tree is the only one that carries chest keys, so a wrong
    hikari guess is the dangerous direction.  ``gt`` / ``$t`` land here
    by name.
    """
    if not member_call:
        scope = t_scopes.get(binding)
        if scope:
            return scope
    return _I18N_SCOPE_APP


def _wrong_resolver_finding(key: str, scope: str) -> str | None:
    """Classify one literal call site against the two-translator model.

    Returns a finding kind (``app_routed`` / ``hikari_routed``) or None
    when the routing is correct.  ``::``-qualified keys routed through
    hikari are trusted upstream scope and are never validated against
    the chest locale trees.
    """
    if "::" in key:
        if scope == _I18N_SCOPE_HIKARI:
            return None
        if any(key.startswith(p) for p in APP_MOUNTED_NS_PREFIXES):
            return None
        return "app_routed"
    if _KEY_LIKE_RE.match(key) and scope == _I18N_SCOPE_HIKARI:
        return "hikari_routed"
    return None


def _resolve_annotations(
    scan_res: ScanResult,
    baseline_keys: set[str],
) -> set[str]:
    """Expand annotation patterns into concrete keys from the baseline."""
    result: set[str] = set()
    for _file, _line, patterns in scan_res.annotations:
        for pat in patterns:
            p = pat
            if not p:
                continue
            if p.endswith(".*"):
                pfx = p[:-2]
                result.update(k for k in baseline_keys if k.startswith(pfx))
            elif p.endswith("."):
                result.update(k for k in baseline_keys if k.startswith(p))
            elif p.startswith("/") and p.endswith("/") and len(p) >= 3:
                try:
                    rx = re.compile(p[1:-1])
                    result.update(k for k in baseline_keys if rx.fullmatch(k) or rx.search(k))
                except re.error:
                    pass
            else:
                if p in baseline_keys:
                    result.add(p)
    return result


@dataclass
class ScanResult:
    used_literal: set[str] = field(default_factory=set)
    wildcard_prefixes: set[str] = field(default_factory=set)
    heuristic_used: set[str] = field(default_factory=set)
    # (file_rel, line_no, [pattern, ...])
    annotations: list[tuple[str, int, list[str]]] = field(default_factory=list)
    # (file_rel, line_no, snippet)
    dynamic_sites: list[tuple[str, int, str]] = field(default_factory=list)
    # wrong-resolver routing: (file_rel, line_no, key, binding, kind)
    scope_findings: list[tuple[str, int, str, str, str]] = field(default_factory=list)
    # literal call sites examined by the wrong-resolver check
    scope_checked: int = 0


def _has_call_on_line(line: str) -> bool:
    return bool(_T_CALL_SITE_RE.search(line))


def scan_sources(namespaces: set[str]) -> ScanResult:
    res = ScanResult()

    ns_re = None
    if namespaces:
        sorted_ns = sorted(namespaces, key=len, reverse=True)
        ns_pat = r"^(" + "|".join(map(re.escape, sorted_ns)) + r")\."
        ns_re = re.compile(ns_pat)

    for path in _iter_source_files():
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        lines = text.splitlines()
        rel = path.relative_to(ROOT).as_posix()

        # ── build line-offset table (byte pos -> 1‑based line number) ──
        line_starts: list[int] = []
        pos = 0
        for raw in lines:
            line_starts.append(pos)
            pos += len(raw) + 1  # +1 for the newline we split on

        def _line_of(byte_pos: int) -> int:
            return bisect.bisect_right(line_starts, byte_pos)

        # ── Phase 1: annotation scope + comment-line detection ──────────
        line_annotations: dict[int, list[str]] = {}  # line_no -> patterns  (trailing/previous)
        comment_lines: set[int] = set()
        in_block = False

        for i, raw in enumerate(lines):
            line_no = i + 1
            stripped = raw.lstrip()

            # comment detection
            if stripped.startswith("//"):
                comment_lines.add(line_no)
            elif stripped.startswith("/*"):
                comment_lines.add(line_no)
                in_block = True
            elif in_block:
                comment_lines.add(line_no)
            if "*/" in raw:
                in_block = False

            ann_match = _ANNOTATION_RE.match(raw)
            trailing_match = _TRAILING_ANNOTATION_RE.search(raw) if not ann_match else None

            if ann_match:
                text_ann = ann_match.group(1).strip()
                # Parse the annotation for validation (the per-line scope loop
                # below re-derives patterns from _ANNOTATION_RE independently).
                if text_ann not in ("end", "off"):
                    _parse_patterns(text_ann)
                continue

            if trailing_match:
                line_annotations[line_no] = _parse_patterns(trailing_match.group(1))

        # per-line scope from sticky annotation
        line_scope: dict[int, list[str]] = {}
        cur_pats: list[str] | None = None
        for i in range(len(lines)):
            line_no = i + 1
            raw = lines[i]
            if _ANNOTATION_RE.match(raw):
                key = _ANNOTATION_RE.match(raw).group(1).strip()
                if key in ("end", "off"):
                    cur_pats = None
                else:
                    cur_pats = _parse_patterns(key)
                continue
            if cur_pats:
                line_scope[line_no] = cur_pats

        # ── Phase 2: full‑text key extraction (DOTALL, multi-line safe) ─
        # Per-file translator scope maps feed the wrong-resolver check:
        # whichever useI18n import a t binding came from decides which
        # key space its literal arguments may target.
        use_scopes = _use_i18n_scopes(text)
        t_scopes = _t_binding_scopes(text, use_scopes)
        quoted_re = _t_call_regex(t_scopes, _T_QUOTED_BODY)
        template_re = _t_call_regex(t_scopes, _T_TEMPLATE_BODY)

        def _scope_check(ln: int, binding: str, key: str, match: re.Match) -> None:
            member_call = match.start() > 0 and text[match.start() - 1] == "."
            scope = _resolve_t_scope(binding, t_scopes, member_call)
            kind = _wrong_resolver_finding(key, scope)
            # ``// i18n: ...`` annotations (sticky or one-shot) suppress
            # the check for their line, same as the other checks.
            if kind and ln not in line_annotations and ln not in line_scope:
                res.scope_findings.append((rel, ln, key, binding, kind))
            res.scope_checked += 1

        for m in quoted_re.finditer(text):
            ln = _line_of(m.start())
            if ln in comment_lines:
                continue
            key = m.group(3)
            if _KEY_LIKE_RE.match(key):
                res.used_literal.add(key)
            # ``::``-qualified literals are consumed by the wrong-resolver
            # check instead: they can never be chest tree keys, so they do
            # not enter used_literal (the old _KEY_LIKE_RE simply and
            # silently dropped them).
            _scope_check(ln, m.group(1), key, m)

        for m in template_re.finditer(text):
            ln = _line_of(m.start())
            if ln in comment_lines:
                continue
            body = m.group(2)
            idx = body.find("${")
            if idx == -1:
                if _KEY_LIKE_RE.match(body):
                    res.used_literal.add(body)
                # Non-interpolated template bodies get the same routing
                # scrutiny as quoted literals.
                _scope_check(ln, m.group(1), body, m)
            else:
                pfx = body[:idx]
                if pfx.strip():
                    res.wildcard_prefixes.add(pfx)
                else:
                    # Empty static head — needs annotation coverage.
                    pats = line_annotations.get(ln) or line_scope.get(ln)
                    if pats:
                        res.annotations.append((rel, ln, pats))

        # ── Phase 3: heuristic — any namespace-prefixed literal ──────────
        if ns_re is not None:
            for m in re.finditer(
                r"""['"]([A-Za-z][A-Za-z0-9]*\.[A-Za-z0-9_][A-Za-z0-9_.\-]*)['"']""",
                text,
            ):
                key = m.group(1)
                if ns_re.match(key):
                    res.heuristic_used.add(key)

        # Phase 3b: i18n descriptor tables. useHeaderSubTabs.setCenterTabs
        # takes label DESCRIPTORS ({ key, fallback }): the i18n key sits in
        # object-literal DATA, not inside a t('...') call, so the t-call
        # scanner never sees it and the namespace heuristic alone (low
        # confidence) would let a key deletion slip through the hard
        # referenced-but-undefined check while every locale silently
        # renders the fallback string (2026-10-29 R2 finding on #1966).
        # A property literally named `key` whose value is a
        # namespace-prefixed dotted literal IS the descriptor shape --
        # promote it to used_literal (high confidence). The dot
        # requirement keeps plain identifiers (column keys, route slugs)
        # out.
        if ns_re is not None:
            for m in re.finditer(
                r"\bkey\s*:\s*[\x27\"]([A-Za-z][A-Za-z0-9]*\.[A-Za-z0-9_][A-Za-z0-9_.\-]*)[\x27\"]",
                text,
            ):
                dkey = m.group(1)
                if ns_re.match(dkey):
                    res.used_literal.add(dkey)

        # Phase 3c: injected-localize call sites. The About rows/links
        # builders take a `localize(key, fallback)` callback injected by
        # the host — not a t() binding this scanner tracks — so their
        # literal keys landed only in the namespace heuristic (low
        # confidence): check_missing_in_source was blind to them, and
        # deleting `about.env.unconnected` from EVERY locale (en included)
        # still passed the hard referenced-but-undefined check while the
        # pill silently rendered its fallback (2026-10-30 #2021 R2
        # finding). A quoted dotted literal as the FIRST argument of a
        # localize( call IS a direct key reference — promote it to
        # used_literal (high confidence). Variable first arguments
        # (`localize(entry[0], entry[1])` in aboutComponentLabel) match
        # nothing here by design and stay with the heuristic.
        if ns_re is not None:
            for m in re.finditer(
                r"\blocalize\s*\(\s*[\x27\"]([A-Za-z][A-Za-z0-9]*\.[A-Za-z0-9_][A-Za-z0-9_.\-]*)[\x27\"]",
                text,
            ):
                ln = _line_of(m.start())
                if ln in comment_lines:
                    continue
                lkey = m.group(1)
                if _KEY_LIKE_RE.match(lkey) and ns_re.match(lkey):
                    res.used_literal.add(lkey)

    return res


# ── checks ─────────────────────────────────────────────────────────────


def _load_allowlist(path: Path | None) -> tuple[set[str], set[str]]:
    exact: set[str] = set()
    prefixes: set[str] = set()
    if path is None or not path.is_file():
        return exact, prefixes
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.endswith(".*"):
            prefixes.add(line[:-2])
        elif line.endswith("."):
            prefixes.add(line[:-1])
        else:
            exact.add(line)
    return exact, prefixes


def check_missing_in_locales(
    locale_keys: dict[str, set[str]],
) -> list[tuple[str, str, str]]:
    errors: list[tuple[str, str, str]] = []
    seen: set[tuple[str, str]] = set()
    for baseline in BASELINE_LANGS:
        base_keys = locale_keys.get(baseline)
        if base_keys is None:
            _fail(f"baseline locale '{baseline}' not found under {LOCALES_DIR}")
            continue
        for lang in sorted(locale_keys):
            if lang == baseline:
                continue
            for key in sorted(base_keys - locale_keys[lang]):
                tag = (lang, key)
                if tag in seen:
                    continue
                seen.add(tag)
                errors.append((baseline, lang, key))
    return errors


def check_missing_in_source(
    baseline_keys: set[str],
    used_keys: set[str],
) -> list[str]:
    """Return keys that are in the used set but absent from baseline."""
    return sorted(k for k in used_keys if k not in baseline_keys)


def check_unused(
    locale_keys: dict[str, set[str]],
    used: set[str],
    wildcards: set[str],
) -> dict[str, list[str]]:
    def exempt(key: str) -> bool:
        if key in used:
            return True
        return any(key.startswith(w) for w in wildcards)

    out: dict[str, list[str]] = {}
    for lang, keys in locale_keys.items():
        dead = sorted(k for k in keys if not exempt(k))
        if dead:
            out[lang] = dead
    return out


# ── main ───────────────────────────────────────────────────────────────


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--no-fail", action="store_true", help="report problems but never exit non-zero")
    p.add_argument("--quiet", action="store_true", help="suppress per-key detail lines")
    p.add_argument("--strict", action="store_true", help="treat unused keys as a hard error too")
    p.add_argument("--json", action="store_true", help="emit machine-readable JSON to stdout")
    p.add_argument(
        "--allowlist", type=Path, default=DEFAULT_ALLOWLIST,
        help=f"unused-key allowlist (default: {DEFAULT_ALLOWLIST.name})",
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()

    if not LOCALES_DIR.is_dir():
        _fail(f"locales directory not found: {LOCALES_DIR}")
        return 1

    locale_keys = load_locale_keys()
    if not locale_keys:
        _fail("no locales discovered")
        return 1

    namespaces = {k.split(".", 1)[0] for k in locale_keys.get("en", set())}
    if not namespaces:
        namespaces = {k.split(".", 1)[0] for ks in locale_keys.values() for k in ks}
    baseline_keys = locale_keys.get("en", set())

    langs = sorted(locale_keys)

    # ── scan ──────────────────────────────────────────────────────────────
    scan = scan_sources(namespaces)
    allow_exact, allow_prefix = _load_allowlist(args.allowlist)

    # Resolve annotations against baseline keys
    annotated_keys = _resolve_annotations(scan, baseline_keys)

    # Union of all "used" signals (feeds unused + missing-in-source)
    used_keys = scan.used_literal | annotated_keys | allow_exact | scan.heuristic_used
    # Wildcards = template static heads + allowlist prefixes
    wildcards = scan.wildcard_prefixes | allow_prefix

    # We need a smaller set for missing-in-source: direct literal refs +
    # annotation-resolved keys (heuristic & allowlist are too loose).
    high_confidence_used = scan.used_literal | annotated_keys | allow_exact

    # ── checks ────────────────────────────────────────────────────────────
    missing_locale = check_missing_in_locales(locale_keys)
    missing_source = check_missing_in_source(baseline_keys, high_confidence_used)
    unused = check_unused(locale_keys, used_keys, wildcards)
    token_parity = check_token_parity(load_locale_values())

    total_unused = sum(len(v) for v in unused.values())

    # ── JSON path ─────────────────────────────────────────────────────────
    if args.json:
        payload = {
            "ok": not (
                missing_locale or missing_source or scan.scope_findings
                or token_parity
                or (args.strict and total_unused)
            ),
            "summary": {
                "locales": {lang: len(locale_keys[lang]) for lang in langs},
                "baselines": list(BASELINE_LANGS),
                "namespaces": sorted(namespaces),
                "literal_references": len(scan.used_literal),
                "annotation_resolved": len(annotated_keys),
                "heuristic_hits": len(scan.heuristic_used),
                "wildcard_prefixes": sorted(scan.wildcard_prefixes),
                "unresolved_dynamic": len(scan.dynamic_sites),
                "scope_checked_sites": scan.scope_checked,
            },
            "missing_in_locales": [
                {"baseline": b, "lang": lang, "key": k} for b, lang, k in missing_locale
            ],
            "missing_in_source": missing_source,
            "token_parity": [
                {"lang": lang, "key": k, "detail": d} for lang, k, d in token_parity
            ],
            "unused": {lang: keys for lang, keys in unused.items()},
            "wrong_resolver": [
                {"file": f, "line": n, "key": k, "binding": b, "kind": kind}
                for f, n, k, b, kind in scan.scope_findings
            ],
            "annotation_sites": [
                {"file": f, "line": n, "patterns": p} for f, n, p in scan.annotations
            ],
            "dynamic_sites": [
                {"file": f, "line": n, "snippet": s} for f, n, s in scan.dynamic_sites
            ],
        }
        json.dump(payload, sys.stdout, indent=2, ensure_ascii=False)
        sys.stdout.write("\n")
        if payload["ok"] or args.no_fail:
            return 0
        return 1

    # ── human-readable path ──────────────────────────────────────────────
    _info(
        f"locales: {', '.join(langs)}  "
        f"({', '.join(f'{lang}:{len(locale_keys[lang])}' for lang in langs)})"
    )
    _dim(
        f"scan: {len(scan.used_literal)} literal refs, "
        f"{len(annotated_keys)} annotation keys, "
        f"{len(scan.wildcard_prefixes)} wildcard prefixes, "
        f"{len(scan.dynamic_sites)} unresolved dynamic site(s)"
    )

    # 1. missing in locales
    if missing_locale:
        _fail(
            f"missing translation keys: {len(missing_locale)} "
            f"(baselines: {', '.join(BASELINE_LANGS)})"
        )
        if not args.quiet:
            by_lang: dict[str, list[tuple[str, str]]] = {}
            for baseline, lang, key in missing_locale:
                by_lang.setdefault(lang, []).append((baseline, key))
            for lang in sorted(by_lang):
                _dim(f"{lang} — {len(by_lang[lang])} missing:")
                for baseline, key in by_lang[lang]:
                    _dim(f"    {key}   (in {baseline}, absent in {lang})")
    else:
        _ok(f"translation completeness ok ({', '.join(BASELINE_LANGS)} baseline)")

    # 2. missing in source
    if missing_source:
        _fail(f"source references undefined key(s): {len(missing_source)}")
        if not args.quiet:
            for key in missing_source:
                # Try to find a file reference from literal matches
                _dim(f"    {key}")
    else:
        _ok("no source→locale key mismatches")

    # 3. unused
    if total_unused:
        level = "ERROR" if args.strict else "WARN"
        _emit(
            level,
            f"potentially unused keys: {total_unused} "
            f"(wildcard prefixes: {', '.join(sorted(scan.wildcard_prefixes)) or 'none'})"
        )
        if not args.quiet:
            for lang in sorted(unused):
                _dim(f"{lang} — {len(unused[lang])} unused:")
                for key in unused[lang]:
                    _dim(f"    {key}")
    else:
        _ok("no unused keys detected")

    # 4. wrong-resolver routing
    if scan.scope_findings:
        _fail(f"wrong-resolver i18n routing: {len(scan.scope_findings)} site(s)")
        if not args.quiet:
            for file, line, key, binding, kind in scan.scope_findings:
                if kind == "app_routed":
                    _dim(
                        f"    namespace-qualified i18n key routed through "
                        f"vue-i18n (renders the raw key): {key} at {file}:{line} via {binding}"
                    )
                else:
                    _dim(
                        f"    app locale key routed through hikari's t "
                        f"(unlocalized fallback or raw key): {key} at {file}:{line} via {binding}"
                    )
    else:
        _ok(f"no wrong-resolver i18n routing ({scan.scope_checked} literal site(s) scope-checked)")

    # 5. cross-locale placeholder-token parity
    if token_parity:
        _fail(
            f"placeholder-token parity broken: {len(token_parity)} value(s) "
            "drop/rename a {token} vs en (renders empty holes at runtime)"
        )
        if not args.quiet:
            by_lang: dict[str, list[tuple[str, str]]] = {}
            for lang, key, detail in token_parity:
                by_lang.setdefault(lang, []).append((key, detail))
            for lang in sorted(by_lang):
                _dim(f"{lang} — {len(by_lang[lang])} value(s):")
                for key, detail in by_lang[lang]:
                    _dim(f"    {key}   ({detail})")
    else:
        _ok("placeholder-token parity ok (all locales vs en)")

    if scan.annotations and not args.quiet:
        _dim(f"annotation sites: {len(scan.annotations)}")
        for file, line, patterns in scan.annotations:
            _dim(f"    {file}:{line}  i18n: {', '.join(patterns)}")

    hard_fail = bool(
        missing_locale or missing_source or scan.scope_findings
        or token_parity
        or (args.strict and total_unused)
    )
    if hard_fail:
        parts = []
        if missing_locale:
            parts.append(f"{len(missing_locale)} missing translation(s)")
        if missing_source:
            parts.append(f"{len(missing_source)} undefined source key(s)")
        if scan.scope_findings:
            parts.append(f"{len(scan.scope_findings)} wrong-resolver site(s)")
        if token_parity:
            parts.append(f"{len(token_parity)} placeholder-token parity break(s)")
        if args.strict and total_unused:
            parts.append(f"{total_unused} unused key(s) [--strict]")
        _fail(f"i18n check failed: {', '.join(parts)}")
        return 0 if args.no_fail else 1

    _ok("i18n check passed")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
