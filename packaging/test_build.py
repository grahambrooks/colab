"""Tests for packaging/build.py, from a fake release: `python3 -m unittest discover -s packaging`."""
import base64
import hashlib
import io
import json
import shutil
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import build  # noqa: E402

VERSION = "2026.1.2"


def archive(dist: Path, target: str, binary: bytes) -> Path:
    path = dist / f"colab-v{VERSION}-{target}.tar.gz"
    with tarfile.open(path, "w:gz") as tar:
        info = tarfile.TarInfo("colab")
        info.size = len(binary)
        info.mode = 0o755
        tar.addfile(info, io.BytesIO(binary))
    return path


class BuildTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.dist = self.tmp / "dist"
        self.dist.mkdir()
        self.out = self.tmp / "out"

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_build(self):
        self.assertEqual(build.main(["build.py", f"v{VERSION}", str(self.dist), str(self.out)]), 0)

    def test_every_target_becomes_an_npm_package_and_a_wheel(self):
        archive(self.dist, "aarch64-apple-darwin", b"mac binary")
        archive(self.dist, "x86_64-unknown-linux-gnu", b"needs GLIBC_2.17 and GLIBC_2.39\0")
        self.run_build()

        launcher = json.loads((self.out / "npm/colab-codemod/package.json").read_text())
        self.assertEqual(launcher["bin"], {"colab": "bin/colab.js", "colab-codemod": "bin/colab.js"})
        self.assertEqual(
            launcher["optionalDependencies"],
            {"colab-codemod-darwin-arm64": VERSION, "colab-codemod-linux-x64": VERSION},
        )
        script = (self.out / "npm/colab-codemod/bin/colab.js").read_text()
        self.assertIn('"linux-x64": "colab-codemod-linux-x64"', script)
        plat = json.loads((self.out / "npm/colab-codemod-linux-x64/package.json").read_text())
        self.assertEqual((plat["os"], plat["cpu"]), (["linux"], ["x64"]))
        exe = self.out / "npm/colab-codemod-linux-x64/bin/colab"
        self.assertTrue(exe.stat().st_mode & 0o111)

        wheels = sorted(p.name for p in (self.out / "pypi").iterdir())
        self.assertEqual(wheels, [
            f"colab_codemod-{VERSION}-py3-none-macosx_11_0_arm64.whl",
            # The newest glibc symbol version decides the manylinux tag.
            f"colab_codemod-{VERSION}-py3-none-manylinux_2_39_x86_64.whl",
        ])

    def test_a_static_binary_is_preferred_and_tagged_for_any_linux(self):
        archive(self.dist, "x86_64-unknown-linux-gnu", b"GLIBC_2.39")
        archive(self.dist, "x86_64-unknown-linux-musl", b"static")
        self.run_build()
        self.assertEqual(
            (self.out / "npm/colab-codemod-linux-x64/bin/colab").read_bytes(), b"static"
        )
        (wheel,) = (self.out / "pypi").iterdir()
        self.assertTrue(wheel.name.endswith("-py3-none-manylinux_2_17_x86_64.musllinux_1_1_x86_64.whl"))

    def test_the_wheel_is_installable(self):
        archive(self.dist, "aarch64-apple-darwin", b"binary")
        self.run_build()
        (wheel,) = (self.out / "pypi").iterdir()
        with zipfile.ZipFile(wheel) as zf:
            names = zf.namelist()
            info = f"colab_codemod-{VERSION}.dist-info"
            self.assertIn(f"colab_codemod-{VERSION}.data/scripts/colab", names)
            mode = zf.getinfo(f"colab_codemod-{VERSION}.data/scripts/colab").external_attr >> 16
            self.assertEqual(mode & 0o777, 0o755)
            self.assertIn("colab-codemod = colab_codemod:main", zf.read(f"{info}/entry_points.txt").decode())
            # RECORD lists every other file with its hash and size.
            for line in zf.read(f"{info}/RECORD").decode().splitlines():
                name, digest, size = line.split(",")
                if name.endswith("RECORD"):
                    continue
                data = zf.read(name)
                want = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
                self.assertEqual((digest, int(size)), (f"sha256={want}", len(data)))

    def test_a_checksum_mismatch_stops_the_build(self):
        path = archive(self.dist, "aarch64-apple-darwin", b"binary")
        (self.dist / "SHA256SUMS").write_text(f"{'0' * 64}  {path.name}\n")
        with self.assertRaises(SystemExit) as caught:
            self.run_build()
        self.assertIn("does not match", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
