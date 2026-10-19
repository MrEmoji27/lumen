# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 37 MB (3 processes) | 151 MB (7 processes) | 75% |
| wiki | 102 MB (9 processes) | 194 MB (7 processes) | 47% |
| youtube | 325 MB (10 processes) | 582 MB (10 processes) | 44% |
| tabs | 423 MB (20 processes) | 1180 MB (20 processes) | 64% |
| tabs, 6.5 min | 128 MB (20 processes) | 1130 MB (20 processes) | 89% |
