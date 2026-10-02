import os
import tempfile
import unittest
from pathlib import Path

import smartpack_ubuntu

WORKSPACE = Path(__file__).resolve().parents[1]


class LegacyExtractionSafetyTests(unittest.TestCase):
    def make_archive(self, root: Path) -> tuple[Path, Path]:
        source = root / "source"
        source.mkdir()
        (source / "payload.txt").write_bytes(b"safe extraction regression" * 32)
        archive = root / "fixture.spk"
        smartpack_ubuntu.pack(source, archive, "fast", 4096, None, None, True, 1)
        return source, archive

    def test_overwrite_replaces_final_symlink_without_following_it(self) -> None:
        with tempfile.TemporaryDirectory(prefix=".smartpack-test-", dir=WORKSPACE) as temp:
            root = Path(temp)
            _, archive = self.make_archive(root)
            outside = root / "outside.txt"
            outside.write_text("must remain unchanged", encoding="utf-8")
            destination = root / "extract"
            target = destination / "source" / "payload.txt"
            target.parent.mkdir(parents=True)
            try:
                os.symlink(outside, target)
            except (NotImplementedError, OSError) as error:
                self.skipTest(f"file symlinks are unavailable: {error}")

            smartpack_ubuntu.unpack(archive, destination, True, False, 8)

            self.assertEqual(outside.read_text(encoding="utf-8"), "must remain unchanged")
            self.assertFalse(target.is_symlink())
            self.assertEqual(target.read_bytes(), b"safe extraction regression" * 32)

    def test_conflicting_paths_are_rejected_during_preflight(self) -> None:
        with tempfile.TemporaryDirectory(prefix=".smartpack-test-", dir=WORKSPACE) as temp:
            root = Path(temp)
            entries = [
                {"type": "file", "path": "Folder/Name.txt"},
                {"type": "file", "path": "folder/name.TXT"},
            ]
            with self.assertRaises(ValueError):
                smartpack_ubuntu.validate_extraction_plan(root, entries, False, False)

    def test_windows_path_aliases_are_rejected(self) -> None:
        for path in ("C:/outside.txt", "../outside.txt", "folder\\outside.txt", "NUL.txt", "file.txt:stream"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                smartpack_ubuntu.safe_arc_path(path)


if __name__ == "__main__":
    unittest.main()
