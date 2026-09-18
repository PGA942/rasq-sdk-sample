#!/usr/bin/env python3
"""schema/ と examples/ の整合を確かめる(CI の `schema` ジョブ、ローカルは `python3 schema/validate.py`)。

- 各 JSON Schema 自体が draft 2020-12 として正しいこと
- examples/manifests/*.json と examples/misskey.function.json → manifest.schema.json
- examples/*.rrl.json → rrl-node.schema.json、examples/*.rsl.json → rsl-span.schema.json、examples/*.model.json → model-manifest.schema.json
- schema/slots.json → slot-registry.schema.json
- 突き合わせ: マニフェストの slots は登録簿にあること(planned は警告)、kind がそのスロットで許されること、
  必要権限を持つこと、登録簿の hostFns は host-fn.schema.json の $defs.fns と boundary の hostFnName にあること
- action-ref.schema.json の examples が自分のパターンに合うこと

依存: pip install jsonschema referencing
"""

from __future__ import annotations

import glob
import json
import os
import re
import sys

from jsonschema import Draft202012Validator
from referencing import Registry, Resource

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCHEMA_DIR = HERE
EXAMPLES = os.path.join(ROOT, "examples")


def load(path: str):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def registry() -> Registry:
    """相対 $ref(`rrl-node.schema.json`、`slot-registry.schema.json#/$defs/kind`)をファイル名で解決する。"""
    reg = Registry()
    for path in glob.glob(os.path.join(SCHEMA_DIR, "*.schema.json")):
        schema = load(path)
        res = Resource.from_contents(schema)
        name = os.path.basename(path)
        reg = reg.with_resource(name, res).with_resource("./" + name, res)
        if "$id" in schema:
            reg = reg.with_resource(schema["$id"], res)
            # 相対 $ref は参照元の $id を基底に解決される(https://rasq.io/schema/<ファイル名>)ので、その名前でも引けるようにする。
            base = schema["$id"].rsplit("/", 1)[0] + "/"
            reg = reg.with_resource(base + name, res)
        reg = reg.with_resource("https://rasq.io/schema/" + name, res)
    return reg


class Report:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self.checked = 0

    def error(self, msg: str) -> None:
        self.errors.append(msg)

    def warn(self, msg: str) -> None:
        self.warnings.append(msg)


def validate(rep: Report, reg: Registry, schema_name: str, doc, label: str) -> bool:
    schema = load(os.path.join(SCHEMA_DIR, schema_name))
    v = Draft202012Validator(schema, registry=reg)
    errs = sorted(v.iter_errors(doc), key=lambda e: list(e.path))
    rep.checked += 1
    for e in errs[:8]:
        where = "/".join(str(p) for p in e.path) or "(root)"
        rep.error(f"{label}: {where}: {e.message[:160]}")
    return not errs


def main() -> int:
    rep = Report()
    reg = registry()

    # 1. スキーマ自体
    for path in sorted(glob.glob(os.path.join(SCHEMA_DIR, "*.schema.json"))):
        try:
            Draft202012Validator.check_schema(load(path))
            rep.checked += 1
        except Exception as e:  # noqa: BLE001
            rep.error(f"{os.path.basename(path)}: schema invalid: {e}")

    # 2. 登録簿
    slots_doc = load(os.path.join(SCHEMA_DIR, "slots.json"))
    validate(rep, reg, "slot-registry.schema.json", slots_doc, "schema/slots.json")
    slots = {s["id"]: s for s in slots_doc["slots"]}
    host_fns = set(load(os.path.join(SCHEMA_DIR, "host-fn.schema.json"))["$defs"]["fns"])
    boundary_fns = set(load(os.path.join(SCHEMA_DIR, "boundary.schema.json"))["$defs"]["hostFnName"]["enum"])
    if host_fns != boundary_fns:
        rep.error(f"host-fn.schema.json の fns と boundary hostFnName がずれています: {sorted(host_fns ^ boundary_fns)}")
    for s in slots_doc["slots"]:
        for fn in s["hostFns"]:
            if fn not in host_fns:
                rep.error(f"slots.json {s['id']}: hostFn {fn} が host-fn.schema.json にありません")

    # 3. マニフェスト(例 + サンプル)
    manifests = sorted(glob.glob(os.path.join(EXAMPLES, "manifests", "*.json"))) + [os.path.join(EXAMPLES, "misskey.function.json")]
    for path in manifests:
        label = os.path.relpath(path, ROOT)
        m = load(path)
        if not validate(rep, reg, "manifest.schema.json", m, label):
            continue
        for sid in m.get("slots", []):
            s = slots.get(sid)
            if s is None:
                rep.error(f"{label}: slot {sid} は登録簿にありません")
                continue
            if s["status"] == "planned":
                rep.warn(f"{label}: slot {sid} は planned(まだホストが無い)")
            if m["kind"] not in s["kinds"]:
                rep.error(f"{label}: kind {m['kind']} は slot {sid} では使えません({s['kinds']})")
            missing = [p for p in s["permissions"]["required"] if p not in m.get("permissions", [])]
            if missing:
                rep.error(f"{label}: slot {sid} に必要な権限が足りません: {missing}")
        for a in m.get("actions", []):
            if ":" in a["id"]:
                rep.error(f"{label}: action id に ':' は使えません: {a['id']}")

    # 4. ほかの例
    for pattern, schema_name in (("*.rrl.json", "rrl-node.schema.json"), ("*.rsl.json", "rsl-span.schema.json"), ("*.model.json", "model-manifest.schema.json")):
        for path in sorted(glob.glob(os.path.join(EXAMPLES, pattern))):
            doc = load(path)
            # snapshot メッセージごと置いてある例(post-tile.rrl.json)は境界スキーマで見る(root が RRL ノード)。
            name = "boundary.schema.json" if isinstance(doc, dict) and doc.get("schema") == "rasq.boundary/1" else schema_name
            validate(rep, reg, name, doc, os.path.relpath(path, ROOT))

    # 5. ActionRef の文法(examples と切り出し規則)
    ar = load(os.path.join(SCHEMA_DIR, "action-ref.schema.json"))["$defs"]
    for key in ("actionRef", "actionKey", "instanceScope"):
        pat = re.compile(ar[key]["pattern"])
        for ex in ar[key]["examples"]:
            rep.checked += 1
            if not pat.match(ex):
                rep.error(f"action-ref {key}: 例 {ex!r} が pattern に合いません")
    ref = "rasq.official.misskey:misskey:misskey.io:yamada:0.1.0:renote"
    first, last = ref.index(":"), ref.rindex(":")
    middle = ref[first + 1 : last]
    parsed = (ref[:first], middle[: middle.rindex(":")], middle[middle.rindex(":") + 1 :], ref[last + 1 :])
    if parsed != ("rasq.official.misskey", "misskey:misskey.io:yamada", "0.1.0", "renote"):
        rep.error(f"action-ref の切り出し規則が合いません: {parsed}")
    rep.checked += 1

    for w in rep.warnings:
        print(f"warn: {w}")
    for e in rep.errors:
        print(f"ERROR: {e}")
    print(f"{rep.checked} checks, {len(rep.errors)} errors, {len(rep.warnings)} warnings")
    return 1 if rep.errors else 0


if __name__ == "__main__":
    sys.exit(main())
