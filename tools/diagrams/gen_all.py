#!/usr/bin/env python3
"""Regenerate every SVG figure in docs/design/img/ (python3 tools/diagrams/gen_all.py)."""
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent.parent / "docs" / "design" / "img"
sys.path.insert(0, str(HERE))

import fig_cpp_rust
import fig_ecu_internal
import fig_ecu_network

FIGURES = {
    "ecu_internal.svg": fig_ecu_internal.build,
    "ecu_network.svg": fig_ecu_network.build,
    "cpp_rust.svg": fig_cpp_rust.build,
}

if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for name, build in FIGURES.items():
        (OUT / name).write_text(build(), encoding="utf-8")
        print("wrote", OUT / name)
