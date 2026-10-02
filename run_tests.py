#!/usr/bin/env python3
"""Runs the bot on every tests/test*.txt through the referee and prints the score per game.

Usage: python run_tests.py [--seed N] [--timeout MS] [--jobs N] [--no-build] [-v] [tests...]
"""
import argparse
import glob
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.abspath(__file__))
EXE = ".exe" if os.name == "nt" else ""
REFEREE = os.path.join(ROOT, "target", "release", "referee" + EXE)
BOT = os.path.join(ROOT, "target", "release", "cg_flame_extinguisher" + EXE)


def run_one(path, seed, timeout, verbose):
    cmd = [REFEREE, path, BOT, "--seed", str(seed), "--timeout", str(timeout)]
    if verbose:
        cmd.append("-v")
    p = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
    if verbose and p.stderr:
        sys.stderr.write(p.stderr)
    m = re.search(r"^RESULT (.*)$", p.stdout, re.M)
    if not m:
        return {"status": "ERROR", "score": 0, "info": (p.stderr or p.stdout).strip()[:200]}
    res = dict(kv.split("=", 1) for kv in m.group(1).split())
    res["score"] = int(res["score"])
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("tests", nargs="*")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=100)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    a = ap.parse_args()

    if not a.no_build:
        if subprocess.run(["cargo", "build", "--release"], cwd=ROOT).returncode != 0:
            sys.exit(1)

    tests = a.tests or sorted(glob.glob(os.path.join(ROOT, "tests", "test*.txt")))
    jobs = 1 if a.verbose else a.jobs
    with ThreadPoolExecutor(jobs) as ex:
        results = list(ex.map(lambda t: run_one(t, a.seed, a.timeout, a.verbose), tests))

    print(f"{'test':<14}{'status':<34}{'score':>6}{'hp':>5}{'cycles':>8}{'turns':>7}{'max_ms':>8}")
    for t, r in zip(tests, results):
        print(f"{os.path.basename(t):<14}{r['status']:<34}{r['score']:>6}{r.get('hp', '-'):>5}"
              f"{r.get('cycles', '-'):>8}{r.get('turns', '-'):>7}{r.get('max_ms', '-'):>8}")
    total = sum(r["score"] for r in results)
    wins = sum(r["status"] == "WIN" for r in results)
    print(f"\nTOTAL {total}  AVG {total / max(1, len(results)):.2f}  WINS {wins}/{len(results)}")


if __name__ == "__main__":
    main()
