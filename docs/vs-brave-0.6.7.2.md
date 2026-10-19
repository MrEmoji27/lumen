# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 36 MB (3 processes) | 151 MB (7 processes) | 76% |
| wiki | 49 MB (9 processes) | 192 MB (7 processes) | 74% |
| youtube | 327 MB (10 processes) | 600 MB (10 processes) | 45% |
| tabs | 331 MB (20 processes) | 1169 MB (20 processes) | 72% |
| tabs, 6.5 min | 124 MB (20 processes) | 1111 MB (20 processes) | 89% |
