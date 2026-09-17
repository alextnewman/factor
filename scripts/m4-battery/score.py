#!/usr/bin/env python3
"""Aggregate M4 battery results into a Markdown score table.

Reads .state/m4-battery/{scores,checks}/m4-<tag>-<task>.json plus models.tsv,
prints a per-(model,task) table and per-model summary lines. No judgment —
the recommendation is written by a human from this table.
"""
import json
import os
import sys

ROOT = os.path.expanduser("~/workspace/winagent32")
BAT = os.path.join(ROOT, ".state/m4-battery")
TASKS = ["t1", "t2", "t3", "t4", "t5", "t6"]
TASK_NAMES = {
    "t1": "file create",
    "t2": "search",
    "t3": "edit line",
    "t4": "terminal persist",
    "t5": "error recovery",
    "t6": "3-step chain",
}


def load(p):
    try:
        with open(p) as f:
            return json.load(f)
    except Exception:
        return {}


def pct(a, b):
    return f"{100.0 * a / b:.0f}%" if b else "n/a"


def main():
    tags = []
    with open(os.path.join(ROOT, "scripts/m4-battery/models.tsv")) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                tags.append(line.split("\t")[0])

    print("| model | task | completed | 1st-try valid | post-warn valid | warn recover | turns | approvals | tool errs |")
    print("|---|---|---|---|---|---|---|---|---|")
    summaries = {}
    for tag in tags:
        comp, ft_a, ft_c, pw_a, pw_c, ep, rec = 0, 0, 0, 0, 0, 0, 0
        n_tasks = 0
        for t in TASKS:
            sid = f"m4-{tag}-{t}"
            s = load(os.path.join(BAT, "scores", sid + ".json"))
            c = load(os.path.join(BAT, "checks", sid + ".json"))
            if not s or "error" in s:
                print(f"| {tag} | {t} ({TASK_NAMES[t]}) | NO DATA | | | | | | |")
                continue
            n_tasks += 1
            done = bool(c.get("completed"))
            comp += done
            ft = s.get("first_try", {})
            pw = s.get("post_warning", {})
            ft_a += ft.get("attempted", 0)
            ft_c += ft.get("clean", 0)
            pw_a += pw.get("attempted", 0)
            pw_c += pw.get("clean", 0)
            ep += s.get("warning_episodes", 0)
            rec += s.get("warning_recoveries", 0)
            mark = "PASS" if done else "FAIL"
            print(f"| {tag} | {t} ({TASK_NAMES[t]}) | {mark} | "
                  f"{pct(ft.get('clean',0), ft.get('attempted',0))} ({ft.get('clean',0)}/{ft.get('attempted',0)}) | "
                  f"{pct(pw.get('clean',0), pw.get('attempted',0))} ({pw.get('clean',0)}/{pw.get('attempted',0)}) | "
                  f"{pct(s.get('warning_recoveries',0), s.get('warning_episodes',0))} | "
                  f"{s.get('turns_used')} | {s.get('approval_chains')} | {s.get('tool_err')} |")
        summaries[tag] = (comp, n_tasks, ft_a, ft_c, pw_a, pw_c, ep, rec)

    print()
    for tag, (comp, n, ft_a, ft_c, pw_a, pw_c, ep, rec) in summaries.items():
        print(f"{tag}: {comp}/{n} tasks completed | "
              f"first-try validity {pct(ft_c, ft_a)} ({ft_c}/{ft_a}) | "
              f"post-warning validity {pct(pw_c, pw_a)} ({pw_c}/{pw_a}) | "
              f"warning recovery {pct(rec, ep)} ({rec}/{ep})")


if __name__ == "__main__":
    main()
