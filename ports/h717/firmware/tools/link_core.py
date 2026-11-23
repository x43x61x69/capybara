"""Link the real Xtensa emulator; never substitute a successful demo build."""
Import("env")
from pathlib import Path
import os

root = Path(env.subst("$PROJECT_DIR")).parent
archive = Path(os.environ.get("H717_CORE_ARCHIVE", str(root / "core/target/xtensa-esp32s3-none-elf/release/libh717_core.a")))
env.Append(LIBPATH=[str(archive.parent)], LIBS=[archive.stem.removeprefix("lib")])

def require_core(source, target, env):
    if not archive.is_file():
        raise RuntimeError(
            "Missing Xtensa Rust emulator: " + str(archive)
            + ". Build ports/h717/core first; see ports/h717/README.md. "
            "A display-only replacement is deliberately not linked."
        )

env.AddPreAction("$BUILD_DIR/firmware.elf", require_core)
