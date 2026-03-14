#!/usr/bin/env python3

from __future__ import annotations

import argparse
import gzip
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
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


def bazel_cache_root() -> Path:
    return bazel_execroot().parents[1]


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


def first_existing_path(paths: list[Path], description: str) -> Path:
    for path in paths:
        if path.exists():
            return path
    raise SystemExit(f"could not find {description}")


def find_single_path(pattern: str, description: str) -> Path:
    matches = sorted(bazel_cache_root().glob(pattern))
    if not matches:
        raise SystemExit(f"could not find {description}")
    return matches[-1]


def platform_bin_dir(platform: str) -> Path:
    return bazel_execroot() / "bazel-out" / f"{platform}-fastbuild" / "bin"


def platform_st_bin_dir(platform: str) -> Path:
    matches = sorted((bazel_execroot() / "bazel-out").glob(f"{platform}-fastbuild-ST-*/bin"))
    if not matches:
        raise SystemExit(f"could not find ST bin dir for {platform}")
    return matches[-1]


def toolchain_dir() -> Path:
    return find_single_path(
        "external/llvm++http_archive+llvm-toolchain-minimal-*",
        "llvm toolchain dir",
    )


def kernel_headers_dir(target: str) -> Path:
    arch = target.split("-", 1)[0]
    arch_patterns = {
        "x86_64": ["*linux_kernel_headers_x86.*"],
        "aarch64": ["*linux_kernel_headers_arm64.*", "*linux_kernel_headers_aarch64.*"],
    }.get(arch, [f"*linux_kernel_headers_{arch}.*"])
    for pattern in arch_patterns:
        matches = sorted((bazel_cache_root() / "external").glob(pattern))
        if matches:
            return matches[-1] / "include"
    raise SystemExit(f"could not find kernel headers dir for {target}")


def musl_generated_include_dir(platform: str, target: str) -> Path:
    arch = target.split("-", 1)[0]
    return platform_bin_dir(platform) / "external/llvm++musl+musl_libc/generated" / arch / "includes"


def static_runtime_libs(platform: str) -> tuple[Path, Path, Path]:
    st_bin = platform_st_bin_dir(platform)
    libcxx = st_bin / "external/llvm++llvm_source+libcxx/libcxx.static_/liblibcxx.static.a"
    libcxxabi = st_bin / "external/llvm++llvm_source+libcxxabi/libcxxabi.static_/liblibcxxabi.static.a"
    libunwind = st_bin / "external/llvm++llvm_source+libunwind/libunwind.static_/liblibunwind.static.a"
    return (
        first_existing_path([libcxx], "libcxx static archive"),
        first_existing_path([libcxxabi], "libcxxabi static archive"),
        first_existing_path([libunwind], "libunwind static archive"),
    )


def v8_crate_dir() -> Path:
    version_suffix = parse_v8_crate_version().replace(".", "_")
    path = bazel_cache_root() / "external" / f"+http_archive+v8_crate_{version_suffix}"
    return first_existing_path([path], "v8 crate dir")


def cargo_runtime_rustflags(platform: str) -> str:
    libcxx_static, libcxxabi_static, libunwind_static = static_runtime_libs(platform)
    return " ".join(
        [
            f"-C link-arg={libcxx_static}",
            f"-C link-arg={libcxxabi_static}",
            f"-C link-arg={libunwind_static}",
            "-C link-arg=-lc",
            "-C link-arg=-lpthread",
            "-C link-arg=-ldl",
        ]
    )


def write_cargo_smoke_project(project_dir: Path, crate_dir: Path) -> None:
    project_dir.mkdir(parents=True, exist_ok=True)
    (project_dir / "src").mkdir(exist_ok=True)
    (project_dir / "Cargo.toml").write_text(
        f"""[package]
name = "cargo_v8_smoke"
version = "0.1.0"
edition = "2021"

[dependencies]
v8 = {{ path = "{crate_dir.as_posix()}" }}
"""
    )
    (project_dir / "src/main.rs").write_text(
        """fn main() {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    {
        let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
        v8::scope!(let handle_scope, isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        let code = v8::String::new(scope, "1 + 2").unwrap();
        let script = v8::Script::compile(scope, code, None).unwrap();
        let result = script.run(scope).unwrap();
        let value = result.integer_value(scope).unwrap();
        assert_eq!(value, 3);
        println!("{value}");
    }

    unsafe {
        v8::V8::dispose();
    }
    v8::V8::dispose_platform();
}
"""
    )


def add_regular_file_to_tar(
    tar: tarfile.TarFile,
    *,
    source: Path,
    arcname: str,
) -> None:
    info = tar.gettarinfo(str(source), arcname=arcname)
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    with source.open("rb") as src:
        tar.addfile(info, src)


def write_bundle_archive(
    *,
    output_path: Path,
    target: str,
    lib_path: Path,
    binding_path: Path,
    include_root: Path,
) -> None:
    bundle_root = f"rusty_v8_{target}"

    with output_path.open("wb") as dst:
        with gzip.GzipFile(
            filename="",
            mode="wb",
            fileobj=dst,
            compresslevel=9,
            mtime=0,
        ) as gz:
            with tarfile.open(
                fileobj=gz,
                mode="w",
                format=tarfile.PAX_FORMAT,
            ) as tar:
                add_regular_file_to_tar(
                    tar,
                    source=lib_path,
                    arcname=f"{bundle_root}/lib/librusty_v8.a",
                )
                add_regular_file_to_tar(
                    tar,
                    source=binding_path,
                    arcname=f"{bundle_root}/src_binding_release.rs",
                )
                for header_path in sorted(include_root.rglob("*")):
                    if not header_path.is_file():
                        continue
                    relpath = header_path.relative_to(include_root.parent)
                    add_regular_file_to_tar(
                        tar,
                        source=header_path,
                        arcname=f"{bundle_root}/{relpath.as_posix()}",
                    )


def validate_bundle(platform: str, target: str, bundle_path: Path) -> None:
    bin_dir = platform_bin_dir(platform)
    toolchain = toolchain_dir()
    kernel_headers = kernel_headers_dir(target)
    musl_includes = musl_generated_include_dir(platform, target)
    compiler_rt = bazel_cache_root() / "external/llvm++llvm_source+compiler-rt/include"
    libcxx_static, libcxxabi_static, libunwind_static = static_runtime_libs(platform)

    with tempfile.TemporaryDirectory(prefix="rusty-v8-bundle-") as tmpdir:
        tmpdir_path = Path(tmpdir)
        with tarfile.open(bundle_path, "r:gz") as tar:
            tar.extractall(tmpdir_path, filter="data")

        bundle_root = tmpdir_path / f"rusty_v8_{target}"
        output_binary = tmpdir_path / "non_bazel_v8_smoke_test"

        cmd = [
            str(toolchain / "bin/clang++"),
            "-target",
            target,
            "--sysroot=/dev/null",
            "-static-pie",
            "-fuse-ld=lld",
            "-rtlib=compiler-rt",
            "-nostdlib++",
            "--unwindlib=none",
            "-resource-dir",
            str(bin_dir / "external/llvm+/runtimes/resource_directory"),
            "-B" + str(bin_dir / "external/llvm+/runtimes/crt_objects_directory_linux"),
            "-L" + str(bin_dir / "external/llvm+/runtimes/libcxx/libcxx_library_search_directory"),
            "-L" + str(bin_dir / "external/llvm+/runtimes/libunwind/libunwind_library_search_directory"),
            "-L" + str(bin_dir / "external/llvm+/runtimes/musl/musl_library_search_directory"),
            "-nostdlibinc",
            "-isystem",
            str(bin_dir / "external/llvm++llvm_source+libcxx/libcxx_headers_include_search_directory"),
            "-isystem",
            str(bin_dir / "external/llvm++llvm_source+libcxxabi/libcxxabi_headers_include_search_directory"),
            "-isystem",
            str(kernel_headers),
            "-isystem",
            str(musl_includes),
            "-isystem",
            str(compiler_rt),
            "-Xclang",
            "-internal-isystem",
            "-Xclang",
            str(toolchain / "lib/clang/22/include"),
            "-std=c++20",
            "-I" + str(bundle_root / "include"),
            "-Wl,-no-as-needed",
            "-Wl,-z,relro,-z,now",
            "-Wl,--push-state",
            "-Wl,--as-needed",
            "-lpthread",
            "-ldl",
            "-Wl,--pop-state",
            str(ROOT / "third_party/v8/smoke_test.cc"),
            str(bundle_root / "lib/librusty_v8.a"),
            str(libcxx_static),
            str(libcxxabi_static),
            str(libunwind_static),
            "-Wl,-S",
            "-Wl,-soname,non_bazel_v8_smoke_test",
            "-Wl,--no-as-needed",
            "-ldl",
            "-pthread",
            "-o",
            str(output_binary),
        ]
        subprocess.run(cmd, cwd=ROOT, check=True)
        subprocess.run([str(output_binary)], cwd=ROOT, check=True)
        print(output_binary)


def validate_cargo_bundle(platform: str, target: str, bundle_path: Path) -> None:
    crate_dir = v8_crate_dir()
    rustflags_parts = [cargo_runtime_rustflags(platform)]
    if os.environ.get("RUSTFLAGS"):
        rustflags_parts.insert(0, os.environ["RUSTFLAGS"])

    with tempfile.TemporaryDirectory(prefix="rusty-v8-cargo-") as tmpdir:
        tmpdir_path = Path(tmpdir)
        with tarfile.open(bundle_path, "r:gz") as tar:
            tar.extractall(tmpdir_path, filter="data")

        bundle_root = tmpdir_path / f"rusty_v8_{target}"
        project_dir = tmpdir_path / "cargo_smoke"
        write_cargo_smoke_project(project_dir, crate_dir)

        env = os.environ.copy()
        env["RUSTY_V8_ARCHIVE"] = str(bundle_root / "lib/librusty_v8.a")
        env["RUSTY_V8_SRC_BINDING_PATH"] = str(bundle_root / "src_binding_release.rs")
        env["RUSTFLAGS"] = " ".join(rustflags_parts)
        subprocess.run(
            ["cargo", "run", "--target", target],
            cwd=project_dir,
            env=env,
            check=True,
        )
        print(project_dir / "target" / target / "debug" / "cargo_v8_smoke")


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
    bundle_name = f"rusty_v8_bundle_{target}.tar.gz"

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
    write_bundle_archive(
        output_path=output_dir / bundle_name,
        target=target,
        lib_path=lib_path,
        binding_path=binding_path,
        include_root=bazel_execroot() / "external" / "v8+" / "include",
    )

    print(output_dir / archive_name)
    print(output_dir / binding_name)
    print(output_dir / bundle_name)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("print-version")

    stage_parser = subparsers.add_parser("stage")
    stage_parser.add_argument("--platform", required=True)
    stage_parser.add_argument("--target", required=True)
    stage_parser.add_argument("--output-dir", required=True)

    validate_parser = subparsers.add_parser("validate-bundle")
    validate_parser.add_argument("--platform", required=True)
    validate_parser.add_argument("--target", required=True)
    validate_parser.add_argument("--bundle-path", required=True)

    cargo_validate_parser = subparsers.add_parser("validate-cargo")
    cargo_validate_parser.add_argument("--platform", required=True)
    cargo_validate_parser.add_argument("--target", required=True)
    cargo_validate_parser.add_argument("--bundle-path", required=True)

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
    if args.command == "validate-bundle":
        validate_bundle(
            platform=args.platform,
            target=args.target,
            bundle_path=Path(args.bundle_path),
        )
        return 0
    if args.command == "validate-cargo":
        validate_cargo_bundle(
            platform=args.platform,
            target=args.target,
            bundle_path=Path(args.bundle_path),
        )
        return 0
    raise SystemExit(f"unsupported command: {args.command}")


if __name__ == "__main__":
    sys.exit(main())
