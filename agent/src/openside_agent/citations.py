"""Sentence-level citation coverage of research answers (A-9).

Standard library only: tools/citation-harness.py loads this file by path, outside the
sidecar. The heuristic is documented in docs/specs/ultrabrowse.md §3:

- The answer is split into sentences per line, after dropping headings, fenced code and
  list markers. Links right after a sentence's end punctuation belong to that sentence
  ("Fact. [src](url) Next fact."), and common abbreviations ("e.g.", "U.S.") or initials
  do not end a sentence.
- A sentence is a factual claim when, with links and URLs removed, it has at least
  MIN_CLAIM_WORDS words and does not end with "?".
- A claim is cited when it contains a URL that, normalized, equals a page visited in the
  session. A link to a page that was never opened counts as uncited.
- Visited pages come only from structured `url` fields of browser tool results, never
  from URLs that appear inside page text.
"""

from __future__ import annotations

import re
from collections.abc import Iterable
from dataclasses import dataclass
from typing import cast
from urllib.parse import urlsplit, urlunsplit

MARKER = "[citation needed]"
MIN_CLAIM_WORDS = 4

_URL_CHAR = r"[^\s<>()\[\]\"'`]"
# One level of balanced parentheses keeps URLs like .../wiki/Nile_(river) whole.
URL = re.compile(rf"https?://(?:{_URL_CHAR}|\({_URL_CHAR}*\))+")
LINK = re.compile(rf"\[[^\]]*\]\((https?://(?:{_URL_CHAR}|\({_URL_CHAR}*\))+)\)")
WORD = re.compile(r"\w+")
SENTENCE_END = re.compile(
    rf"(?<=[.!?])((?:\s*\[[^\]]*\]\((?:{_URL_CHAR}|\({_URL_CHAR}*\))+\))*)\s+"
)
LIST_MARKER = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+")
# "etc." and "no." often end sentences, so they still split.
ABBREVIATIONS = frozenset(
    "e.g. i.e. cf. vs. approx. ca. dr. mr. mrs. ms. prof. st. fig. inc. ltd. co. jr. "
    "sr. u.s. u.k. u.n. e.u. a.m. p.m.".split()
)
INITIALS = re.compile(r"(?:[^\W\d_]\.){1,2}")


@dataclass(frozen=True)
class Claim:
    text: str
    urls: tuple[str, ...]
    cited: bool
    # Offset just past the sentence in the checked answer, where annotate() marks it.
    end: int


@dataclass(frozen=True)
class Report:
    claims: tuple[Claim, ...]

    @property
    def uncited(self) -> list[Claim]:
        return [claim for claim in self.claims if not claim.cited]

    @property
    def ratio(self) -> float:
        return len(self.uncited) / len(self.claims) if self.claims else 0.0

    def to_json(self) -> dict[str, object]:
        return {
            "claims": len(self.claims),
            "uncited": len(self.uncited),
            "uncited_ratio": round(self.ratio, 4),
            "uncited_claims": [claim.text for claim in self.uncited],
        }


def normalize(url: str) -> str:
    """Scheme/host case, query, fragment, trailing slash or punctuation do not count."""
    url = url.rstrip(".,;:!?")
    parts = urlsplit(url)
    path = parts.path.rstrip("/")
    return urlunsplit((parts.scheme.lower(), parts.netloc.lower(), path, "", ""))


def result_urls(value: object) -> set[str]:
    """Normalized page URLs named by browser tool results.

    Accepts a URL string, a result object (its top-level `url` field) or a list of
    those. Other strings inside a result (snapshot text, console output, return values)
    are page content and are deliberately ignored.
    """
    items: list[object] = (
        list(cast(Iterable[object], value))
        if isinstance(value, (list, tuple))
        else [value]
    )
    found: set[str] = set()
    for item in items:
        url = (
            cast(dict[str, object], item).get("url") if isinstance(item, dict) else item
        )
        if isinstance(url, str) and url.startswith(("http://", "https://")):
            found.add(normalize(url))
    return found


def _plain(sentence: str) -> str:
    return URL.sub(" ", LINK.sub(" ", sentence.replace(MARKER, " ")))


def _abbreviated(line: str, period: int) -> bool:
    """Whether the '.' at `period` ends an abbreviation or initial, not a sentence."""
    if line[period] != ".":
        return False
    token = line[: period + 1].rsplit(None, 1)[-1].lstrip("([\"'").lower()
    return token in ABBREVIATIONS or bool(INITIALS.fullmatch(token))


def sentence_spans(answer: str) -> list[tuple[int, int]]:
    """(start, end) offsets of each sentence in `answer`."""
    spans: list[tuple[int, int]] = []

    def add(start: int, end: int) -> None:
        if start >= end:
            return
        if spans and not WORD.search(_plain(answer[start:end])):
            spans[-1] = (spans[-1][0], end)
        else:
            spans.append((start, end))

    fenced = False
    offset = 0
    for raw in answer.splitlines(keepends=True):
        base = offset
        offset += len(raw)
        line = raw.rstrip("\r\n")
        stripped = line.strip()
        if stripped.startswith("```"):
            fenced = not fenced
            continue
        if fenced or not stripped or stripped.startswith("#"):
            continue
        marker = LIST_MARKER.match(line)
        start = marker.end() if marker else len(line) - len(line.lstrip())
        for gap in SENTENCE_END.finditer(line, start):
            if not gap.group(1) and _abbreviated(line, gap.start() - 1):
                continue
            add(base + start, base + gap.end(1))
            start = gap.end()
        add(base + start, base + len(line.rstrip()))
    return spans


def sentences(answer: str) -> list[str]:
    return [answer[start:end] for start, end in sentence_spans(answer)]


def check(answer: str, visited: Iterable[str]) -> Report:
    sources = {normalize(url) for url in visited}
    claims: list[Claim] = []
    for start, end in sentence_spans(answer):
        sentence = answer[start:end]
        plain = _plain(sentence).strip()
        if len(WORD.findall(plain)) < MIN_CLAIM_WORDS or plain.endswith("?"):
            continue
        urls = tuple(URL.findall(sentence))
        cited = any(normalize(url) in sources for url in urls)
        claims.append(Claim(text=sentence, urls=urls, cited=cited, end=end))
    return Report(claims=tuple(claims))


def annotate(answer: str, report: Report) -> str:
    """Append MARKER after each uncited claim; the answer is otherwise unchanged."""
    out = answer
    for claim in reversed(report.uncited):
        if not claim.text.endswith(MARKER):
            out = f"{out[: claim.end]} {MARKER}{out[claim.end :]}"
    return out
