# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 36 MB (3 processes) | 152 MB (7 processes) | 76% |
| wiki | 101 MB (9 processes) | 183 MB (7 processes) | 45% |
| youtube | 320 MB (10 processes) | 597 MB (10 processes) | 46% |
| tabs | 214 MB (20 processes) | 1115 MB (20 processes) | 81% |
| tabs, 6.5 min | 142 MB (19 processes) | 933 MB (20 processes) | 85% |
