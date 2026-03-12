#!/usr/bin/env python3

from __future__ import annotations

import argparse
import gzip
import re
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_BAZEL = ROOT / "MODULE.bazel"


def parse_v8_crate_version() -> str:
    text = MODULE_BAZEL.read_text()
    match = re.search(
        r"https://static\.crates\.io/crates/v8/v8-([0-9.]+)\.crate",
        text,
    )
    if match is None:
        raise SystemExit("could not determine v8 crate version from MODULE.bazel")
    return match.group(1)


def bazel_execroot() -> Path:
    result = subprocess.run(
        ["bazel", "info", "execution_root"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(result.stdout.strip())


def bazel_output_files(platform: str, labels: list[str]) -> list[Path]:
    expression = "set(" + " ".join(labels) + ")"
    result = subprocess.run(
        [
            "bazel",
            "cquery",
            f"--platforms=@llvm//platforms:{platform}",
            "--output=files",
            expression,
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    execroot = bazel_execroot()
    return [execroot / line.strip() for line in result.stdout.splitlines() if line.strip()]


def stage_release_assets(platform: str, target: str, output_dir: Path) -> None:
    target_suffix = target.replace("-", "_")
    version_suffix = parse_v8_crate_version().replace(".", "_")
    lib_label = f"//third_party/v8:v8_{version_suffix}_{target_suffix}"
    binding_label = f"//third_party/v8:src_binding_release_{target_suffix}"

    outputs = bazel_output_files(platform, [lib_label, binding_label])
    try:
        lib_path = next(path for path in outputs if path.suffix == ".a")
    except StopIteration as exc:
        raise SystemExit(f"missing static archive output for {target}") from exc
    try:
        binding_path = next(path for path in outputs if path.suffix == ".rs")
    except StopIteration as exc:
        raise SystemExit(f"missing binding output for {target}") from exc

    output_dir.mkdir(parents=True, exist_ok=True)
    archive_name = f"librusty_v8_release_{target}.a.gz"
    binding_name = f"src_binding_release_{target}.rs"

    with lib_path.open("rb") as src, (output_dir / archive_name).open("wb") as dst:
        with gzip.GzipFile(
            filename="",
            mode="wb",
            fileobj=dst,
            compresslevel=9,
            mtime=0,
        ) as gz:
            shutil.copyfileobj(src, gz)

    shutil.copyfile(binding_path, output_dir / binding_name)

    print(output_dir / archive_name)
    print(output_dir / binding_name)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("print-version")

    stage_parser = subparsers.add_parser("stage")
    stage_parser.add_argument("--platform", required=True)
    stage_parser.add_argument("--target", required=True)
    stage_parser.add_argument("--output-dir", required=True)

    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.command == "print-version":
        print(parse_v8_crate_version())
        return 0
    if args.command == "stage":
        stage_release_assets(
            platform=args.platform,
            target=args.target,
            output_dir=Path(args.output_dir),
        )
        return 0
    raise SystemExit(f"unsupported command: {args.command}")


if __name__ == "__main__":
    sys.exit(main())
