#!/usr/bin/env python3
"""Patch Statsig use_hidden_models=false in ChatGPT Chromium Local Storage (LevelDB).

Primary writer for Codex Provider Hub picker guard. Requires plyvel.
Encoding observed in production: UTF-16-LE with a 1-byte Chromium type prefix.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

CONFIG_ID = "107580212"
KEY_HINT = b"statsig.cached.evaluations"


def decode_value(raw: bytes) -> list[tuple[str, str]]:
    candidates: list[tuple[str, str]] = []
    if len(raw) >= 2:
        try:
            candidates.append(("utf-16-le", raw.decode("utf-16-le")))
        except Exception:
            pass
    if len(raw) > 1:
        try:
            candidates.append(("utf-16-le+1", raw[1:].decode("utf-16-le")))
        except Exception:
            pass
        try:
            candidates.append(("utf-8+1", raw[1:].decode("utf-8")))
        except Exception:
            pass
    try:
        candidates.append(("utf-8", raw.decode("utf-8")))
    except Exception:
        pass
    return candidates


def set_flag_in_text(text: str) -> tuple[str, int]:
    patterns = [
        (r'(use_hidden_models\\?"?\s*:\s*)true\b', r"\1false"),
        (r'(use_hidden_models\\?"?\s*:\s*)"true"', r'\1"false"'),
        (r'(use_hidden_models\\?"?\s*:\s*)\\"true\\"', r'\1\\"false\\"'),
        (r'(use_hidden_models\\":\s*)true\b', r"\1false"),
        (r'(use_hidden_models\\":\s*)\\"true\\"', r'\1\\"false\\"'),
        (r'(use_hidden_models":\s*)true\b', r"\1false"),
        (r'(use_hidden_models":\s*)"true"', r'\1"false"'),
    ]
    out = text
    hits = 0
    for pat, repl in patterns:
        out, n = re.subn(pat, repl, out)
        hits += n
    return out, hits


def encode_value(enc: str, new_text: str, original: bytes) -> bytes:
    if enc == "utf-16-le+1":
        return original[:1] + new_text.encode("utf-16-le")
    if enc == "utf-16-le":
        if original[:2] == b"\xff\xfe":
            return b"\xff\xfe" + new_text.encode("utf-16-le")
        return new_text.encode("utf-16-le")
    if enc == "utf-8+1":
        return original[:1] + new_text.encode("utf-8")
    return new_text.encode("utf-8")


def true_false_counts(text: str) -> tuple[bool, bool]:
    has_true = bool(re.search(r'use_hidden_models\\?"?\s*:\s*"?true"?', text))
    has_false = bool(re.search(r'use_hidden_models\\?"?\s*:\s*"?false"?', text))
    return has_true, has_false


def patch_db(db_path: Path, dry_run: bool = False) -> dict:
    try:
        import plyvel
    except ImportError as e:
        return {
            "ok": False,
            "error": f"plyvel not importable: {e}",
            "patched": 0,
            "still_true": None,
            "now_false": None,
        }

    if not db_path.is_dir():
        return {
            "ok": False,
            "error": f"leveldb missing: {db_path}",
            "patched": 0,
            "still_true": None,
            "now_false": None,
        }

    db = plyvel.DB(str(db_path), create_if_missing=False)
    patched = 0
    inspected = 0
    details = []

    try:
        for key, val in db:
            if KEY_HINT not in key and KEY_HINT not in val:
                continue
            inspected += 1
            key_s = key.decode("utf-8", errors="replace")
            best = None
            for enc, text in decode_value(val):
                if "use_hidden_models" in text or CONFIG_ID in text:
                    best = (enc, text)
                    break
            if best is None:
                continue
            enc, text = best
            has_true, has_false = true_false_counts(text)
            new_text, hits = set_flag_in_text(text)
            entry = {
                "key": key_s[:160],
                "enc": enc,
                "has_true": has_true,
                "has_false": has_false,
                "hits": hits,
            }
            if hits == 0:
                details.append(entry)
                continue
            if not dry_run:
                db.put(key, encode_value(enc, new_text, val))
            patched += 1
            entry["patched"] = True
            details.append(entry)
    finally:
        db.close()

    # Verify
    still_true = 0
    now_false = 0
    db = plyvel.DB(str(db_path), create_if_missing=False)
    try:
        for key, val in db:
            if KEY_HINT not in key and KEY_HINT not in val:
                continue
            for enc, text in decode_value(val):
                if "use_hidden_models" not in text:
                    continue
                t, f = true_false_counts(text)
                if t:
                    still_true += 1
                if f:
                    now_false += 1
                break
    finally:
        db.close()

    return {
        "ok": still_true == 0 and (patched > 0 or now_false > 0),
        "inspected": inspected,
        "patched": patched,
        "still_true": still_true,
        "now_false": now_false,
        "details": details,
        "error": None
        if still_true == 0
        else f"still saw use_hidden_models=true in {still_true} value(s)",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--db",
        default=str(
            Path.home()
            / "Library/Application Support/Codex/Default/Local Storage/leveldb"
        ),
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    result = patch_db(Path(args.db), dry_run=args.dry_run)
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result.get("ok") else 2


if __name__ == "__main__":
    sys.exit(main())
