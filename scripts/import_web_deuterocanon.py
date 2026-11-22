#!/usr/bin/env python3
"""Import the Deuterocanonical books from the public-domain WEB Updated USFX archive.

Usage: python3 scripts/import_web_deuterocanon.py /path/to/WEB.zip

Writes DokuWiki chapter pages under src/starter/bible and a Rust include list in
src/deuterocanon.rs. Existing canonical WEB pages are deliberately untouched.
"""

from __future__ import annotations

import re
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "WEB/engwebu_usfx/engwebu_usfx.xml"
BOOKS = [
    ("TOB", "tobit", "Tobit", 14, ""),
    ("JDT", "judith", "Judith", 16, ""),
    ("ESG", "esther_greek", "Esther (Greek edition)", 10, "This is the full Greek Septuagint edition of Esther, including its Deuterocanonical additions. The existing Esther page remains the standard WEB edition."),
    ("WIS", "wisdom", "Wisdom", 19, ""),
    ("SIR", "sirach", "Sirach", 51, "Also known as Ecclesiasticus."),
    ("BAR", "baruch", "Baruch", 6, "Chapter 6 contains the Letter of Jeremiah."),
    ("1MA", "1_maccabees", "1 Maccabees", 16, ""),
    ("2MA", "2_maccabees", "2 Maccabees", 15, ""),
    ("1ES", "1_esdras", "1 Esdras", 9, ""),
    ("MAN", "prayer_of_manasses", "Prayer of Manasses", 1, ""),
    ("PS2", "psalm_151", "Psalm 151", 1, ""),
    ("3MA", "3_maccabees", "3 Maccabees", 7, ""),
    ("2ES", "2_esdras", "2 Esdras", 16, "Also called 4 Ezra in some traditions."),
    ("4MA", "4_maccabees", "4 Maccabees", 18, ""),
    ("DAG", "daniel_greek", "Daniel (Greek edition)", 14, "This is the full Greek edition of Daniel, including its Deuterocanonical sections. The existing Daniel page remains the standard WEB edition."),
]


def clean(text: str) -> str:
    return re.sub(r"\s+", " ", text or "").strip()


def inline_text(elem: ET.Element) -> str:
    """Flatten USFX inline markup while keeping footnotes readable."""
    chunks: list[str] = [elem.text or ""]
    for child in elem:
        if child.tag == "f":
            note = clean("".join(child.itertext()))
            if note:
                chunks.append(f" (({note})) ")
        elif child.tag not in {"ve", "v"}:
            chunks.append(inline_text(child))
        chunks.append(child.tail or "")
    return "".join(chunks)


def paragraph_verses(block: ET.Element) -> list[str]:
    verses: list[str] = []
    active: str | None = None
    saw_verse = False
    for child in block:
        if child.tag == "v":
            saw_verse = True
            if active is not None:
                verses.append(active.strip())
            active = f"**{child.get('id', '')}** "
            active += child.text or ""
        elif child.tag == "ve":
            if active is not None:
                active += child.text or ""
                verses.append(active.strip())
                active = None
        elif active is not None:
            if child.tag == "f":
                note = clean("".join(child.itertext()))
                if note:
                    active += f" (({note})) "
            else:
                active += inline_text(child)
        if active is not None:
            active += child.tail or ""
    if active is not None:
        verses.append(active.strip())
    if not saw_verse:
        continuation = clean(inline_text(block))
        return [continuation] if continuation else []
    return [re.sub(r"\s+", " ", v).strip() for v in verses if v.strip()]


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    archive = zipfile.ZipFile(sys.argv[1])
    root = ET.fromstring(archive.read(SOURCE))
    by_id = {book.get("id"): book for book in root.findall("book")}
    pages: list[Path] = []
    total_chapters = 0

    for code, slug, title, expected, description in BOOKS:
        book = by_id.get(code)
        if book is None:
            raise SystemExit(f"Missing source book: {code}")
        children = list(book)
        chapters = [(i, node.get("id")) for i, node in enumerate(children) if node.tag == "c"]
        if len(chapters) != expected:
            raise SystemExit(f"{code}: expected {expected} chapters, found {len(chapters)}")
        total_chapters += len(chapters)
        book_dir = ROOT / "src/starter/bible" / slug
        book_dir.mkdir(parents=True, exist_ok=True)

        chapter_ids = [int(ch) for _, ch in chapters]
        start = [f"====== {title} ======", "", "[[bible:start|WEB Bible]]", ""]
        if description:
            start += [description, ""]
        start += ["===== Choose a chapter =====", ""]
        for group_start in range(0, len(chapter_ids), 10):
            group = chapter_ids[group_start:group_start + 10]
            start.append(" · ".join(f"[[bible:{slug}:{n}|{n}]]" for n in group))
            start.append("")
        start += [f"[[bible:{slug}:1|Begin reading {title} →]]", ""]
        start_path = book_dir / "start.txt"
        start_path.write_text("\n".join(start), encoding="utf-8")
        pages.append(start_path)

        for ci, (idx, chap) in enumerate(chapters):
            end = chapters[ci + 1][0] if ci + 1 < len(chapters) else len(children)
            blocks: list[str] = []
            for node in children[idx + 1:end]:
                if node.tag in {"p", "q"}:
                    verses = paragraph_verses(node)
                    if verses:
                        blocks.append(" ".join(verses))
                    elif node.get("sfm") in {"s", "s1", "s2", "d"}:
                        heading = clean("".join(node.itertext()))
                        if heading:
                            blocks.append(f"==== {heading} ====")
                    elif node.get("sfm") in {"ip", "im", "pi"}:
                        text = clean("".join(node.itertext()))
                        if text:
                            blocks.append(text)
            if not blocks:
                raise SystemExit(f"No text found for {code} chapter {chap}")
            n = int(chap)
            prev_link = f"[[bible:{slug}:{n-1}|← Previous]]" if n > 1 else f"[[bible:{slug}:start|← {title}]]"
            next_link = f"[[bible:{slug}:{n+1}|Next →]]" if n < expected else f"[[bible:{slug}:start|{title} index →]]"
            body = [f"====== {title} {n} ======", "", f"[[bible:start|WEB Bible]] · [[bible:{slug}:start|{title}]]", "", "----", ""]
            body.extend(blocks)
            body += ["", "----", "", f"{prev_link} | {next_link}", "", "For study: [[bible:notes:entry:new|Create a study note]] or record a passage reference in your [[journal:start|Journal]]. Your notes stay separate from the Bible text.", ""]
            path = book_dir / f"{n}.txt"
            path.write_text("\n".join(body), encoding="utf-8")
            pages.append(path)

    rust = ["// Generated by scripts/import_web_deuterocanon.py; do not edit by hand.", "pub const PAGES: &[(&str, &str)] = &["]
    for path in pages:
        rel = path.relative_to(ROOT / "src").as_posix()
        wiki_rel = "data/pages/" + path.relative_to(ROOT / "src/starter").as_posix()
        rust.append(f'    ("{wiki_rel}", include_str!("{rel}")),')
    rust.append("];\n")
    (ROOT / "src/deuterocanon.rs").write_text("\n".join(rust), encoding="utf-8")
    print(f"Generated {len(BOOKS)} books, {total_chapters} chapters, {len(pages)} pages.")


if __name__ == "__main__":
    main()
