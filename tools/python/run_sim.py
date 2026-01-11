#!/usr/bin/env python3

import subprocess
import sys
from pathlib import Path

def main():
    root = Path(__file__).resolve().parents[2]
    build = root / "build"
    build.mkdir(exist_ok=True)

    # Configure + build
    subprocess.check_call(["cmake", "-S", str(root), "-B", str(build)])
    subprocess.check_call(["cmake", "--build", str(build), "-j"])

    # Run
    subprocess.check_call([str(build / "sdv_sim")])

if __name__ == "__main__":
    main()
