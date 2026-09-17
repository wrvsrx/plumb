#!/usr/bin/env python3
"""Preview first, then explicitly apply a version-matched resource migration."""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

EXCLUDED = {".git", ".stversions", ".nsfw"}
REPO = Path(__file__).resolve().parent.parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(command, data=None):
    result = subprocess.run(command, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"{command[0]} failed: {result.stderr.decode(errors='replace')}")
    return result.stdout


def export(plumb, data):
    return json.loads(run([plumb, "export"], data))


def check_tool(plumb):
    probe = b'`->{probe.png `+{embed}}\n\n`->{probe.mp4 `+{embed}}\n'
    document = export(plumb, probe)
    blocks = document["blocks"]
    image, attachment = blocks[0]["c"][0], blocks[1]["c"][0]
    if image["t"] != "Image" or attachment["t"] != "Link" or ["data-plumb-facet", "embed"] not in attachment["c"][0][2]:
        raise RuntimeError("selected plumb does not support resource link facets; update it before migrating")
    invalid = export(plumb, b'{probe.png `+{embed}}\n')
    if '"t": "Image"' in json.dumps(invalid):
        raise RuntimeError("selected plumb still accepts anonymous resource owners; update it first")


def files(root):
    result = []
    for directory, dirs, names in os.walk(root, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d not in EXCLUDED and not (Path(directory) / d).is_symlink())
        result.extend(Path(directory) / name for name in sorted(names)
                      if name.endswith(".plumb") and not (Path(directory) / name).is_symlink())
    return sorted(result)


def inventory(root):
    return {str(path.relative_to(root)): digest(path.read_bytes()) for path in files(root)}


def helper_binary(explicit):
    if explicit:
        return str(Path(explicit).resolve(strict=True))
    output = run(["cargo", "build", "--locked", "--manifest-path", str(REPO / "Cargo.toml"),
                  "-p", "plumb", "--example", "migrate_resource_links", "--message-format=json"])
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == "migrate_resource_links":
            return item["executable"]
    raise RuntimeError("migration helper build produced no executable")


def preview(args):
    root = Path(args.root).resolve(strict=True)
    output = Path(args.output).resolve()
    if output == root or root in output.parents:
        raise RuntimeError("preview directory must be outside the notes root")
    if output.exists():
        raise RuntimeError("preview directory already exists; choose a new directory")
    helper = helper_binary(args.helper)
    initial = inventory(root)
    changes = []
    for relative in initial:
        before = (root / relative).read_bytes()
        try:
            result = json.loads(run([helper], before))
            after = result["source"].encode()
            if before != after:
                # Validate with the actual selected installed executable as well.
                export(args.plumb, after)
                changes.append({"path": relative, "before": before.decode(), "after": after.decode(),
                                "before_sha256": digest(before), "after_sha256": digest(after),
                                "references": result["count"]})
        except Exception as error:
            raise RuntimeError(f"{relative}: {error}") from error
    if inventory(root) != initial:
        raise RuntimeError("notes changed while generating preview; retry")
    output.mkdir(mode=0o700, parents=True)
    patch = []
    for item in changes:
        for folder, field in [("before", "before"), ("after", "after")]:
            path = output / folder / item["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(item[field].encode())
        patch.extend(difflib.unified_diff(item["before"].splitlines(True), item["after"].splitlines(True),
                                         fromfile="a/" + item["path"], tofile="b/" + item["path"]))
    (output / "changes.diff").write_text("".join(patch))
    plan = {"schema": 1, "root": str(root), "inventory": initial, "changes": changes}
    (output / "plan.json").write_text(json.dumps(plan, ensure_ascii=False, indent=2) + "\n")
    print(f"Scanned {len(initial)} documents; {len(changes)} files / {sum(c['references'] for c in changes)} references to migrate.")
    print(f"Excluded: {', '.join(sorted(EXCLUDED))}, symlinks. Preview: {output / 'changes.diff'}")


def apply(args):
    directory = Path(args.apply).resolve(strict=True)
    plan = json.loads((directory / "plan.json").read_text())
    if plan["schema"] != 1:
        raise RuntimeError("unsupported migration plan")
    root = Path(plan["root"]).resolve(strict=True)
    if inventory(root) != plan["inventory"]:
        raise RuntimeError("notes changed since preview; generate a fresh plan")
    prepared = []
    for item in plan["changes"]:
        relative = Path(item["path"])
        if relative.is_absolute() or ".." in relative.parts or EXCLUDED.intersection(relative.parts):
            raise RuntimeError("unsafe path in plan")
        target = root / relative
        if target.resolve() != target or not target.is_file():
            raise RuntimeError("symlink or missing target in plan")
        before, after = item["before"].encode(), item["after"].encode()
        if digest(before) != item["before_sha256"] or digest(after) != item["after_sha256"]:
            raise RuntimeError("plan checksum mismatch")
        if target.read_bytes() != before or (directory / "before" / relative).read_bytes() != before:
            raise RuntimeError("source or backup differs from preview")
        if (directory / "after" / relative).read_bytes() != after:
            raise RuntimeError("preview was modified; regenerate the plan")
        export(args.plumb, after)
        prepared.append((target, before, after))
    # All files and exports pass before the first write. Each replacement is atomic;
    # the before/ tree remains a backup if an I/O error interrupts the batch.
    for target, before, after in prepared:
        if target.read_bytes() != before:
            raise RuntimeError(f"concurrent edit: {target}; stopped, backups remain in {directory / 'before'}")
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".plumb-migrate-", delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(after)
                stream.flush()
                os.fsync(stream.fileno())
            shutil.copymode(target, temporary)
            os.replace(temporary, target)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
    print(f"Applied {len(prepared)} files. Backups: {directory / 'before'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", help="notes root (preview only)")
    parser.add_argument("--output", help="new preview/backup directory outside notes root")
    parser.add_argument("--apply", metavar="PREVIEW_DIR", help="apply exactly a previously reviewed plan")
    parser.add_argument("--plumb", default="plumb", help="updated installed plumb executable")
    parser.add_argument("--helper", help="prebuilt migration helper; otherwise build from this checkout")
    args = parser.parse_args()
    if args.apply and (args.root or args.output) or not args.apply and not (args.root and args.output):
        parser.error("use --root ROOT --output DIR, or --apply DIR")
    check_tool(args.plumb)
    apply(args) if args.apply else preview(args)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, IndexError) as error:
        raise SystemExit(str(error))
