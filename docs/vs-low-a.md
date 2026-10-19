# Lumen against Brave

Made by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own blocking on. Private working set of the whole process tree, median of five samples.

| Scenario | Lumen | Brave | Lumen saves |
|---|---|---|---|
| idle | 36 MB (3 processes) | 151 MB (7 processes) | 76% |
| wiki | 76 MB (9 processes) | 193 MB (7 processes) | 61% |
| youtube | 318 MB (10 processes) | 551 MB (10 processes) | 42% |
| tabs | 299 MB (20 processes) | 1171 MB (20 processes) | 74% |
| tabs, 6.5 min | 130 MB (20 processes) | 1137 MB (20 processes) | 89% |
