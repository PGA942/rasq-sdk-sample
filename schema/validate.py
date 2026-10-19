#!/usr/bin/env python3
"""schema/ と examples/ の整合を確かめる(CI の `schema` ジョブ、ローカルは `python3 schema/validate.py`)。

- 各 JSON Schema 自体が draft 2020-12 として正しいこと
- examples/manifests/*.json と examples/misskey.function.json → manifest.schema.json
- examples/*.rrl.json → rrl-node.schema.json、examples/*.rsl.json → rsl-span.schema.json、examples/*.model.json → model-manifest.schema.json
- examples/*.bundle.json → bundle.schema.json(.rspkg の目次、D-244)。子の id は root.id + '.' で始まり、root の kind が sns でなければ children は空
- schema/slots.json → slot-registry.schema.json
- 突き合わせ: マニフェストの slots は登録簿にあること(planned は警告)、kind がそのスロットで許されること、
  必要権限を持つこと、登録簿の hostFns は host-fn.schema.json の $defs.fns と boundary の hostFnName にあること
- 着せ替え(kind = theme、docs/slots.md §2.12): theme.surfaces.*.layers[].asset と theme.icons の asset:<id> が
  assets[] に use = theme で載っていること(assets[].id の重複も見る)。v0.17 のグラデーションのレイヤーは参照先が無いので
  見ず、代わりに gradient.stops が colors と同じ数で昇順であることを見る
- ページ型(slot page、v0.18、docs/slots.md §2.13): page.mode = solo なのに shared.session / maxParticipants があれば警告
  (page が要ること、shared なら shared.session、astell は shared だけ、profile.host は profile と一緒(v0.21)、はスキーマが見る)
- i18n(v0.24、PROTOCOL.md §4.6): locales の actions / assets の id が manifest にあること、pageTitle は page があるときだけ、
  タグの重複(大文字小文字を畳んで)は誤り、defaultLocale と同じタグは警告。手で書いた正 / 誤の例(I18N_CASES)でこの検査そのものも確かめる
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


def theme_problems(m: dict) -> list[str]:
    """着せ替え(kind = theme)の整合。スキーマでは書けない「id の参照先がある」「stops が colors と同じ数で昇順」を見る
    (rasq check の pack::theme_problems と同じ規則)。"""
    out: list[str] = []
    ids: list[str] = [a["id"] for a in m.get("assets", [])]
    for dup in sorted({i for i in ids if ids.count(i) > 1}):
        out.append(f"asset id が重複しています: {dup}")
    theme_ids = {a["id"] for a in m.get("assets", []) if a.get("use") == "theme"}
    t = m.get("theme") or {}
    for name, surface in (t.get("surfaces") or {}).items():
        for i, layer in enumerate(surface.get("layers", [])):
            where = f"theme.surfaces.{name}.layers[{i}]"
            if "asset" in layer and layer["asset"] not in theme_ids:
                out.append(f"{where}.asset {layer['asset']!r} が assets(use = theme)にありません")
            g = layer.get("gradient")
            if g is not None and "stops" in g:
                if len(g["stops"]) != len(g["colors"]):
                    out.append(f"{where}.gradient.stops が {len(g['stops'])} 個、colors が {len(g['colors'])} 個(同じ数にする)")
                if any(a > b for a, b in zip(g["stops"], g["stops"][1:])):
                    out.append(f"{where}.gradient.stops が昇順ではありません: {g['stops']}")
    for icon, ref in (t.get("icons") or {}).items():
        if ref.startswith("asset:") and ref[len("asset:") :] not in theme_ids:
            out.append(f"theme.icons.{icon} の {ref!r} が assets(use = theme)にありません")
    return out


def locale_problems(m: dict) -> tuple[list[str], list[str]]:
    """i18n(v0.24): スキーマでは書けない照合(rasq check の schema::locale_problems と同じ規則)。(errors, warnings) を返す。"""
    errors: list[str] = []
    warnings: list[str] = []
    locales = m.get("locales") or {}
    action_ids = {a.get("id") for a in m.get("actions", [])}
    asset_ids = {a.get("id") for a in m.get("assets", [])}
    base = (m.get("defaultLocale") or "ja").lower()
    seen: dict[str, str] = {}
    for tag, text in locales.items():
        folded = tag.lower()
        if folded in seen:
            errors.append(f"locales: {seen[folded]!r} と {tag!r} は同じ言語です(大文字小文字は区別しない)")
        seen[folded] = tag
        if folded == base:
            warnings.append(f"locales.{tag} は defaultLocale({base})と同じ言語です(素の欄を書き換えるだけ)")
        if not isinstance(text, dict):
            continue
        for aid in text.get("actions", {}):
            if aid not in action_ids:
                errors.append(f"locales.{tag}.actions.{aid} が manifest.actions にありません")
        for aid in text.get("assets", {}):
            if aid not in asset_ids:
                errors.append(f"locales.{tag}.assets.{aid} が manifest.assets にありません")
        if "pageTitle" in text and "page" not in m:
            errors.append(f"locales.{tag}.pageTitle は page があるときだけ書けます")
    return errors, warnings


# i18n の検査そのものの例(v0.24)。(名前, マニフェスト, スキーマに通るか, 照合の誤りの数, 照合の警告の数)。
_I18N_BASE = {"schema": "rasq.plugin-manifest/1", "id": "sample.i18n", "name": "いいねボタン", "version": "0.1.0", "kind": "action", "description": "説明", "slots": ["post.action"], "actions": [{"id": "like", "label": "いいね", "tags": ["Reaction"]}]}
I18N_CASES = [
    ("素のまま(後方互換)", {}, True, 0, 0),
    ("名前・説明・ボタン", {"defaultLocale": "ja", "locales": {"en": {"name": "Like button", "description": "desc", "actions": {"like": {"label": "Like"}}}, "zh-Hant": {"name": "按讚"}}}, True, 0, 0),
    ("タグの形が違う", {"locales": {"en_US": {"name": "x"}}}, False, 0, 0),
    ("空の言語", {"locales": {"en": {}}}, False, 0, 0),
    ("知らない欄", {"locales": {"en": {"version": "1.0.0"}}}, False, 0, 0),
    ("名前が空", {"locales": {"en": {"name": ""}}}, False, 0, 0),
    ("defaultLocale の形", {"defaultLocale": "日本語"}, False, 0, 0),
    ("無いボタン", {"locales": {"en": {"actions": {"nope": {"label": "x"}}}}}, True, 1, 0),
    ("無い画像", {"locales": {"en": {"assets": {"nope": {"label": "x"}}}}}, True, 1, 0),
    ("page が無いのに pageTitle", {"locales": {"en": {"pageTitle": "x"}}}, True, 1, 0),
    ("同じ言語が 2 つ", {"locales": {"en": {"name": "a"}, "EN": {"name": "b"}}}, True, 1, 0),
    ("基準と同じ言語", {"locales": {"ja": {"name": "別名"}}}, True, 0, 1),
]


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
            if "slot" in a and a["slot"] not in m.get("slots", []):
                rep.error(f"{label}: action {a['id']} の slot {a['slot']} が manifest.slots にありません")
            if "output" in a and a.get("slot") != "composer.media":
                rep.error(f"{label}: action {a['id']} の output は slot = composer.media の宣言にだけ書けます")
            if "accepts" in a and a.get("slot") != "composer.media":
                rep.error(f"{label}: action {a['id']} の accepts は slot = composer.media の宣言にだけ書けます")
        if m["kind"] == "theme":
            for problem in theme_problems(m):
                rep.error(f"{label}: {problem}")
        if "composer.media" in m.get("slots", []) and not any(a.get("slot") == "composer.media" for a in m.get("actions", [])):
            rep.warn(f"{label}: 加工画面の下段バーのボタン宣言(actions[].slot = composer.media)が無い(ホストが既定の見た目で、画像にだけ出す)")
        pg = m.get("page") or {}
        if pg.get("mode") == "solo" and "shared.session" in m.get("permissions", []):
            rep.warn(f"{label}: page.mode = solo なのに permissions に shared.session がある(solo はセッションに入らないので過剰宣言)")
        if pg.get("mode") == "solo" and "maxParticipants" in pg:
            rep.warn(f"{label}: page.maxParticipants は shared のときだけ意味がある")
        errs, warns = locale_problems(m)
        for e in errs:
            rep.error(f"{label}: {e}")
        for w in warns:
            rep.warn(f"{label}: {w}")
        if "viewer.overlay" in m.get("slots", []) and not any(a.get("slot") == "viewer.overlay" for a in m.get("actions", [])):
            rep.warn(f"{label}: 全画面アクションバーのボタン宣言(actions[].slot = viewer.overlay)が無い(ホストが既定の見た目で出す)")

    # 3b. i18n(v0.24)の検査そのもの: 正 / 誤の例が期待どおりに通る / 落ちること
    manifest_validator = Draft202012Validator(load(os.path.join(SCHEMA_DIR, "manifest.schema.json")), registry=reg)
    for name, extra, schema_ok, n_errors, n_warnings in I18N_CASES:
        m = {**_I18N_BASE, **extra}
        rep.checked += 1
        ok = not list(manifest_validator.iter_errors(m))
        if ok != schema_ok:
            rep.error(f"i18n の例「{name}」: スキーマに{'通るはず' if schema_ok else '通らないはず'}が{'通りません' if schema_ok else '通りました'}")
        if ok:
            errs, warns = locale_problems(m)
            if (len(errs), len(warns)) != (n_errors, n_warnings):
                rep.error(f"i18n の例「{name}」: 照合の誤り / 警告が {len(errs)} / {len(warns)}(期待 {n_errors} / {n_warnings}): {errs + warns}")

    # 4. ほかの例
    for pattern, schema_name in (("*.rrl.json", "rrl-node.schema.json"), ("*.rsl.json", "rsl-span.schema.json"), ("*.model.json", "model-manifest.schema.json")):
        for path in sorted(glob.glob(os.path.join(EXAMPLES, pattern))):
            doc = load(path)
            # snapshot メッセージごと置いてある例(post-tile.rrl.json)は境界スキーマで見る(root が RRL ノード)。
            name = "boundary.schema.json" if isinstance(doc, dict) and doc.get("schema") == "rasq.boundary/1" else schema_name
            validate(rep, reg, name, doc, os.path.relpath(path, ROOT))

    # 4b. 配布物の目次(.rspkg の bundle.json、docs/cli.md §6)
    for path in sorted(glob.glob(os.path.join(EXAMPLES, "*.bundle.json"))):
        label = os.path.relpath(path, ROOT)
        b = load(path)
        if not validate(rep, reg, "bundle.schema.json", b, label):
            continue
        root = b["root"]
        for c in b["children"]:
            if not c["id"].startswith(root["id"] + "."):
                rep.error(f"{label}: child {c['id']} は root.id + '.' で始まっていません")
            if c["kind"] == "sns":
                rep.error(f"{label}: child {c['id']} は kind=sns にできません(SNS プラグインは root だけ)")
        if root["kind"] != "sns" and b["children"]:
            rep.error(f"{label}: root.kind が sns でないのに children があります(配下にバンドルできるのは SNS プラグインだけ)")
        if root["kind"] != "sns" and not root.get("requires"):
            rep.warn(f"{label}: スロットプラグイン単体の配布物なのに requires(土台の SNS プラグイン)が無い")
        paths = [root["manifest"], root["wasm"]["path"]] + [x for c in b["children"] for x in (c["manifest"], c["wasm"]["path"])] + [l["wasm"]["path"] for l in b["libs"]] + [a["path"] for a in b["assets"]] + [m["manifest"] for m in b["models"]]
        dup = sorted({x for x in paths if paths.count(x) > 1})
        if dup:
            rep.error(f"{label}: 配布物内のパスが重複しています: {dup}")
        rep.checked += 1

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
