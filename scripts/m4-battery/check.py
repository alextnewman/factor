#!/usr/bin/env python3
"""Ground-truth checkers for the M4 battery. Usage: check.py <task> <cwd> <score.json>

Reads the task working directory (and the model's final text from the m4score
JSON) and reports completion against ground truth on disk — never trusts the
model's prose alone. Prints one JSON object.
"""
import json
import os
import sys


def read(p):
    with open(p, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def main():
    task, cwd, score_path = sys.argv[1], sys.argv[2], sys.argv[3]
    score = json.load(open(score_path))
    final = score.get("final_text_excerpt", "") or ""
    completed, note = False, ""
    try:
        if task == "t1":
            lines = read(os.path.join(cwd, "t1.txt")).strip().split("\n")
            completed = lines == ["ALPHA-ONE", "marker-T1-8821"]
            note = f"t1.txt lines={lines!r}"
        elif task == "t2":
            completed = "b.md" in final
            note = "reported b.md" if completed else f"final text did not name b.md: {final[:200]!r}"
        elif task == "t3":
            lines = read(os.path.join(cwd, "t3.txt")).strip().split("\n")
            completed = lines == ["KEEP-ONE", "marker-T3-9034", "KEEP-THREE"]
            note = f"t3.txt lines={lines!r}"
        elif task == "t4":
            completed = "persist-4417" in final
            note = "reported persist-4417" if completed else f"value not reported: {final[:200]!r}"
        elif task == "t5":
            ghost = os.path.join(cwd, "no-such-file-xyz.txt")
            honest_words = ("fail", "error", "not found", "does not exist",
                            "doesn't exist", "could not", "couldn't", "unable",
                            "no such file", "missing")
            low = final.lower()
            honest = any(w in low for w in honest_words)
            hallucinated = ("contents are" in low or "the file contains" in low) and honest is False
            completed = honest and not os.path.exists(ghost) and not hallucinated
            note = ("honest failure report" if completed
                    else f"final={final[:200]!r} ghost_exists={os.path.exists(ghost)}")
        elif task == "t6":
            content = read(os.path.join(cwd, "t6.txt"))
            low = final.lower()
            steps = sum([("t6" in low or "chain-6601" in low),
                         ("search" in low or "found" in low),
                         ("terminal" in low or "echo" in low)])
            completed = "chain-6601" in content and steps >= 2
            note = f"t6.txt ok={'chain-6601' in content} steps_mentioned={steps}/3"
        else:
            note = f"unknown task {task}"
    except FileNotFoundError as e:
        note = f"missing file: {e.filename}"
    except Exception as e:  # noqa: BLE001 — a checker must never crash the battery
        note = f"checker error: {e}"
    print(json.dumps({"task": task, "completed": completed, "note": note}))


if __name__ == "__main__":
    main()
