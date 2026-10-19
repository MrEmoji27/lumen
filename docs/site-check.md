# Site check

Made by `tools/site_check.py`. One site per fresh headless Lumen, release build, signed out.

| Site | Blocker | Works | Load | Lag (long tasks) | Worst | Memory | Idle CPU | Errors | Blocked |
|---|---|---|---|---|---|---|---|---|---|
| youtube | on | yes (video at 9.0s) | 2192 ms | 10 = 1726 ms | 464 ms | 823 MB | 2.5% | 30 | 20 |
| youtube | off | yes (video at 8.3s) | 1732 ms | 10 = 1506 ms | 447 ms | 904 MB | 0.75% | 0 | 0 |
| youtube-search | on | yes (14 results) | 1393 ms | 9 = 1298 ms | 519 ms | 738 MB | 0.12% | 4 | 8 |
| youtube-search | off | yes (17 results) | 2661 ms | 9 = 1340 ms | 547 ms | 699 MB | 0.36% | 0 | 0 |
| google | on | yes (9 results) | 1146 ms | 3 = 262 ms | 114 ms | 338 MB | 0.0% | 16 | 12 |
| google | off | yes (9 results) | 1391 ms | 4 = 305 ms | 112 ms | 351 MB | 0.01% | 0 | 0 |
| reddit | on | **no** (0 posts, 0 ad bits) | 1056 ms | 1 = 64 ms | 64 ms | 339 MB | 0.11% | 0 | 0 |
| reddit | off | **no** (0 posts, 0 ad bits) | 965 ms | 1 = 61 ms | 61 ms | 328 MB | 0.17% | 0 | 0 |
| github | on | yes (readme) | 1261 ms | 2 = 216 ms | 164 ms | 353 MB | 0.05% | 1 | 4 |
| github | off | yes (readme) | 728 ms | 4 = 343 ms | 154 ms | 355 MB | 0.0% | 1 | 0 |
| wikipedia | on | yes (119 paragraphs) | 817 ms | 0 = 0 ms | 0 ms | 277 MB | 0.0% | 0 | 1 |
| wikipedia | off | yes (119 paragraphs) | 561 ms | 0 = 0 ms | 0 ms | 269 MB | 0.0% | 0 | 0 |
