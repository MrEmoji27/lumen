# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 37 MB (3 processes) | 156 MB (7 processes) | 76% |
| wiki | 218 MB (9 processes) | 193 MB (7 processes) | -13% |
| youtube | 653 MB (10 processes) | 553 MB (10 processes) | -18% |
| tabs | 706 MB (20 processes) | 1069 MB (20 processes) | 34% |
| tabs, 6.5 min | 311 MB (20 processes) | 1057 MB (20 processes) | 71% |
