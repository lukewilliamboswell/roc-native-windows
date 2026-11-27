"""Render reviewed Windows release notes against the staged public assets."""

import argparse
from pathlib import Path
import re
import subprocess
from tempfile import TemporaryDirectory


VERSION = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)
MARKER = re.compile(r"\{[a-z][a-z0-9-]*-(?:url|version)\}")
REQUIRED = {"{platform-url}", "{cli-url}", "{roc-version}"}
OPTIONAL = {"{launcher-url}", "{checksums-url}"}


def version_key(version):
    match = VERSION.fullmatch(version)
    if not match:
        raise ValueError(f"Invalid release version: {version}")
    major, minor, patch, suffix = match.groups()
    prerelease = []
    for part in suffix.split(".") if suffix else ():
        if part.isdigit():
            if len(part) > 1 and part.startswith("0"):
                raise ValueError(f"Invalid numeric prerelease identifier: {part}")
            prerelease.append((0, int(part)))
        else:
            prerelease.append((1, part))
    return int(major), int(minor), int(patch), suffix is None, prerelease


def select_version(directory, requested):
    if requested:
        version_key(requested)
        return requested
    versions = [
        path.stem
        for path in directory.glob("*.adoc")
        if VERSION.fullmatch(path.stem)
    ]
    if not versions:
        raise ValueError(f"No versioned notes found in {directory}")
    return max(versions, key=version_key)


def read_notes(directory, version):
    path = directory / f"{version}.adoc"
    if not path.is_file():
        raise ValueError(f"Missing release notes: {path}")
    source = path.read_text(encoding="utf-8")
    title, _, _ = source.partition("\n")
    expected = f"= roc-native-windows {version}"
    if title != expected:
        raise ValueError(f"{path} must open with {expected!r}")
    fenced = False
    for line in source.splitlines():
        if line == "----":
            fenced = not fenced
        elif not fenced and (
            line.lstrip().startswith(("//", ":", "ifdef::", "ifndef::", "endif::", "ifeval::"))
            or re.search(r"\b(?:include::|xref:)", line)
        ):
            raise ValueError(f"{path} contains an author prompt or a non-standalone directive")
    if fenced:
        raise ValueError(f"{path} has an unclosed source block")
    markers = set(MARKER.findall(source))
    unknown = markers - REQUIRED - OPTIONAL
    missing = REQUIRED - markers
    if unknown or missing:
        raise ValueError(f"{path} has unknown bindings {sorted(unknown)} or missing bindings {sorted(missing)}")
    return source


def unique_bundle(assets, prefix):
    pattern = re.compile(re.escape(prefix) + r"[A-Za-z0-9]{43,44}\.tar\.zst")
    matches = [path.name for path in assets.iterdir() if path.is_file() and pattern.fullmatch(path.name)]
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one staged bundle with prefix {prefix!r}")
    return matches[0]


def resolve(source, version, assets, base_url, compiler):
    if not re.fullmatch(r"Roc compiler version ([0-9A-Za-z._-]+)", compiler):
        raise ValueError(f"Unexpected roc version output: {compiler!r}")
    bundle_version = version.replace(".", "_")
    names = {
        "{platform-url}": unique_bundle(assets, f"roc-native-windows-{bundle_version}-"),
        "{cli-url}": unique_bundle(assets, f"windows-{bundle_version}-"),
        "{launcher-url}": f"windows-{version}.roc",
        "{checksums-url}": "SHA256SUMS",
    }
    for marker in set(MARKER.findall(source)) - {"{roc-version}"}:
        name = names[marker]
        if not (assets / name).is_file():
            raise ValueError(f"Missing staged release asset: {name}")
        source = source.replace(marker, f"{base_url}/{name}")
    return source.replace("{roc-version}", compiler.removeprefix("Roc compiler version "))


def github_body(markdown, version):
    title, _, body = markdown.partition("\n")
    heading_id = re.compile(r" \{#[^{}\s]+\}$")
    if heading_id.sub("", title) != f"# roc-native-windows {version}":
        raise ValueError("Prepared release notes have an unexpected title")
    lines = []
    fence = ""
    for line in body.splitlines():
        block = re.match(r"^(`{3,})(.*)$", line)
        if block and not fence:
            fence = block[1]
        elif block and len(block[1]) >= len(fence) and not block[2].strip():
            fence = ""
        elif not fence and re.match(r"^#{1,6} ", line):
            line = heading_id.sub("", line)
        lines.append(line)
    result = "\n".join(lines).strip()
    if fence or not result or MARKER.search(result):
        raise ValueError("Prepared release notes are empty, have an unclosed fence, or retain a release placeholder")
    return result + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path("docs/releases"))
    parser.add_argument("--version")
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--base-url")
    parser.add_argument("--renderer")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    render_args = (args.assets, args.base_url, args.renderer, args.output)
    if any(render_args) and not all(render_args):
        parser.error("rendering requires --assets, --base-url, --renderer and --output")
    try:
        version = select_version(args.directory, args.version)
        source = read_notes(args.directory, version)
        if not args.output:
            print(version)
            return
        if not args.base_url.startswith("https://") or args.base_url.rsplit("/", 1)[-1] != version:
            raise ValueError("Release base URL must be HTTPS and end with the selected version")
        compiler = subprocess.check_output(["roc", "version"], text=True).strip()
        resolved = resolve(source, version, args.assets, args.base_url, compiler)
        with TemporaryDirectory(prefix="windows-release-notes-") as temp:
            workspace = Path(temp)
            docs = workspace / "docs"
            docs.mkdir()
            (docs / "notes.adoc").write_text(resolved, encoding="utf-8", newline="\n")
            subprocess.run(
                [args.renderer, "prepare", "--workspace", temp, "--entrypoint", "notes.adoc",
                 "--output-directory", "prepared", "--docs-version", version],
                check=True,
            )
            markdown = (workspace / "prepared/pages/notes.md").read_text(encoding="utf-8")
            args.output.write_text(github_body(markdown, version), encoding="utf-8", newline="\n")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release notes: {error}\n")


if __name__ == "__main__":
    main()
