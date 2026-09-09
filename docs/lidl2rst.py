#!/usr/bin/env python3
"""Render a LIDL module contract as reStructuredText for Sphinx.

This is the chat module's equivalent of the Doxygen step in the other Logos
modules: it turns the contract's declarations and their ``;`` comments into
``.rst`` fragments that ``docs/pages/api_reference.rst`` includes.

It exists because the canonical LIDL frontend (``logos-co/logos-lidl``) parses
to an AST that carries no comments -- its own spec states that whitespace and
comments are not preserved, and ``MethodDecl.description`` is only ever filled
by the Rust ``--from-rust`` extractor, not by reading a ``.lidl`` file. Every
word of prose in ``rust-lib/chat_module.lidl`` lives in ``;`` comments, so
reaching it means parsing the text.

Usage::

    ./docs/lidl2rst.py rust-lib/chat_module.lidl -o docs/_generated

Writes ``methods.rst``, ``events.rst`` and ``records.rst`` into the output
directory. Exits non-zero on a parse error or if a declaration present in the
source is missing from the output, so a broken contract fails the docs build
rather than silently publishing a short API reference.
"""

import argparse
import os
import re
import textwrap
import sys


# A comment that only separates the file into regions, e.g. "; --- events ---".
SECTION_MARKER = re.compile(r"-{2,}.*-{2,}\Z")

TOKEN = re.compile(
    r"""
      (?P<ws>\s+)
    | (?P<str>"(?:\\.|[^"\\])*")
    | (?P<ident>[A-Za-z_][A-Za-z0-9_]*)
    | (?P<punct>->|[{}\[\](),:?])
    """,
    re.VERBOSE,
)


class LidlError(Exception):
    pass


# ── source → comments + tokens ────────────────────────────────────────────────


def split_comments(source):
    """Strip ``;`` comments, returning the code text and what was stripped.

    Comments come back as ``{line: (text, own_line)}``, 1-based, where
    ``own_line`` is true when nothing but the comment was on that line.
    """
    comments = {}
    code_lines = []
    for lineno, line in enumerate(source.splitlines(), start=1):
        in_string = False
        escaped = False
        cut = None
        for i, ch in enumerate(line):
            if escaped:
                escaped = False
            elif ch == "\\" and in_string:
                escaped = True
            elif ch == '"':
                in_string = not in_string
            elif ch == ";" and not in_string:
                cut = i
                break
        if cut is None:
            code_lines.append(line)
            continue
        before = line[:cut]
        comments[lineno] = (line[cut + 1:].strip(), not before.strip())
        code_lines.append(before)
    return "\n".join(code_lines), comments


def tokenize(code):
    tokens = []
    lineno = 1
    pos = 0
    while pos < len(code):
        m = TOKEN.match(code, pos)
        if not m:
            raise LidlError(f"line {lineno}: unexpected character {code[pos]!r}")
        kind = m.lastgroup
        value = m.group()
        if kind != "ws":
            tokens.append((kind, value, lineno))
        lineno += value.count("\n")
        pos = m.end()
    tokens.append(("eof", "", lineno))
    return tokens


# ── tokens → AST ──────────────────────────────────────────────────────────────


class Parser:
    def __init__(self, tokens):
        self.tokens = tokens
        self.i = 0

    def peek(self):
        return self.tokens[self.i]

    def next(self):
        tok = self.tokens[self.i]
        self.i += 1
        return tok

    def expect(self, kind, value=None):
        kind_, value_, line = self.next()
        if kind_ != kind or (value is not None and value_ != value):
            want = value if value is not None else kind
            raise LidlError(f"line {line}: expected {want!r}, found {value_!r}")
        return value_, line

    def accept(self, value):
        if self.peek()[1] == value:
            self.next()
            return True
        return False

    def parse_module(self):
        self.expect("ident", "module")
        name, line = self.expect("ident")
        self.expect("punct", "{")
        module = {
            "name": name,
            "line": line,
            "types": [],
            "methods": [],
            "events": [],
        }
        while not self.accept("}"):
            kind, value, line = self.peek()
            if kind == "eof":
                raise LidlError("unexpected end of input: unclosed module body")
            if value in ("version", "description", "category"):
                self.next()
                self.expect("str")
            elif value in ("depends", "optional_depends"):
                self.next()
                self.expect("punct", "[")
                while not self.accept("]"):
                    self.expect("ident")
                    self.accept(",")
            elif value == "type":
                module["types"].append(self.parse_type())
            elif value == "method":
                module["methods"].append(self.parse_method())
            elif value == "event":
                module["events"].append(self.parse_event())
            else:
                raise LidlError(f"line {line}: unexpected declaration {value!r}")
        return module

    def parse_type(self):
        _, line = self.expect("ident", "type")
        name, _ = self.expect("ident")
        self.expect("punct", "{")
        fields = []
        while not self.accept("}"):
            optional = self.accept("?")
            fname, fline = self.expect("ident")
            self.expect("punct", ":")
            ftype = self.parse_type_expr()
            fields.append(
                {
                    "name": fname,
                    "type": ftype,
                    "optional": optional or ftype.startswith("?"),
                    "line": fline,
                }
            )
        return {"name": name, "line": line, "fields": fields}

    def parse_method(self):
        _, line = self.expect("ident", "method")
        name, _ = self.expect("ident")
        params = self.parse_params()
        returns = self.parse_type_expr() if self.accept("->") else None
        return {"name": name, "line": line, "params": params, "returns": returns}

    def parse_event(self):
        _, line = self.expect("ident", "event")
        name, _ = self.expect("ident")
        return {"name": name, "line": line, "params": self.parse_params()}

    def parse_params(self):
        self.expect("punct", "(")
        params = []
        while not self.accept(")"):
            pname, _ = self.expect("ident")
            self.expect("punct", ":")
            params.append({"name": pname, "type": self.parse_type_expr()})
            self.accept(",")
        return params

    def parse_type_expr(self):
        kind, value, line = self.next()
        if value == "?":
            return "?" + self.parse_type_expr()
        if value == "[":
            inner = self.parse_type_expr()
            self.expect("punct", "]")
            return f"[{inner}]"
        if value == "{":
            key = self.parse_type_expr()
            self.expect("punct", ":")
            val = self.parse_type_expr()
            self.expect("punct", "}")
            return f"{{{key}: {val}}}"
        if kind == "ident":
            return value
        raise LidlError(f"line {line}: expected a type, found {value!r}")


# ── comment attachment ────────────────────────────────────────────────────────


def doc_for(line, comments):
    """The paragraphs documenting the declaration starting at ``line``.

    Own-line comments immediately above it, plus a trailing comment on the line
    itself. A blank comment starts a new paragraph; a section marker ends the
    block and is dropped.
    """
    block = []
    probe = line - 1
    while probe in comments and comments[probe][1]:
        text = comments[probe][0]
        if SECTION_MARKER.fullmatch(text):
            break
        block.append(text)
        probe -= 1
    block.reverse()

    trailing = comments.get(line)
    if trailing and not trailing[1] and not SECTION_MARKER.fullmatch(trailing[0]):
        block.append("")
        block.append(trailing[0])

    paragraphs = []
    current = []
    for text in block:
        if text:
            current.append(text)
        elif current:
            paragraphs.append(" ".join(current))
            current = []
    if current:
        paragraphs.append(" ".join(current))
    return paragraphs


# ── rendering ─────────────────────────────────────────────────────────────────


def escape(text):
    return re.sub(r"([\\*|_`])", r"\\\1", text)


def rst_text(text):
    """Contract prose as RST: backtick spans become literals, the rest is escaped."""
    parts = text.split("`")
    if len(parts) % 2 == 0:
        return escape(text)
    rendered = []
    for i, part in enumerate(parts):
        if i % 2:
            rendered.append(f"``{part}``")
        else:
            # RST needs whitespace or punctuation after an inline literal.
            if rendered and part[:1].isalnum():
                rendered.append("\\ ")
            rendered.append(escape(part))
    return "".join(rendered)


def wrap(text, width=79, initial="   ", subsequent="   "):
    return textwrap.fill(
        text,
        width=width,
        initial_indent=initial,
        subsequent_indent=subsequent,
        break_long_words=False,
        break_on_hyphens=False,
    )


def indent(text, prefix="   "):
    return "\n".join(prefix + line if line else "" for line in text.split("\n"))


def signature(decl):
    params = ", ".join(f"{p['name']}: {p['type']}" for p in decl["params"])
    sig = f"{decl['name']}({params})"
    if decl.get("returns"):
        sig += f" -> {decl['returns']}"
    return sig


def heading(title, char="~"):
    return f"{title}\n{char * len(title)}\n"


def render_callable(decl, comments):
    """A section per declaration, so it gets an anchor and a page-nav entry."""
    out = [heading(f"{decl['name']}()"), f".. describe:: {signature(decl)}", ""]
    for paragraph in doc_for(decl["line"], comments):
        out.append(wrap(rst_text(paragraph)))
        out.append("")
    return "\n".join(out)


def render_record(decl, comments):
    # "type Name", not "Name": a record and a method may differ only in case,
    # and reStructuredText would give the two sections the same anchor.
    out = [heading(f"type {decl['name']}")]
    for paragraph in doc_for(decl["line"], comments):
        out.append(wrap(rst_text(paragraph), initial="", subsequent=""))
        out.append("")
    for field in decl["fields"]:
        ftype = field["type"].lstrip("?")
        marker = f"``{ftype}``, optional" if field["optional"] else f"``{ftype}``"
        bullet = f"- ``{field['name']}`` ({marker})"
        paragraphs = doc_for(field["line"], comments)
        if paragraphs:
            bullet += " -- " + rst_text(paragraphs[0])
        out.append(wrap(bullet, initial="", subsequent="  "))
        for extra in paragraphs[1:]:
            out.append("")
            out.append(wrap(rst_text(extra), initial="  ", subsequent="  "))
        out.append("")
    return "\n".join(out)


HEADER = """.. Generated by docs/lidl2rst.py from {source} -- do not edit.
"""


def render(module, comments, source_name):
    sections = {
        "methods.rst": [render_callable(m, comments) for m in module["methods"]],
        "events.rst": [render_callable(e, comments) for e in module["events"]],
        "records.rst": [render_record(t, comments) for t in module["types"]],
    }
    return {
        name: HEADER.format(source=source_name) + "\n" + "\n".join(blocks)
        for name, blocks in sections.items()
    }


# ── entry point ───────────────────────────────────────────────────────────────


def clashing_anchors(module):
    """Section titles that reStructuredText would give the same anchor."""
    titles = (
        [f"{decl['name']}()" for decl in module["methods"]]
        + [f"{decl['name']}()" for decl in module["events"]]
        + [f"type {decl['name']}" for decl in module["types"]]
    )
    seen = {}
    for title in titles:
        seen.setdefault(re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-"), []).append(
            title
        )
    return {anchor: names for anchor, names in seen.items() if len(names) > 1}


def count_declarations(source):
    """A parser-independent count, so a silently dropped declaration is caught."""
    code, _ = split_comments(source)
    return {
        keyword: len(re.findall(rf"^\s*{keyword}\s+\w", code, re.MULTILINE))
        for keyword in ("type", "method", "event")
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("contract", help="path to the .lidl contract")
    parser.add_argument("-o", "--out", required=True, help="output directory")
    args = parser.parse_args()

    with open(args.contract, encoding="utf-8") as handle:
        source = handle.read()

    try:
        code, comments = split_comments(source)
        module = Parser(tokenize(code)).parse_module()
    except LidlError as error:
        sys.exit(f"{args.contract}: {error}")

    expected = count_declarations(source)
    actual = {
        "type": len(module["types"]),
        "method": len(module["methods"]),
        "event": len(module["events"]),
    }
    if expected != actual:
        sys.exit(
            f"{args.contract}: parsed {actual} declarations but the source has "
            f"{expected}; the contract uses a construct this renderer drops."
        )

    clashes = clashing_anchors(module)
    if clashes:
        detail = "; ".join(f"{a}: {', '.join(n)}" for a, n in clashes.items())
        sys.exit(f"{args.contract}: declarations would share a page anchor -- {detail}")

    os.makedirs(args.out, exist_ok=True)
    for name, text in render(module, comments, os.path.basename(args.contract)).items():
        with open(os.path.join(args.out, name), "w", encoding="utf-8") as handle:
            handle.write(text)

    print(
        f"lidl2rst: {module['name']} -- {actual['method']} methods, "
        f"{actual['event']} events, {actual['type']} records -> {args.out}"
    )


if __name__ == "__main__":
    main()
