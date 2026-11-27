"""Collect locked dependency notices and bind SPDX 2.3 to staged release assets."""

import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tomllib
from urllib.request import urlopen
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo


ROOT = Path(__file__).resolve().parents[1]
REPO = "https://github.com/lukewilliamboswell/roc-native-windows"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
NOTICE = re.compile(r"^(?:licen[cs]e|copying|copyright|notice|authors)(?:$|[._-])", re.I)
TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "macos-x64": "x86_64-apple-darwin",
    "windows-arm64": "aarch64-pc-windows-gnullvm",
    "windows-x64": "x86_64-pc-windows-gnullvm",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "linux-x64": "x86_64-unknown-linux-musl",
}
# Source revisions behind the content-addressed Roc dependency URLs. The
# notices come from the original revisions, independently of release tarballs.
ROC_DEPENDENCIES = {
    "basic-cli": ("roc-lang", "0.28.1", "4bc39663b90e5117e3fd5e4fbb740e7138df5965", "UPL-1.0"),
    "http": ("roc-lang", "1.0.0", "74f70267e74979be40f8396fcb54eaa1a7a77459", "UPL-1.0"),
    "weaver": ("lukewilliamboswell", "0.10.0", "23a61441a1afd9f913bbc43b55bfc66041d8d522", "UPL-1.0"),
    "roc-zip": ("lukewilliamboswell", "0.3.0-rc4", "302ff68787c69a0b5b2913436687fc77686e434c", "Apache-2.0"),
    "roc-crc32": ("niclas-ahden", "2.0.1", "4c358f787ad3ba5fdb85f312a77f12ed26ed7076", "Apache-2.0"),
    "roc-deflate": ("lukewilliamboswell", "0.6.0-rc1", "8a2d340925b81844e446088b4be83a75a0b7d554", "Apache-2.0"),
    "roc-signals": ("lukewilliamboswell", "0.8.0-rc2", "482e4c994301a05a10b37a9c8712df921b48c817", "UPL-1.0"),
    "roc-ansi": ("lukewilliamboswell", "0.13.3", "31a6c97b1647c7785eb71d1f2bd6e6096c7b714e", "UPL-1.0"),
    "path": ("roc-lang", "4.0.0", "87577de0197ccb1bcf52df79723010fa53021382", "UPL-1.0"),
    "roc-fuzz": ("lukewilliamboswell", "0.7.0", "df47430d5bb06f6155019569d251fcea59d625d7", "UPL-1.0"),
}


def run(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def digest(path):
    with path.open("rb") as src:
        return hashlib.file_digest(src, "sha256").hexdigest()


def download(url):
    with urlopen(url, timeout=60) as response:
        data = response.read(64 * 1024 * 1024 + 1)
    if not data or len(data) > 64 * 1024 * 1024:
        raise ValueError(f"unexpected download size from {url}")
    return data


def package(name, version, url="NOASSERTION", license_="NOASSERTION", *, sha="", purl="", comment=""):
    key = "-".join((name, str(version), purl, comment)).encode()
    result = {
        "name": name,
        "SPDXID": "SPDXRef-" + hashlib.sha256(key).hexdigest()[:24],
        "versionInfo": version,
        "supplier": "NOASSERTION",
        "downloadLocation": url,
        "filesAnalyzed": False,
        "licenseDeclared": license_ or "NOASSERTION",
        "licenseConcluded": "NOASSERTION",
        "copyrightText": "NOASSERTION",
        "comment": comment,
    }
    if sha:
        result["checksums"] = [{"algorithm": "SHA256", "checksumValue": sha}]
    if purl:
        result["externalRefs"] = [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": purl}]
    return result


def relation(source, kind, dest, comment=""):
    return {"spdxElementId": source["SPDXID"], "relationshipType": kind, "relatedSpdxElement": dest["SPDXID"], "comment": comment}


def archive_notices(pkg, checksum, notices):
    name, version = pkg["name"], pkg["version"]
    stem = f"{name}-{version}"
    url = f"https://static.crates.io/crates/{name}/{stem}.crate"
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    archives = sorted((cargo_home / "registry/cache").glob(f"*/{stem}.crate"))
    data = archives[0].read_bytes() if archives else download(url)
    if hashlib.sha256(data).hexdigest() != checksum:
        raise ValueError(f"{stem} does not match Cargo.lock")
    license_file = pkg.get("license_file")
    if license_file:
        license_file = Path(license_file).resolve().relative_to(Path(pkg["manifest_path"]).resolve().parent).as_posix()
    prefix = f"crates/{stem}/"
    found = False
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as packed:
        for entry in packed:
            path = PurePosixPath(entry.name)
            if not entry.isfile() or len(path.parts) < 2 or path.parts[0] != stem or ".." in path.parts or "\\" in entry.name:
                raise ValueError(f"unexpected member in {stem}: {entry.name}")
            relative = PurePosixPath(*path.parts[1:]).as_posix()
            if NOTICE.match(path.name) or relative == license_file or relative in {"Cargo.toml", "Cargo.toml.orig"}:
                notices[prefix + relative] = packed.extractfile(entry).read()
                found |= relative not in {"Cargo.toml", "Cargo.toml.orig"}
    if not found:
        # Preserve publisher sources for crates whose notices live in comments.
        notices[f"sources/{stem}.crate"] = data


def rust_graph(manifest, target, notices):
    manifest = ROOT / manifest
    cargo_lock = tomllib.loads(manifest.with_name("Cargo.lock").read_text(encoding="utf-8"))
    locked = {(p["name"], p["version"]): p["checksum"] for p in cargo_lock["package"] if p.get("source") == REGISTRY}
    metadata = json.loads(run("cargo", "metadata", "--locked", "--format-version=1", "--manifest-path", str(manifest), "--filter-platform", target))
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    records = {pkg["id"]: pkg for pkg in metadata["packages"]}
    root = metadata["resolve"]["root"]
    used = set()
    edges = []

    def visit(current):
        if current in used:
            return
        used.add(current)
        for dep in nodes[current]["deps"]:
            kinds = {kind["kind"] for kind in dep["dep_kinds"]} - {"dev"}
            if not kinds:
                continue
            ident = dep["pkg"]
            proc_macro = any("proc-macro" in target["kind"] for target in records[ident]["targets"])
            edge = "BUILD_DEPENDENCY_OF" if proc_macro or kinds == {"build"} else "DEPENDENCY_OF"
            edges.append((ident, edge, current))
            visit(ident)

    visit(root)
    components = {}
    for ident in sorted(used):
        pkg = records[ident]
        name, version = pkg["name"], pkg["version"]
        if ident == root:
            components[ident] = package(name, version, REPO, "UPL-1.0", comment=f"Workspace crate for {target}.")
            continue
        if pkg["source"] != REGISTRY:
            raise ValueError(f"unsupported Cargo source for {name}")
        checksum = locked[(name, version)]
        download_url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
        evidence_path = f"crates/{name}-{version}/Cargo.toml"
        if evidence_path not in notices:
            archive_notices(pkg, checksum, notices)
        components[ident] = package(
            name, version, download_url, pkg.get("license"), sha=checksum, purl=f"pkg:cargo/{name}@{version}",
            comment="SHA-256 identifies the published crate source archive, as selected by Cargo.lock.",
        )
    return components[root], list(components.values()), [(components[a], kind, components[b], "") for a, kind, b in edges]


def roc_components(notices):
    configured_urls = "\n".join((ROOT / path).read_text(encoding="utf-8") for path in (
        "Blueprint.roc", "platform/main.roc", "cli/windows.roc", "examples/starter/tests/fuzz/main.roc",
    ))
    components = {}
    for name, (owner, release, revision, license_) in ROC_DEPENDENCIES.items():
        pattern = rf'https://github\.com/{owner}/{name}/releases/download/{re.escape(release)}/[^"\s]+\.tar\.zst'
        match = re.search(pattern, configured_urls)
        if not match:
            raise ValueError(f"review the source revision and license for {owner}/{name} in ROC_DEPENDENCIES")
        url = match.group()
        notices[f"roc/{name}-{release}/LICENSE"] = download(f"https://raw.githubusercontent.com/{owner}/{name}/{revision}/LICENSE")
        components[name] = package(name, release, url, license_, comment=f"Reviewed source revision {revision}. The Roc URL embeds the archive content hash.")
    compiler = re.search(r'compiler: "(nightly-[^"]+)"', configured_urls)[1]
    revision = compiler.rsplit("-", 1)[1]
    notices[f"roc/{compiler}/LICENSE"] = download(f"https://raw.githubusercontent.com/roc-lang/roc/{revision}/LICENSE")
    return components, package("Roc", compiler, f"https://github.com/roc-lang/nightlies/releases/tag/{compiler}", "UPL-1.0")


def toolchain_notices(notices):
    rust_version = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]
    rust_docs = Path(run("rustc", "--print", "sysroot")) / "share/doc/rust"
    for file in [rust_docs / "COPYRIGHT-library.html", *sorted((rust_docs / "licenses").glob("*.txt"))]:
        notices[f"rust-{rust_version}/{file.relative_to(rust_docs).as_posix()}"] = file.read_bytes()
    zig_version = run("zig", "version")
    zig_lib = Path(json.loads(re.search(r'^\s*\.lib_dir = (".*"),$', run("zig", "env"), re.M)[1]))
    for path in ("../LICENSE", "libc/mingw/COPYING", "libunwind/LICENSE.TXT", "libc/musl/COPYRIGHT"):
        source = zig_lib / path
        if source.is_file():
            notices[f"zig-{zig_version}/{path.removeprefix('../')}"] = source.read_bytes()
        elif "musl" not in path:
            raise ValueError(f"Zig distribution is missing {path}")
    rust = package("Rust standard library", rust_version, f"https://github.com/rust-lang/rust/tree/{rust_version}/library", "MIT OR Apache-2.0")
    zig = package("Zig MinGW-w64 and compiler runtime inputs", zig_version, f"https://ziglang.org/download/{zig_version}/zig-{zig_version}.tar.xz",
                  comment="The Zig distribution supplies link inputs. See the dependency notice archive for Zig, MinGW-w64 and libunwind notices.")
    return rust, zig


def spdx(file, version, base, commit, created, components, edges, gap):
    sha256 = digest(file)
    subject = package(file.name, version, f"{base}/{file.name}", sha=sha256, comment=gap)
    links = [relation(subject if a is None else a, kind, subject if b is None else b, note) for a, kind, b, note in edges]
    doc = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": file.name,
        "documentNamespace": f"{REPO}/sbom/{commit}/{file.name}/{sha256}",
        "creationInfo": {
            "created": created,
            "creators": ["Organization: roc-native-windows contributors", "Tool: roc-native-windows-release-evidence"],
        },
        "documentDescribes": [subject["SPDXID"]],
        "packages": [subject, *sorted({item["SPDXID"]: item for item in components}.values(), key=lambda item: item["SPDXID"])],
        "relationships": list({tuple(link.values()): link for link in links}.values()),
    }
    ids = {item["SPDXID"] for item in doc["packages"]}
    for edge in doc["relationships"]:
        if edge["spdxElementId"] not in ids or edge["relatedSpdxElement"] not in ids:
            raise ValueError(f"{file.name}: unresolved SBOM relationship")
    file.with_name(file.name + ".spdx.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def collect(directory, version, base):
    assets = {file.name: file for file in directory.iterdir()}
    packagers = {f"roc-windows-msi-{version}-{name}" + (".exe" if name.startswith("windows-") else ""): target for name, target in TARGETS.items()}
    platform, = directory.glob(f"roc-native-windows-{version.replace('.', '_')}-*.tar.zst")
    tool, = directory.glob(f"windows-{version.replace('.', '_')}-*.tar.zst")
    launcher = f"windows-{version}.roc"
    notice_names = {f"LICENSE-{version}.txt", f"THIRD_PARTY_LICENSES-{version}.md"}
    expected = {*packagers, platform.name, tool.name, launcher, *notice_names}
    if assets.keys() != expected or any(not file.is_file() or file.is_symlink() for file in assets.values()):
        raise ValueError("release directory must contain exactly the original staged assets")
    commit, created = run("git", "show", "-s", "--format=%H%n%cI", "HEAD").splitlines()
    created = datetime.fromisoformat(created).astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    notices = {"roc/roc-glue/LICENSE": (ROOT / "host/glue/LICENSE").read_bytes()}
    roc, compiler = roc_components(notices)
    rust, zig = toolchain_notices(notices)
    hosted_windows = package("Windows system DLLs", "Windows 10 and later", "https://learn.microsoft.com/windows/win32/", comment="Provided by the operating system.")

    for filename, target in packagers.items():
        root, components, edges = rust_graph("packaging/Cargo.toml", target, notices)
        components.append(rust)
        links = [(None, "CONTAINS", root, target), (root, "STATIC_LINK", rust, "Rust standard library for " + target)]
        if "windows" in target:
            components += [zig, hosted_windows]
            links += [(root, "STATIC_LINK", zig, "Windows GNU runtime inputs"), (root, "DEPENDS_ON", hosted_windows, "Windows runtime dependencies")]
        gap = f"Native MSI compiler for {target}. Cargo's resolved graph identifies component inputs. Native system libraries and post-optimization symbol coverage are incomplete. Contact {REPO}/issues."
        spdx(assets[filename], version, base, commit, created, components, links + edges, gap)

    components = [roc["roc-signals"], rust, zig, compiler, hosted_windows]
    links = [(None, "DEPENDS_ON", roc["roc-signals"], "Resolved when compiling an app"), (compiler, "BUILD_DEPENDENCY_OF", None, "Bundles the platform")]
    for target in (TARGETS["windows-x64"], TARGETS["windows-arm64"]):
        root, crates, edges = rust_graph("host/Cargo.toml", target, notices)
        components += crates
        links += [(None, "CONTAINS", root, f"Win32 host for {target}"), (root, "STATIC_LINK", rust, target), (root, "DEPENDS_ON", hosted_windows, target)]
        links += edges
    for member in run("tar", "-tf", str(platform)).splitlines():
        data = subprocess.check_output(["tar", "-xOf", str(platform), member], cwd=ROOT)
        artifact = package(member, version, sha=hashlib.sha256(data).hexdigest(), comment="Exact member of the shipped platform bundle.")
        components.append(artifact)
        links.append((None, "CONTAINS", artifact, "Platform bundle member"))
        if member.endswith(("/crt2.obj", "/runtime.lib")):
            links.append((artifact, "GENERATED_FROM", zig, "Zig distribution"))
    spdx(platform, version, base, commit, created, components, links,
         f"The Windows x64 and ARM64 host archive inventories use the resolved Cargo graph. Toolchain runtime symbol composition and roc-signals transitive coverage are incomplete. Contact {REPO}/issues.")

    tool_deps = [roc[name] for name in ("basic-cli", "http", "weaver", "roc-zip", "roc-crc32", "roc-deflate", "roc-ansi", "path", "roc-fuzz")]
    wrapper = package("Reading List scaffold", version, REPO, "UPL-1.0", comment="Embedded by scripts/release.roc from examples/starter/scaffold.txt.")
    platform_dep = package(platform.name, version, f"{base}/{platform.name}", sha=digest(platform))
    for cli in (tool, assets[launcher]):
        links = [(None, "CONTAINS", wrapper, "Embedded starter"), (None, "DEPENDS_ON", platform_dep, "Downloaded when building apps")]
        links += [(None, "DEPENDS_ON", dep, "Roc package dependency or dependency of the embedded scaffold") for dep in tool_deps]
        links += [(compiler, "BUILD_DEPENDENCY_OF", None, "Validates the source and bundles the release tool")]
        spdx(cli, version, base, commit, created, [wrapper, platform_dep, compiler, *tool_deps], links,
             f"Roc source and scaffold dependency inventory. Supplier native runtime and further transitive Roc package coverage are incomplete. Contact {REPO}/issues.")

    archive = directory / f"DEPENDENCY-LICENSES-{version}.zip"
    with ZipFile(archive, "x") as out:
        for name, data in sorted(notices.items()):
            out.writestr(ZipInfo(name), data, compress_type=ZIP_DEFLATED)
    for file in [archive, *(assets[name] for name in sorted(notice_names))]:
        spdx(file, version, base, commit, created, [], [], "License and attribution material distributed alongside the software release.")
    files = sorted(directory.iterdir())
    (directory / "SHA256SUMS").write_text("".join(f"{digest(file)}  {file.name}\n" for file in files), encoding="utf-8", newline="\n")
    print(f"Bound {len(expected) + 1} SPDX documents and recorded SHA-256 for {len(files)} release files.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("version")
    parser.add_argument("base", help="Public HTTPS location of the release assets")
    args = parser.parse_args()
    collect(args.directory.resolve(strict=True), args.version, args.base.rstrip("/"))
