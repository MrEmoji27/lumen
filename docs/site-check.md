# Site check

Made by `tools/site_check.py`. One site per fresh headless Lumen, release build, signed out.

| Site | Blocker | Works | Load | Lag (long tasks) | Worst | Memory | Idle CPU | Errors | Blocked |
|---|---|---|---|---|---|---|---|---|---|
| youtube | on | yes (video at 10.3s) | 1879 ms | 18 = 2524 ms | 583 ms | 724 MB | 2.23% | 6 | 20 |
| youtube | off | yes (video at 10.4s) | 1870 ms | 15 = 1935 ms | 503 ms | 739 MB | 3.95% | 6 | 0 |
| youtube-search | on | yes (14 results) | 1459 ms | 10 = 1417 ms | 611 ms | 588 MB | 0.09% | 28 | 8 |
| youtube-search | off | yes (13 results) | 1442 ms | 10 = 1457 ms | 643 ms | 554 MB | 0.05% | 0 | 0 |
| ytmusic | on | yes (67 items) | 2272 ms | 7 = 1047 ms | 386 ms | 447 MB | 0.04% | 2 | 4 |
| ytmusic | off | yes (67 items) | 2239 ms | 7 = 1056 ms | 407 ms | 455 MB | 0.14% | 0 | 0 |
| google | on | yes (9 results) | 1305 ms | 3 = 270 ms | 124 ms | 265 MB | 0.01% | 18 | 12 |
| google | off | yes (9 results) | 736 ms | 21 = 1187 ms | 116 ms | 437 MB | 0.02% | 0 | 0 |
| gmail | on | yes (accounts.google.com) | 2010 ms | 1 = 83 ms | 83 ms | 246 MB | 0.04% | 2 | 1 |
| gmail | off | yes (accounts.google.com) | 2427 ms | 1 = 85 ms | 85 ms | 242 MB | 0.0% | 0 | 0 |
| instagram | on | yes (login form) | 799 ms | 1 = 67 ms | 67 ms | 233 MB | 0.16% | 0 | 5 |
| instagram | off | yes (login form) | 810 ms | 1 = 70 ms | 70 ms | 230 MB | 0.17% | 0 | 0 |
| discord | on | yes (login form) | 2429 ms | 4 = 1229 ms | 612 ms | 432 MB | 0.04% | 0 | 0 |
| discord | off | yes (login form) | 2161 ms | 3 = 928 ms | 480 ms | 452 MB | 0.03% | 0 | 0 |
| reddit | on | **no** (0 posts) | 937 ms | 1 = 60 ms | 60 ms | 271 MB | 0.07% | 0 | 0 |
| reddit | off | **no** (0 posts) | 995 ms | 1 = 67 ms | 67 ms | 288 MB | 0.04% | 0 | 0 |
| x | on | yes (373 chars) | 823 ms | 5 = 493 ms | 141 ms | 331 MB | 5.2% | 2 | 0 |
| x | off | yes (373 chars) | 806 ms | 5 = 432 ms | 132 ms | 323 MB | 4.03% | 0 | 0 |
| github | on | yes (readme) | 3072 ms | 1 = 136 ms | 136 ms | 272 MB | 0.0% | 1 | 4 |
| github | off | yes (readme) | 1654 ms | 3 = 245 ms | 135 ms | 254 MB | 0.04% | 1 | 0 |
| wikipedia | on | yes (119 paragraphs) | 824 ms | 0 = 0 ms | 0 ms | 211 MB | 0.02% | 0 | 1 |
| wikipedia | off | yes (119 paragraphs) | 603 ms | 0 = 0 ms | 0 ms | 210 MB | 0.02% | 0 | 0 |
| twitch | on | yes (65 cards) | 889 ms | 5 = 643 ms | 231 ms | 931 MB | -0.34% | 4 | 2 |
| twitch | off | yes (65 cards) | 830 ms | 3 = 482 ms | 217 ms | 907 MB | -0.39% | 0 | 0 |
| spotify | on | yes (2451 chars) | 4956 ms | 8 = 1177 ms | 408 ms | 507 MB | 1.01% | 29 | 3 |
| spotify | off | yes (2702 chars) | 2075 ms | 6 = 596 ms | 204 ms | 549 MB | 1.04% | 1 | 0 |
