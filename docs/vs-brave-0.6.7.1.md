# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 36 MB (3 processes) | 154 MB (7 processes) | 77% |
| wiki | 140 MB (9 processes) | 195 MB (7 processes) | 28% |
| youtube | 657 MB (10 processes) | 557 MB (10 processes) | -18% |
| tabs | 811 MB (20 processes) | 1188 MB (20 processes) | 32% |
| tabs, 6.5 min | 271 MB (20 processes) | 1131 MB (20 processes) | 76% |
